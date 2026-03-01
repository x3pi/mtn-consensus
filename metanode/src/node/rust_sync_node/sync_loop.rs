// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

//! Main sync loop and sync_once logic for RustSyncNode.

use super::RustSyncNode;
use anyhow::Result;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tracing::{debug, error, info, warn};

use std::time::Instant;

use crate::network::peer_rpc::{query_peer_epoch_boundary_data, query_peer_epochs_network};

impl RustSyncNode {
    /// Start the sync node
    pub fn start(self) -> super::RustSyncHandle {
        info!(
            "🚀 [RUST-SYNC] Starting P2P block sync for epoch {}, commit_index={}",
            self.current_epoch.load(Ordering::SeqCst),
            self.last_synced_commit_index.load(Ordering::SeqCst)
        );

        let has_network = self.network_client.is_some();
        if has_network {
            info!("🌐 [RUST-SYNC] P2P networking initialized - will fetch from peers");
        } else {
            info!("⚠️ [RUST-SYNC] No P2P network - will only monitor Go progress");
        }

        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel();

        let task_handle = tokio::spawn(async move {
            self.sync_loop(&mut shutdown_rx).await;
        });

        super::RustSyncHandle::new(shutdown_tx, task_handle)
    }

    /// Main sync loop - fetches commits from peers
    /// Uses adaptive interval: turbo mode (200ms) when catching up, normal (2s) otherwise
    /// STABILITY FIX: Tracks consecutive errors and resets connections after 5 failures
    async fn sync_loop(mut self, shutdown_rx: &mut tokio::sync::oneshot::Receiver<()>) {
        let normal_interval = Duration::from_secs(self.config.fetch_interval_secs);
        let turbo_interval = Duration::from_millis(self.config.turbo_fetch_interval_ms);
        let mut is_turbo_mode = false;

        // STABILITY FIX: Track consecutive sync errors to detect stale connections
        let mut consecutive_errors: u32 = 0;
        const MAX_CONSECUTIVE_ERRORS: u32 = 5;

        loop {
            // Determine if we're catching up (behind network)
            let catching_up = {
                let queue = self.block_queue.lock().await;
                let pending = queue.pending_count();
                // Turbo mode ONLY if we have pending commits waiting to be drained
                // Previously had `queue.next_expected() > 1` which was always true!
                pending > 0
            };

            // Check if peer epoch is ahead (triggers turbo mode)
            let go_epoch = self.executor_client.get_current_epoch().await.unwrap_or(0);
            let rust_epoch = self.current_epoch.load(Ordering::SeqCst);
            let epoch_behind = rust_epoch < go_epoch;

            let new_turbo = catching_up || epoch_behind;
            if new_turbo != is_turbo_mode {
                is_turbo_mode = new_turbo;
                if is_turbo_mode {
                    info!(
                        "🚀 [RUST-SYNC] TURBO MODE ENABLED - interval={}ms, batch_size={}",
                        self.config.turbo_fetch_interval_ms, self.config.turbo_batch_size
                    );
                } else {
                    info!(
                        "🐢 [RUST-SYNC] Normal mode - interval={}s, batch_size={}",
                        self.config.fetch_interval_secs, self.config.fetch_batch_size
                    );
                }
            }

            let sleep_duration = if is_turbo_mode {
                turbo_interval
            } else {
                normal_interval
            };

            tokio::select! {
                _ = tokio::time::sleep(sleep_duration) => {
                    let timer = self.metrics.sync_round_duration_seconds.start_timer();
                    match self.sync_once().await {
                        Ok(_) => {
                            // Success - reset error counter
                            if consecutive_errors > 0 {
                                info!("✅ [RUST-SYNC] Sync recovered after {} errors", consecutive_errors);
                            }
                            consecutive_errors = 0;
                        }
                        Err(e) => {
                            consecutive_errors += 1;
                            self.metrics.sync_errors_total.inc();
                            self.metrics.peer_errors_total.with_label_values(&["sync_round"]).inc();
                            warn!("[RUST-SYNC] Sync error #{}: {}", consecutive_errors, e);

                            // STABILITY FIX: After MAX_CONSECUTIVE_ERRORS, force reset connections
                            // This handles stale connections after Go restart
                            if consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
                                warn!(
                                    "🔄 [RUST-SYNC] {} consecutive errors detected. Resetting connections to recover from potential stale socket...",
                                    consecutive_errors
                                );
                                self.executor_client.reset_connections().await;
                                consecutive_errors = 0;

                                // Extra delay after reset to allow Go to accept new connection
                                tokio::time::sleep(Duration::from_millis(500)).await;
                            }
                        }
                    }
                    timer.observe_duration();
                }
                _ = &mut *shutdown_rx => {
                    info!("[RUST-SYNC] Shutdown signal received");
                    return;
                }
            }
        }
    }

    /// Single sync iteration using queue-based pattern:
    /// 1. Sync queue with Go's progress (authoritative)
    /// 2. Fetch commits from peers → push to queue
    /// 3. Drain ready commits from queue → send to Go sequentially
    async fn sync_once(&mut self) -> Result<()> {
        // Get current state from Go - this is the AUTHORITATIVE source of truth
        let go_query_start = Instant::now();
        let go_last_block = self.executor_client.get_last_block_number().await?;
        let go_epoch = self.executor_client.get_current_epoch().await?;
        self.metrics
            .go_state_query_seconds
            .observe(go_query_start.elapsed().as_secs_f64());

        let rust_epoch = self.current_epoch.load(Ordering::SeqCst);
        let _epoch_base = self.epoch_base_index.load(Ordering::SeqCst);

        // Update metrics gauges
        self.metrics.current_epoch.set(rust_epoch as f64);
        self.metrics.current_block.set(go_last_block as f64);
        {
            let queue = self.block_queue.lock().await;
            self.metrics.queue_depth.set(queue.pending_count() as f64);
        }
        // Update peers_in_backoff metric
        {
            let health = self.peer_health.lock().await;
            let committee_guard = self.committee.read().expect("committee lock poisoned");
            if let Some(ref committee) = *committee_guard {
                let in_backoff = committee
                    .authorities()
                    .filter(|(idx, _)| !health.is_healthy(idx.value() as u32))
                    .count();
                self.metrics.peers_in_backoff.set(in_backoff as f64);
            }
        }

        // =====================================================================
        // AUTO-EPOCH-SYNC: Detect when Go is ahead and auto-update internal state
        // This prevents sync from getting stuck when epoch_monitor advances Go
        // but RustSyncNode's internal state was never updated.
        // =====================================================================
        if go_epoch > rust_epoch {
            self.auto_epoch_sync(go_epoch, rust_epoch).await;
        } else if go_epoch == rust_epoch {
            self.committee_refresh(go_epoch).await;
        }

        // Re-read epoch values after potential update
        let rust_epoch = self.current_epoch.load(Ordering::SeqCst);
        let epoch_base = self.epoch_base_index.load(Ordering::SeqCst);

        // =====================================================================
        // EPOCH DATA INTEGRITY CHECK
        // =====================================================================
        if rust_epoch > 0 && epoch_base > 0 && (go_last_block as u64) < epoch_base {
            self.epoch_data_integrity_check(go_last_block, epoch_base, rust_epoch)
                .await;
        }

        // PHASE 1: Sync queue with Go's progress
        {
            let mut queue = self.block_queue.lock().await;
            queue.sync_with_go(go_last_block as u64);

            let pending = queue.pending_count();
            let next_expected = queue.next_expected();

            // CRITICAL DEBUG: Log at INFO level to understand queue state
            info!(
                "[RUST-SYNC] State after sync_with_go: go_block={}, go_epoch={}, queue_next_expected={}, pending_count={}, epoch_base={}",
                go_last_block,
                go_epoch,
                next_expected,
                pending,
                self.epoch_base_index.load(Ordering::SeqCst),
            );
        }

        // PHASE 2: Fetch commits from peers and push to queue
        if let Some(ref network_client) = self.network_client {
            // Clone committee before await to drop guard immediately
            let committee_opt: Option<consensus_config::Committee> = {
                let guard = self.committee.read().expect("committee lock poisoned");
                guard.clone()
            }; // guard dropped here

            if let Some(ref committee) = committee_opt {
                let fetch_from_index = {
                    let queue = self.block_queue.lock().await;
                    queue.next_expected() - 1 // Fetch from the block before what we need
                };

                info!(
                    "[RUST-SYNC] PHASE 2: Fetching from index {} (committee has {} authorities)",
                    fetch_from_index,
                    committee.size()
                );

                match self
                    .fetch_and_queue(network_client, committee, fetch_from_index as u32)
                    .await
                {
                    Ok(pushed) => {
                        if pushed == 0 {
                            debug!("[RUST-SYNC] PHASE 2: No commits fetched this iteration");
                        }
                    }
                    Err(e) => {
                        warn!("[RUST-SYNC] PHASE 2 Fetch error: {}, will retry", e);
                    }
                }
            } else {
                warn!("[RUST-SYNC] PHASE 2 SKIPPED: committee is None");
            }
        } else {
            // PHASE 2.5: Use Peer Go Sync when network_client is None (SyncOnly mode)
            info!("[RUST-SYNC] PHASE 2.5: No Mysticeti network, using Peer Go Sync");

            let peer_addresses = self.get_peer_go_addresses();
            if !peer_addresses.is_empty() {
                let fetch_from = {
                    let queue = self.block_queue.lock().await;
                    queue.next_expected()
                };

                // Use turbo batch size for faster catchup - SyncOnly without network usually needs to catch up fast
                let batch_size = self.config.turbo_batch_size as u64;

                match self
                    .fetch_blocks_from_peer_go(&peer_addresses, fetch_from, batch_size)
                    .await
                {
                    Ok(synced) => {
                        if synced > 0 {
                            info!("✅ [PEER-GO-SYNC] Synced {} blocks via Peer Go", synced);
                        }
                    }
                    Err(e) => {
                        debug!("[PEER-GO-SYNC] Peer Go sync failed: {}", e);
                    }
                }
            } else {
                warn!("[RUST-SYNC] PHASE 2.5 SKIPPED: No peer addresses available (network_client is None and committee is None)");
            }
        }

        // PHASE 3: Drain ready commits from queue and send to Go
        let blocks_sent = self.process_queue().await?;
        if blocks_sent > 0 {
            info!(
                "📥 [RUST-SYNC] Sent {} blocks to Go (queue-based)",
                blocks_sent
            );
            // Update throughput gauge
            let round_elapsed = go_query_start.elapsed().as_secs_f64();
            if round_elapsed > 0.0 {
                self.metrics
                    .blocks_per_second
                    .set(blocks_sent as f64 / round_elapsed);
            }
        }
        // =============================================================================
        // PHASE 4: Check pending epoch transitions (from deferred AdvanceEpoch)
        // If sync has caught up to a pending transition's boundary, process it now
        // =============================================================================
        self.check_and_process_pending_epoch_transitions(go_last_block)
            .await;

        // NOTE: Epoch transitions are now handled CENTRALLY by epoch_monitor.rs
        // The unified epoch monitor polls Go/peers every 10s and triggers transitions.
        // This simplifies the sync code and prevents duplicate advance_epoch calls.
        // See epoch_monitor.rs:start_unified_epoch_monitor()

        Ok(())
    }

    /// AUTO-EPOCH-SYNC: Update internal state when Go epoch is ahead
    async fn auto_epoch_sync(&mut self, go_epoch: u64, rust_epoch: u64) {
        info!(
            "🔄 [EPOCH-AUTO-SYNC] Go epoch {} > Rust epoch {}. Updating internal state...",
            go_epoch, rust_epoch
        );

        // Fetch new epoch boundary data from Go
        match self.executor_client.get_epoch_boundary_data(go_epoch).await {
            Ok((_epoch, _ts, new_epoch_base, validators)) => {
                let old_epoch_base = self.epoch_base_index.load(Ordering::SeqCst);

                // CRITICAL: Always rebuild committee on epoch change!
                // Validators may change (added, removed, or replaced) between epochs.
                if !validators.is_empty() {
                    let old_committee_size = {
                        let committee_guard =
                            self.committee.read().expect("committee lock poisoned");
                        committee_guard.as_ref().map(|c| c.size()).unwrap_or(0)
                    };

                    info!(
                        "🔄 [EPOCH-AUTO-SYNC] Epoch changed: {} → {}. Rebuilding committee (old={}, new={} validators)...",
                        rust_epoch, go_epoch, old_committee_size, validators.len()
                    );

                    match crate::node::committee::build_committee_from_validator_list(
                        validators, go_epoch,
                    ) {
                        Ok(new_committee) => {
                            info!(
                                "✅ [EPOCH-AUTO-SYNC] Committee rebuilt with {} authorities for epoch {}",
                                new_committee.size(), go_epoch
                            );

                            // UPDATE NETWORK CLIENT WITH NEW COMMITTEE
                            if let (Some(context), Some(keypair)) =
                                (&self.context, &self.network_keypair)
                            {
                                info!("🔄 [EPOCH-AUTO-SYNC] Rebuilding TonicClient with new committee for epoch {}", go_epoch);
                                let new_context = Arc::new(
                                    (**context).clone().with_committee(new_committee.clone()),
                                );
                                let new_client =
                                    consensus_core::TonicClient::new(new_context, keypair.clone());
                                self.network_client = Some(Arc::new(new_client));
                            }

                            *self.committee.write().expect("committee lock poisoned") =
                                Some(new_committee);
                        }
                        Err(e) => {
                            warn!(
                                "⚠️ [EPOCH-AUTO-SYNC] Failed to rebuild committee: {}. Keeping old committee.",
                                e
                            );
                        }
                    }
                }

                info!(
                    "📊 [EPOCH-AUTO-SYNC] Updated: epoch {} → {}, epoch_base {} → {}",
                    rust_epoch, go_epoch, old_epoch_base, new_epoch_base
                );
                self.current_epoch.store(go_epoch, Ordering::SeqCst);
                self.epoch_base_index
                    .store(new_epoch_base, Ordering::SeqCst);
            }
            Err(e) => {
                warn!(
                    "⚠️ [EPOCH-AUTO-SYNC] Failed to get epoch {} boundary from Go: {}. Will retry next sync cycle.",
                    go_epoch, e
                );
            }
        }
    }

    /// COMMITTEE REFRESH: Check if committee is stale even within the same epoch
    async fn committee_refresh(&mut self, go_epoch: u64) {
        let current_committee_size = {
            let committee_guard = self.committee.read().expect("committee lock poisoned");
            committee_guard.as_ref().map(|c| c.size()).unwrap_or(0)
        };

        // Check if we need to refresh committee (e.g., new validator joined)
        match self.executor_client.get_epoch_boundary_data(go_epoch).await {
            Ok((_epoch, _ts, _boundary, validators)) => {
                if !validators.is_empty() && validators.len() != current_committee_size {
                    info!(
                        "🔄 [COMMITTEE-REFRESH] Committee size mismatch! Current={}, Go has={}. Rebuilding for epoch {}...",
                        current_committee_size, validators.len(), go_epoch
                    );

                    match crate::node::committee::build_committee_from_validator_list(
                        validators, go_epoch,
                    ) {
                        Ok(new_committee) => {
                            info!(
                                "✅ [COMMITTEE-REFRESH] Committee rebuilt with {} authorities for epoch {}",
                                new_committee.size(), go_epoch
                            );

                            // UPDATE NETWORK CLIENT WITH NEW COMMITTEE
                            if let (Some(context), Some(keypair)) =
                                (&self.context, &self.network_keypair)
                            {
                                info!("🔄 [COMMITTEE-REFRESH] Rebuilding TonicClient with new committee for epoch {}", go_epoch);
                                let new_context = Arc::new(
                                    (**context).clone().with_committee(new_committee.clone()),
                                );
                                let new_client =
                                    consensus_core::TonicClient::new(new_context, keypair.clone());
                                self.network_client = Some(Arc::new(new_client));
                            }

                            *self.committee.write().expect("committee lock poisoned") =
                                Some(new_committee);
                        }
                        Err(e) => {
                            warn!(
                                "⚠️ [COMMITTEE-REFRESH] Failed to rebuild committee: {}. Keeping old committee.",
                                e
                            );
                        }
                    }
                }
            }
            Err(_) => {
                // Ignore errors - this is just a refresh check
            }
        }
    }

    /// EPOCH DATA INTEGRITY CHECK
    /// Detect corrupted epoch boundary data that happens when:
    /// - Node advanced epoch via RPC while Go blocks were not synced
    /// - This causes epoch_base to be stuck at an old value
    async fn epoch_data_integrity_check(
        &mut self,
        go_last_block: u64,
        epoch_base: u64,
        rust_epoch: u64,
    ) {
        warn!(
            "🚨 [RUST-SYNC] EPOCH DATA INTEGRITY ISSUE DETECTED! \
             go_block={} < epoch_base={} but epoch={}. \
             This indicates corrupted epoch boundary data (premature epoch advance).",
            go_last_block, epoch_base, rust_epoch
        );

        // Try to refetch epoch boundary from Go Master
        if let Ok((_epoch, _timestamp, new_boundary, _validators)) = self
            .executor_client
            .get_epoch_boundary_data(rust_epoch)
            .await
        {
            if new_boundary != epoch_base {
                info!(
                    "🔄 [RUST-SYNC] Updating epoch_base_index: {} -> {} (from Go Master)",
                    epoch_base, new_boundary
                );
                self.epoch_base_index.store(new_boundary, Ordering::SeqCst);
            } else {
                // Even refetched data is same (corrupted) - try peer discovery!
                warn!(
                    "⚠️ [RUST-SYNC] Go local data still corrupted. Attempting peer discovery for epoch {}...",
                    rust_epoch
                );

                // Build peer RPC addresses from committee - extract before await!
                let peer_addresses: Vec<String> = {
                    let committee_guard = self.committee.read().expect("committee lock poisoned");
                    if let Some(ref committee) = *committee_guard {
                        committee
                            .authorities()
                            .filter_map(|(_, auth)| {
                                let addr_str = auth.address.to_string();
                                if let Some(host) = addr_str.split('/').nth(2) {
                                    Some(format!("{}:8000", host))
                                } else {
                                    None
                                }
                            })
                            .collect()
                    } else {
                        Vec::new()
                    }
                }; // guard dropped here before await

                if !peer_addresses.is_empty() {
                    // Query peers for correct epoch boundary
                    match query_peer_epochs_network(&peer_addresses).await {
                        Ok((peer_epoch, peer_block, best_peer, _peer_global_exec_index)) => {
                            info!(
                                "🌐 [PEER-DISCOVERY] Best peer: epoch={}, block={}, addr={}",
                                peer_epoch, peer_block, best_peer
                            );

                            // If peer has the same epoch, query epoch boundary data
                            if peer_epoch == rust_epoch {
                                match query_peer_epoch_boundary_data(&best_peer, rust_epoch).await {
                                    Ok(boundary_data) => {
                                        let peer_boundary = boundary_data.boundary_block;
                                        if peer_boundary > epoch_base {
                                            info!(
                                                "✅ [PEER-DISCOVERY] Correcting epoch_base_index: {} -> {} (from peer {})",
                                                epoch_base, peer_boundary, best_peer
                                            );
                                            self.epoch_base_index
                                                .store(peer_boundary, Ordering::SeqCst);
                                        } else {
                                            warn!(
                                                "⚠️ [PEER-DISCOVERY] Peer boundary {} not better than local {}",
                                                peer_boundary, epoch_base
                                            );
                                        }
                                    }
                                    Err(e) => {
                                        warn!("⚠️ [PEER-DISCOVERY] Failed to get epoch boundary from peer: {}", e);
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            warn!("⚠️ [PEER-DISCOVERY] No peers responded: {}", e);
                        }
                    }
                }

                // If still not fixed, log error
                let current_base = self.epoch_base_index.load(Ordering::SeqCst);
                if go_last_block as u64 <= current_base {
                    error!(
                        "❌ [RUST-SYNC] CRITICAL: Epoch boundary data is still corrupted! \
                         epoch={}, boundary={}, go_block={}. \
                         MANUAL FIX REQUIRED: Clear epoch data in Go and restart node.",
                        rust_epoch, current_base, go_last_block
                    );
                }
            }
        }
    }
}

// Need Arc in scope for auto_epoch_sync and committee_refresh
use std::sync::Arc;
