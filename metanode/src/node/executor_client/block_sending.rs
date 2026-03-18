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
use super::proto::{ExecutableBlock, TransactionExe};
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


        // Count total transactions BEFORE conversion (to detect if transactions are lost)
        let total_tx_before: usize = subdag.blocks.iter().map(|b| b.transactions().len()).sum();

        // 🔍 DIAGNOSTIC: Log ALL commits with transactions (not just trace level)
        if total_tx_before > 0 {
            let block_details: Vec<String> = subdag.blocks.iter().enumerate().map(|(i, b)| {
                format!("block[{}]: {} txs, {} bytes each", i, b.transactions().len(),
                    b.transactions().first().map(|t| t.data().len()).unwrap_or(0))
            }).collect();
            info!("🔍 [DIAG] send_committed_subdag: global_exec_index={}, commit_index={}, epoch={}, total_tx_before={}, blocks={}, details=[{}]",
                global_exec_index, subdag.commit_ref.index, epoch, total_tx_before, subdag.blocks.len(), block_details.join(", "));
        }

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
        // OPTIMIZATION: For empty commits (0 transactions), use fast-path that bypasses
        // the expensive per-tx hash/filter/sort logic in convert_to_protobuf
        let epoch_data_bytes = if total_tx_before == 0 {
            self.convert_to_protobuf_empty(subdag, epoch, global_exec_index, leader_address)?
        } else {
            info!("🔍 [DIAG] Using FULL convert_to_protobuf path for global_exec_index={} (total_tx_before={})",
                global_exec_index, total_tx_before);
            self.convert_to_protobuf(subdag, epoch, global_exec_index, leader_address)?
        };

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
            info!("📦 [SEQUENTIAL-BUFFER] Added block to buffer: global_exec_index={}, commit_index={}, epoch={}, blocks={}, total_tx={}, buffer_size={}",
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
    /// OPTIMIZATION: Batches consecutive socket writes and reduces lock contention
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
                info!("📊 [FLUSH BUFFER] Buffer status: size={}, range={}..{}, next_expected={}, gap={}", 
                    buffer.len(), min_buffered, max_buffered, *next_expected, gap);
            }
        }

        // CRITICAL: Do NOT skip blocks - ensure all blocks are sent sequentially
        {
            let buffer = self.send_buffer.lock().await;
            let next_expected = self.next_expected_index.lock().await;
            if !buffer.is_empty() {
                let min_buffered = *buffer.keys().next().unwrap_or(&0);
                let gap = min_buffered.saturating_sub(*next_expected);

                if gap > 100 {
                    drop(buffer);
                    drop(next_expected);

                    warn!("⚠️  [SEQUENTIAL-BUFFER] Large gap detected: min_buffered={}, gap={}. Syncing with Go using fast 2-second timeout...", 
                        min_buffered, gap);

                    // CRITICAL FIX: Use get_last_global_exec_index() instead of get_last_block_number()
                    // get_last_block_number() returns Go block NUMBER (counts only non-empty commits)
                    // but next_expected_index tracks GEI (counts ALL commits including empty ones)
                    // Using block number (e.g. 6) when GEI is ~9000 creates a permanent gap > 100,
                    // causing an infinite sync loop where TX blocks are buffered but never sent.
                    let sync_future = self.get_last_global_exec_index();
                    if let Ok(Ok(go_last_gei)) =
                        tokio::time::timeout(tokio::time::Duration::from_secs(2), sync_future).await
                    {
                        let go_next_expected = go_last_gei + 1;

                        let mut next_expected_guard = self.next_expected_index.lock().await;
                        if go_next_expected > *next_expected_guard {
                            info!("📊 [SINGLE-SOURCE-TRUTH] Updating next_expected from {} to {} (from Go last_gei={})",
                                *next_expected_guard, go_next_expected, go_last_gei);
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
                        warn!("⚠️  [SEQUENTIAL-BUFFER] get_last_global_exec_index timed out or failed. Continuing buffered sender...");
                    }
                } else if gap > 0 {
                    trace!("⏸️  [SEQUENTIAL-BUFFER] Small gap={} (normal during high throughput), waiting for blocks to arrive", gap);
                }
            }
        }

        // ═══════════════════════════════════════════════════════════════════
        // BATCHED FLUSH: Collect consecutive blocks and write them in a
        // single batched operation. This reduces socket flushes from N to 1
        // and batches lock operations for sent_indices and persistence.
        // ═══════════════════════════════════════════════════════════════════
        const BATCH_WRITE_LIMIT: usize = 500; // Max blocks per batch write
        const PERSIST_INTERVAL: u64 = 100; // Persist to disk every N commits

        // Phase 1: Collect consecutive blocks from buffer
        let mut batch: Vec<(u64, Vec<u8>, u64, u32)> = Vec::new(); // (global_exec_index, data, epoch, commit_index)
        {
            let mut buffer = self.send_buffer.lock().await;
            let next_expected = self.next_expected_index.lock().await;
            let mut idx = *next_expected;
            while batch.len() < BATCH_WRITE_LIMIT {
                if let Some((data, epoch, commit_index)) = buffer.remove(&idx) {
                    batch.push((idx, data, epoch, commit_index));
                    idx += 1;
                } else {
                    break; // Gap — stop collecting
                }
            }
        }

        if batch.is_empty() {
            // Nothing to send
            return Ok(());
        }

        let batch_size = batch.len();
        let first_idx = batch[0].0;
        let last_idx = batch[batch_size - 1].0;
        let last_commit_index = batch[batch_size - 1].3;

        // Phase 2: Write all blocks to socket in a single batch (1 flush)
        {
            // Auto-reconnect if needed
            {
                let conn_check = self.connection.lock().await;
                if conn_check.is_none() {
                    drop(conn_check);
                    self.connect().await?;
                }
            }

            let mut conn_guard = self.connection.lock().await;
            if let Some(ref mut stream) = *conn_guard {
                use tokio::time::{timeout, Duration};
                // Scale timeout with batch size (30s base + 1s per 100 blocks)
                let send_timeout = Duration::from_secs(30 + (batch_size as u64 / 100));

                let send_result = timeout(send_timeout, async {
                    for (_, data, _, _) in &batch {
                        let mut len_buf = Vec::new();
                        write_uvarint(&mut len_buf, data.len() as u64)
                            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
                        stream.write_all(&len_buf).await?;
                        stream.write_all(data).await?;
                    }
                    stream.flush().await?; // Single flush for entire batch
                    Ok::<(), std::io::Error>(())
                })
                .await;

                match send_result {
                    Ok(Ok(())) => {
                        self.record_send_success().await;
                        if batch_size > 1 {
                            info!("⚡ [BATCH-SEND] Sent {} blocks in 1 batch (GEI {}→{})",
                                batch_size, first_idx, last_idx);
                        }
                    }
                    Ok(Err(e)) => {
                        warn!("⚠️  [BATCH-SEND] Write failed at batch GEI {}→{}: {}, re-adding to buffer",
                            first_idx, last_idx, e);
                        *conn_guard = None;
                        self.record_send_failure().await;
                        // Re-add all blocks to buffer
                        let mut buffer = self.send_buffer.lock().await;
                        for (idx, data, epoch, ci) in batch {
                            buffer.insert(idx, (data, epoch, ci));
                        }
                        return Ok(());
                    }
                    Err(_) => {
                        warn!("⏱️  [BATCH-SEND] Timeout sending {} blocks (GEI {}→{})",
                            batch_size, first_idx, last_idx);
                        *conn_guard = None;
                        self.record_send_failure().await;
                        let mut buffer = self.send_buffer.lock().await;
                        for (idx, data, epoch, ci) in batch {
                            buffer.insert(idx, (data, epoch, ci));
                        }
                        return Ok(());
                    }
                }
            } else {
                self.record_send_failure().await;
                // Re-add all blocks
                let mut buffer = self.send_buffer.lock().await;
                for (idx, data, epoch, ci) in batch {
                    buffer.insert(idx, (data, epoch, ci));
                }
                return Err(anyhow::anyhow!("Connection lost during batch send"));
            }
        }

        // Phase 3: Update tracking state (batched — 1 lock per collection)
        {
            let mut next_expected = self.next_expected_index.lock().await;
            *next_expected = last_idx + 1;
        }
        {
            let mut sent = self.sent_indices.lock().await;
            for idx in first_idx..=last_idx {
                sent.insert(idx);
            }
            // Limit memory: trim if too large
            while sent.len() > 10000 {
                if let Some(&min_idx) = sent.iter().min() {
                    sent.remove(&min_idx);
                }
            }
        }

        // Phase 4: Persist — only at intervals or end of batch (not every commit)
        if let Some(ref storage_path) = self.storage_path {
            if last_idx % PERSIST_INTERVAL == 0 || batch_size > 1 {
                if let Err(e) =
                    persist_last_sent_index(storage_path, last_idx, last_commit_index).await
                {
                    warn!(
                        "⚠️ [PERSIST] Failed to persist last_sent_index={}: {}",
                        last_idx, e
                    );
                }
            }

            // Phase 4.5: Store ExecutableBlock bytes for sync peers
            // This allows sync nodes to fetch blocks directly from Rust RocksDB
            // without going through Go PebbleDB.
            {
                let block_refs: Vec<(u64, &[u8])> = batch
                    .iter()
                    .map(|(gei, data, _, _)| (*gei, data.as_slice()))
                    .collect();
                if let Err(e) =
                    super::block_store::store_executable_blocks_batch(storage_path, &block_refs)
                        .await
                {
                    warn!(
                        "⚠️ [BLOCK STORE] Failed to store executable blocks (GEI {}→{}): {}",
                        first_idx, last_idx, e
                    );
                }
            }
        }

        // Phase 5: Go verification (unchanged — periodic check)
        if last_idx % GO_VERIFICATION_INTERVAL == 0 {
            if let Ok(go_last_block) = self.get_last_block_number().await {
                let mut last_verified = self.last_verified_go_index.lock().await;
                if go_last_block < *last_verified {
                    error!("🚨 [FORK DETECTED] Go's block number DECREASED! last_verified={}, go_now={}. CRITICAL: Possible fork or Go state corruption!",
                        *last_verified, go_last_block);
                }
                *last_verified = go_last_block;
                let lag = last_idx.saturating_sub(go_last_block);
                if let Some(ref handle) = self.go_lag_handle {
                    handle.store(lag, std::sync::atomic::Ordering::Relaxed);
                }
                if lag > 100 {
                    warn!(
                        "⚠️ [GO LAG] Go is {} blocks behind Rust. sent={}, go={}",
                        lag, last_idx, go_last_block
                    );
                } else {
                    trace!(
                        "✓ [GO VERIFY] Go verified at block {}. Rust sent={}, lag={}",
                        go_last_block, last_idx, lag
                    );
                }
            }
        }

        // If buffer still has consecutive blocks, recurse to flush remaining
        {
            let buffer = self.send_buffer.lock().await;
            let next_expected = self.next_expected_index.lock().await;
            if buffer.contains_key(&*next_expected) {
                drop(buffer);
                drop(next_expected);
                return Box::pin(self.flush_buffer()).await;
            }
        }

        // Log remaining buffer status
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
    pub async fn send_block_data(
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

    fn convert_to_protobuf_empty(
        &self,
        subdag: &CommittedSubDag,
        epoch: u64,
        global_exec_index: u64,
        leader_address: Option<Vec<u8>>,
    ) -> Result<Vec<u8>> {
        let epoch_data = ExecutableBlock {
            transactions: Vec::new(),
            global_exec_index,
            commit_index: subdag.commit_ref.index as u32,
            epoch,
            commit_timestamp_ms: subdag.timestamp_ms,
            leader_author_index: subdag.leader.author.value() as u32,
            leader_address: leader_address.unwrap_or_default(),
        };

        let mut buf = Vec::new();
        epoch_data.encode(&mut buf)?;
        Ok(buf)
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
        // Extract all transactions with hash for deterministic deduplication and sorting
        // CRITICAL FORK-SAFETY: Deduplicate and sort transactions by hash to ensure all nodes send same order
        let mut all_transactions_with_hash: Vec<(&[u8], Vec<u8>)> = Vec::new(); // (tx_data_ref, tx_hash)
        let mut skipped_count = 0;
        let mut system_tx_skipped = 0;
        let mut protobuf_invalid_skipped = 0;

        for (block_idx, block) in subdag.blocks.iter().enumerate() {
            let total_tx_in_block = block.transactions().len();
            info!("🔍 [DIAG] convert_to_protobuf: block[{}] has {} transactions, global_exec_index={}",
                block_idx, total_tx_in_block, global_exec_index);
            for (tx_idx, tx) in block.transactions().iter().enumerate() {
                // Get transaction data (raw bytes) - Go needs transaction data, not digest
                let tx_data = tx.data();
                use crate::types::tx_hash::calculate_transaction_hash;
                let tx_hash = calculate_transaction_hash(tx_data);
                let tx_hash_hex = hex::encode(&tx_hash[..8.min(tx_hash.len())]);

                info!("🔍 [DIAG] TX[{}/{}]: hash={}..., size={} bytes",
                    tx_idx, total_tx_in_block, tx_hash_hex, tx_data.len());

                // 🔍 FILTER: Check if this is a SystemTransaction (BCS format) - skip if so
                if SystemTransaction::from_bytes(tx_data).is_ok() {
                    info!("⚠️ [SYSTEM TX FILTER] Skipping SystemTransaction (BCS format) in block {} tx {}: hash={}..., size={} bytes",
                        block_idx, tx_idx, tx_hash_hex, tx_data.len());
                    skipped_count += 1;
                    system_tx_skipped += 1;
                    continue;
                }

                // Verify transaction data is valid protobuf before sending to Go
                use crate::types::tx_hash::verify_transaction_protobuf;
                if !verify_transaction_protobuf(tx_data) {
                    info!("⚠️ [TX FILTER] Non-user transaction REJECTED by verify_transaction_protobuf (hash={}..., size={} bytes, global_exec_index={}, commit_index={}). First 32 bytes: {:?}", 
                        tx_hash_hex, tx_data.len(), global_exec_index, subdag.commit_ref.index,
                        &tx_data[..tx_data.len().min(32)]);
                    skipped_count += 1;
                    protobuf_invalid_skipped += 1;
                    continue;
                }

                info!("✅ [DIAG] TX[{}/{}] PASSED all filters: hash={}..., size={} bytes",
                    tx_idx, total_tx_in_block, tx_hash_hex, tx_data.len());
                all_transactions_with_hash.push((tx_data, tx_hash));
            }
        }

        info!("🔍 [DIAG] convert_to_protobuf SUMMARY: global_exec_index={}, total_input={}, passed={}, system_tx_skipped={}, protobuf_invalid_skipped={}",
            global_exec_index, skipped_count + all_transactions_with_hash.len(), all_transactions_with_hash.len(),
            system_tx_skipped, protobuf_invalid_skipped);

        // CRITICAL FORK-SAFETY: Deduplicate and Sort transactions by hash (deterministic ordering)
        // Step 1: Dedup by txHash — keep first occurrence
        let original_len = all_transactions_with_hash.len();
        let mut unique_txs = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for (tx_data, tx_hash) in all_transactions_with_hash {
            if seen.insert(tx_hash.clone()) {
                unique_txs.push((tx_data, tx_hash));
            }
        }
        if unique_txs.len() < original_len {
            trace!("🔍 [FORK-SAFETY] Deduped transactions: {} → {} unique", original_len, unique_txs.len());
        }

        // Step 2: Sort by txHash for deterministic ordering across all nodes
        unique_txs.sort_by(|(_, hash_a), (_, hash_b)| hash_a.cmp(hash_b));

        // Convert to TransactionExe messages after sorting
        let mut transactions = Vec::with_capacity(unique_txs.len());
        for (_sorted_idx, (tx_data_ref, tx_hash)) in unique_txs.iter().enumerate() {
            let tx_hash_hex = hex::encode(&tx_hash[..8.min(tx_hash.len())]);
            trace!(
                "📋 [FORK-SAFETY] Sorted transaction: hash={}",
                tx_hash_hex
            );

            let tx_exe = TransactionExe {
                digest: tx_data_ref.to_vec(),
                worker_id: 0,
            };
            transactions.push(tx_exe);
        }

        // Create ExecutableBlock message using generated protobuf code
        let epoch_data = ExecutableBlock {
            transactions,
            global_exec_index,
            commit_index: subdag.commit_ref.index as u32,
            epoch,
            commit_timestamp_ms: subdag.timestamp_ms,
            leader_author_index: subdag.leader.author.value() as u32,
            leader_address: leader_address.unwrap_or_default(),
        };

        // Encode to protobuf bytes using prost::Message::encode
        let mut buf = Vec::new();
        epoch_data.encode(&mut buf)?;

        let total_tx_encoded = epoch_data.transactions.len();
        info!("📦 [TX INTEGRITY] Encoded ExecutableBlock: global_exec_index={}, commit_index={}, epoch={}, total_tx={}, total_size={} bytes (using protobuf encoding)", 
            global_exec_index, subdag.commit_ref.index, epoch, total_tx_encoded, buf.len());

        Ok(buf)
    }
}
