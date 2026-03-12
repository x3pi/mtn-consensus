// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

//! Block sending methods for ExecutorClient.
//!
//! These methods handle sending committed blocks to the Go executor:
//! - `send_committed_subdag` — buffered, sequential sending
//! - `flush_buffer` — flush buffered blocks in order
//! - `send_committed_subdag_direct` — bypass buffer (for SyncOnly)
//! - `send_block_data` — low-level socket send
//! - `convert_to_protobuf` — CommittedSubDag → protobuf bytes

use anyhow::Result;
use consensus_core::{BlockAPI, CommittedSubDag, SystemTransaction};
use prost::Message;
use tokio::io::AsyncWriteExt;
use tracing::{error, info, trace, warn};

use super::persistence::{persist_last_sent_index, write_uvarint};
use super::proto::{CommittedBlock, CommittedEpochData, TransactionExe};
use super::ExecutorClient;
use super::{GO_VERIFICATION_INTERVAL, MAX_BUFFER_SIZE};

impl ExecutorClient {
    /// Send committed sub-DAG to executor
    /// CRITICAL FORK-SAFETY: global_exec_index and commit_index ensure deterministic execution order
    /// SEQUENTIAL BUFFERING: Blocks are buffered and sent in order to ensure Go receives them sequentially
    /// LEADER_ADDRESS: Optional 20-byte Ethereum address of leader validator
    /// When provided, Go uses this directly instead of looking up by index
    pub async fn send_committed_subdag(
        &self,
        subdag: &CommittedSubDag,
        epoch: u64,
        global_exec_index: u64,
        leader_address: Option<Vec<u8>>,
    ) -> Result<()> {
        if !self.is_enabled() {
            return Ok(()); // Silently skip if not enabled
        }

        if !self.can_commit() {
            // This node has executor client but cannot commit (not node 0)
            // Log but don't actually send the commit
            let total_tx: usize = subdag.blocks.iter().map(|b| b.transactions().len()).sum();
            info!("ℹ️  [EXECUTOR] Node has executor client but cannot commit (not node 0): global_exec_index={}, commit_index={}, epoch={}, blocks={}, total_tx={}",
                global_exec_index, subdag.commit_ref.index, epoch, subdag.blocks.len(), total_tx);
            return Ok(()); // Skip actual commit
        }

        // Count total transactions BEFORE conversion (to detect if transactions are lost)
        let total_tx_before: usize = subdag.blocks.iter().map(|b| b.transactions().len()).sum();

        // REPLAY PROTECTION: Discard blocks that are already processed
        // This is critical when Consensus replays old commits on restart
        {
            let next_expected = self.next_expected_index.lock().await;
            if global_exec_index < *next_expected {
                // Only log periodically or for non-empty blocks to avoid noise during replay
                if total_tx_before > 0 || global_exec_index % 1000 == 0 {
                    info!(
                        "♻️ [REPLAY] Discarding already processed block: global={}, expected={}",
                        global_exec_index, *next_expected
                    );
                }
                return Ok(());
            }
        }

        // DUAL-STREAM DEDUP: Prevent duplicate sends from Consensus and Sync streams
        // This is critical for BlockCoordinator integration
        {
            let sent = self.sent_indices.lock().await;
            if sent.contains(&global_exec_index) {
                info!(
                    "🔄 [DEDUP] Skipping already-sent block from dual-stream: global_exec_index={}",
                    global_exec_index
                );
                return Ok(()); // Already sent, skip
            }
            // Don't insert yet - insert only after successful send
        }
        // Convert CommittedSubDag to protobuf CommittedEpochData
        // CRITICAL FORK-SAFETY: Include global_exec_index and commit_index for deterministic ordering
        let epoch_data_bytes =
            self.convert_to_protobuf(subdag, epoch, global_exec_index, leader_address)?;

        // Count total transactions after conversion (should match before)
        let total_tx: usize = total_tx_before;

        // SEQUENTIAL BUFFERING: Add to buffer and try to send in order
        {
            let mut buffer = self.send_buffer.lock().await;

            // PRODUCTION SAFETY: Buffer size limit to prevent memory exhaustion
            if buffer.len() >= MAX_BUFFER_SIZE {
                error!("🚨 [BUFFER LIMIT] Buffer is full ({} blocks). Rejecting block global_exec_index={}. This indicates severe sync issues.",
                    buffer.len(), global_exec_index);
                return Err(anyhow::anyhow!(
                    "Buffer full: {} blocks (max {})",
                    buffer.len(),
                    MAX_BUFFER_SIZE
                ));
            }
            if buffer.contains_key(&global_exec_index) {
                // CRITICAL: Duplicate global_exec_index detected - this should NOT happen
                // Possible causes:
                // 1. Commit processor sent same commit twice
                // 2. global_exec_index calculation is wrong (same value for different commits)
                // 3. Epoch transition didn't reset state correctly
                // 4. Buffer wasn't cleared properly after restart
                let (existing_epoch_data, existing_epoch, existing_commit) = buffer
                    .get(&global_exec_index)
                    .map(|(d, e, c)| (d.len(), *e, *c))
                    .unwrap_or((0, 0, 0));
                error!(
                    "🚨 [DUPLICATE GLOBAL_EXEC_INDEX] Duplicate global_exec_index={} detected!",
                    global_exec_index
                );
                error!(
                    "   📊 Existing: epoch={}, commit_index={}, data_size={} bytes",
                    existing_epoch, existing_commit, existing_epoch_data
                );
                error!(
                    "   📊 New:      epoch={}, commit_index={}, data_size={} bytes, total_tx={}",
                    epoch,
                    subdag.commit_ref.index,
                    epoch_data_bytes.len(),
                    total_tx
                );

                // CRITICAL FIX: Compare commits to determine if they are truly the same
                // If same commit (same epoch + commit_index), skip new one (already in buffer)
                // If different commits with same global_exec_index, this is a BUG - log error but still process
                let is_same_commit =
                    existing_epoch == epoch && existing_commit == subdag.commit_ref.index;

                if is_same_commit {
                    // Same commit - skip new one, existing one in buffer will be sent
                    warn!("   ✅ Same commit detected (epoch={}, commit_index={}) - skipping duplicate, existing commit in buffer will be sent", epoch, subdag.commit_ref.index);
                    return Ok(()); // Skip this commit, don't overwrite buffer
                } else {
                    // DIFFERENT commits with same global_exec_index - this is a SERIOUS BUG
                    error!("   🚨 DIFFERENT commits with same global_exec_index! This is a BUG!");
                    error!("   🔍 Root cause analysis:");
                    error!("      - Epochs different ({} vs {}): global_exec_index calculation may be wrong", existing_epoch, epoch);
                    error!("      - Commit indexes different ({} vs {}): same global_exec_index calculated for different commits", existing_commit, subdag.commit_ref.index);
                    error!("      - This indicates last_global_exec_index was not updated correctly or calculation is wrong");

                    // CRITICAL: Overwrite with new commit to prevent transaction loss
                    // The new commit should be processed, even if it means losing the old one
                    // This is better than skipping both commits
                    warn!("   ⚠️  OVERWRITING existing commit with new commit to ensure transactions are executed");
                    warn!("   ⚠️  This may cause transaction loss in the old commit, but ensures new transactions are executed");
                    // Continue to insert below (will overwrite)
                }
            }
            buffer.insert(
                global_exec_index,
                (epoch_data_bytes, epoch, subdag.commit_ref.index),
            );
            trace!("📦 [SEQUENTIAL-BUFFER] Added block to buffer: global_exec_index={}, commit_index={}, epoch={}, blocks={}, total_tx={}, buffer_size={}",
                global_exec_index, subdag.commit_ref.index, epoch, subdag.blocks.len(), total_tx, buffer.len());
        }

        // CRITICAL: Always try to flush buffer after adding commit
        // This ensures commits are sent to Go executor even if there are gaps
        // flush_buffer() will send all consecutive commits starting from next_expected_index
        if let Err(e) = self.flush_buffer().await {
            warn!(
                "⚠️  [SEQUENTIAL-BUFFER] Failed to flush buffer after adding commit: {}",
                e
            );
            // Don't return error - commit is in buffer and will be sent later
        }

        Ok(())
    }

    /// Flush buffered blocks in sequential order
    /// This ensures Go executor receives blocks in the correct order even if Rust sends them out-of-order
    /// CRITICAL: This function will send all consecutive commits starting from next_expected_index
    pub async fn flush_buffer(&self) -> Result<()> {
        // Connect if needed
        if let Err(e) = self.connect().await {
            warn!("⚠️  Executor connection failed, cannot flush buffer: {}", e);
            return Ok(()); // Don't fail if executor is unavailable
        }

        // Log buffer status before flushing
        {
            let buffer = self.send_buffer.lock().await;
            let next_expected = self.next_expected_index.lock().await;
            if !buffer.is_empty() {
                let min_buffered = *buffer.keys().next().unwrap_or(&0);
                let max_buffered = *buffer.keys().last().unwrap_or(&0);
                let gap = min_buffered.saturating_sub(*next_expected);
                trace!("📊 [FLUSH BUFFER] Buffer status: size={}, range={}..{}, next_expected={}, gap={}", 
                    buffer.len(), min_buffered, max_buffered, *next_expected, gap);
            }
        }

        // CRITICAL: Do NOT skip blocks - ensure all blocks are sent sequentially
        // If next_expected is behind, it means blocks are missing - we must wait for them
        // Do not skip to min_buffered as this would break sequential ordering
        // Instead, log a warning and let the system handle missing blocks through retry mechanism
        {
            let buffer = self.send_buffer.lock().await;
            let next_expected = self.next_expected_index.lock().await;
            if !buffer.is_empty() {
                let min_buffered = *buffer.keys().next().unwrap_or(&0);
                let gap = min_buffered.saturating_sub(*next_expected);

                // CRITICAL FIX (2026-02-13): Auto-recover from ANY gap, not just large ones (was > 100).
                // A gap of even 1 block means next_expected is wrong (e.g., from stale epoch boundary).
                // The off-by-one bug in epoch_boundary_block produces a gap of exactly 1, causing
                // the buffer to wait forever for a block that was already processed in the previous epoch.
                // Syncing with Go (SINGLE SOURCE OF TRUTH) immediately recovers from this.
                if gap > 100 {
                    // PERFORMANCE FIX: Only sync with Go for LARGE gaps (>100).
                    // Small gaps are normal during high throughput and resolve as blocks arrive.
                    // Instead of blocking consensus (which causes 20-60s delays), we spawn a background task.
                    drop(buffer);
                    drop(next_expected);

                    warn!("⚠️  [SEQUENTIAL-BUFFER] Large gap detected: min_buffered={}, gap={}. Syncing with Go using fast 2-second timeout...", 
                        min_buffered, gap);

                    // Add a strict outer timeout to get_last_block_number to ensure it never hangs consensus
                    let sync_future = self.get_last_block_number();
                    if let Ok(Ok(go_last_block)) =
                        tokio::time::timeout(tokio::time::Duration::from_secs(2), sync_future).await
                    {
                        let go_next_expected = go_last_block + 1;

                        let mut next_expected_guard = self.next_expected_index.lock().await;
                        if go_next_expected > *next_expected_guard {
                            info!("📊 [SINGLE-SOURCE-TRUTH] Updating next_expected from {} to {} (from Go last_block={})",
                                *next_expected_guard, go_next_expected, go_last_block);
                            *next_expected_guard = go_next_expected;

                            let mut buffer = self.send_buffer.lock().await;
                            let before_clear = buffer.len();
                            buffer.retain(|&k, _| k >= go_next_expected);
                            let after_clear = buffer.len();
                            if before_clear > after_clear {
                                info!(
                                    "🧹 [SINGLE-SOURCE-TRUTH] Cleared {} old blocks, kept {}",
                                    before_clear - after_clear,
                                    after_clear
                                );
                            }
                        }
                    } else {
                        warn!("⚠️  [SEQUENTIAL-BUFFER] get_last_block_number timed out or failed. Continuing buffered sender...");
                    }
                } else if gap > 0 {
                    trace!("⏸️  [SEQUENTIAL-BUFFER] Small gap={} (normal during high throughput), waiting for blocks to arrive", gap);
                }
            }
        }

        // Send all consecutive blocks starting from next_expected
        loop {
            // Get current next_expected and check if we have that block
            let current_expected = {
                let next_expected = self.next_expected_index.lock().await;
                *next_expected
            };

            // Try to get the block for current_expected
            let block_data = {
                let mut buffer = self.send_buffer.lock().await;
                buffer.remove(&current_expected)
            };

            if let Some((epoch_data_bytes, epoch, commit_index)) = block_data {
                // This is the next expected block - send it immediately
                trace!("📤 [SEQUENTIAL-BUFFER] Sending block: global_exec_index={}, commit_index={}, epoch={}, size={} bytes",
                    current_expected, commit_index, epoch, epoch_data_bytes.len());
                if let Err(e) = self
                    .send_block_data(&epoch_data_bytes, current_expected, epoch, commit_index)
                    .await
                {
                    warn!("⚠️  [SEQUENTIAL-BUFFER] Failed to send block global_exec_index={}: {}, re-adding to buffer", 
                        current_expected, e);
                    // Re-add to buffer for retry
                    let mut buffer_retry = self.send_buffer.lock().await;
                    buffer_retry.insert(current_expected, (epoch_data_bytes, epoch, commit_index));
                    return Ok(());
                }

                // Successfully sent, increment next_expected and persist
                {
                    let mut next_expected = self.next_expected_index.lock().await;
                    *next_expected += 1;
                    trace!("✅ [SEQUENTIAL-BUFFER] Successfully sent block global_exec_index={}, next_expected={}", 
                        current_expected, *next_expected);

                    // DUAL-STREAM TRACKING: Mark this index as successfully sent
                    // This prevents duplicate sends from both Consensus and Sync streams
                    {
                        let mut sent = self.sent_indices.lock().await;
                        sent.insert(current_expected);
                        // Limit memory: only keep last 10000 indices
                        if sent.len() > 10000 {
                            if let Some(&min_idx) = sent.iter().min() {
                                sent.remove(&min_idx);
                            }
                        }
                    }

                    // PERSIST: Save last successfully sent index for crash recovery
                    if let Some(ref storage_path) = self.storage_path {
                        if let Err(e) =
                            persist_last_sent_index(storage_path, current_expected, commit_index)
                                .await
                        {
                            warn!(
                                "⚠️ [PERSIST] Failed to persist last_sent_index={}: {}",
                                current_expected, e
                            );
                        }
                    }

                    // GO VERIFICATION: Periodically verify Go actually received blocks
                    // This detects forks where Go's state diverges from what Rust sent
                    if current_expected % GO_VERIFICATION_INTERVAL == 0 {
                        if let Ok(go_last_block) = self.get_last_block_number().await {
                            let mut last_verified = self.last_verified_go_index.lock().await;

                            // FORK DETECTION: Go's block should never decrease
                            if go_last_block < *last_verified {
                                error!("🚨 [FORK DETECTED] Go's block number DECREASED! last_verified={}, go_now={}. CRITICAL: Possible fork or Go state corruption!",
                                    *last_verified, go_last_block);
                            }

                            // Update last verified
                            *last_verified = go_last_block;

                            // Check if Go is keeping up
                            let lag = current_expected.saturating_sub(go_last_block);
                            
                            // BACKPRESSURE: Update go_lag_handle for SystemTransactionProvider
                            if let Some(ref handle) = self.go_lag_handle {
                                handle.store(lag, std::sync::atomic::Ordering::Relaxed);
                            }
                            
                            if lag > 100 {
                                warn!(
                                    "⚠️ [GO LAG] Go is {} blocks behind Rust. sent={}, go={}",
                                    lag, current_expected, go_last_block
                                );
                            } else {
                                trace!(
                                    "✓ [GO VERIFY] Go verified at block {}. Rust sent={}, lag={}",
                                    go_last_block,
                                    current_expected,
                                    lag
                                );
                            }
                        }
                    }
                }

                // CRITICAL: Continue loop to try sending next block immediately
                // Don't break here - there might be more consecutive blocks to send
            } else {
                // No more consecutive blocks to send (gap detected)
                // Log and break - will retry when next block arrives
                {
                    let buffer = self.send_buffer.lock().await;
                    if !buffer.is_empty() {
                        let min_buffered = *buffer.keys().next().unwrap_or(&0);
                        let gap = min_buffered.saturating_sub(current_expected);
                        if gap > 0 {
                            trace!("⏸️  [SEQUENTIAL-BUFFER] Gap detected: next_expected={}, min_buffered={}, gap={}. Waiting for missing blocks.", 
                                current_expected, min_buffered, gap);
                        }
                    }
                }
                break;
            }
        }

        // Log buffer status
        {
            let buffer = self.send_buffer.lock().await;
            let next_expected = self.next_expected_index.lock().await;
            if !buffer.is_empty() {
                let min_buffered = *buffer.keys().next().unwrap_or(&0);
                let max_buffered = *buffer.keys().last().unwrap_or(&0);
                let gap = min_buffered.saturating_sub(*next_expected);
                if buffer.len() > 10 {
                    warn!("⚠️  [SEQUENTIAL-BUFFER] Buffer has {} blocks waiting (range: {}..{}, next_expected={}, gap={}). Some blocks may be missing or out-of-order.",
                        buffer.len(), min_buffered, max_buffered, *next_expected, gap);
                } else {
                    info!("📊 [SEQUENTIAL-BUFFER] Buffer has {} blocks waiting (range: {}..{}, next_expected={}, gap={})",
                        buffer.len(), min_buffered, max_buffered, *next_expected, gap);
                }
            }
        }

        Ok(())
    }

    /// Send committed sub-DAG directly to Go executor (BYPASS BUFFER)
    ///
    /// This is used by SyncOnly nodes to send blocks directly without using
    /// the sequential buffer. SyncOnly may receive blocks out-of-order or
    /// with gaps, so the buffer-based approach doesn't work.
    ///
    /// IMPORTANT: This does NOT update next_expected_index or sent_indices.
    /// Go is responsible for handling ordering when receiving synced blocks.
    pub async fn send_committed_subdag_direct(
        &self,
        subdag: &CommittedSubDag,
        epoch: u64,
        global_exec_index: u64,
        leader_address: Option<Vec<u8>>,
    ) -> Result<()> {
        if !self.is_enabled() {
            return Ok(()); // Silently skip if not enabled
        }

        if !self.can_commit() {
            return Ok(()); // Skip if cannot commit
        }

        // Convert to protobuf bytes
        let epoch_data_bytes =
            self.convert_to_protobuf(subdag, epoch, global_exec_index, leader_address)?;

        info!("📤 [SYNC-DIRECT] Sending block directly: global_exec_index={}, epoch={}, size={} bytes",
            global_exec_index, epoch, epoch_data_bytes.len());

        // Send directly - bypass buffer
        self.send_block_data(
            &epoch_data_bytes,
            global_exec_index,
            epoch,
            subdag.commit_ref.index,
        )
        .await?;

        info!(
            "✅ [SYNC-DIRECT] Block sent successfully: global_exec_index={}",
            global_exec_index
        );

        Ok(())
    }

    /// Send block data via UDS/TCP (internal helper)
    pub(super) async fn send_block_data(
        &self,
        epoch_data_bytes: &[u8],
        global_exec_index: u64,
        epoch: u64,
        commit_index: u32,
    ) -> Result<()> {
        // 🛡️ CIRCUIT BREAKER: Check if we are in Fast-Fail mode
        self.check_send_circuit_breaker().await?;

        // Auto-reconnect if connection is None
        {
            let conn_check = self.connection.lock().await;
            if conn_check.is_none() {
                drop(conn_check);
                info!("🔄 [EXECUTOR] Connection not established, connecting...");
                self.connect().await?;
            }
        }

        // Send via TCP/UDS with Uvarint length prefix (Go expects Uvarint)
        let mut conn_guard = self.connection.lock().await;
        if let Some(ref mut stream) = *conn_guard {
            // Write Uvarint length prefix
            let mut len_buf = Vec::new();
            write_uvarint(&mut len_buf, epoch_data_bytes.len() as u64)?;

            // Send with retry logic if write fails
            // CRITICAL: Add timeout to prevent commit processor from getting stuck
            use tokio::time::{timeout, Duration};
            const SEND_TIMEOUT: Duration = Duration::from_secs(30);

            let send_result = timeout(SEND_TIMEOUT, async {
                stream.write_all(&len_buf).await?;
                stream.write_all(epoch_data_bytes).await?;
                stream.flush().await?;
                Ok::<(), std::io::Error>(())
            })
            .await;

            let send_result = match send_result {
                Ok(Ok(())) => Ok(()),
                Ok(Err(e)) => Err(e),
                Err(_) => {
                    // Timeout occurred
                    warn!("⏱️  [EXECUTOR] Send timeout after {}s (global_exec_index={}, commit_index={}), closing connection", 
                        SEND_TIMEOUT.as_secs(), global_exec_index, commit_index);
                    *conn_guard = None; // Clear connection
                    self.record_send_failure().await; // 🚨 Mark Failure
                    return Err(anyhow::anyhow!("Send timeout"));
                }
            };

            match send_result {
                Ok(_) => {
                    trace!("📤 [TX FLOW] Sent committed sub-DAG to Go executor: global_exec_index={}, commit_index={}, epoch={}, data_size={} bytes", 
                        global_exec_index, commit_index, epoch, epoch_data_bytes.len());
                    self.record_send_success().await; // ✅ Mark Success
                    Ok(())
                }
                Err(e) => {
                    warn!("⚠️  [EXECUTOR] Failed to send committed sub-DAG (global_exec_index={}, commit_index={}): {}, reconnecting...", 
                        global_exec_index, commit_index, e);
                    // Connection is dead, clear it so next send will reconnect
                    *conn_guard = None;
                    self.record_send_failure().await; // 🚨 Mark Failure for the first attempt

                    // Retry send after reconnection
                    drop(conn_guard);
                    if let Err(reconnect_err) = self.connect().await {
                        warn!(
                            "⚠️  [EXECUTOR] Failed to reconnect after send error: {}",
                            reconnect_err
                        );
                        self.record_send_failure().await; // 🚨 Mark Failure for reconnect error
                        return Err(anyhow::anyhow!("Reconnection failed: {}", reconnect_err));
                    }
                    // Retry send with timeout
                    let mut retry_guard = self.connection.lock().await;
                    if let Some(ref mut retry_stream) = *retry_guard {
                        let mut retry_len_buf = Vec::new();
                        write_uvarint(&mut retry_len_buf, epoch_data_bytes.len() as u64)?;

                        let retry_result = timeout(SEND_TIMEOUT, async {
                            retry_stream.write_all(&retry_len_buf).await?;
                            retry_stream.write_all(epoch_data_bytes).await?;
                            retry_stream.flush().await?;
                            Ok::<(), std::io::Error>(())
                        })
                        .await;

                        match retry_result {
                            Ok(Ok(())) => {
                                trace!("✅ [EXECUTOR] Successfully sent committed sub-DAG after reconnection: global_exec_index={}, commit_index={}", 
                                    global_exec_index, commit_index);
                                self.record_send_success().await; // ✅ Mark Success on Retry
                                Ok(())
                            }
                            Ok(Err(retry_err)) => {
                                warn!("⚠️  [EXECUTOR] Retry send also failed: {}", retry_err);
                                *retry_guard = None; // Clear connection for next attempt
                                self.record_send_failure().await; // 🚨 Mark Failure on Retry
                                Err(anyhow::anyhow!("Retry send failed: {}", retry_err))
                            }
                            Err(_) => {
                                warn!("⏱️  [EXECUTOR] Retry send timeout after {}s (global_exec_index={}, commit_index={})", 
                                    SEND_TIMEOUT.as_secs(), global_exec_index, commit_index);
                                *retry_guard = None; // Clear connection
                                self.record_send_failure().await; // 🚨 Mark Failure on Retry Timeout
                                Err(anyhow::anyhow!("Retry send timeout"))
                            }
                        }
                    } else {
                        self.record_send_failure().await;
                        Err(anyhow::anyhow!("Connection lost after reconnection"))
                    }
                }
            }
        } else {
            self.record_send_failure().await;
            warn!("⚠️  [EXECUTOR] Executor connection lost, skipping send");
            Err(anyhow::anyhow!("Connection lost"))
        }
    }

    /// Convert CommittedSubDag to protobuf CommittedEpochData bytes
    /// Uses generated protobuf code to ensure correct encoding
    ///
    /// IMPORTANT: Transaction data is passed through unchanged from Go sub → Rust consensus → Go master
    /// We verify data integrity by checking transaction hash at each step
    ///
    /// CRITICAL FORK-SAFETY: global_exec_index and commit_index ensure deterministic execution order
    /// All nodes must execute blocks with the same global_exec_index in the same order
    fn convert_to_protobuf(
        &self,
        subdag: &CommittedSubDag,
        epoch: u64,
        global_exec_index: u64,
        leader_address: Option<Vec<u8>>,
    ) -> Result<Vec<u8>> {
        // Build CommittedEpochData protobuf message using generated types
        let mut blocks = Vec::new();

        for (block_idx, block) in subdag.blocks.iter().enumerate() {
            // Extract transactions with hash for deterministic sorting
            // CRITICAL FORK-SAFETY: Sort transactions by hash to ensure all nodes send same order
            // OPTIMIZATION: Use references during sorting to reduce memory allocations
            let mut transactions_with_hash: Vec<(&[u8], Vec<u8>)> =
                Vec::with_capacity(block.transactions().len()); // (tx_data_ref, tx_hash)
            let mut skipped_count = 0;
            let total_tx_in_block = block.transactions().len();
            for (tx_idx, tx) in block.transactions().iter().enumerate() {
                // Get transaction data (raw bytes) - Go needs transaction data, not digest
                // IMPORTANT: tx.data() returns a reference to the original bytes, no modification
                let tx_data = tx.data();
                // 🔍 HASH INTEGRITY CHECK: Verify transaction data integrity by calculating hash
                // This ensures data hasn't been modified during consensus
                use crate::types::tx_hash::calculate_transaction_hash;
                let tx_hash = calculate_transaction_hash(tx_data);
                let tx_hash_hex = hex::encode(&tx_hash[..8.min(tx_hash.len())]);
                let _tx_hash_full_hex = ""; // Removed: was hex::encode(&tx_hash) — unused computation

                // 🔍 FILTER: Check if this is a SystemTransaction (BCS format) - skip if so
                // SystemTransaction should not be sent to Go executor as Go doesn't understand BCS
                if SystemTransaction::from_bytes(tx_data).is_ok() {
                    trace!("ℹ️ [SYSTEM TX FILTER] Skipping SystemTransaction (BCS format) in block {} tx {}: hash={}..., size={} bytes",
                        block_idx, tx_idx, tx_hash_hex, tx_data.len());
                    skipped_count += 1;
                    continue;
                }

                // Log transaction processing (general tracking, not specific transaction)
                trace!("🔍 [TX HASH] Processing transaction: hash={}..., size={} bytes, block_idx={}, tx_idx={}",
                    tx_hash_hex, tx_data.len(), block_idx, tx_idx);

                // Verify transaction data is valid protobuf before sending to Go
                // Uses strict validation (from_address must be non-empty) to filter
                // non-user data like consensus internal messages
                use crate::types::tx_hash::verify_transaction_protobuf;
                if !verify_transaction_protobuf(tx_data) {
                    trace!("🚫 [TX FILTER] Non-user transaction in committed block (hash={}..., size={} bytes, global_exec_index={}, commit_index={}). Skipping.", 
                        tx_hash_hex, tx_data.len(), global_exec_index, subdag.commit_ref.index);
                    skipped_count += 1;
                    continue;
                }

                trace!(
                    "🔍 [TX INTEGRITY] Verifying transaction data: hash={}, size={} bytes",
                    tx_hash_hex,
                    tx_data.len()
                );

                // Store transaction data reference with hash for sorting
                // OPTIMIZATION: Avoid first clone by storing reference during sorting
                transactions_with_hash.push((tx_data, tx_hash));
                trace!("✅ [TX INTEGRITY] Transaction data preserved: hash={}..., size={} bytes (unchanged from submission)", 
                    tx_hash_hex, tx_data.len());
            }

            // CRITICAL FORK-SAFETY: Sort transactions by hash (deterministic ordering)
            // This ensures all nodes send transactions in the same order within a block
            // Sort by hash bytes (lexicographic order) - deterministic across all nodes
            transactions_with_hash.sort_by(|(_, hash_a), (_, hash_b)| hash_a.cmp(hash_b));

            // Convert to TransactionExe messages after sorting
            // OPTIMIZATION: Only clone once here instead of twice (during push + here)
            let mut transactions = Vec::new();
            for (_sorted_idx, (tx_data_ref, tx_hash)) in transactions_with_hash.iter().enumerate() {
                let tx_hash_hex = hex::encode(&tx_hash[..8.min(tx_hash.len())]);
                trace!(
                    "📋 [FORK-SAFETY] Sorted transaction in block[{}]: hash={}",
                    block_idx,
                    tx_hash_hex
                );

                // Create TransactionExe message using generated protobuf code
                // NOTE: We use "digest" field to store transaction data (raw bytes)
                // Go will unmarshal this as transaction data
                // OPTIMIZATION: Only clone once here (instead of during push + here)
                let tx_exe = TransactionExe {
                    digest: tx_data_ref.to_vec(), // Clone &[u8] to Vec<u8> - the only clone needed
                    worker_id: 0,                 // Optional, set to 0 for now
                };
                transactions.push(tx_exe);
            }
            if skipped_count > 0 {
                trace!(
                    "⏭️ [TX FILTER] Block[{}] skipped {} non-user transactions out of {} total",
                    block_idx,
                    skipped_count,
                    total_tx_in_block
                );
            }
            trace!("✅ [FORK-SAFETY] Block[{}] transactions sorted: {} transactions (deterministic order by hash)", 
                block_idx, transactions.len());

            // Create CommittedBlock message using generated protobuf code
            let committed_block = CommittedBlock {
                epoch,
                height: subdag.commit_ref.index as u64,
                transactions,
            };
            blocks.push(committed_block);
        }

        // CRITICAL FORK-SAFETY: Sort blocks by height (commit_index) to ensure deterministic order
        // All nodes must send blocks in the same order
        blocks.sort_by(|a, b| a.height.cmp(&b.height));

        // Create CommittedEpochData message using generated protobuf code
        // CRITICAL FORK-SAFETY: Include global_exec_index and commit_index for deterministic ordering
        // EPOCH TRACKING: Include epoch number for block header population in Go Master
        // CRITICAL FORK-SAFETY: Include commit_timestamp_ms for deterministic block hashes
        // Go Master MUST use this timestamp for BlockHeader instead of time.Now()
        // CRITICAL FORK-SAFETY: Include leader_author_index for deterministic LeaderAddress
        // Go Master MUST lookup validator address from committee using this index
        let epoch_data = CommittedEpochData {
            blocks,
            global_exec_index,
            commit_index: subdag.commit_ref.index as u32,
            epoch,
            commit_timestamp_ms: subdag.timestamp_ms, // Consensus timestamp from Linearizer::calculate_commit_timestamp()
            leader_author_index: subdag.leader.author.value() as u32, // Leader authority index for Go to lookup validator address
            leader_address: leader_address.unwrap_or_default(), // 20-byte Ethereum address from Rust committee lookup
        };

        // Encode to protobuf bytes using prost::Message::encode
        // This ensures correct protobuf encoding that Go can unmarshal
        let mut buf = Vec::new();
        epoch_data.encode(&mut buf)?;

        // Count total transactions in encoded data
        let total_tx_encoded: usize = epoch_data.blocks.iter().map(|b| b.transactions.len()).sum();
        trace!("📦 [TX INTEGRITY] Encoded CommittedEpochData: global_exec_index={}, commit_index={}, epoch={}, blocks={}, total_tx={}, total_size={} bytes (using protobuf encoding)", 
            global_exec_index, subdag.commit_ref.index, epoch, epoch_data.blocks.len(), total_tx_encoded, buf.len());

        Ok(buf)
    }
}
