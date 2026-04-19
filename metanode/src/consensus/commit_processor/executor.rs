// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

use anyhow::Result;
use consensus_core::{BlockAPI, CommittedSubDag};
use std::sync::Arc;
use tokio::time::{sleep, Duration};
use tracing::{error, info, trace, warn};

/// T2-5: Bounded semaphore for deferred TX tracking and persistence tasks.
/// Prevents unbounded tokio::spawn accumulation under extreme commit rates
/// (e.g., 10K+ commits/sec during epoch transitions or burst load).
/// 64 permits = practical upper bound; exceeding this drops the task with a warning.
static DEFERRED_TASK_SEMAPHORE: std::sync::LazyLock<Arc<tokio::sync::Semaphore>> =
    std::sync::LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(64)));

use crate::node::executor_client::ExecutorClient;

pub async fn dispatch_commit(
    subdag: &CommittedSubDag,
    global_exec_index: u64,
    epoch: u64,
    executor_client: Option<Arc<ExecutorClient>>,
    pending_transactions_queue: Option<Arc<tokio::sync::Mutex<Vec<Vec<u8>>>>>,
    shared_last_global_exec_index: Option<Arc<tokio::sync::Mutex<u64>>>,
    validator_eth_addresses: Arc<tokio::sync::RwLock<std::collections::HashMap<u64, Vec<Vec<u8>>>>>,
) -> Result<u64> {
    let commit_index = subdag.commit_ref.index;
    let mut total_transactions = 0;

    for block in subdag.blocks.iter() {
        total_transactions += block.transactions().len();
    }

    let has_system_tx = subdag.extract_end_of_epoch_transaction().is_some();

    // CC-1: Unified batch_id for end-to-end tracing
    let batch_id = format!("E{}C{}G{}", epoch, commit_index, global_exec_index);

    // ═══════════════════════════════════════════════════════════════════════════════
    // 🛡️ RUST-DRIVEN LEADER SELECTION (Critical Fork-Safety)
    // Calculate leader_address for ALL commits (empty or not)
    // NEVER send None - if we can't determine leader, BLOCK/PANIC
    // ═══════════════════════════════════════════════════════════════════════════════
    let leader_address: Option<Vec<u8>> = if executor_client.is_some() {
        let leader_author_index = subdag.leader.author.value();

        // STEP 1: Validate committee data exists (with retry for startup race condition)
        let mut retry_attempts = 0;
        let max_retries = 10; // 10 * 200ms = 2 seconds max wait

        let resolved_address = loop {
            let epoch_addresses = validator_eth_addresses.read().await;

            // Check if committee HashMap is loaded
            if epoch_addresses.is_empty() {
                drop(epoch_addresses);
                retry_attempts += 1;
                if retry_attempts > max_retries {
                    error!("🚨 [FATAL] epoch_eth_addresses STILL EMPTY after {} retries! Committee not loaded.", max_retries);
                    error!("🚨 [FATAL] Cannot process commit #{} (global_exec_index={}) without valid committee data!", 
                            commit_index, global_exec_index);
                    anyhow::bail!(
                            "Committee data empty after {} retries — cannot process commit #{} (GEI={}). Node requires restart.",
                            max_retries, commit_index, global_exec_index
                        );
                }
                warn!(
                    "⏳ [LEADER] epoch_eth_addresses empty, waiting for committee... retry {}/{}",
                    retry_attempts, max_retries
                );
                sleep(Duration::from_millis(200)).await;
                continue;
            }

            // Try to get committee for commit's epoch, with fallback to current or previous epoch
            let eth_addresses = if let Some(addrs) = epoch_addresses.get(&epoch) {
                addrs
            } else if epoch > 0 {
                // Try previous epoch (common during transition)
                if let Some(addrs) = epoch_addresses.get(&(epoch - 1)) {
                    warn!("⚠️ [LEADER] Using epoch {} committee for commit from epoch {} (during transition)",
                            epoch - 1, epoch);
                    addrs
                } else {
                    // Last resort: use any available epoch
                    if let Some((available_epoch, addrs)) = epoch_addresses.iter().next() {
                        warn!("⚠️ [LEADER] Using epoch {} committee for commit from epoch {} (only available)",
                                available_epoch, epoch);
                        addrs
                    } else {
                        error!("🚨 [FATAL] No committees available in cache!");
                        anyhow::bail!("No committee data available in cache — cannot determine leader for epoch {}.", epoch);
                    }
                }
            } else {
                // epoch == 0 but not found - use any available
                if let Some((available_epoch, addrs)) = epoch_addresses.iter().next() {
                    warn!(
                        "⚠️ [LEADER] Using epoch {} committee for commit from epoch 0",
                        available_epoch
                    );
                    addrs
                } else {
                    error!("🚨 [FATAL] No committees available in cache!");
                    anyhow::bail!(
                        "No committee data available in cache (epoch 0) — cannot determine leader."
                    );
                }
            };

            // STEP 2: Validate leader index is in range
            let committee_size = eth_addresses.len();
            if leader_author_index >= committee_size {
                // SELF-RECOVERY: Instead of panic, try to refresh the cache
                drop(epoch_addresses); // Release lock before refresh

                retry_attempts += 1;
                if retry_attempts > max_retries {
                    error!(
                        "🚨 [FATAL] leader_author_index {} >= committee_size {} after {} retries!",
                        leader_author_index, committee_size, max_retries
                    );
                    error!("🚨 [FATAL] Committee size mismatch - expected at least {} validators but have {}!",
                            leader_author_index + 1, committee_size);
                    anyhow::bail!(
                            "Committee size mismatch: leader_index {} >= committee_size {} after {} retries for epoch {}.",
                            leader_author_index, committee_size, max_retries, epoch
                        );
                }

                warn!(
                        "⚠️ [LEADER] leader_index {} >= committee_size {} for epoch {}. Refreshing cache... retry {}/{}",
                        leader_author_index, committee_size, epoch, retry_attempts, max_retries
                    );

                // Try to refresh epoch_eth_addresses from Go
                if let Some(ref client) = executor_client {
                    match client.get_epoch_boundary_data(epoch).await {
                        Ok((returned_epoch, _ts, _boundary, validators, _, _))
                            if returned_epoch == epoch =>
                        {
                            // Sort validators same way as committee builder
                            let mut sorted_validators: Vec<_> = validators.into_iter().collect();
                            sorted_validators.sort_by(|a, b| a.authority_key.cmp(&b.authority_key));

                            let mut new_eth_addresses = Vec::new();
                            for validator in &sorted_validators {
                                let eth_addr_bytes = if validator.address.starts_with("0x")
                                    && validator.address.len() == 42
                                {
                                    match hex::decode(&validator.address[2..]) {
                                        Ok(bytes) if bytes.len() == 20 => bytes,
                                        _ => vec![],
                                    }
                                } else {
                                    vec![]
                                };
                                new_eth_addresses.push(eth_addr_bytes);
                            }

                            // Update the cache
                            let mut cache = validator_eth_addresses.write().await;
                            cache.insert(epoch, new_eth_addresses);
                            info!(
                                    "🔄 [LEADER] Refreshed epoch_eth_addresses for epoch {}: now have {} validators (cache size: {})",
                                    epoch, sorted_validators.len(), cache.len()
                                );
                        }
                        Ok((returned_epoch, _, _, _, _, _)) => {
                            warn!(
                                    "⚠️ [LEADER] Go returned epoch {} but requested epoch {}. Retrying...",
                                    returned_epoch, epoch
                                );
                        }
                        Err(e) => {
                            warn!("⚠️ [LEADER] Failed to refresh epoch_eth_addresses: {}. Retrying...", e);
                        }
                    }
                }

                sleep(Duration::from_millis(500)).await;
                continue; // Retry the whole loop
            }

            // STEP 3: Validate ETH address is valid (20 bytes)
            let addr = eth_addresses[leader_author_index].clone();
            if addr.len() != 20 {
                // SELF-RECOVERY: Try to refresh for invalid address too
                drop(epoch_addresses);

                retry_attempts += 1;
                if retry_attempts > max_retries {
                    error!(
                            "🚨 [FATAL] eth_address at index {} has invalid length {} (expected 20) after {} retries!",
                            leader_author_index, addr.len(), max_retries
                        );
                    anyhow::bail!(
                            "Invalid ETH address length {} at index {} after {} retries — committee data corrupted.",
                            addr.len(), leader_author_index, max_retries
                        );
                }

                warn!(
                        "⚠️ [LEADER] Invalid eth_address length at index {}. Refreshing cache... retry {}/{}",
                        leader_author_index, retry_attempts, max_retries
                    );
                sleep(Duration::from_millis(500)).await;
                continue;
            }

            // SUCCESS: Valid leader address found
            if total_transactions > 0 || has_system_tx {
                info!(
                    "✅ [LEADER] Resolved leader for commit #{} (epoch {}): index={} -> 0x{}",
                    commit_index,
                    epoch,
                    leader_author_index,
                    hex::encode(&addr)
                );
            }
            break Some(addr);
        };

        resolved_address
    } else {
        None // No executor client = no need for leader address
    };

    if total_transactions > 0 || has_system_tx {
        trace!(
                "🔷 [batch_id={}] [Global Index: {}] Executing commit #{} (epoch={}): {} blocks, {} txs, has_system_tx={}",
                batch_id, global_exec_index, commit_index, epoch, subdag.blocks.len(), total_transactions, has_system_tx
            );
    } else {
        // Still log empty commits but as trace/debug to avoid spam
        tracing::trace!(
                "⏭️ [batch_id={}] [TX FLOW] Forwarding empty commit to Go Master (for sequence sync): global_exec_index={}, commit_index={}",
                batch_id, global_exec_index, commit_index
            );
    }

    // ═══════════════════════════════════════════════════════════════════
    // GEI GUARD: Skip commits that Go has already executed.
    //
    // Since Phase 1 (sync_and_execute_blocks), Go's GEI is ALWAYS accurate
    // (reflects actually-executed state, never inflated). This single path
    // handles all deduplication correctly — no cold_start guard needed.
    //
    // EndOfEpoch commits always pass through for epoch transition safety.
    // ═══════════════════════════════════════════════════════════════════
    if let Some(ref client) = executor_client {
        let go_current_gei = client.get_last_global_exec_index().await.unwrap_or(0);
        if go_current_gei >= global_exec_index && global_exec_index > 0 {
            let has_end_of_epoch = subdag.extract_end_of_epoch_transaction().is_some();
            if !has_end_of_epoch {
                info!(
                    "⏭️ [GEI GUARD] Skipping commit #{}: Go GEI={} >= commit GEI={}.",
                    commit_index, go_current_gei, global_exec_index
                );
                return Ok(1);
            } else {
                info!(
                    "⚠️ [GEI GUARD] Go GEI={} >= commit GEI={}, but commit #{} \
                         contains EndOfEpoch — processing for epoch transition safety.",
                    go_current_gei, global_exec_index, commit_index
                );
            }
        }

        // ═══════════════════════════════════════════════════════════════
        // GEI GAP GUARD: Block sending when commit is WAY ahead of Go.
        //
        // After snapshot restore, the DAG cold-starts and the first
        // commit may have GEI=2342 while Go is only at GEI=1575. Sending
        // this immediately would cause Go to skip ~767 GEIs, resulting
        // in out-of-order processing and stalled execution.
        //
        // Instead, wait for SYNC-FIRST GEI sync (or epoch transition)
        // to fill the gap. The buffered sender's RESTORE-GAP-BRIDGE
        // logic handles the actual Go-side advancement.
        //
        // Max wait: 60s (300 * 200ms). EndOfEpoch commits bypass.
        // ═══════════════════════════════════════════════════════════════
        const MAX_ALLOWED_GEI_GAP: u64 = 200;
        const GAP_WAIT_INTERVAL_MS: u64 = 200;
        const MAX_GAP_WAIT_ATTEMPTS: u64 = 300; // 60 seconds

        if global_exec_index > go_current_gei + MAX_ALLOWED_GEI_GAP
            && go_current_gei > 0
            && subdag.extract_end_of_epoch_transaction().is_none()
        {
            info!(
                    "⏳ [GEI GAP GUARD] Commit GEI={} is {} ahead of Go GEI={}. Waiting for gap to close...",
                    global_exec_index, global_exec_index - go_current_gei, go_current_gei
                );

            for attempt in 0..MAX_GAP_WAIT_ATTEMPTS {
                tokio::time::sleep(tokio::time::Duration::from_millis(GAP_WAIT_INTERVAL_MS)).await;
                let current_go_gei = client.get_last_global_exec_index().await.unwrap_or(0);

                if global_exec_index <= current_go_gei + MAX_ALLOWED_GEI_GAP {
                    info!(
                            "✅ [GEI GAP GUARD] Gap closed! Go GEI={}, commit GEI={}, gap={}. Proceeding after {}ms.",
                            current_go_gei, global_exec_index,
                            global_exec_index.saturating_sub(current_go_gei),
                            (attempt + 1) * GAP_WAIT_INTERVAL_MS
                        );
                    break;
                }

                // Re-check if Go already processed this commit
                if current_go_gei >= global_exec_index {
                    info!(
                            "⏭️ [GEI GAP GUARD] Go caught up past commit: Go GEI={} >= commit GEI={}. Skipping.",
                            current_go_gei, global_exec_index
                        );
                    return Ok(1);
                }

                if attempt % 25 == 0 {
                    warn!(
                            "⏳ [GEI GAP GUARD] Still waiting: commit GEI={}, Go GEI={}, gap={}, elapsed={}s",
                            global_exec_index, current_go_gei,
                            global_exec_index.saturating_sub(current_go_gei),
                            (attempt + 1) * GAP_WAIT_INTERVAL_MS / 1000
                        );
                }
            }
        }
    }

    if let Some(ref client) = executor_client {
        // leader_address already calculated and validated above

        let mut retry_count = 0;
        let geis_consumed: u64 = loop {
            match client
                .send_committed_subdag(subdag, epoch, global_exec_index, leader_address.clone())
                .await
            {
                Ok(geis_consumed) => {
                    trace!("✅ [batch_id={}] [TX FLOW] Successfully sent committed subdag: global_exec_index={}, commit_index={}, geis_consumed={}",
                                batch_id, global_exec_index, commit_index, geis_consumed);

                    // Update shared_last_global_exec_index to the LAST GEI of the fragment range
                    let last_gei = global_exec_index + geis_consumed - 1;
                    if let Some(shared_index) = shared_last_global_exec_index.clone() {
                        let mut index_guard = shared_index.lock().await;
                        *index_guard = last_gei;
                        info!("📊 [GLOBAL_EXEC_INDEX] Updated shared last_global_exec_index to {} after successful send (geis_consumed={})", last_gei, geis_consumed);
                    }

                    // Track lag every 100 commits
                    if commit_index % 100 == 0 {
                        if let Ok(go_gei) = client.get_last_global_exec_index().await {
                            let lag = global_exec_index.saturating_sub(go_gei);
                            if lag > 500 {
                                tracing::warn!(
                                    "⚠️ [EXEC-LAG] Rust GEI={} vs Go GEI={} — gap={} blocks",
                                    global_exec_index,
                                    go_gei,
                                    lag
                                );
                            }
                        }
                    }

                    // Track committed transaction hashes to prevent duplicates during epoch transitions
                    // CRITICAL: Only track when commit is actually processed, not just submitted
                    //
                    // FIX C1: When try_lock() fails (transition handler holds node lock),
                    // spawn a deferred task to retry tracking after a short backoff.
                    // Previously, we simply skipped tracking, which could cause tx_recycler
                    // to resubmit already-committed TXs in the new epoch.
                    if let Some(node_arc) = crate::node::get_transition_handler_node().await {
                        // Use try_lock() instead of lock().await to avoid blocking
                        let node_guard = match node_arc.try_lock() {
                            Ok(guard) => guard,
                            Err(_) => {
                                // Lock held by transition handler — defer tracking to a background task
                                trace!("⏭️ [TX TRACKING] Deferring tracking for commit {} — node lock held by transition handler", commit_index);
                                let node_arc_clone = node_arc.clone();
                                let subdag_blocks: Vec<Vec<Vec<u8>>> = subdag
                                    .blocks
                                    .iter()
                                    .map(|b| {
                                        b.transactions()
                                            .iter()
                                            .map(|tx| tx.data().to_vec())
                                            .collect()
                                    })
                                    .collect();
                                let deferred_commit_index = commit_index;
                                // T2-5: Bounded deferred task — acquire semaphore permit before spawning
                                let sem = DEFERRED_TASK_SEMAPHORE.clone();
                                match sem.try_acquire_owned() {
                                    Ok(permit) => {
                                        tokio::spawn(async move {
                                            let _permit = permit; // held until task completes
                                                                  // Wait for transition handler to release lock
                                            tokio::time::sleep(Duration::from_millis(500)).await;
                                            if let Ok(guard) = node_arc_clone.try_lock() {
                                                let mut hashes_guard =
                                                    guard.committed_transaction_hashes.lock().await;
                                                let mut count = 0;
                                                for block_txs in &subdag_blocks {
                                                    for tx_data in block_txs {
                                                        let tx_hash = crate::types::tx_hash::calculate_transaction_hash_single(tx_data);
                                                        hashes_guard.insert(tx_hash);
                                                        count += 1;
                                                    }
                                                }
                                                if count > 0 {
                                                    info!("💾 [TX TRACKING DEFERRED] Successfully tracked {} hashes for commit #{} after backoff", count, deferred_commit_index);
                                                }
                                            } else {
                                                warn!("⚠️ [TX TRACKING DEFERRED] Still cannot acquire lock for commit #{}. TX tracking skipped.", deferred_commit_index);
                                            }
                                        });
                                    }
                                    Err(_) => {
                                        warn!("⚠️ [TX TRACKING DEFERRED] Semaphore full (64 tasks in-flight). Dropping deferred tracking for commit #{}.", deferred_commit_index);
                                    }
                                }
                                break geis_consumed; // Exit retry loop, commit was sent successfully
                            }
                        };
                        let mut hashes_guard = node_guard.committed_transaction_hashes.lock().await;

                        let mut tracked_count = 0;
                        let mut batch_hashes = Vec::new();
                        for block in &subdag.blocks {
                            for tx in block.transactions() {
                                let tx_hash =
                                    crate::types::tx_hash::calculate_transaction_hash_single(
                                        tx.data(),
                                    );
                                hashes_guard.insert(tx_hash.clone());
                                batch_hashes.push(tx_hash);
                                tracked_count += 1;
                            }
                        }

                        // TPS OPT: Defer disk persist to background — TX hashes are only used for
                        // epoch transition recovery, not state computation. Async persist is fork-safe.
                        if !batch_hashes.is_empty() {
                            let storage_path = node_guard.storage_path.clone();
                            let hashes_count = batch_hashes.len();
                            let persist_epoch = epoch;
                            // T2-5: Bounded persistence task — acquire semaphore permit
                            let sem = DEFERRED_TASK_SEMAPHORE.clone();
                            match sem.try_acquire_owned() {
                                Ok(permit) => {
                                    tokio::spawn(async move {
                                        let _permit = permit; // held until task completes
                                        if let Err(e) = crate::node::transition::save_committed_transaction_hashes_batch(
                                                    &storage_path, persist_epoch, &batch_hashes
                                                ).await {
                                                warn!("⚠️ [TX TRACKING] Failed to persist committed hashes after commit: {}", e);
                                            } else {
                                                trace!("💾 [TX TRACKING] Persisted {} committed hashes for epoch {}", hashes_count, persist_epoch);
                                            }
                                    });
                                }
                                Err(_) => {
                                    warn!("⚠️ [TX TRACKING] Semaphore full (64 tasks). Skipping async persist for {} hashes (epoch {}). Will re-persist on next commit.", hashes_count, persist_epoch);
                                }
                            }
                        }

                        if tracked_count > 0 {
                            trace!("💾 [TX TRACKING] Tracked {} committed transaction hashes after processing commit #{} (global_exec_index={})",
                                          tracked_count, commit_index, global_exec_index);
                        }
                    }

                    // NEW: Send ForceCommit request to Go via isolated deferred task
                    // This triggers Event-Driven Block Generation in the Go execution engine
                    let client_clone = client.clone();
                    let reason = format!("commit_g{}_e{}", global_exec_index, epoch);
                    let sem = DEFERRED_TASK_SEMAPHORE.clone();
                    match sem.try_acquire_owned() {
                        Ok(permit) => {
                            tokio::spawn(async move {
                                let _permit = permit;
                                if let Err(e) = client_clone.send_force_commit(reason).await {
                                    trace!("📝 [FORCE COMMIT] Failed to send ForceCommit (non-critical): {}", e);
                                }
                            });
                        }
                        Err(_) => {
                            trace!("📝 [FORCE COMMIT] Semaphore full (64 tasks), skipping force commit trigger");
                        }
                    }

                    break geis_consumed;
                }
                Err(e) => {
                    // Case 1: Duplicate index - Critical Fork Safety check
                    if e.to_string().contains("Duplicate global_exec_index") {
                        error!("🚨 [FORK-SAFETY] Duplicate global_exec_index={} detected! Skipping commit {} to prevent fork. Error: {}", 
                                    global_exec_index, commit_index, e);
                        break 1;
                    }

                    // Case 2: System Transaction (EndOfEpoch) failed - Retry needed
                    if has_system_tx {
                        retry_count += 1;
                        error!("🚨 [CRITICAL] Failed to send commit {} containing EndOfEpoch transaction (Attempt {}). Retrying in 1s... Error: {}", 
                                    commit_index, retry_count, e);

                        sleep(Duration::from_secs(1)).await;
                        continue;
                    }

                    // Case 3: Regular transaction failure
                    warn!("⚠️  [TX FLOW] Failed to send committed subdag: {}", e);
                    if let Some(ref queue) = pending_transactions_queue {
                        super::epoch::queue_commit_transactions_for_next_epoch(
                            subdag,
                            queue,
                            commit_index,
                            global_exec_index,
                            epoch,
                        )
                        .await;
                    } else {
                        warn!("⚠️  [TX FLOW] No pending_transactions_queue - transactions may be lost!");
                    }
                    break 1;
                }
            }
        };
        return Ok(geis_consumed);
    } else {
        info!("ℹ️  [TX FLOW] Executor client not enabled, skipping send");
    }

    Ok(1)
}
