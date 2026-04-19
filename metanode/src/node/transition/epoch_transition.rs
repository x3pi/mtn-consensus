// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

//! Full epoch transitions (epoch N → N+1).

use crate::config::NodeConfig;
use crate::node::executor_client::ExecutorClient;
use crate::node::{ConsensusNode, NodeMode};
use anyhow::Result;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info, warn};

use super::consensus_setup::{setup_synconly_sync, setup_validator_consensus};
use super::demotion::determine_role_and_check_transition;
use super::mode_transition::transition_mode_only;
use super::tx_recovery::recover_epoch_pending_transactions;
use super::verification::{
    verify_epoch_consistency, wait_for_commit_processor_completion, wait_for_consensus_ready,
};

pub async fn transition_to_epoch_from_system_tx(
    node: &mut ConsensusNode,
    new_epoch: u64,
    boundary_timestamp_ms: u64,
    boundary_block: u64,
    synced_global_exec_index: u64,
    config: &NodeConfig,
) -> Result<()> {
    // CRITICAL FIX: Prevent duplicate epoch transitions
    // Multiple EndOfEpoch transactions can trigger multiple transitions to the same epoch
    // This causes RocksDB lock conflicts when trying to open the same DB path twice
    let is_sync_only = matches!(node.node_mode, NodeMode::SyncOnly);
    let is_same_epoch = node.current_epoch == new_epoch;

    // CASE 1: Same epoch, but SyncOnly needs to become Validator
    // This is a MODE-ONLY transition - skip full epoch transition, just start authority
    if is_same_epoch && is_sync_only {
        return super::mode_transition::handle_synconly_upgrade_wait(
            node,
            new_epoch,
            boundary_block, // Or timestamp, whichever handle_synconly_upgrade_wait expects, but it's ignored mostly
            synced_global_exec_index,
            config,
        )
        .await;
    }

    // CASE 2: Already at this epoch and already Validator - skip
    if node.current_epoch >= new_epoch && !is_sync_only {
        info!(
            "ℹ️ [TRANSITION SKIP] Already at epoch {} (requested: {}) and already Validator. Skipping.",
            node.current_epoch, new_epoch
        );
        return Ok(());
    }

    // CASE 3: Current epoch ahead of requested - skip
    if node.current_epoch > new_epoch {
        info!(
            "ℹ️ [TRANSITION SKIP] Current epoch {} is AHEAD of requested {}. Skipping.",
            node.current_epoch, new_epoch
        );
        return Ok(());
    }

    // CASE 4: Full epoch transition (epoch actually changing)
    info!(
        "🔄 [FULL EPOCH TRANSITION] Processing: epoch {} -> {} (current_mode={:?})",
        node.current_epoch, new_epoch, node.node_mode
    );

    if node.is_transitioning.swap(true, Ordering::SeqCst) {
        warn!("⚠️ Full epoch transition already in progress, skipping.");
        node.is_transitioning.store(false, Ordering::SeqCst);
        return Ok(());
    }

    info!(
        "🔄 FULL TRANSITION: epoch {} -> {}",
        node.current_epoch, new_epoch
    );

    // Reset flag guard
    struct Guard(Arc<std::sync::atomic::AtomicBool>);
    impl Drop for Guard {
        fn drop(&mut self) {
            if self.0.load(Ordering::SeqCst) {
                self.0.store(false, Ordering::SeqCst);
            }
        }
    }
    let _guard = Guard(node.is_transitioning.clone());

    // =============================================================================
    // FIX 2026-02-06: Call advance_epoch on Go FIRST, before fetching committee!
    //
    // PROBLEM: determine_role_and_check_transition() calls fetch_committee() which
    // waits for Go to have epoch N data. But Go doesn't have it because advance_epoch
    // was only called AFTER this check (line 558). Circular dependency = deadlock!
    //
    // SOLUTION: Call advance_epoch() FIRST, so Go stores the epoch boundary data.
    // Then fetch_committee can succeed.
    // =============================================================================

    // Initialize checkpoint manager for crash recovery
    let checkpoint_manager = crate::node::epoch_checkpoint::CheckpointManager::new(
        &config.storage_path,
        &format!("node-{}", config.node_id),
    );

    // Check for incomplete transition from previous crash
    if let Ok(Some(incomplete)) = checkpoint_manager.get_incomplete_transition().await {
        info!(
            "🔄 [CHECKPOINT] Found incomplete transition: state={}, epoch={:?}",
            incomplete.state.name(),
            incomplete.state.epoch()
        );
        // For now, just log and continue - future: implement resume logic
    }

    // =========================================================================
    // [FIX 2026-03-21]: MULTI-EPOCH SEQUENTIAL CATCH-UP BEFORE ADVANCE_EPOCH
    // When node is behind, it MUST sync ALL intermediate blocks from peers
    // to Go BEFORE calling advance_epoch. Otherwise, Go will transition
    // to the new epoch using the currently synced (outdated) block as the boundary,
    // causing a non-deterministic genesis state and network fork!
    // =========================================================================
    let early_executor_client = crate::node::executor_client::ExecutorClient::new(
        true,
        false,
        config.executor_send_socket_path.clone(),
        config.executor_receive_socket_path.clone(),
        None,
    );

    let (final_epoch, final_boundary) = catch_up_to_network_epoch(
        node,
        new_epoch,
        boundary_block,
        synced_global_exec_index,
        &early_executor_client,
        config,
    )
    .await?;

    // Update new_epoch if catch-up advanced us further than the original target
    let new_epoch = final_epoch;
    let synced_global_exec_index = final_boundary;

    // Removed eager `advance_epoch` call previously here. We now sequence epoch transition properly.

    let committee_source = crate::node::committee_source::CommitteeSource::discover(config).await?;

    if !committee_source.validate_epoch(new_epoch) {
        warn!(
            "⚠️ [TRANSITION] Epoch mismatch detected. Expected={}, Source={}. Proceeding with source epoch.",
            new_epoch, committee_source.epoch
        );
    }

    let executor_client =
        committee_source.create_executor_client(&config.executor_send_socket_path);

    // ═══════════════════════════════════════════════════════════════════
    // FORK-SAFETY FIX (C4): Ensure provisional_timestamp is DETERMINISTIC.
    //
    // Previously, if boundary_timestamp_ms == 0, the code used SystemTime::now()
    // which returns different values on different nodes → epoch boundary mismatch → fork.
    //
    // Fix: Query Go's epoch boundary data for a deterministic timestamp.
    // This timestamp is derived from consensus-certified block headers and is
    // identical across all nodes that have synced to the same block height.
    // ═══════════════════════════════════════════════════════════════════
    let provisional_timestamp = if boundary_timestamp_ms > 0 {
        boundary_timestamp_ms
    } else {
        warn!(
            "⚠️ [EPOCH TRANSITION] boundary_timestamp_ms is 0 for epoch {}. Querying Go for deterministic fallback.",
            new_epoch
        );
        // Query Go for a deterministic timestamp from the previous epoch's boundary data
        // This is consensus-derived and identical across all nodes
        let prev_epoch = if new_epoch > 0 { new_epoch - 1 } else { 0 };
        match executor_client.get_epoch_boundary_data(prev_epoch).await {
            Ok((_epoch, stored_ts, _boundary, _, _, _)) if stored_ts > 0 => {
                info!(
                    "✅ [EPOCH TRANSITION] Using Go epoch {} boundary timestamp {} ms as provisional",
                    prev_epoch, stored_ts
                );
                stored_ts
            }
            _ => {
                // Last resort: use non-zero sentinel. system_transaction_provider
                // will correct this from Go's boundary data later when committee is fetched.
                warn!(
                    "⚠️ [EPOCH TRANSITION] Cannot get Go boundary timestamp. Using 1 ms (minimum sentinel).",
                );
                1 // Non-zero sentinel: prevents zero-timestamp panic in Go layer
            }
        }
    };

    node.system_transaction_provider
        .update_epoch(new_epoch, provisional_timestamp)
        .await;

    // [FIX 2026-01-29]: Calculate correct target_commit_index from synced_global_exec_index
    // FORMULA: global_exec_index = last_global_exec_index + commit_index
    // Therefore: target_commit_index = synced_global_exec_index - last_global_exec_index
    // This ensures we compare commit_index with commit_index (same metric)
    let target_commit_index = if synced_global_exec_index > node.last_global_exec_index {
        (synced_global_exec_index - node.last_global_exec_index) as u32
    } else {
        // Fallback: if somehow global_exec_index is less, use it directly (shouldn't happen)
        synced_global_exec_index as u32
    };
    info!(
        "⏳ [TRANSITION] Waiting for commit_processor: target_commit_index={}, current_commit_index={}, synced_global_exec_index={}, last_global_exec_index={}",
        target_commit_index,
        node.current_commit_index.load(Ordering::SeqCst),
        synced_global_exec_index,
        node.last_global_exec_index
    );

    // Wait for processor to reach the target commit index (ensure sequential block processing)
    // AUTO-DETECT: SyncOnly nodes can skip this wait since Go already has blocks from Rust P2P sync
    // Validator nodes MUST wait to ensure all blocks are committed before stopping authority
    let is_sync_only = matches!(node.node_mode, crate::node::NodeMode::SyncOnly);

    let timeout_secs = if is_sync_only {
        // SyncOnly: skip wait entirely - Go already has blocks from P2P sync
        0
    } else if config.epoch_transition_optimization == "fast" {
        // Validator fast mode: 5s wait
        5
    } else {
        // Validator balanced/default: 10s wait
        10
    };

    if timeout_secs > 0 {
        let _ = wait_for_commit_processor_completion(node, target_commit_index, timeout_secs).await;
    } else {
        info!(
            "⚡ [TRANSITION] SyncOnly mode detected: skipping commit_processor wait (Go already synced via P2P)"
        );
    }

    // =============================================================================
    // CRITICAL FIX: Stop old authority FIRST before fetching synced_index from Go
    // This prevents race condition where:
    // 1. We fetch synced_index=14400 from Go
    // 2. Old epoch sends more blocks (global_exec_index=14405, 14406, ..., 14409)
    // 3. New epoch starts with epoch_base_index=14400
    // 4. New epoch's commit_index=9 → global_exec_index=14409 (COLLISION!)
    //
    // By stopping old authority FIRST, we ensure all blocks from old epoch
    // have been sent to Go before we fetch epoch_base_index for new epoch.
    // =============================================================================

    let synced_index =
        stop_authority_and_poll_go(node, new_epoch, &executor_client, &committee_source).await?;

    // ═══════════════════════════════════════════════════════════════
    // DISK CLEANUP: Remove old epoch directories beyond epochs_to_keep.
    // ═══════════════════════════════════════════════════════════════
    if config.epochs_to_keep > 0 {
        let keep_from = new_epoch.saturating_sub(config.epochs_to_keep as u64);
        let epochs_dir = node.storage_path.join("epochs");
        if epochs_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&epochs_dir) {
                for entry in entries.flatten() {
                    if let Some(name) = entry.file_name().to_str() {
                        if let Some(epoch_str) = name.strip_prefix("epoch_") {
                            if let Ok(epoch) = epoch_str.parse::<u64>() {
                                if epoch < keep_from {
                                    info!(
                                        "🗑️ [EPOCH CLEANUP] Removing old epoch {} directory \
                                        (keep_from={}, epochs_to_keep={}, current={})",
                                        epoch, keep_from, config.epochs_to_keep, new_epoch
                                    );
                                    if let Err(e) = std::fs::remove_dir_all(entry.path()) {
                                        warn!(
                                            "⚠️ [EPOCH CLEANUP] Failed to remove epoch {} dir: {}",
                                            epoch, e
                                        );
                                    }
                                    node.legacy_store_manager.remove_store(epoch);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Update state
    node.current_epoch = new_epoch;
    node.current_commit_index.store(0, Ordering::SeqCst);

    let effective_synced = std::cmp::max(synced_index, synced_global_exec_index);
    if effective_synced > synced_index {
        info!(
            "📊 [SYNC FLOOR] Using catch-up boundary {} instead of Go-reported {} as epoch base",
            synced_global_exec_index, synced_index
        );
    }
    {
        let mut g = node.shared_last_global_exec_index.lock().await;
        *g = effective_synced;
    }
    node.last_global_exec_index = effective_synced;

    // MEMORY LEAK FIX
    {
        let mut hashes = node.committed_transaction_hashes.lock().await;
        let old_count = hashes.len();
        hashes.clear();
        hashes.shrink_to_fit();
        if old_count > 0 {
            info!(
                "🧹 [MEMORY CLEANUP] Cleared {} committed_transaction_hashes from previous epoch",
                old_count
            );
        }
    }
    node.update_execution_lock_epoch(new_epoch).await;

    info!(
        "📊 [EPOCH STATE UPDATED] epoch={}, last_global_exec_index={}, commit_index={}, mode={:?}",
        node.current_epoch,
        node.last_global_exec_index,
        node.current_commit_index.load(Ordering::SeqCst),
        node.node_mode
    );

    // =============================================================================
    // GO-AUTHORITATIVE EPOCH BOUNDARY & ADVANCE EPOCH
    // =============================================================================
    let go_boundary_for_advance_tuple = match executor_client.get_last_block_number().await {
        Ok(bn) => {
            info!(
                "✅ [EPOCH ADVANCE] Got Go's deterministic last_block_number={} (GEI was {})",
                bn.0, effective_synced
            );
            bn
        }
        Err(e) => {
            warn!(
                "⚠️ [EPOCH ADVANCE] Failed to get Go's last_block_number: {}. Using effective_synced={} as fallback.",
                e, effective_synced
            );
            (effective_synced, 0, false)
        }
    };
    let go_boundary_for_advance = go_boundary_for_advance_tuple.0;

    info!(
        "📤 [EPOCH ADVANCE] Notifying Go about epoch {} transition (boundary: go_block_{}, gei={})",
        new_epoch, go_boundary_for_advance, effective_synced
    );
    // Advance Go's Epoch explicitly using the boundary block retrieved AFTER execution halt
    if let Err(e) = executor_client
        .advance_epoch(
            new_epoch,
            provisional_timestamp,
            go_boundary_for_advance,
            effective_synced,
        )
        .await
    {
        warn!(
            "⚠️ [EPOCH ADVANCE] Failed to notify Go about epoch {}: {}. Continuing anyway...",
            new_epoch, e
        );
    }

    if let Err(e) = checkpoint_manager
        .checkpoint_advance_epoch(
            new_epoch,
            effective_synced,
            provisional_timestamp,
            go_boundary_for_advance,
        )
        .await
    {
        warn!("⚠️ Failed to save checkpoint: {}", e);
    }

    // =============================================================================
    // STEP ROLE-FIRST: Determine node's role for the new epoch
    // =============================================================================
    // Now that Go's epoch has advanced and boundary is stored, we can successfully
    // query the NEW epoch committee to determine our mode for this new epoch!
    let own_protocol_pubkey = node.protocol_keypair.public();
    let (target_role, needs_mode_change) = determine_role_and_check_transition(
        new_epoch,
        &node.node_mode,
        &own_protocol_pubkey,
        config,
    )
    .await?;

    info!(
        "📋 [ROLE-FIRST] Epoch {} transition: target_role={:?}, needs_mode_change={}, current_mode={:?}",
        new_epoch, target_role, needs_mode_change, node.node_mode
    );

    node.close_user_certs().await;

    // Fetch the unified timestamp and committee from Go!
    // We already advanced the epoch up above, so now Go will deterministically
    // reply with the unified block timestamp when fetching the committee.
    let epoch_boundary_block: u64 = go_boundary_for_advance;

    match executor_client.get_epoch_boundary_data(new_epoch).await {
        Ok((stored_epoch, _stored_timestamp, stored_boundary, _, _, _)) => {
            if stored_boundary != go_boundary_for_advance {
                error!(
                    "🚨 [BOUNDARY MISMATCH] Go stored boundary={} but we sent go_block_{}! Potential block skip! (gei={})",
                    stored_boundary, go_boundary_for_advance, effective_synced
                );
            } else {
                info!(
                    "✅ [CONTINUITY VERIFIED] Go confirmed epoch {} boundary: block={}",
                    stored_epoch, stored_boundary
                );
            }
        }
        Err(e) => {
            warn!("⚠️ [VALIDATION SKIP] Cannot verify boundary storage: {}", e);
        }
    }

    let db_path = node
        .storage_path
        .join("epochs")
        .join(format!("epoch_{}", new_epoch))
        .join("consensus_db");
    if db_path.exists() {
        let _ = std::fs::remove_dir_all(&db_path);
    }
    std::fs::create_dir_all(&db_path)?;

    // Reset fragment offset for the new epoch to prevent GEI double-counting
    if let Err(e) =
        crate::node::executor_client::persistence::reset_fragment_offset(&node.storage_path).await
    {
        warn!("⚠️ [PERSIST] Failed to reset fragment offset: {}", e);
    }

    info!(
        "📋 [COMMITTEE] Fetching committee for epoch {} from {} (epoch={})",
        new_epoch, committee_source.socket_path, committee_source.epoch
    );
    let (committee, epoch_timestamp_to_use) = committee_source
        .fetch_committee_with_timestamp(&config.executor_send_socket_path, new_epoch)
        .await?;

    info!(
        "✅ [UNIFIED TIMESTAMP] Using Go's block-header-derived timestamp for epoch {}: {} ms",
        new_epoch, epoch_timestamp_to_use
    );

    node.system_transaction_provider
        .update_epoch(new_epoch, epoch_timestamp_to_use)
        .await;

    // Clone committee for later use in Validator Priority check
    // (original committee will be moved into ConsensusAuthority::start())
    let committee_for_priority_check = committee.clone();

    // Update epoch_eth_addresses cache with new epoch's committee
    if let Err(e) = committee_source
        .fetch_and_update_epoch_eth_addresses(
            &config.executor_send_socket_path,
            new_epoch,
            &node.epoch_eth_addresses,
        )
        .await
    {
        warn!(
            "⚠️ [TRANSITION] Failed to update epoch_eth_addresses: {}",
            e
        );
    }

    // =========================================================================
    // MEMORY LEAK FIX: Prune old epochs from epoch_eth_addresses
    // Only keep current + previous epoch (2 entries max).
    // Old epoch committees are never needed after transition completes.
    // =========================================================================
    {
        let mut addrs = node.epoch_eth_addresses.write().await;
        let before_count = addrs.len();
        if before_count > 2 {
            let min_epoch_to_keep = new_epoch.saturating_sub(1);
            addrs.retain(|&epoch, _| epoch >= min_epoch_to_keep);
            info!(
                "🧹 [MEMORY CLEANUP] Pruned epoch_eth_addresses: {} -> {} entries (keeping epochs >= {})",
                before_count, addrs.len(), min_epoch_to_keep
            );
        }
    }

    node.check_and_update_node_mode(&committee, config, true)
        .await?;

    // FIX: Use protocol_key matching for consistent identity
    let own_protocol_pubkey = node.protocol_keypair.public();
    if let Some((idx, _)) = committee
        .authorities()
        .find(|(_, a)| a.protocol_key == own_protocol_pubkey)
    {
        node.own_index = idx;
        info!(
            "✅ [TRANSITION] Found self in new committee at index {}",
            idx
        );
    } else {
        node.own_index = consensus_config::AuthorityIndex::ZERO;
        info!("ℹ️ [TRANSITION] Not in new committee (protocol_key not found)");
    }

    // Setup consensus components based on mode
    if matches!(node.node_mode, NodeMode::Validator) {
        setup_validator_consensus(
            node,
            new_epoch,
            effective_synced,
            epoch_timestamp_to_use,
            db_path,
            committee,
            config,
        )
        .await?;
    } else {
        setup_synconly_sync(
            node,
            new_epoch,
            effective_synced,
            epoch_timestamp_to_use,
            committee,
            config,
        )
        .await?;
    }

    // Wait for consensus to stabilize with proper synchronization instead of fixed sleep
    if wait_for_consensus_ready(node).await {
        info!("✅ Consensus ready.");
    }

    // Recover transactions from previous epoch that were not committed
    let _ = recover_epoch_pending_transactions(node).await;

    node.is_transitioning.store(false, Ordering::SeqCst);
    let _ = node.submit_queued_transactions().await;

    // =========================================================================
    // VALIDATOR PRIORITY FIX: After SyncOnly setup, re-check if we should be Validator
    //
    // Problem: When a SyncOnly node transitions epochs, check_and_update_node_mode()
    // is called BEFORE committee is fetched for the new epoch. If the new epoch's
    // committee includes this node, we must upgrade to Validator.
    //
    // Solution: After SyncOnly mode is established and sync starts, re-check
    // committee membership. If we're now in the committee, trigger mode upgrade.
    // =========================================================================
    if matches!(node.node_mode, NodeMode::SyncOnly) {
        let own_protocol_pubkey = node.protocol_keypair.public();
        let is_now_in_committee = committee_for_priority_check
            .authorities()
            .any(|(_, authority)| authority.protocol_key == own_protocol_pubkey);

        if is_now_in_committee {
            info!(
                "🚀 [VALIDATOR PRIORITY] SyncOnly node IS in committee for epoch {}! Triggering upgrade.",
                new_epoch
            );

            // Trigger upgrade: SyncOnly → Validator
            // Use synced_index as boundary for the mode-only transition
            // Use epoch_boundary_block (from get_epoch_boundary_data) instead of synced_index
            // This ensures we use the correct epoch boundary, not just the last committed block
            let _upgrade_result = transition_mode_only(
                node,
                new_epoch,
                epoch_boundary_block, // boundary_block from get_epoch_boundary_data
                effective_synced,     // synced_global_exec_index
                config,
            )
            .await;

            info!(
                "✅ [VALIDATOR PRIORITY] Mode upgrade complete: now {:?}",
                node.node_mode
            );
        } else {
            info!(
                "ℹ️ [VALIDATOR PRIORITY] Node NOT in committee for epoch {}. Staying SyncOnly.",
                new_epoch
            );
        }
    }

    node.reset_reconfig_state().await;

    // Post-transition verification: epoch consistency + timestamp sync
    verify_epoch_consistency(node, new_epoch, epoch_timestamp_to_use, &executor_client).await?;

    Ok(())
}

/// Stop old authority, preserve store, and poll Go until it has all old-epoch blocks.
/// Returns the verified synced_index from Go.
///
/// CRITICAL: Before stopping the authority, we must flush the executor client's
/// send buffer to Go. Otherwise, blocks that have been committed by consensus
/// but are still in the buffer will be lost when auth.stop() kills the commit
/// processor task — causing Go to get stuck at a stale block number.
pub(super) async fn stop_authority_and_poll_go(
    node: &mut ConsensusNode,
    new_epoch: u64,
    executor_client: &ExecutorClient,
    committee_source: &crate::node::committee_source::CommitteeSource,
) -> Result<u64> {
    info!("🛑 [TRANSITION] Stopping old authority BEFORE fetching synced_index...");

    let expected_last_block = {
        let shared_index = node.shared_last_global_exec_index.lock().await;
        *shared_index
    };
    info!(
        "📊 [TRANSITION] Expected last block after old epoch: {}",
        expected_last_block
    );

    // ═══════════════════════════════════════════════════════════════════════
    // ROOT CAUSE FIX: Flush the executor client's block buffer BEFORE
    // stopping the authority. auth.stop() kills the commit processor task,
    // which would otherwise drop any blocks still in the send buffer.
    // This ensures Go receives ALL committed blocks before we stop.
    // ═══════════════════════════════════════════════════════════════════════
    if let Some(ref exec_client) = node.executor_client {
        info!("🔄 [TRANSITION] Flushing executor client buffer BEFORE authority shutdown...");
        match exec_client.flush_buffer().await {
            Ok(_) => info!("✅ [TRANSITION] Pre-shutdown buffer flush completed"),
            Err(e) => warn!(
                "⚠️ [TRANSITION] Pre-shutdown buffer flush failed: {}. Will retry after stop.",
                e
            ),
        }
    }

    // Extract store and stop old authority
    if let Some(auth) = node.authority.take() {
        let old_store = auth.take_store();
        let old_epoch = new_epoch.saturating_sub(1);
        node.legacy_store_manager.add_store(old_epoch, old_store);
        info!(
            "📦 [TRANSITION] Extracted store from epoch {} for legacy sync",
            old_epoch
        );

        auth.stop().await;
        info!("✅ [TRANSITION] Old authority stopped. Store preserved in LegacyEpochStoreManager.");
    }

    // ═══════════════════════════════════════════════════════════════════════
    // POST-SHUTDOWN FLUSH: Flush again after authority stop. The commit
    // processor may have added more blocks to the buffer between our
    // pre-flush and auth.stop(). The buffer survives in the Arc<ExecutorClient>.
    // ═══════════════════════════════════════════════════════════════════════
    if let Some(ref exec_client) = node.executor_client {
        info!("🔄 [TRANSITION] Flushing executor client buffer AFTER authority shutdown...");
        // Retry flush up to 5 times with small delay to handle transient connection issues
        for retry in 0..5 {
            match exec_client.flush_buffer().await {
                Ok(_) => {
                    info!(
                        "✅ [TRANSITION] Post-shutdown buffer flush completed (attempt {})",
                        retry + 1
                    );
                    break;
                }
                Err(e) => {
                    if retry < 4 {
                        warn!("⚠️ [TRANSITION] Post-shutdown buffer flush attempt {} failed: {}. Retrying...", retry + 1, e);
                        tokio::time::sleep(Duration::from_millis(200)).await;
                    } else {
                        error!("❌ [TRANSITION] Post-shutdown buffer flush FAILED after 5 attempts: {}", e);
                    }
                }
            }
        }

        // Log remaining buffer state for debugging
        let buffer = exec_client.send_buffer.lock().await;
        if !buffer.is_empty() {
            error!(
                "🚨 [TRANSITION] Buffer still has {} blocks after flush! Keys: {:?}",
                buffer.len(),
                buffer.keys().take(10).collect::<Vec<_>>()
            );
        } else {
            info!("✅ [TRANSITION] Send buffer is empty — all blocks sent to Go");
        }
    }

    // STRICT SEQUENTIAL GUARANTEE: Poll Go until it confirms receiving expected_last_block
    // CRITICAL FIX: Use get_last_global_exec_index() instead of get_last_block_number()!
    // BlockNumber only counts non-empty commits (blocks with txs), while expected_last_block
    // is global_exec_index which counts ALL commits (including empty ones).
    // Comparing BlockNumber vs GEI would NEVER match when empty commits exist.
    //
    // ARCHITECTURE NOTE:
    // - Node 0 (executor, can_commit=true): Rust sends blocks to Go Master →
    //   Go Master executes TXs (EVM) → updates GEI → broadcasts results to Sub nodes.
    //   The sync wait ensures Go has executed ALL blocks before epoch transition.
    // - Node 1,2,3 (non-executor, can_commit=false): Rust handles consensus sync
    //   independently via legacy epoch store (epochs_to_keep). Go Sub receives
    //   block data from Node 0 via network replication. Go Master does NOT execute
    //   blocks, so GEI is never updated (always 0). Polling GEI here would deadlock.
    let is_executor = node
        .executor_client
        .as_ref()
        .map(|ec| ec.can_commit())
        .unwrap_or(false);

    if !is_executor {
        info!(
            "⏩ [SYNC SKIP] Non-executor node — skipping Go GEI sync wait \
            (expected_gei={}, new_epoch={}). Rust consensus sync is independent. \
            Go Sub receives blocks from Node 0 via network replication.",
            expected_last_block, new_epoch
        );

        // Log epoch lag for diagnostics. Each peer decides independently how
        // many epochs to keep (epochs_to_keep), so we only report the lag here.
        // The actual "data unavailable" warning comes from the sync mechanism
        // when a peer responds that it no longer stores the requested epoch.
        let go_epoch = match node.executor_client.as_ref() {
            Some(ec) => ec.get_current_epoch().await.unwrap_or(0),
            None => 0,
        };
        let epoch_lag = new_epoch.saturating_sub(go_epoch);
        if epoch_lag > 0 {
            info!(
                "📊 [EPOCH LAG] Node is {} epoch(s) behind (go_epoch={}, network_epoch={}). \
                Rust will attempt to catch up from peers.",
                epoch_lag, go_epoch, new_epoch
            );
        }
    }

    let poll_interval = Duration::from_millis(100);
    let mut attempt = 0u64;
    let max_wait = Duration::from_secs(300); // 5-minute safety timeout
    let wait_start = std::time::Instant::now();

    if is_executor {
        let mut last_peer_fetch = std::time::Instant::now();
        loop {
            attempt += 1;

            // Safety timeout to prevent infinite wait
            if wait_start.elapsed() > max_wait {
                warn!(
                    "⏱️ [SYNC TIMEOUT] Giving up after {:?}. Go may still be processing blocks. expected_gei={}, continuing with transition...",
                    wait_start.elapsed(), expected_last_block
                );
                break;
            }

            match executor_client.get_last_global_exec_index().await {
                Ok(go_last_gei) => {
                    if go_last_gei >= expected_last_block {
                        info!(
                            "✅ [SYNC VERIFIED] Go confirmed processing all commits: go_gei={} >= expected_gei={} (took {} attempts, {:?})",
                            go_last_gei, expected_last_block, attempt, wait_start.elapsed()
                        );
                        break;
                    } else if attempt.is_multiple_of(100) {
                        warn!(
                            "⏳ [SYNC WAIT] Waiting for Go to catch up: go_gei={}, expected_gei={} (waiting for {:?})",
                            go_last_gei, expected_last_block, wait_start.elapsed()
                        );

                        // If we've been waiting too long, try flushing buffer again
                        if attempt.is_multiple_of(300) {
                            if let Some(ref exec_client) = node.executor_client {
                                warn!(
                                    "🔄 [SYNC WAIT] Re-flushing buffer after {:?} of waiting...",
                                    wait_start.elapsed()
                                );
                                let _ = exec_client.flush_buffer().await;
                            }
                        }
                    }

                    // ACTIVE BLOCK FETCH FIX: Every 10 seconds, fetch missing blocks from
                    // peers and sync them to Go. Without this, the SYNC WAIT loop just polls
                    // Go's GEI which can't advance if the blocks were never delivered.
                    // This is the same approach used in handle_deferred_epoch_transition.
                    if last_peer_fetch.elapsed() > Duration::from_secs(10)
                        && go_last_gei < expected_last_block
                        && !node.peer_rpc_addresses.is_empty()
                    {
                        last_peer_fetch = std::time::Instant::now();
                        let fetch_from = go_last_gei + 1;
                        info!(
                            "🔄 [SYNC WAIT] Actively fetching blocks {} to {} from peers (Go at {}/{})",
                            fetch_from, expected_last_block, go_last_gei, expected_last_block
                        );
                        match crate::network::peer_rpc::fetch_executable_blocks_from_peer(
                            &node.peer_rpc_addresses,
                            fetch_from,
                            expected_last_block,
                        )
                        .await
                        {
                            Ok(blocks) if !blocks.is_empty() => {
                                info!(
                                    "✅ [SYNC WAIT] Fetched {} executable blocks from peers, syncing...",
                                    blocks.len()
                                );
                                if let Some(ref exec_client) = node.executor_client {
                                    let mut synced = 0u64;
                                    for (gei_num, block_bytes) in blocks {
                                        match exec_client
                                            .send_block_data(
                                                block_bytes.as_slice(),
                                                gei_num,
                                                new_epoch,
                                                0,
                                            )
                                            .await
                                        {
                                            Ok(_) => {
                                                synced += 1;
                                            }
                                            Err(e) => {
                                                warn!("⚠️ [SYNC WAIT] Block send failed for GEI {}: {}", gei_num, e);
                                                break;
                                            }
                                        }
                                    }
                                    if synced > 0 {
                                        info!(
                                            "✅ [SYNC WAIT] Synced {} executable blocks to Go",
                                            synced
                                        );
                                    }
                                }
                            }
                            Ok(_) => {}
                            Err(e) => {
                                warn!("⚠️ [SYNC WAIT] Peer block fetch failed: {}", e);
                            }
                        }
                    }
                }
                Err(e) => {
                    if attempt.is_multiple_of(100) {
                        error!(
                            "❌ [SYNC POLL] Cannot reach Go (attempt {}): {}. Will keep trying...",
                            attempt, e
                        );
                    }
                }
            }
            tokio::time::sleep(poll_interval).await;
        }
    } // end if is_executor

    // Fetch final synced_index from Go
    // Try GEI first (works on executor/Node 0), then block_number as fallback
    let raw_synced_gei = executor_client
        .get_last_global_exec_index()
        .await
        .unwrap_or(0);
    // CRITICAL FIX: Also get block_number as floor!
    // On non-executor nodes (Node 1,2,3), GEI is ALWAYS 0 because only Node 0
    // updates GEI when it executes transactions. But sync_blocks correctly
    // updates block_number via storage.UpdateLastBlockNumber(). Without this
    // floor, epoch_base_index=0 after snapshot restore, causing commit_syncer
    // to search commits in the wrong global range.
    let raw_synced_block = executor_client
        .get_last_block_number()
        .await
        .map(|(b, _, _)| b)
        .unwrap_or(0);
    let raw_synced = std::cmp::max(raw_synced_gei, raw_synced_block);

    info!(
        "📊 [SYNC] Go state: gei={}, block={}, using max={}",
        raw_synced_gei, raw_synced_block, raw_synced
    );

    // Additional floors: committee source and expected_last_block
    let committee_floor = if committee_source.last_block > 0 {
        info!(
            "📊 [SYNC] Committee source last block: {} (from {})",
            committee_source.last_block,
            if committee_source.is_peer {
                "peer"
            } else {
                "local"
            }
        );
        committee_source.last_block
    } else {
        0
    };

    // SAFETY FLOOR: Never let synced_index go below what the commit processor already sent.
    // This prevents the catastrophic bug where epoch_base_index=0 after epoch transition
    // when Go's RPC returns stale/zero GEI.
    let synced_index = *[raw_synced, expected_last_block, committee_floor]
        .iter()
        .max()
        .unwrap();
    if synced_index > raw_synced {
        warn!(
            "🚨 [SYNC SAFETY] Go returned max(gei,block)={} < floor={}. Using floor to prevent epoch_base regression!",
            raw_synced, synced_index
        );
    }

    info!(
        "📊 Snapshot: Last committed block from Go: {} (raw_gei={}, raw_block={}, expected_floor={}, committee_floor={})",
        synced_index, raw_synced_gei, raw_synced_block, expected_last_block, committee_floor
    );
    Ok(synced_index)
}

/// Handle deferred epoch transition when Go hasn't synced to the required boundary yet.
/// IMPROVED: Instead of just queuing (which causes deadlock if no consensus is running),
/// poll Go with a timeout. If Go catches up, return Ok so the caller can proceed inline.
/// Falls back to queue only after timeout.
pub(super) async fn handle_deferred_epoch_transition(
    node: &mut ConsensusNode,
    new_epoch: u64,
    epoch_timestamp: u64,
    required_boundary: u64,
    go_current_block: u64,
    synced_global_exec_index: u64,
) -> Result<()> {
    info!(
        "📋 [DEFERRED EPOCH] Go block {} < required boundary {}. Fetching blocks from peers for epoch {} then polling Go.",
        go_current_block, required_boundary, new_epoch
    );

    // =========================================================================
    // PHASE 1: Fetch missing blocks from peer nodes via HTTP /get_blocks
    // This is the key fix: instead of just polling Go (which can't catch up
    // when no consensus is running), actively fetch blocks from peers and
    // write them to local Go first.
    // =========================================================================
    if !node.peer_rpc_addresses.is_empty() {
        let missing_from = go_current_block + 1;
        if missing_from <= required_boundary {
            info!(
                "🔄 [DEFERRED EPOCH] Fetching blocks {} to {} from {} peer(s)",
                missing_from,
                required_boundary,
                node.peer_rpc_addresses.len()
            );

            match crate::network::peer_rpc::fetch_blocks_from_peer(
                &node.peer_rpc_addresses,
                missing_from,
                required_boundary,
            )
            .await
            {
                Ok(blocks) => {
                    if !blocks.is_empty() {
                        info!(
                            "✅ [DEFERRED EPOCH] Fetched {} blocks from peers. Syncing to local Go...",
                            blocks.len()
                        );

                        // Phase 1 fix: Execute blocks through NOMT (not just store)
                        if let Some(ref exec_client) = node.executor_client {
                            match exec_client.sync_and_execute_blocks(blocks).await {
                                Ok((synced, last_block, _gei)) => {
                                    info!(
                                        "✅ [DEFERRED EPOCH] Executed {} blocks to local Go (last: {})",
                                        synced, last_block
                                    );
                                }
                                Err(e) => {
                                    warn!(
                                        "⚠️ [DEFERRED EPOCH] Failed to execute blocks to local Go: {}",
                                        e
                                    );
                                }
                            }
                        }
                    } else {
                        warn!("⚠️ [DEFERRED EPOCH] Fetched 0 blocks from peers");
                    }
                }
                Err(e) => {
                    warn!(
                        "⚠️ [DEFERRED EPOCH] Failed to fetch blocks from peers: {}. Will try polling Go directly.",
                        e
                    );
                }
            }
        }
    } else {
        info!(
            "⚠️ [DEFERRED EPOCH] No peer_rpc_addresses configured. Cannot fetch blocks from peers."
        );
    }

    // =========================================================================
    // PHASE 2: Poll Go with timeout (should succeed quickly after block sync)
    // =========================================================================
    let poll_timeout = Duration::from_secs(120);
    let poll_interval = Duration::from_millis(200);
    let start = std::time::Instant::now();

    // Try to use the node's existing executor client for polling
    // GEI is always 0 on non-executor nodes (can_commit=false) because only Node 0
    // updates GEI when it executes transactions. sync_blocks writes to LevelDB
    // and updates block_number but NOT GEI. So we must compare block numbers.
    let mut go_caught_up = false;
    let mut last_refetch_time = std::time::Instant::now();
    if let Some(ref exec_client) = node.executor_client {
        loop {
            match exec_client.get_last_block_number().await {
                Ok((last_block, _, _)) => {
                    let gei = exec_client.get_last_global_exec_index().await.unwrap_or(0);
                    let current_gei = if synced_global_exec_index > 0 && gei > 0 {
                        gei
                    } else {
                        last_block
                    };
                    let target = if synced_global_exec_index > 0 {
                        synced_global_exec_index
                    } else {
                        required_boundary
                    };

                    if current_gei >= target {
                        info!(
                            "✅ [DEFERRED EPOCH] Go caught up! gei {} >= target {} (waited {:?})",
                            current_gei,
                            target,
                            start.elapsed()
                        );
                        go_caught_up = true;
                        break;
                    }
                    if start.elapsed() > poll_timeout {
                        warn!(
                            "⚠️ [DEFERRED EPOCH] Timeout waiting for Go: gei {} < target {} after {:?}. \
                             This may cause DEADLOCK if no consensus authority is running!",
                            current_gei, target, start.elapsed()
                        );
                        break;
                    }

                    // RETRY FETCH: Every 10 seconds, re-fetch missing blocks from peers
                    if last_refetch_time.elapsed() > Duration::from_secs(10)
                        && current_gei < target
                        && !node.peer_rpc_addresses.is_empty()
                    {
                        last_refetch_time = std::time::Instant::now();
                        let fetch_from = current_gei + 1;
                        info!(
                            "🔄 [DEFERRED EPOCH] Re-fetching blocks {} to {} from peers (Go at {}/{})",
                            fetch_from, target, current_gei, target
                        );
                        match crate::network::peer_rpc::fetch_executable_blocks_from_peer(
                            &node.peer_rpc_addresses,
                            fetch_from,
                            target,
                        )
                        .await
                        {
                            Ok(blocks) if !blocks.is_empty() => {
                                info!(
                                    "✅ [DEFERRED EPOCH] Re-fetched {} blocks from peers, syncing...",
                                    blocks.len()
                                );
                                let mut synced = 0;
                                for (gei_num, block_bytes) in blocks {
                                    let err_str = (exec_client
                                        .send_block_data(
                                            block_bytes.as_slice(),
                                            gei_num,
                                            new_epoch,
                                            0,
                                        )
                                        .await)
                                        .err();
                                    if let Some(ref e) = err_str {
                                        warn!(
                                            "⚠️ [DEFERRED EPOCH] Re-sync failed for GEI {}: {}",
                                            gei_num, e
                                        );
                                        break;
                                    }
                                    synced += 1;
                                }
                                info!(
                                    "✅ [DEFERRED EPOCH] Re-synced {} executable blocks to Go",
                                    synced
                                );
                            }
                            Ok(_) => {
                                warn!("[DEFERRED EPOCH] Re-fetch returned 0 blocks");
                            }
                            Err(e) => {
                                warn!("[DEFERRED EPOCH] Re-fetch failed: {}", e);
                            }
                        }
                    }

                    if start.elapsed().as_secs().is_multiple_of(5)
                        && start.elapsed().as_millis() % 5000 < 200
                    {
                        info!(
                            "⏳ [DEFERRED EPOCH] Waiting for Go: gei {} / {} (elapsed {:?})",
                            current_gei,
                            target,
                            start.elapsed()
                        );
                    }
                }
                Err(e) => {
                    if start.elapsed() > poll_timeout {
                        warn!("⚠️ [DEFERRED EPOCH] Timeout + error polling Go: {}", e);
                        break;
                    }
                }
            }
            tokio::time::sleep(poll_interval).await;
        }
    }

    if go_caught_up {
        // Go has caught up - update state and return Ok so caller can proceed
        info!(
            "✅ [DEFERRED EPOCH] Go synced to boundary {}. Proceeding with epoch {} inline.",
            required_boundary, new_epoch
        );
        // Don't queue - just update sync state and let caller handle the rest
        // The caller should re-check go_current_block and proceed past the deferred block
        node.current_commit_index.store(0, Ordering::SeqCst);
        {
            let mut g = node.shared_last_global_exec_index.lock().await;
            *g = required_boundary;
        }
        node.last_global_exec_index = required_boundary;
        node.is_transitioning.store(false, Ordering::SeqCst);
        return Ok(());
    }

    // Fallback: Queue transition for later processing
    warn!(
        "🚨 [DEFERRED EPOCH] Go did NOT catch up to boundary {} within timeout. Queuing epoch {}. \
         If no consensus is running, this will cause DEADLOCK!",
        required_boundary, new_epoch
    );

    {
        let mut pending = node.pending_epoch_transitions.lock().await;
        pending.push(crate::node::PendingEpochTransition {
            epoch: new_epoch,
            timestamp_ms: epoch_timestamp,
            boundary_block: required_boundary,
            boundary_gei: synced_global_exec_index,
        });
    }

    // CRITICAL: Update SYNC-RELATED state so sync can fetch from new epoch
    // BUT do NOT update node.current_epoch!
    // If we set current_epoch = new_epoch now, the real transition will SKIP
    // because it checks "current_epoch >= new_epoch"
    info!(
        "📋 [DEFERRED EPOCH] Updating sync state ONLY (NOT current_epoch) for epoch {} (base={})",
        new_epoch, required_boundary
    );

    node.current_commit_index.store(0, Ordering::SeqCst);
    {
        let mut g = node.shared_last_global_exec_index.lock().await;
        *g = required_boundary;
    }
    node.last_global_exec_index = required_boundary;

    node.is_transitioning.store(false, Ordering::SeqCst);
    info!(
        "📋 [DEFERRED EPOCH] Sync state updated. Full transition queued for when Go reaches block {}",
        required_boundary
    );
    Ok(())
}

// =============================================================================
// MULTI-EPOCH SEQUENTIAL CATCH-UP FUNCTIONS (2026-02-11)
// =============================================================================

/// Query peers via TCP RPC to find the highest epoch in the network.
async fn get_current_network_epoch(config: &NodeConfig) -> Result<u64> {
    let mut best_epoch = 0u64;
    for peer in &config.peer_rpc_addresses {
        match crate::network::peer_rpc::query_peer_info(peer).await {
            Ok(info) => {
                if info.epoch > best_epoch {
                    best_epoch = info.epoch;
                    info!(
                        "📊 [NETWORK EPOCH] Peer {} reports epoch={}, block={}",
                        peer, info.epoch, info.last_block
                    );
                }
            }
            Err(e) => {
                warn!("⚠️ [NETWORK EPOCH] Failed to query peer {}: {}", peer, e);
            }
        }
    }
    if best_epoch == 0 {
        warn!("⚠️ [NETWORK EPOCH] No peers responded, using epoch 0");
    }
    Ok(best_epoch)
}

/// Get epoch boundary data from any available peer.
/// Returns (boundary_block, timestamp_ms) for the given epoch.
#[allow(dead_code)]
async fn get_epoch_boundary_from_peers(config: &NodeConfig, epoch: u64) -> Result<(u64, u64)> {
    for peer in &config.peer_rpc_addresses {
        let peer_addr: std::net::SocketAddr = match peer.parse() {
            Ok(addr) => addr,
            Err(e) => {
                warn!("⚠️ [PEER BOUNDARY] Invalid peer address {}: {}", peer, e);
                continue;
            }
        };
        let client = crate::node::peer_go_client::PeerGoClient::new(peer_addr);
        match client.get_epoch_boundary_data(epoch).await {
            Ok((_epoch, timestamp, boundary_block, _validators, _)) => {
                info!(
                    "✅ [PEER BOUNDARY] epoch={}, boundary={}, timestamp={} (from {})",
                    epoch, boundary_block, timestamp, peer
                );
                return Ok((boundary_block, timestamp));
            }
            Err(e) => {
                warn!(
                    "⚠️ [PEER BOUNDARY] Failed to get epoch {} boundary from {}: {}",
                    epoch, peer, e
                );
            }
        }
    }
    Err(anyhow::anyhow!(
        "No peer could provide boundary data for epoch {}",
        epoch
    ))
}

/// Fetch blocks from peers and sync to local Go executor.
async fn fetch_and_sync_blocks_to_go(
    node: &mut ConsensusNode,
    from_block: u64,
    to_block: u64,
) -> Result<()> {
    if node.peer_rpc_addresses.is_empty() {
        return Err(anyhow::anyhow!(
            "No peer_rpc_addresses configured for block fetch"
        ));
    }

    info!(
        "🔄 [BLOCK SYNC] Fetching blocks {} → {} from {} peer(s)",
        from_block,
        to_block,
        node.peer_rpc_addresses.len()
    );

    let blocks = crate::network::peer_rpc::fetch_blocks_from_peer(
        &node.peer_rpc_addresses,
        from_block,
        to_block,
    )
    .await?;

    if blocks.is_empty() {
        warn!(
            "⚠️ [BLOCK SYNC] Fetched 0 blocks from peers (expected {} → {})",
            from_block, to_block
        );
        return Ok(());
    }

    info!(
        "📦 [BLOCK SYNC] Fetched {} blocks, syncing to Go...",
        blocks.len()
    );

    if let Some(ref exec_client) = node.executor_client {
        // Phase 1 fix: Execute blocks through NOMT (not just store)
        let (synced, last_block, _gei) = exec_client.sync_and_execute_blocks(blocks).await?;
        info!(
            "✅ [BLOCK SYNC] Executed {} blocks to Go (last_block={})",
            synced, last_block
        );
    } else {
        return Err(anyhow::anyhow!(
            "No executor_client available for sync_and_execute_blocks"
        ));
    }

    Ok(())
}

/// Multi-epoch sequential catch-up.
///
/// When node restarts multiple epochs behind, this function:
/// 1. Queries peers for the current network epoch
/// 2. Loops through each intermediate epoch:
///    - Fetches boundary data from peers
///    - Syncs blocks for that epoch range
///    - Advances Go to the intermediate epoch
/// 3. Returns the final epoch & boundary for authority startup
///
/// **GUARANTEE**: Every block is executed by Go in its correct epoch context.
async fn catch_up_to_network_epoch(
    node: &mut ConsensusNode,
    requested_epoch: u64,
    requested_boundary_block: u64,
    requested_boundary_gei: u64,
    executor_client: &ExecutorClient,
    config: &NodeConfig,
) -> Result<(u64, u64)> {
    // Step 1: Determine how far behind we are
    let network_epoch = get_current_network_epoch(config)
        .await
        .unwrap_or(requested_epoch);
    let current_epoch = node.current_epoch;
    let epoch_gap = if network_epoch > current_epoch {
        network_epoch - current_epoch
    } else {
        requested_epoch - current_epoch
    };

    // Single-epoch advance — use simple deferred sync
    if epoch_gap <= 1 {
        let go_current_gei = executor_client
            .get_last_global_exec_index()
            .await
            .unwrap_or(0);
        if go_current_gei < requested_boundary_gei {
            info!(
                "🔄 [EPOCH SYNC] Single epoch, Go behind: GEI {} < boundary_gei {}. Syncing.",
                go_current_gei, requested_boundary_gei
            );

            let go_current_block = executor_client
                .get_last_block_number()
                .await
                .map(|(b, _, _)| b)
                .unwrap_or(0);

            handle_deferred_epoch_transition(
                node,
                requested_epoch,
                0, // timestamp will be set later
                requested_boundary_block,
                go_current_block,
                requested_boundary_gei,
            )
            .await?;
            info!("✅ [EPOCH SYNC] Single-epoch deferred sync completed.");
        } else {
            info!(
                "✅ [EPOCH SYNC] Go synced: GEI {} >= boundary_gei {}. Proceeding.",
                go_current_gei, requested_boundary_gei
            );
        }
        return Ok((requested_epoch, requested_boundary_gei));
    }

    // Multi-epoch catch-up needed
    info!(
        "🔄 [MULTI-EPOCH CATCHUP] Traversing epochs {} → {} (network at {}, gap={})",
        current_epoch, network_epoch, network_epoch, epoch_gap
    );

    let target_epoch = network_epoch.max(requested_epoch);
    let mut last_synced_boundary = node.last_global_exec_index;

    // Step 2: Try per-epoch sequential catch-up first
    let mut per_epoch_failed = false;

    for intermediate_epoch in (current_epoch + 1)..=target_epoch {
        info!(
            "📦 [CATCHUP {}/{}] Processing epoch {}",
            intermediate_epoch - current_epoch,
            target_epoch - current_epoch,
            intermediate_epoch
        );

        // a. Get boundary data for this epoch
        let (boundary_block, boundary_gei, timestamp) = if intermediate_epoch == requested_epoch {
            // For the originally requested epoch, use the data we already have
            (requested_boundary_block, requested_boundary_gei, 0u64)
        } else {
            // Query local Go Master for historical epoch boundary data
            let peer_rpc_addr = &config.peer_rpc_addresses;
            match executor_client
                .get_safe_epoch_boundary_data(intermediate_epoch, peer_rpc_addr)
                .await
            {
                Ok((_epoch, timestamp, b_block, _validators, _, b_gei)) => {
                    info!(
                        "✅ [CATCHUP] Local Go boundary: epoch={}, boundary_block={}, boundary_gei={}, timestamp={}",
                        intermediate_epoch, b_block, b_gei, timestamp
                    );
                    (b_block, b_gei, timestamp)
                }
                Err(e) => {
                    warn!(
                        "⚠️ [CATCHUP] No boundary data for epoch {}: {}. Falling back to direct-jump.",
                        intermediate_epoch, e
                    );
                    per_epoch_failed = true;
                    break;
                }
            }
        };

        // b. Sync blocks from Go's current position to this boundary
        let go_current = executor_client
            .get_last_block_number()
            .await
            .map(|(b, _, _)| b)
            .unwrap_or(0);
        if go_current < boundary_block {
            info!(
                "🔄 [CATCHUP] Syncing block {} → {} for epoch {}",
                go_current + 1,
                boundary_block,
                intermediate_epoch
            );

            // Fetch blocks from peers and write to Go
            if let Err(e) = fetch_and_sync_blocks_to_go(node, go_current + 1, boundary_block).await
            {
                warn!(
                    "⚠️ [CATCHUP] Block sync failed for epoch {}: {}. Trying deferred sync.",
                    intermediate_epoch, e
                );
                handle_deferred_epoch_transition(
                    node,
                    intermediate_epoch,
                    timestamp,
                    boundary_block,
                    go_current,
                    boundary_gei, // Pass true GEI boundary
                )
                .await?;
            }

            // Verify Go caught up
            let go_after = executor_client
                .get_last_block_number()
                .await
                .map(|(b, _, _)| b)
                .unwrap_or(0);
            if go_after < boundary_block {
                warn!(
                    "⚠️ [CATCHUP] Go still behind after sync: block {} < boundary {}. Polling...",
                    go_after, boundary_block
                );
                let poll_timeout = Duration::from_secs(30);
                let poll_start = std::time::Instant::now();
                loop {
                    if poll_start.elapsed() > poll_timeout {
                        return Err(anyhow::anyhow!(
                            "Go failed to reach boundary {} for epoch {} (stuck at block {})",
                            boundary_block,
                            intermediate_epoch,
                            go_after
                        ));
                    }
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    let current = executor_client
                        .get_last_block_number()
                        .await
                        .map(|(b, _, _)| b)
                        .unwrap_or(0);
                    if current >= boundary_block {
                        break;
                    }
                }
            }
        } else {
            info!(
                "✅ [CATCHUP] Go already at block {} >= boundary {} for epoch {}",
                go_current, boundary_block, intermediate_epoch
            );
        }

        // c. Advance Go to this intermediate epoch
        let use_timestamp = if timestamp > 0 {
            timestamp
        } else {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64
        };

        let go_epoch = executor_client.get_current_epoch().await.unwrap_or(0);
        if go_epoch < intermediate_epoch {
            info!(
                "📤 [CATCHUP] Advancing Go: epoch {} → {} (boundary_block={}, boundary_gei={})",
                go_epoch, intermediate_epoch, boundary_block, boundary_gei
            );
            if let Err(e) = executor_client
                .advance_epoch(
                    intermediate_epoch,
                    use_timestamp,
                    boundary_block,
                    boundary_gei,
                )
                .await
            {
                warn!(
                    "⚠️ [CATCHUP] Failed to advance Go to epoch {}: {}. Continuing.",
                    intermediate_epoch, e
                );
            }
        }

        // d. Update state for this epoch
        last_synced_boundary = boundary_gei;

        info!(
            "✅ [CATCHUP {}/{}] Epoch {} complete (boundary={})",
            intermediate_epoch - current_epoch,
            target_epoch - current_epoch,
            intermediate_epoch,
            boundary_block
        );
    }

    // Fallback: Direct-jump when intermediate boundaries are unavailable
    // This happens on fresh restarts where local Go has no historical epoch data
    if per_epoch_failed {
        info!(
            "🔄 [DIRECT-JUMP] Boundaries unavailable. Syncing blocks to {} and jumping to epoch {}",
            requested_boundary_block, requested_epoch
        );

        // Sync all blocks up to the requested boundary
        let go_current = executor_client
            .get_last_block_number()
            .await
            .map(|(b, _, _)| b)
            .unwrap_or(0);
        if go_current < requested_boundary_block {
            info!(
                "🔄 [DIRECT-JUMP] Syncing block {} → {} via deferred transition",
                go_current, requested_boundary_block
            );
            if let Err(e) = handle_deferred_epoch_transition(
                node,
                requested_epoch,
                0,
                requested_boundary_block,
                go_current,
                requested_boundary_gei,
            )
            .await
            {
                warn!(
                    "⚠️ [DIRECT-JUMP] Block sync failed: {}. Proceeding anyway.",
                    e
                );
            }
        }

        // Advance Go directly to the target epoch
        let timestamp_now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let go_epoch = executor_client.get_current_epoch().await.unwrap_or(0);
        if go_epoch < requested_epoch {
            info!(
                "📤 [DIRECT-JUMP] Advancing Go: epoch {} → {} (boundary_block={}, boundary_gei={})",
                go_epoch, requested_epoch, requested_boundary_block, requested_boundary_gei
            );
            if let Err(e) = executor_client
                .advance_epoch(
                    requested_epoch,
                    timestamp_now,
                    requested_boundary_block,
                    requested_boundary_gei,
                )
                .await
            {
                warn!(
                    "⚠️ [DIRECT-JUMP] Failed to advance Go to epoch {}: {}",
                    requested_epoch, e
                );
            }
        }

        last_synced_boundary = requested_boundary_gei;
    }

    // Step 3: Return the final epoch we synced to
    let final_epoch = {
        let go_epoch = executor_client
            .get_current_epoch()
            .await
            .unwrap_or(requested_epoch);
        go_epoch.max(requested_epoch)
    };

    info!(
        "✅ [MULTI-EPOCH CATCHUP] Complete! Synced through {} epochs. Final: epoch={}, boundary={}",
        epoch_gap, final_epoch, last_synced_boundary
    );

    Ok((final_epoch, last_synced_boundary))
}
