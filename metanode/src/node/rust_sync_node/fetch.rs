// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

//! Commit fetching from peers via P2P (epoch-local and global range).

use super::block_queue::CommitData;
use super::RustSyncNode;
use anyhow::Result;
use consensus_config::{AuthorityIndex, Committee};
use consensus_core::{
    BlockAPI, Commit, CommitAPI, CommitRange, GlobalCommitInfo, NetworkClient, SignedBlock,
    TonicClient, VerifiedBlock,
};
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::bytes::Bytes;
use tracing::{debug, info, trace, warn};

impl RustSyncNode {
    pub(super) async fn fetch_and_queue(
        &self,
        network_client: &Arc<TonicClient>,
        committee: &Committee,
        from_global_exec_index: u32,
    ) -> Result<usize> {
        let timeout = Duration::from_secs(self.config.fetch_timeout_secs);
        let batch_size = self.config.fetch_batch_size;
        let current_epoch = self.current_epoch.load(Ordering::SeqCst);
        let epoch_base = self.epoch_base_index.load(Ordering::SeqCst);

        // 🔍 DIAGNOSTIC: Log all key parameters for debugging cross-epoch sync issues
        info!(
            "🔍 [RUST-SYNC-DIAG] fetch_and_queue called: from_global={}, epoch_base={}, current_epoch={}, queue_next_expected={}",
            from_global_exec_index, epoch_base, current_epoch, from_global_exec_index + 1
        );

        // CRITICAL FIX: Convert global_exec_index to epoch-local commit_index
        let from_local_commit = if from_global_exec_index as u64 >= epoch_base {
            // Normal case: requesting blocks within current epoch
            (from_global_exec_index as u64 - epoch_base) as u32
        } else if current_epoch == 1 && epoch_base > 0 {
            // CROSS-EPOCH FIX: Need blocks from epoch 0 (while in epoch 1)
            warn!(
                 "[RUST-SYNC] Fetching pre-epoch blocks for Epoch 1 transition: global={}, epoch_base={}",
                 from_global_exec_index, epoch_base
             );
            from_global_exec_index
        } else {
            // ⚠️ CROSS-EPOCH GAP: from_global_exec_index < epoch_base AND epoch > 1
            warn!(
                "🔄 [RUST-SYNC] CROSS-EPOCH GAP: Go block {} < epoch_base {} (epoch {}). \
                 Using fetch_commits_by_global_range RPC.",
                from_global_exec_index, epoch_base, current_epoch
            );

            return self
                .fetch_and_queue_by_global_range(
                    network_client,
                    committee,
                    from_global_exec_index as u64,
                    (from_global_exec_index + batch_size) as u64,
                )
                .await;
        };

        let commit_range =
            CommitRange::new((from_local_commit + 1)..=(from_local_commit + batch_size));

        info!(
            "[RUST-SYNC] Fetching: global_exec_index={} -> epoch_local_commit={} (epoch_base={}), range={:?}",
            from_global_exec_index, from_local_commit, epoch_base, commit_range
        );

        // =====================================================================
        // PARALLEL FETCH: Race all validators simultaneously for same range
        // =====================================================================
        info!(
            "🚀 [PARALLEL-FETCH] Racing {} validators for range {:?}",
            committee.size(),
            commit_range
        );

        let authorities: Vec<_> = committee
            .authorities()
            .filter(|(_, auth)| {
                // Filter out self to avoid "Connection refused" on loopback
                if let Some(keypair) = &self.network_keypair {
                    auth.network_key != keypair.public()
                } else {
                    true
                }
            })
            .map(|(idx, _)| idx)
            .collect();

        // Filter unhealthy peers via circuit breaker
        let healthy_authorities: Vec<_> = {
            let health = self.peer_health.lock().await;
            authorities
                .iter()
                .filter(|&&idx| health.is_healthy(idx.value() as u32))
                .copied()
                .collect()
        };
        // Fall back to all peers if all are in backoff (to avoid complete stall)
        let active_authorities = if healthy_authorities.is_empty() {
            if !authorities.is_empty() {
                warn!(
                    "⚠️ [CIRCUIT-BREAKER] All {} peers in backoff! Using all peers as fallback.",
                    authorities.len()
                );
            }
            &authorities
        } else {
            if healthy_authorities.len() < authorities.len() {
                info!(
                    "🛡️ [CIRCUIT-BREAKER] Filtered {}/{} unhealthy peers",
                    authorities.len() - healthy_authorities.len(),
                    authorities.len()
                );
            }
            &healthy_authorities
        };

        if active_authorities.is_empty() {
            warn!("⚠️ [RUST-SYNC] No peers available (all filtered out or committee empty).");
            return Ok(0);
        }

        let primary_idx = (commit_range.start() as usize) % active_authorities.len();
        let primary_peer = active_authorities[primary_idx];

        let fetch_timer = self.metrics.peer_fetch_duration_seconds.start_timer();

        info!(
            "🎯 [SMART-SHARDING] Assigned primary peer {} for range {:?} (Hedging in 500ms)",
            primary_peer, commit_range
        );

        let mut fetch_futures = FuturesUnordered::new();

        // Helper to spawn a request
        let spawn_req = |peer_idx: AuthorityIndex,
                         client: Arc<TonicClient>,
                         range: CommitRange,
                         timeout: Duration| {
            async move {
                match client.fetch_commits(peer_idx, range.clone(), timeout).await {
                    Ok((commits, certs)) => {
                        if commits.is_empty() {
                            Err(anyhow::anyhow!("Empty response from {}", peer_idx))
                        } else {
                            Ok((peer_idx, commits, certs))
                        }
                    }
                    Err(e) => Err(anyhow::anyhow!("Peer {} failed: {:?}", peer_idx, e)),
                }
            }
        };

        // Start Primary
        fetch_futures.push(spawn_req(
            primary_peer,
            network_client.clone(),
            commit_range.clone(),
            timeout,
        ));

        let mut hedging_timer = Box::pin(tokio::time::sleep(Duration::from_millis(500)));
        let mut hedged = false;

        let mut serialized_commits: Vec<Bytes> = Vec::new();
        let mut winning_peer = None;

        loop {
            tokio::select! {
                // Trigger Hedging if timer expires (and we haven't hedged yet)
                _ = &mut hedging_timer, if !hedged => {
                    hedged = true;
                    if active_authorities.len() > 1 {
                        info!("🚀 [HEDGING] Primary {} slow (500ms). Racing against others...", primary_peer);
                        for &peer_idx in active_authorities {
                            if peer_idx != primary_peer {
                                fetch_futures.push(spawn_req(peer_idx, network_client.clone(), commit_range.clone(), timeout));
                            }
                        }
                    }
                }

                // Process results
                res = fetch_futures.next() => {
                    match res {
                        Some(Ok((peer_idx, commits, _))) => {
                             if !commits.is_empty() {
                                // Winner! Record health success
                                {
                                    let mut health = self.peer_health.lock().await;
                                    health.record_success(peer_idx.value() as u32);
                                }
                                info!("✅ [RUST-SYNC] Received data from peer {}", peer_idx);
                                serialized_commits = commits;
                                winning_peer = Some(peer_idx);
                                break; // Stop waiting
                             }
                        }
                        Some(Err(e)) => {
                            self.metrics.peer_errors_total.with_label_values(&["fetch"]).inc();
                            trace!("Peer error: {}", e);
                            // If primary failed instantly, trigger hedging NOW
                            if !hedged {
                                hedged = true;
                                info!("⚠️ [RUST-SYNC] Primary failed immediately. Triggering fallback race.");
                                for &peer_idx in active_authorities {
                                    if peer_idx != primary_peer {
                                        fetch_futures.push(spawn_req(peer_idx, network_client.clone(), commit_range.clone(), timeout));
                                    }
                                }
                            }
                        }
                        None => break, // All futures finished (and failed)
                    }
                }
            }
        }

        fetch_timer.observe_duration();

        if serialized_commits.is_empty() {
            // Fallback to global range sync
            warn!(
                "⚠️ [PARALLEL-FETCH] All validators returned empty/error for range {:?}. Falling back to global range!",
                commit_range
            );
            return self
                .fetch_and_queue_by_global_range(
                    network_client,
                    committee,
                    from_global_exec_index as u64,
                    (from_global_exec_index + batch_size) as u64,
                )
                .await;
        }

        let authority_idx = winning_peer
            .ok_or_else(|| anyhow::anyhow!("winning_peer is None despite non-empty commits"))?;
        info!(
            "✅ [PARALLEL-FETCH] Peer {} won race with {} commits",
            authority_idx,
            serialized_commits.len()
        );

        // Now process the winning response
        // Phase 1: Deserialize commits to get block references
        let mut all_block_refs = Vec::new();
        let mut temp_commits = Vec::new();

        let deser_start = Instant::now();
        for serialized in &serialized_commits {
            match bcs::from_bytes::<Commit>(serialized) {
                Ok(commit) => {
                    // Collect all block refs from this commit
                    for block_ref in commit.blocks() {
                        all_block_refs.push(block_ref.clone());
                    }
                    temp_commits.push(commit);
                }
                Err(e) => {
                    warn!("[RUST-SYNC] Failed to deserialize commit: {}", e);
                }
            }
        }
        self.metrics
            .deserialize_duration_seconds
            .observe(deser_start.elapsed().as_secs_f64());

        if all_block_refs.is_empty() {
            info!(
                "📥 [RUST-SYNC] Fetched {} commits with 0 block refs from peer {}",
                serialized_commits.len(),
                authority_idx
            );
        } else {
            info!(
                "📥 [RUST-SYNC] Fetched {} commits with {} block refs from peer {}, fetching actual blocks...",
                serialized_commits.len(),
                all_block_refs.len(),
                authority_idx
            );
        }

        // Phase 2: Fetch actual DAG blocks using fetch_blocks API
        let mut block_map = std::collections::HashMap::new();

        if !all_block_refs.is_empty() {
            // Deduplicate block refs
            all_block_refs.sort();
            all_block_refs.dedup();

            match network_client
                .fetch_blocks(
                    authority_idx,
                    all_block_refs.clone(),
                    vec![], // No highest_accepted_rounds for commit sync
                    false,  // Not breadth first
                    timeout,
                )
                .await
            {
                Ok(serialized_blocks) => {
                    info!(
                        "📦 [RUST-SYNC] Fetched {} actual blocks from peer {}",
                        serialized_blocks.len(),
                        authority_idx
                    );

                    let block_deser_start = Instant::now();
                    for serialized in &serialized_blocks {
                        match bcs::from_bytes::<SignedBlock>(serialized) {
                            Ok(signed_block) => {
                                let verified =
                                    VerifiedBlock::new_verified(signed_block, serialized.clone());
                                block_map.insert(verified.reference(), verified);
                            }
                            Err(e) => {
                                warn!("[RUST-SYNC] Failed to deserialize block: {}", e);
                            }
                        }
                    }
                    self.metrics
                        .deserialize_duration_seconds
                        .observe(block_deser_start.elapsed().as_secs_f64());
                }
                Err(e) => {
                    warn!(
                        "⚠️ [RUST-SYNC] Failed to fetch blocks from peer {}: {:?}",
                        authority_idx, e
                    );
                }
            }
        }

        // Phase 3: Push commits with their blocks to queue
        let mut pushed = 0;
        let mut write_batch = consensus_core::storage::WriteBatch::default();
        {
            let mut queue = self.block_queue.lock().await;

            for (commit, serialized_commit) in temp_commits.into_iter().zip(serialized_commits) {
                // Collect blocks for this commit
                let mut commit_blocks = Vec::new();
                for block_ref in commit.blocks() {
                    if let Some(block) = block_map.get(block_ref) {
                        commit_blocks.push(block.clone());
                    }
                }

                let expected_blocks = commit.blocks().len();
                let actual_blocks = commit_blocks.len();

                if actual_blocks < expected_blocks {
                    debug!(
                        "[RUST-SYNC] Commit {} has {}/{} blocks",
                        commit.index(),
                        actual_blocks,
                        expected_blocks
                    );
                }

                // Log commit details to debug missing blocks
                let global_idx = commit.global_exec_index();
                let commit_idx = commit.index();
                let next_expected = queue.next_expected();
                let will_add =
                    global_idx >= next_expected && !queue.pending.contains_key(&global_idx);

                // 🔍 DIAGNOSTIC: Detect gap and trigger global range fallback
                let gap = if global_idx > next_expected {
                    global_idx - next_expected
                } else {
                    0
                };

                // EPOCH MISMATCH DETECTION
                let negative_gap = if next_expected > global_idx {
                    next_expected - global_idx
                } else {
                    0
                };

                if negative_gap > 1000 && pushed == 0 {
                    warn!(
                        "🚨 [EPOCH-MISMATCH] CRITICAL! commit[{}].global_exec_index={} but queue_next_expected={}. \
                         Negative gap: {} blocks. This indicates EPOCH MISMATCH!",
                        commit_idx, global_idx, next_expected, negative_gap
                    );
                    drop(queue);
                    return Ok(0); // Return 0 pushed to trigger epoch recovery in caller
                }

                if gap > 10 && pushed == 0 {
                    warn!(
                        "🔄 [RUST-SYNC] GAP DETECTED! commit[{}].global_exec_index={} but queue_next_expected={}. \
                         Gap size: {} blocks. Falling back to global range RPC!",
                        commit_idx, global_idx, next_expected, gap
                    );
                    drop(queue);
                    return self
                        .fetch_and_queue_by_global_range(
                            network_client,
                            committee,
                            next_expected,
                            global_idx.saturating_sub(1),
                        )
                        .await;
                }

                if pushed < 5 || (global_idx <= 5) {
                    info!(
                        "📋 [RUST-SYNC] Commit: commit_index={}, global_exec_index={}, blocks={}, will_add={}, queue_next_expected={}{}",
                        commit_idx, global_idx, actual_blocks, will_add, next_expected,
                        if !will_add { " ⚠️ REJECTED (global_idx < next_expected or already pending)" } else { "" }
                    );
                }

                // Push to queue - it will deduplicate and sort automatically
                // CRITICAL FIX: Derive epoch from consensus blocks, NOT sync node's current_epoch.
                // Each VerifiedBlock carries the epoch it was created in during consensus.
                // This prevents epoch mismatch when syncing blocks across epoch boundaries
                // (e.g., blocks committed in epoch 0 being incorrectly stamped as epoch 1).
                let block_epoch = commit_blocks
                    .first()
                    .map(|b| b.epoch() as u64)
                    .unwrap_or(current_epoch);
                if block_epoch != current_epoch {
                    info!(
                        "🔄 [EPOCH-FIX] Using block epoch {} (from consensus) instead of current_epoch {} for commit global_exec_index={}",
                        block_epoch, current_epoch, global_idx
                    );
                }

                // Prepare storage persistence
                if will_add {
                    write_batch.blocks.extend(commit_blocks.clone());
                    let trusted_commit = consensus_core::TrustedCommit::new_trusted(
                        commit.clone(),
                        serialized_commit.into(),
                    );
                    write_batch.commits.push(trusted_commit);
                }

                queue.push(CommitData {
                    commit,
                    blocks: commit_blocks,
                    epoch: block_epoch,
                });
                pushed += 1;
            }
        }

        // Persist to store if configured
        if pushed > 0 {
            if let Some(store) = &self.store {
                if !write_batch.commits.is_empty() || !write_batch.blocks.is_empty() {
                    if let Err(e) = store.write(write_batch) {
                        warn!(
                            "⚠️ [RUST-SYNC] Failed to persist fetched blocks/commits to Store: {}",
                            e
                        );
                    } else {
                        debug!("💾 [RUST-SYNC] Persisted synced commits and blocks to Store");
                    }
                }
            }
        }

        self.metrics.blocks_received_total.inc_by(pushed as f64);

        info!(
            "✅ [RUST-SYNC] Pushed {} commits with {} blocks to queue",
            pushed,
            block_map.len()
        );

        Ok(pushed)
    }

    /// Fetch commits using global execution index range (cross-epoch safe)
    /// This bypasses epoch-local coordinate conversion entirely
    pub(super) async fn fetch_and_queue_by_global_range(
        &self,
        network_client: &Arc<TonicClient>,
        committee: &Committee,
        start_global_index: u64,
        end_global_index: u64,
    ) -> Result<usize> {
        let timeout = Duration::from_secs(self.config.fetch_timeout_secs);
        let _current_epoch = self.current_epoch.load(Ordering::SeqCst);

        info!(
            "🌐 [GLOBAL-SYNC] Fetching commits by global range [{}, {}]",
            start_global_index, end_global_index
        );

        // Try each validator in turn until we succeed (skip unhealthy peers)
        for (authority_idx, _authority) in committee.authorities() {
            // Circuit breaker: skip unhealthy peers
            {
                let health = self.peer_health.lock().await;
                if !health.is_healthy(authority_idx.value() as u32) {
                    debug!("🛡️ [GLOBAL-SYNC] Skipping unhealthy peer {}", authority_idx);
                    continue;
                }
            }
            match network_client
                .fetch_commits_by_global_range(
                    authority_idx,
                    start_global_index,
                    end_global_index,
                    timeout,
                )
                .await
            {
                Ok(global_commits) => {
                    if global_commits.is_empty() {
                        debug!(
                            "[GLOBAL-SYNC] Peer {} returned empty commits for range [{}, {}]",
                            authority_idx, start_global_index, end_global_index
                        );
                        continue;
                    }

                    info!(
                        "📥 [GLOBAL-SYNC] Got {} commits from peer {} for global range [{}, {}]",
                        global_commits.len(),
                        authority_idx,
                        start_global_index,
                        end_global_index
                    );

                    // Collect block refs from all commits
                    let mut all_block_refs: Vec<consensus_types::block::BlockRef> = Vec::new();
                    let mut temp_commits: Vec<(GlobalCommitInfo, Commit)> = Vec::new();

                    for global_info in global_commits {
                        // Deserialize the commit
                        match bcs::from_bytes::<Commit>(&global_info.commit_data) {
                            Ok(commit) => {
                                // Collect block refs
                                for block_ref in commit.blocks() {
                                    all_block_refs.push(block_ref.clone());
                                }
                                temp_commits.push((global_info, commit));
                            }
                            Err(e) => {
                                warn!("[GLOBAL-SYNC] Failed to deserialize commit: {}", e);
                            }
                        }
                    }

                    // Fetch actual blocks
                    let mut block_map = std::collections::HashMap::new();
                    if !all_block_refs.is_empty() {
                        all_block_refs.sort();
                        all_block_refs.dedup();

                        // Convert CommitRef to BlockRef for fetch_blocks
                        let block_refs_for_fetch: Vec<_> = all_block_refs
                            .iter()
                            .filter_map(|cr| {
                                // CommitRef is actually BlockRef in this context
                                Some(cr.clone())
                            })
                            .collect();

                        match network_client
                            .fetch_blocks(
                                authority_idx,
                                block_refs_for_fetch,
                                vec![],
                                false,
                                timeout,
                            )
                            .await
                        {
                            Ok(serialized_blocks) => {
                                for serialized in &serialized_blocks {
                                    match bcs::from_bytes::<SignedBlock>(serialized) {
                                        Ok(signed_block) => {
                                            let verified = VerifiedBlock::new_verified(
                                                signed_block,
                                                serialized.clone(),
                                            );
                                            block_map.insert(verified.reference(), verified);
                                        }
                                        Err(e) => {
                                            warn!(
                                                "[GLOBAL-SYNC] Failed to deserialize block: {}",
                                                e
                                            );
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                warn!("[GLOBAL-SYNC] Failed to fetch blocks: {:?}", e);
                            }
                        }
                    }

                    // Push commits with their blocks to queue
                    let mut pushed = 0;
                    let mut chunk_write_batch = consensus_core::storage::WriteBatch::default();
                    {
                        let mut queue = self.block_queue.lock().await;

                        for (global_info, commit) in temp_commits {
                            let mut commit_blocks = Vec::new();
                            for block_ref in commit.blocks() {
                                if let Some(block) = block_map.get(block_ref) {
                                    commit_blocks.push(block.clone());
                                }
                            }

                            let epoch_from_peer = global_info.epoch;
                            let global_idx = global_info.global_exec_index;

                            let will_add = global_idx >= queue.next_expected()
                                && !queue.pending.contains_key(&global_idx);

                            if will_add {
                                chunk_write_batch.blocks.extend(commit_blocks.clone());
                                let trusted_commit = consensus_core::TrustedCommit::new_trusted(
                                    commit.clone(),
                                    global_info.commit_data.clone(),
                                );
                                chunk_write_batch.commits.push(trusted_commit);
                            }

                            info!(
                                "📋 [GLOBAL-SYNC] Commit: global_exec_index={}, epoch={}, local_commit={}, blocks={}",
                                global_idx, epoch_from_peer, global_info.local_commit_index, commit_blocks.len()
                            );

                            queue.push(CommitData {
                                commit,
                                blocks: commit_blocks,
                                epoch: epoch_from_peer,
                            });
                            pushed += 1;
                        }
                    }

                    // Persist to store if configured
                    if pushed > 0 {
                        if let Some(store) = &self.store {
                            if !chunk_write_batch.commits.is_empty()
                                || !chunk_write_batch.blocks.is_empty()
                            {
                                if let Err(e) = store.write(chunk_write_batch) {
                                    warn!("⚠️ [GLOBAL-SYNC] Failed to persist blocks/commits to Store: {}", e);
                                } else {
                                    debug!("💾 [GLOBAL-SYNC] Persisted synced commits and blocks to Store");
                                }
                            }
                        }
                    }

                    self.metrics.blocks_received_total.inc_by(pushed as f64);

                    info!(
                        "✅ [GLOBAL-SYNC] Pushed {} commits via global range RPC",
                        pushed
                    );

                    // Record successful peer interaction
                    {
                        let mut health = self.peer_health.lock().await;
                        health.record_success(authority_idx.value() as u32);
                    }

                    return Ok(pushed);
                }
                Err(e) => {
                    // Record failed peer interaction
                    {
                        let mut health = self.peer_health.lock().await;
                        health.record_failure(authority_idx.value() as u32);
                    }
                    debug!("[GLOBAL-SYNC] Peer {} fetch failed: {:?}", authority_idx, e);
                    continue;
                }
            }
        }

        warn!(
            "[GLOBAL-SYNC] All peers failed for global range [{}, {}]",
            start_global_index, end_global_index
        );
        Ok(0)
    }

    /// Fetch blocks from peer's Go layer using PeerGoClient
    /// This is used when network_client is None (SyncOnly) or as a fallback
    /// Returns the number of blocks successfully sent to local Go
    pub async fn fetch_blocks_from_peer_go(
        &self,
        peer_addresses: &[String],
        from_block: u64,
        batch_size: u64,
    ) -> Result<usize> {
        use crate::node::peer_go_client::PeerGoClient;

        if peer_addresses.is_empty() {
            return Err(anyhow::anyhow!("No peer addresses configured"));
        }

        let to_block = from_block + batch_size - 1;

        info!(
            "🌐 [PEER-GO-SYNC] Fetching blocks {} to {} from peer Go layers",
            from_block, to_block
        );

        // Try each peer until one succeeds
        for (peer_index, peer_addr) in peer_addresses.iter().enumerate() {
            // Circuit breaker: skip unhealthy peers (use address index as peer_id)
            {
                let health = self.peer_health.lock().await;
                if !health.is_healthy(peer_index as u32) {
                    debug!(
                        "🛡️ [PEER-GO-SYNC] Skipping unhealthy peer {} ({})",
                        peer_index, peer_addr
                    );
                    continue;
                }
            }
            let client = match PeerGoClient::from_str(peer_addr) {
                Ok(c) => c,
                Err(e) => {
                    debug!(
                        "⚠️ [PEER-GO-SYNC] Invalid peer address {}: {}",
                        peer_addr, e
                    );
                    continue;
                }
            };

            match client.get_blocks_range(from_block, to_block).await {
                Ok(blocks) => {
                    if blocks.is_empty() {
                        debug!("[PEER-GO-SYNC] Peer {} returned 0 blocks", peer_addr);
                        continue;
                    }

                    info!(
                        "✅ [PEER-GO-SYNC] Got {} blocks from peer {}, sending to local Go",
                        blocks.len(),
                        peer_addr
                    );

                    // Send blocks to local Go via ExecutorClient
                    for block in &blocks {
                        let block_num = block.block_number;
                        let epoch = block.epoch;

                        // Check epoch transition
                        let current_epoch =
                            self.current_epoch.load(std::sync::atomic::Ordering::SeqCst);
                        if epoch > current_epoch {
                            info!(
                                "🎯 [PEER-GO-SYNC] Epoch transition detected in block {}: {} -> {}",
                                block_num, current_epoch, epoch
                            );

                            // Trigger epoch transition
                            if let Ok((_e, timestamp, boundary, _, _)) =
                                self.executor_client.get_epoch_boundary_data(epoch).await
                            {
                                let _ = self
                                    .epoch_transition_sender
                                    .send((epoch, timestamp, boundary));
                                self.current_epoch
                                    .store(epoch, std::sync::atomic::Ordering::SeqCst);
                            }
                        }
                    }

                    // Use sync_blocks to write all blocks at once
                    match self.executor_client.sync_blocks(blocks).await {
                        Ok((synced, last_block)) => {
                            self.metrics.blocks_received_total.inc_by(synced as f64);
                            // Record successful peer interaction
                            {
                                let mut health = self.peer_health.lock().await;
                                health.record_success(peer_index as u32);
                            }
                            info!(
                                "✅ [PEER-GO-SYNC] Synced {} blocks to local Go (last: {})",
                                synced, last_block
                            );
                            return Ok(synced as usize);
                        }
                        Err(e) => {
                            // Record failed peer interaction
                            {
                                let mut health = self.peer_health.lock().await;
                                health.record_failure(peer_index as u32);
                            }
                            warn!("⚠️ [PEER-GO-SYNC] Failed to sync blocks to local Go: {}", e);
                            return Err(e);
                        }
                    }
                }
                Err(e) => {
                    // Record failed peer interaction
                    {
                        let mut health = self.peer_health.lock().await;
                        health.record_failure(peer_index as u32);
                    }
                    debug!("[PEER-GO-SYNC] Peer {} fetch failed: {}", peer_addr, e);
                    continue;
                }
            }
        }

        Err(anyhow::anyhow!("All peers failed to provide blocks"))
    }

    /// Get peer Go addresses from committee (if available) or config
    pub fn get_peer_go_addresses(&self) -> Vec<String> {
        let committee_guard = self.committee.read().expect("committee lock poisoned");
        if let Some(ref committee) = *committee_guard {
            committee
                .authorities()
                .filter_map(|(_, auth)| {
                    let addr_str = auth.address.to_string();
                    // Extract host from /dns/hostname/tcp/port format
                    if let Some(host) = addr_str.split('/').nth(2) {
                        // Use peer_rpc_port (8000 base)
                        Some(format!("{}:8000", host))
                    } else {
                        None
                    }
                })
                .collect()
        } else {
            Vec::new()
        }
    }
}
