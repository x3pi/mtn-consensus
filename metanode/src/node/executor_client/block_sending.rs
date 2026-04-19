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

/// Maximum transactions per Go block.
/// When a DAG commit exceeds this threshold, Rust splits it into multiple
/// ExecutableBlock payloads with incrementing global_exec_index values.
/// Go EVM performs best with 5000-10000 TXs per block (balances parallelism
/// vs IntermediateRoot overhead). Splitting at 50000 reduces per-block overhead
/// while keeping GC pressure and EVM contention manageable.
/// FORK-SAFETY: All nodes use the same threshold → deterministic split.
pub const MAX_TXS_PER_GO_BLOCK: usize = 50000;

impl ExecutorClient {
    /// Send committed sub-DAG to executor, with automatic fragmentation for large commits.
    ///
    /// BLOCK FRAGMENTATION: When a commit contains more than MAX_TXS_PER_GO_BLOCK
    /// transactions, it is split into N smaller ExecutableBlock payloads:
    ///   - Fragment 0: global_exec_index=GEI,   TXs[0..5000]
    ///   - Fragment 1: global_exec_index=GEI+1,  TXs[5000..10000]
    ///   - Fragment 2: global_exec_index=GEI+2,  TXs[10000..12479]
    /// Each fragment is sent as a separate block.
    ///
    /// Returns the number of GEI slots consumed (1 for normal, N for fragmented).
    /// The caller (CommitProcessor) must advance its global_exec_index tracking by this amount.
    ///
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
    ) -> Result<u64> {
        if !self.is_enabled() {
            return Ok(1); // Silently skip if not enabled
        }

        // Count total transactions BEFORE conversion (to detect if transactions are lost)
        let total_tx_before: usize = subdag.blocks.iter().map(|b| b.transactions().len()).sum();

        // T2-6: Unified batch_id for cross-process tracing (matches Go format)
        let batch_id = format!(
            "E{}C{}G{}",
            epoch, subdag.commit_ref.index, global_exec_index
        );

        // 🔍 DIAGNOSTIC: Log ALL commits with transactions (not just trace level)
        if total_tx_before > 0 {
            let block_details: Vec<String> = subdag
                .blocks
                .iter()
                .enumerate()
                .map(|(i, b)| {
                    format!(
                        "block[{}]: {} txs, {} bytes each",
                        i,
                        b.transactions().len(),
                        b.transactions()
                            .first()
                            .map(|t| t.data().len())
                            .unwrap_or(0)
                    )
                })
                .collect();
            info!("[batch_id={}] 🔍 [DIAG] send_committed_subdag: total_tx={}, blocks={}, details=[{}]",
                batch_id, total_tx_before, subdag.blocks.len(), block_details.join(", "));
        }

        // REPLAY PROTECTION: Discard blocks that are already processed
        // This is critical when Consensus replays old commits on restart
        {
            let next_expected = self.next_expected_index.lock().await;
            if global_exec_index < *next_expected {
                // Only log periodically or for non-empty blocks to avoid noise during replay
                if total_tx_before > 0 || global_exec_index.is_multiple_of(1000) {
                    info!(
                        "♻️ [REPLAY] Discarding already processed block: global={}, expected={}",
                        global_exec_index, *next_expected
                    );
                }
                return Ok(1);
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
                return Ok(1); // Already sent, skip
            }
            // Don't insert yet - insert only after successful send
        }

        // ═══════════════════════════════════════════════════════════════
        // BLOCK FRAGMENTATION: If commit has more TXs than MAX_TXS_PER_GO_BLOCK,
        // split into N smaller ExecutableBlock payloads.
        // CRITICAL FORK-SAFETY: All nodes use the same threshold and same
        // transaction order → deterministic split → identical GEI mapping.
        // ═══════════════════════════════════════════════════════════════
        if total_tx_before > MAX_TXS_PER_GO_BLOCK {
            let num_fragments = total_tx_before.div_ceil(MAX_TXS_PER_GO_BLOCK);
            info!("🔪 [FRAGMENT] Splitting large commit: {} TXs → {} fragments of ≤{} TXs each (global_exec_index={}, commit_index={}, epoch={})",
                total_tx_before, num_fragments, MAX_TXS_PER_GO_BLOCK, global_exec_index, subdag.commit_ref.index, epoch);

            // Build the full sorted+deduped transaction list first
            let all_proto_txs = self.build_sorted_transactions(subdag)?;
            let total_after_dedup = all_proto_txs.len();

            if total_after_dedup == 0 {
                // All TXs were filtered out — send as empty commit
                let block_number = {
                    let mut next_bn = self.next_block_number.lock().await;
                    let mut last_ep = self.last_processed_epoch.lock().await;
                    let is_epoch_boundary = epoch > *last_ep;
                    if epoch > *last_ep {
                        *last_ep = epoch;
                    }
                    if is_epoch_boundary {
                        let bn = *next_bn;
                        *next_bn += 1;
                        bn
                    } else {
                        0
                    }
                };

                let empty_bytes = self.convert_to_protobuf_empty(
                    subdag,
                    epoch,
                    global_exec_index,
                    leader_address,
                    block_number,
                )?;
                self.buffer_and_flush(
                    global_exec_index,
                    empty_bytes,
                    epoch,
                    subdag.commit_ref.index,
                    0,
                )
                .await?;
                return Ok(1);
            }

            // Recalculate fragments after dedup
            let actual_fragments = total_after_dedup.div_ceil(MAX_TXS_PER_GO_BLOCK);

            for frag_idx in 0..actual_fragments {
                let start = frag_idx * MAX_TXS_PER_GO_BLOCK;
                let end = std::cmp::min(start + MAX_TXS_PER_GO_BLOCK, total_after_dedup);
                let fragment_txs: Vec<TransactionExe> = all_proto_txs[start..end].to_vec();
                let fragment_gei = global_exec_index + frag_idx as u64;

                let block_number = {
                    let mut next_bn = self.next_block_number.lock().await;
                    let mut last_ep = self.last_processed_epoch.lock().await;
                    // Fragment with txs > 0 ALWAYS consumes a block number
                    if epoch > *last_ep {
                        *last_ep = epoch;
                    }
                    // Since fragment total_tx_before > MAX_TXS_PER_GO_BLOCK, it definitely has txs > 0
                    // unless somehow after dedup all fragments have 0 txs, which is handled above.
                    // So we always allocate a block number.
                    let bn = *next_bn;
                    *next_bn += 1;
                    bn
                };

                let epoch_data = ExecutableBlock {
                    transactions: fragment_txs,
                    global_exec_index: fragment_gei,
                    commit_index: subdag.commit_ref.index,
                    epoch,
                    commit_timestamp_ms: subdag.timestamp_ms,
                    leader_author_index: subdag.leader.author.value() as u32,
                    leader_address: leader_address.clone().unwrap_or_default(),
                    block_number,
                };

                let tx_count = epoch_data.transactions.len();
                let mut buf = Vec::new();
                epoch_data.encode(&mut buf)?;

                info!(
                    "🔪 [FRAGMENT {}/{}] GEI={}, TXs={}, size={} bytes",
                    frag_idx + 1,
                    actual_fragments,
                    fragment_gei,
                    tx_count,
                    buf.len()
                );

                self.buffer_and_flush(fragment_gei, buf, epoch, subdag.commit_ref.index, tx_count)
                    .await?;
            }

            info!(
                "✅ [FRAGMENT] Completed fragmentation: {} TXs → {} blocks (GEI {}→{})",
                total_after_dedup,
                actual_fragments,
                global_exec_index,
                global_exec_index + actual_fragments as u64 - 1
            );

            return Ok(actual_fragments as u64);
        }

        // ═══════════════════════════════════════════════════════════════
        // NORMAL PATH: Commit fits within MAX_TXS_PER_GO_BLOCK
        // ═══════════════════════════════════════════════════════════════

        let block_number = {
            let mut next_bn = self.next_block_number.lock().await;
            let mut last_ep = self.last_processed_epoch.lock().await;
            let is_epoch_boundary = epoch > *last_ep;
            if epoch > *last_ep {
                *last_ep = epoch;
            }
            if total_tx_before > 0 || is_epoch_boundary {
                let bn = *next_bn;
                *next_bn += 1;
                bn
            } else {
                0
            }
        };

        // Convert CommittedSubDag to protobuf CommittedEpochData
        // OPTIMIZATION: For empty commits (0 transactions), use fast-path that bypasses
        // the expensive per-tx hash/filter/sort logic in convert_to_protobuf
        let epoch_data_bytes = if total_tx_before == 0 {
            self.convert_to_protobuf_empty(
                subdag,
                epoch,
                global_exec_index,
                leader_address,
                block_number,
            )?
        } else {
            info!("🔍 [DIAG] Using FULL convert_to_protobuf path for global_exec_index={} (total_tx_before={})",
                global_exec_index, total_tx_before);
            self.convert_to_protobuf(
                subdag,
                epoch,
                global_exec_index,
                leader_address,
                block_number,
            )?
        };

        // Count total transactions after conversion (should match before)
        let total_tx: usize = total_tx_before;

        self.buffer_and_flush(
            global_exec_index,
            epoch_data_bytes,
            epoch,
            subdag.commit_ref.index,
            total_tx,
        )
        .await?;

        Ok(1) // Normal commit consumes 1 GEI
    }

    /// Buffer an ExecutableBlock and flush.
    /// This is the shared logic extracted from send_committed_subdag to support fragmentation.
    async fn buffer_and_flush(
        &self,
        global_exec_index: u64,
        epoch_data_bytes: Vec<u8>,
        epoch: u64,
        commit_index: u32,
        total_tx: usize,
    ) -> Result<()> {
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
                    commit_index,
                    epoch_data_bytes.len(),
                    total_tx
                );

                let is_same_commit = existing_epoch == epoch && existing_commit == commit_index;

                if is_same_commit {
                    warn!("   ✅ Same commit detected (epoch={}, commit_index={}) - skipping duplicate, existing commit in buffer will be sent", epoch, commit_index);
                    return Ok(());
                } else {
                    error!("   🚨 DIFFERENT commits with same global_exec_index! This is a BUG!");
                    error!("   🔍 Root cause analysis:");
                    error!("      - Epochs different ({} vs {}): global_exec_index calculation may be wrong", existing_epoch, epoch);
                    error!("      - Commit indexes different ({} vs {}): same global_exec_index calculated for different commits", existing_commit, commit_index);
                    error!("      - This indicates last_global_exec_index was not updated correctly or calculation is wrong");
                    warn!("   ⚠️  OVERWRITING existing commit with new commit to ensure transactions are executed");
                    warn!("   ⚠️  This may cause transaction loss in the old commit, but ensures new transactions are executed");
                }
            }
            buffer.insert(global_exec_index, (epoch_data_bytes, epoch, commit_index));
            info!("[batch_id=E{}C{}G{}] 📦 [SEQUENTIAL-BUFFER] Added block: total_tx={}, buffer_size={}",
                epoch, commit_index, global_exec_index, total_tx, buffer.len());
        }

        // CRITICAL: Flush buffer iteratively after adding commit.
        // Loop ensures we drain all consecutive blocks without recursion (C5 fix).
        loop {
            if let Err(e) = self.flush_buffer().await {
                warn!(
                    "⚠️  [SEQUENTIAL-BUFFER] Failed to flush buffer after adding commit: {}",
                    e
                );
                break;
            }
            // Check if there are more consecutive blocks to flush
            let has_more = {
                let buffer = self.send_buffer.lock().await;
                let next_expected = self.next_expected_index.lock().await;
                buffer.contains_key(&*next_expected)
            };
            if !has_more {
                break;
            }
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
                        } else {
                            // Go is BEHIND — likely after restore + SyncOnly→Validator transition.
                            // The buffer has consensus blocks far ahead of Go. The empty GEIs
                            // between Go and the buffer are consensus rounds that were never
                            // captured (sync stopped before consensus started).
                            // SAFE: advance next_expected to min_buffered so flush_buffer
                            // can start sending. Go will receive blocks in sequential GEI
                            // order from flush_buffer (BTreeMap + sequential iteration).
                            let buffer = self.send_buffer.lock().await;
                            let min_buf = *buffer.keys().next().unwrap_or(&0);
                            if min_buf > *next_expected_guard {
                                warn!("🚀 [RESTORE-GAP-BRIDGE] Go is behind (gei={}), buffer starts at {}. Advancing next_expected {} → {} to bridge transition gap",
                                    go_last_gei, min_buf, *next_expected_guard, min_buf);
                                *next_expected_guard = min_buf;
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

        // Phase 2: Write all blocks to FFI in a single batch (1 flush)
        {
            let mut sent_count = 0usize;
            if let Some(c_fn) = crate::ffi::GO_CALLBACKS.get().and_then(|c| c.execute_block) {
                for (idx, data, _, _) in &batch {
                    let success = c_fn(data.as_ptr(), data.len());
                    if success {
                        sent_count += 1;
                    } else {
                        warn!("⚠️  [BLOCK-SEND] FFI execute_block failed at GEI={}", idx);
                        self.record_send_failure().await;
                        break;
                    }
                }
            } else {
                warn!("⚠️  [BLOCK-SEND] FFI execute_block not registered");
                self.record_send_failure().await;
            }

            if sent_count > 0 {
                self.record_send_success().await;
                if batch_size > 1 && sent_count == batch_size {
                    info!(
                        "[batch_id=G{}..{}] ⚡ [BATCH-SEND] Sent {} blocks sequentially to Go FFI",
                        first_idx, last_idx, batch_size
                    );
                }
            }

            // Re-add unsent blocks back to buffer
            if sent_count < batch_size {
                let mut buffer = self.send_buffer.lock().await;
                for (idx, data, epoch, ci) in batch.iter().skip(sent_count) {
                    buffer.insert(*idx, (data.clone(), *epoch, *ci));
                }
                warn!(
                    "🔄 [BLOCK-SEND] Re-buffered {} unsent blocks (sent {}/{})",
                    batch_size - sent_count,
                    sent_count,
                    batch_size
                );
                if sent_count == 0 {
                    return Ok(());
                }
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
            // H2 FIX: Fast removal using BTreeSet pop_first() (O(log N))
            // instead of O(N) iteration in HashSet, which caused O(N^2) loop.
            while sent.len() > 10000 {
                sent.pop_first();
            }
        }

        // Phase 4: Persist — only at intervals or end of batch (not every commit)
        if let Some(ref storage_path) = self.storage_path {
            if last_idx.is_multiple_of(PERSIST_INTERVAL) || batch_size > 1 {
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

        // T2-2: Immediate lag estimate from buffer size (no RPC needed)
        // This runs after every flush — provides near-real-time feedback to
        // SystemTransactionProvider via go_lag_handle, even between GO_VERIFICATION_INTERVAL checks.
        {
            let buffer = self.send_buffer.lock().await;
            let buffer_lag = buffer.len() as u64;
            if let Some(ref handle) = self.go_lag_handle {
                // Use buffer size as minimum lag estimate — actual lag may be higher
                // (Go may be further behind), but buffer_lag is available immediately
                let current_lag = handle.load(std::sync::atomic::Ordering::Relaxed);
                if buffer_lag > current_lag {
                    handle.store(buffer_lag, std::sync::atomic::Ordering::Relaxed);
                }
            }
        }

        // Phase 5: Go verification (periodic RPC check)
        if last_idx.is_multiple_of(GO_VERIFICATION_INTERVAL) {
            if let Ok((go_last_block, _, _)) = self.get_last_block_number().await {
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
                        go_last_block,
                        last_idx,
                        lag
                    );
                }
            }
        }

        // ═══════════════════════════════════════════════════════════════════
        // STABILITY FIX (C5): Iterative flush instead of recursive Box::pin.
        // Previously: `Box::pin(self.flush_buffer()).await` — each recursion
        // allocates a new pinned Future on the heap. With 50K+ buffered blocks
        // (snapshot restore), this could exhaust memory or stack.
        // Now: signal the outer loop to continue flushing.
        // ═══════════════════════════════════════════════════════════════════
        {
            let buffer = self.send_buffer.lock().await;
            let next_expected = self.next_expected_index.lock().await;
            if buffer.contains_key(&*next_expected) {
                drop(buffer);
                drop(next_expected);
                // Signal caller to re-invoke flush_buffer (handled by flush_buffer_loop)
                return Ok(());
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
    #[allow(dead_code)]
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
            self.convert_to_protobuf(subdag, epoch, global_exec_index, leader_address, 0)?;

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
    #[allow(dead_code)]
    pub async fn send_block_data(
        &self,
        epoch_data_bytes: &[u8],
        global_exec_index: u64,
        epoch: u64,
        commit_index: u32,
    ) -> Result<()> {
        // 🛡️ CIRCUIT BREAKER: Check if we are in Fast-Fail mode
        self.check_send_circuit_breaker().await?;

        // FFI INTEGRATION: Send directly to Go via CGo callback
        if let Some(c_fn) = crate::ffi::GO_CALLBACKS.get().and_then(|c| c.execute_block) {
            let success = c_fn(epoch_data_bytes.as_ptr(), epoch_data_bytes.len());
            if success {
                trace!("📤 [TX FLOW] Sent committed sub-DAG to Go executor via FFI: global_exec_index={}, commit_index={}, epoch={}, data_size={} bytes", 
                    global_exec_index, commit_index, epoch, epoch_data_bytes.len());
                self.record_send_success().await; // ✅ Mark Success
                Ok(())
            } else {
                warn!(
                    "⚠️  [EXECUTOR] Go FFI execute_block failed for global_exec_index={}",
                    global_exec_index
                );
                self.record_send_failure().await; // 🚨 Mark Failure
                Err(anyhow::anyhow!("Go FFI execute_block returned false"))
            }
        } else {
            warn!("⚠️  [EXECUTOR] FFI GO_CALLBACKS not registered or execute_block is null");
            self.record_send_failure().await;
            Err(anyhow::anyhow!("FFI execute_block not registered"))
        }
    }

    fn convert_to_protobuf_empty(
        &self,
        subdag: &CommittedSubDag,
        epoch: u64,
        global_exec_index: u64,
        leader_address: Option<Vec<u8>>,
        block_number: u64,
    ) -> Result<Vec<u8>> {
        let epoch_data = ExecutableBlock {
            transactions: Vec::new(),
            global_exec_index,
            commit_index: subdag.commit_ref.index,
            epoch,
            commit_timestamp_ms: subdag.timestamp_ms,
            leader_author_index: subdag.leader.author.value() as u32,
            leader_address: leader_address.unwrap_or_default(),
            block_number,
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
        block_number: u64,
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
                // tx.data() là single Transaction bytes từ DAG — không cần heuristic
                use crate::types::tx_hash::calculate_transaction_hash_single;
                let tx_hash = calculate_transaction_hash_single(tx_data);
                let tx_hash_hex = hex::encode(&tx_hash[..8.min(tx_hash.len())]);

                info!(
                    "🔍 [DIAG] TX[{}/{}]: hash={}..., size={} bytes",
                    tx_idx,
                    total_tx_in_block,
                    tx_hash_hex,
                    tx_data.len()
                );

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

                info!(
                    "✅ [DIAG] TX[{}/{}] PASSED all filters: hash={}..., size={} bytes",
                    tx_idx,
                    total_tx_in_block,
                    tx_hash_hex,
                    tx_data.len()
                );
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
            trace!(
                "🔍 [FORK-SAFETY] Deduped transactions: {} → {} unique",
                original_len,
                unique_txs.len()
            );
        }

        // Step 2: Sort by txHash for deterministic ordering across all nodes
        unique_txs.sort_by(|(_, hash_a), (_, hash_b)| hash_a.cmp(hash_b));

        // Convert to TransactionExe messages after sorting
        let mut transactions = Vec::with_capacity(unique_txs.len());
        for (tx_data_ref, tx_hash) in unique_txs.iter() {
            let tx_hash_hex = hex::encode(&tx_hash[..8.min(tx_hash.len())]);
            trace!("📋 [FORK-SAFETY] Sorted transaction: hash={}", tx_hash_hex);

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
            commit_index: subdag.commit_ref.index,
            epoch,
            commit_timestamp_ms: subdag.timestamp_ms,
            leader_author_index: subdag.leader.author.value() as u32,
            leader_address: leader_address.unwrap_or_default(),
            block_number,
        };

        // Encode to protobuf bytes using prost::Message::encode
        let mut buf = Vec::new();
        epoch_data.encode(&mut buf)?;

        let total_tx_encoded = epoch_data.transactions.len();
        info!("📦 [TX INTEGRITY] Encoded ExecutableBlock: global_exec_index={}, commit_index={}, epoch={}, total_tx={}, total_size={} bytes (using protobuf encoding)", 
            global_exec_index, subdag.commit_ref.index, epoch, total_tx_encoded, buf.len());

        Ok(buf)
    }

    /// Build sorted, deduplicated TransactionExe list from a CommittedSubDag.
    ///
    /// This extracts the filter → dedup → sort logic from convert_to_protobuf
    /// so it can be reused by the fragmentation path. Returns Vec<TransactionExe>
    /// in deterministic order (sorted by tx hash).
    fn build_sorted_transactions(&self, subdag: &CommittedSubDag) -> Result<Vec<TransactionExe>> {
        use crate::types::tx_hash::{
            calculate_transaction_hash_single, verify_transaction_protobuf,
        };

        let mut all_transactions_with_hash: Vec<(&[u8], Vec<u8>)> = Vec::new();
        let mut skipped_count = 0;

        for (block_idx, block) in subdag.blocks.iter().enumerate() {
            for (tx_idx, tx) in block.transactions().iter().enumerate() {
                let tx_data = tx.data();
                let tx_hash = calculate_transaction_hash_single(tx_data);

                // Filter: Skip SystemTransaction (BCS format)
                if SystemTransaction::from_bytes(tx_data).is_ok() {
                    skipped_count += 1;
                    continue;
                }

                // Filter: Skip non-protobuf transactions
                if !verify_transaction_protobuf(tx_data) {
                    let tx_hash_hex = hex::encode(&tx_hash[..8.min(tx_hash.len())]);
                    trace!("⚠️ [FRAGMENT-FILTER] Skipping non-protobuf tx in block {} tx {}: hash={}...", block_idx, tx_idx, tx_hash_hex);
                    skipped_count += 1;
                    continue;
                }

                all_transactions_with_hash.push((tx_data, tx_hash));
            }
        }

        // Dedup by txHash
        let original_len = all_transactions_with_hash.len();
        let mut unique_txs = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for (tx_data, tx_hash) in all_transactions_with_hash {
            if seen.insert(tx_hash.clone()) {
                unique_txs.push((tx_data, tx_hash));
            }
        }
        let dedup_removed = original_len - unique_txs.len();

        // Sort by txHash for deterministic ordering
        unique_txs.sort_by(|(_, hash_a), (_, hash_b)| hash_a.cmp(hash_b));

        // Convert to TransactionExe
        let transactions: Vec<TransactionExe> = unique_txs
            .iter()
            .map(|(tx_data_ref, _)| TransactionExe {
                digest: tx_data_ref.to_vec(),
                worker_id: 0,
            })
            .collect();

        info!(
            "🔪 [FRAGMENT-BUILD] Built sorted TX list: input={}, filtered={}, deduped={}, final={}",
            original_len + skipped_count,
            skipped_count,
            dedup_removed,
            transactions.len()
        );

        Ok(transactions)
    }
}
