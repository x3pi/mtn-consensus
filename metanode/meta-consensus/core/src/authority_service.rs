// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use std::{
    collections::{BTreeMap, BTreeSet},
    pin::Pin,
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use bytes::Bytes;
use consensus_config::AuthorityIndex;
use consensus_types::block::{BlockRef, Round};
use futures::{ready, stream, task, Stream, StreamExt};
use meta_macros::fail_point_async;
use mysten_metrics::spawn_monitored_task;
use parking_lot::RwLock;
use rand::seq::SliceRandom as _;
use tap::TapFallible;
use tokio::sync::broadcast;
use tokio_util::sync::ReusableBoxFuture;
use tracing::{debug, info, warn};

use crate::{
    block::{BlockAPI as _, ExtendedBlock, SignedBlock, VerifiedBlock, GENESIS_ROUND},
    block_verifier::BlockVerifier,
    commit::{CommitAPI as _, CommitRange, TrustedCommit},
    commit_vote_monitor::CommitVoteMonitor,
    context::Context,
    core_thread::CoreThreadDispatcher,
    dag_state::DagState,
    epoch_change::{EpochChangeProposal, EpochChangeVote},
    epoch_change_provider::EpochChangeProcessor,
    error::{ConsensusError, ConsensusResult},
    legacy_store::LegacyEpochStoreManager,
    network::{BlockStream, ExtendedSerializedBlock, NetworkService},
    round_tracker::PeerRoundTracker,
    stake_aggregator::{QuorumThreshold, StakeAggregator},
    storage::Store,
    synchronizer::SynchronizerHandle,
    transaction_certifier::TransactionCertifier,
    CommitIndex,
};

pub(crate) const COMMIT_LAG_MULTIPLIER: u32 = 10000;

/// Authority's network service implementation, agnostic to the actual networking stack used.
pub(crate) struct AuthorityService<C: CoreThreadDispatcher> {
    context: Arc<Context>,
    commit_vote_monitor: Arc<CommitVoteMonitor>,
    block_verifier: Arc<dyn BlockVerifier>,
    synchronizer: Arc<SynchronizerHandle>,
    core_dispatcher: Arc<C>,
    rx_block_broadcast: broadcast::Receiver<ExtendedBlock>,
    subscription_counter: Arc<SubscriptionCounter>,
    transaction_certifier: TransactionCertifier,
    dag_state: Arc<RwLock<DagState>>,
    store: Arc<dyn Store>,
    round_tracker: Arc<RwLock<PeerRoundTracker>>,
    epoch_change_processor: Arc<RwLock<Option<Box<dyn EpochChangeProcessor>>>>,
    /// Legacy store manager for querying previous epochs (optional for backward compatibility)
    legacy_store_manager: Option<Arc<LegacyEpochStoreManager>>,
    /// Epoch base index (boundary block of the previous epoch). Used when there are no
    /// commits in the store (e.g., snapshot restore cold start) to correctly compute
    /// global_exec_index ranges.
    epoch_base_index: u64,
    /// Cache of recently verified block refs to skip re-verification.
    /// Bounded to prevent unbounded memory growth.
    #[allow(dead_code)]
    recently_verified_blocks: Arc<RwLock<BTreeSet<BlockRef>>>,
}

impl<C: CoreThreadDispatcher> AuthorityService<C> {
    pub(crate) fn new(
        context: Arc<Context>,
        block_verifier: Arc<dyn BlockVerifier>,
        commit_vote_monitor: Arc<CommitVoteMonitor>,
        round_tracker: Arc<RwLock<PeerRoundTracker>>,
        synchronizer: Arc<SynchronizerHandle>,
        core_dispatcher: Arc<C>,
        rx_block_broadcast: broadcast::Receiver<ExtendedBlock>,
        transaction_certifier: TransactionCertifier,
        dag_state: Arc<RwLock<DagState>>,
        store: Arc<dyn Store>,
        epoch_change_processor: Option<Box<dyn EpochChangeProcessor>>,
        legacy_store_manager: Option<Arc<LegacyEpochStoreManager>>,
        epoch_base_index: u64,
    ) -> Self {
        let subscription_counter = Arc::new(SubscriptionCounter::new(context.clone()));
        Self {
            context,
            block_verifier,
            commit_vote_monitor,
            synchronizer,
            core_dispatcher,
            rx_block_broadcast,
            subscription_counter,
            transaction_certifier,
            dag_state,
            store,
            round_tracker,
            epoch_change_processor: Arc::new(RwLock::new(epoch_change_processor)),
            legacy_store_manager,
            epoch_base_index,
            recently_verified_blocks: Arc::new(RwLock::new(BTreeSet::new())),
        }
    }

    // Parses and validates serialized excluded ancestors.
    fn parse_excluded_ancestors(
        &self,
        peer: AuthorityIndex,
        block: &VerifiedBlock,
        mut excluded_ancestors: Vec<Vec<u8>>,
    ) -> ConsensusResult<Vec<BlockRef>> {
        let peer_hostname = &self.context.committee.authority(peer).hostname;

        let excluded_ancestors_limit = self.context.committee.size() * 2;
        if excluded_ancestors.len() > excluded_ancestors_limit {
            debug!(
                "Dropping {} excluded ancestor(s) from {} {} due to size limit",
                excluded_ancestors.len() - excluded_ancestors_limit,
                peer,
                peer_hostname,
            );
            excluded_ancestors.truncate(excluded_ancestors_limit);
        }

        let excluded_ancestors = excluded_ancestors
            .into_iter()
            .map(|serialized| {
                let block_ref: BlockRef =
                    bcs::from_bytes(&serialized).map_err(ConsensusError::MalformedBlock)?;
                if !self.context.committee.is_valid_index(block_ref.author) {
                    return Err(ConsensusError::InvalidAuthorityIndex {
                        index: block_ref.author,
                        max: self.context.committee.size(),
                    });
                }
                if block_ref.round >= block.round() {
                    return Err(ConsensusError::InvalidAncestorRound {
                        ancestor: block_ref.round,
                        block: block.round(),
                    });
                }
                Ok(block_ref)
            })
            .collect::<ConsensusResult<Vec<BlockRef>>>()?;

        for excluded_ancestor in &excluded_ancestors {
            let excluded_ancestor_hostname = &self
                .context
                .committee
                .authority(excluded_ancestor.author)
                .hostname;
            self.context
                .metrics
                .node_metrics
                .network_excluded_ancestors_count_by_authority
                .with_label_values(&[excluded_ancestor_hostname])
                .inc();
        }
        self.context
            .metrics
            .node_metrics
            .network_received_excluded_ancestors_from_authority
            .with_label_values(&[peer_hostname])
            .inc_by(excluded_ancestors.len() as u64);

        Ok(excluded_ancestors)
    }
}

#[async_trait]
impl<C: CoreThreadDispatcher> NetworkService for AuthorityService<C> {
    async fn handle_send_block(
        &self,
        peer: AuthorityIndex,
        serialized_block: ExtendedSerializedBlock,
    ) -> ConsensusResult<()> {
        fail_point_async!("consensus-rpc-response");

        let peer_hostname = &self.context.committee.authority(peer).hostname;

        // Dedup block verifications: skip expensive signature check if we
        // already verified this block recently (e.g., from broadcast + fetch).
        let signed_block: SignedBlock =
            bcs::from_bytes(&serialized_block.block).map_err(ConsensusError::MalformedBlock)?;

        // Reject blocks not produced by the peer.
        if peer != signed_block.author() {
            self.context
                .metrics
                .node_metrics
                .invalid_blocks
                .with_label_values(&[peer_hostname, "handle_send_block", "UnexpectedAuthority"])
                .inc();
            let e = ConsensusError::UnexpectedAuthority(signed_block.author(), peer);
            info!("Block with wrong authority from {}: {}", peer, e);
            return Err(e);
        }
        let peer_hostname = &self.context.committee.authority(peer).hostname;

        // Reject blocks failing validations.
        let (verified_block, reject_txn_votes) = self
            .block_verifier
            .verify_and_vote(signed_block, serialized_block.block)
            .tap_err(|e| {
                self.context
                    .metrics
                    .node_metrics
                    .invalid_blocks
                    .with_label_values(&[peer_hostname, "handle_send_block", e.name()])
                    .inc();
                info!("Invalid block from {}: {}", peer, e);
            })?;
        let block_ref = verified_block.reference();
        debug!("Received block {} via send block.", block_ref);

        let now = self.context.clock.timestamp_utc_ms();
        let forward_time_drift =
            Duration::from_millis(verified_block.timestamp_ms().saturating_sub(now));

        self.context
            .metrics
            .node_metrics
            .block_timestamp_drift_ms
            .with_label_values(&[peer_hostname, "handle_send_block"])
            .inc_by(forward_time_drift.as_millis() as u64);

        // Observe the block for the commit votes. When local commit is lagging too much,
        // commit sync loop will trigger fetching.
        self.commit_vote_monitor.observe_block(&verified_block);

        // Reject blocks when local commit index is lagging too far from quorum commit index,
        // to avoid the memory overhead from suspended blocks.
        //
        // IMPORTANT: this must be done after observing votes from the block, otherwise
        // observed quorum commit will no longer progress.
        //
        // Since the main issue with too many suspended blocks is memory usage not CPU,
        // it is ok to reject after block verifications instead of before.
        let last_commit_index = self.dag_state.read().last_commit_index();
        let quorum_commit_index = self.commit_vote_monitor.quorum_commit_index();
        // The threshold to ignore block should be larger than commit_sync_batch_size,
        // to avoid excessive block rejections and synchronizations.
        if last_commit_index
            + self.context.parameters.commit_sync_batch_size * COMMIT_LAG_MULTIPLIER
            < quorum_commit_index
        {
            self.context
                .metrics
                .node_metrics
                .rejected_blocks
                .with_label_values(&["commit_lagging"])
                .inc();
            debug!(
                "Block {:?} is rejected because last commit index is lagging quorum commit index too much ({} < {})",
                block_ref, last_commit_index, quorum_commit_index,
            );
            return Err(ConsensusError::BlockRejected {
                block_ref,
                reason: format!(
                    "Last commit index is lagging quorum commit index too much ({} < {})",
                    last_commit_index, quorum_commit_index,
                ),
            });
        }

        self.context
            .metrics
            .node_metrics
            .verified_blocks
            .with_label_values(&[peer_hostname])
            .inc();

        // The block is verified and current, so it can be processed in the fastpath.
        if self.context.protocol_config.mysticeti_fastpath() {
            self.transaction_certifier
                .add_voted_blocks(vec![(verified_block.clone(), reject_txn_votes)]);
        }

        // Process epoch change data from block before accepting into DAG
        let proposal_bytes = verified_block.epoch_change_proposal().map(|v| v.as_slice());
        let votes_bytes: Vec<Vec<u8>> = verified_block
            .epoch_change_votes()
            .iter()
            .map(|v| v.clone())
            .collect();
        if proposal_bytes.is_some() || !votes_bytes.is_empty() {
            crate::epoch_change_provider::process_block_epoch_change(proposal_bytes, &votes_bytes);
        }

        // Try to accept the block into the DAG.
        let missing_ancestors = self
            .core_dispatcher
            .add_blocks(vec![verified_block.clone()])
            .await
            .map_err(|_| ConsensusError::Shutdown)?;

        // Schedule fetching missing ancestors from this peer in the background.
        if !missing_ancestors.is_empty() {
            self.context
                .metrics
                .node_metrics
                .handler_received_block_missing_ancestors
                .with_label_values(&[peer_hostname])
                .inc_by(missing_ancestors.len() as u64);
            let synchronizer = self.synchronizer.clone();
            spawn_monitored_task!(async move {
                // This does not wait for the fetch request to complete.
                // It only waits for synchronizer to queue the request to a peer.
                // When this fails, it usually means the queue is full.
                // The fetch will retry from other peers via live and periodic syncs.
                if let Err(err) = synchronizer.fetch_blocks(missing_ancestors, peer).await {
                    debug!("Failed to fetch missing ancestors via synchronizer: {err}");
                }
            });
        }

        // ------------ After processing the block, process the excluded ancestors ------------

        let excluded_ancestors = self
            .parse_excluded_ancestors(peer, &verified_block, serialized_block.excluded_ancestors)
            .tap_err(|e| {
                debug!("Failed to parse excluded ancestors from {peer} {peer_hostname}: {e}");
                self.context
                    .metrics
                    .node_metrics
                    .invalid_blocks
                    .with_label_values(&[peer_hostname, "handle_send_block", e.name()])
                    .inc();
            })?;

        self.round_tracker
            .write()
            .update_from_verified_block(&ExtendedBlock {
                block: verified_block,
                excluded_ancestors: excluded_ancestors.clone(),
            });

        let missing_excluded_ancestors = self
            .core_dispatcher
            .check_block_refs(excluded_ancestors)
            .await
            .map_err(|_| ConsensusError::Shutdown)?;

        // Schedule fetching missing soft links from this peer in the background.
        if !missing_excluded_ancestors.is_empty() {
            self.context
                .metrics
                .node_metrics
                .network_excluded_ancestors_sent_to_fetch
                .with_label_values(&[peer_hostname])
                .inc_by(missing_excluded_ancestors.len() as u64);

            let synchronizer = self.synchronizer.clone();
            spawn_monitored_task!(async move {
                if let Err(err) = synchronizer
                    .fetch_blocks(missing_excluded_ancestors, peer)
                    .await
                {
                    debug!("Failed to fetch excluded ancestors via synchronizer: {err}");
                }
            });
        }

        Ok(())
    }

    async fn handle_subscribe_blocks(
        &self,
        peer: AuthorityIndex,
        last_received: Round,
    ) -> ConsensusResult<BlockStream> {
        fail_point_async!("consensus-rpc-response");

        let dag_state = self.dag_state.read();
        // Find recent own blocks that have not been received by the peer.
        // If last_received is a valid and more blocks have been proposed since then, this call is
        // guaranteed to return at least some recent blocks, which will help with liveness.
        let missed_blocks = stream::iter(
            dag_state
                .get_cached_blocks(self.context.own_index, last_received + 1)
                .into_iter()
                .map(|block| ExtendedSerializedBlock {
                    block: block.serialized().clone(),
                    excluded_ancestors: vec![],
                }),
        );

        let broadcasted_blocks = BroadcastedBlockStream::new(
            peer,
            self.rx_block_broadcast.resubscribe(),
            self.subscription_counter.clone(),
        );

        // Return a stream of blocks that first yields missed blocks as requested, then new blocks.
        Ok(Box::pin(missed_blocks.chain(
            broadcasted_blocks.map(ExtendedSerializedBlock::from),
        )))
    }

    // Handles two types of requests:
    // 1. Missing block for block sync:
    //    - uses highest_accepted_rounds.
    //    - max_blocks_per_sync blocks should be returned.
    // 2. Committed block for commit sync:
    //    - does not use highest_accepted_rounds.
    //    - max_blocks_per_fetch blocks should be returned.
    async fn handle_fetch_blocks(
        &self,
        _peer: AuthorityIndex,
        mut block_refs: Vec<BlockRef>,
        highest_accepted_rounds: Vec<Round>,
        breadth_first: bool,
    ) -> ConsensusResult<Vec<Bytes>> {
        fail_point_async!("consensus-rpc-response");

        if !highest_accepted_rounds.is_empty()
            && highest_accepted_rounds.len() != self.context.committee.size()
        {
            return Err(ConsensusError::InvalidSizeOfHighestAcceptedRounds(
                highest_accepted_rounds.len(),
                self.context.committee.size(),
            ));
        }

        // Some quick validation of the requested block refs
        let max_response_num_blocks = if !highest_accepted_rounds.is_empty() {
            self.context.parameters.max_blocks_per_sync
        } else {
            self.context.parameters.max_blocks_per_fetch
        };
        if block_refs.len() > max_response_num_blocks {
            block_refs.truncate(max_response_num_blocks);
        }

        // Validate the requested block refs.
        for block in &block_refs {
            // CROSS-EPOCH FIX: Only validate AuthorityIndex strictly for current epoch blocks.
            // Historical blocks from previous epochs may have different committee sizes.
            // When legacy_store_manager is present, we allow any index and let the store lookup
            // return blocks (or empty) naturally - blocks from other epochs will be found in legacy stores.
            if self.legacy_store_manager.is_none() {
                // No legacy stores = current epoch only, strict validation
                if !self.context.committee.is_valid_index(block.author) {
                    return Err(ConsensusError::InvalidAuthorityIndex {
                        index: block.author,
                        max: self.context.committee.size(),
                    });
                }
            }
            if block.round == GENESIS_ROUND {
                return Err(ConsensusError::UnexpectedGenesisBlockRequested);
            }
        }

        // Get requested blocks from store.
        let mut blocks = if !highest_accepted_rounds.is_empty() {
            block_refs.sort();
            block_refs.dedup();
            let mut blocks = self
                .dag_state
                .read()
                .get_blocks(&block_refs)
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();

            if breadth_first {
                // Get unique missing ancestor blocks of the requested blocks.
                let mut missing_ancestors = blocks
                    .iter()
                    .flat_map(|block| block.ancestors().to_vec())
                    .filter(|block_ref| highest_accepted_rounds[block_ref.author] < block_ref.round)
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>();

                // If there are too many missing ancestors, randomly select a subset to avoid
                // fetching duplicated blocks across peers.
                let selected_num_blocks = max_response_num_blocks.saturating_sub(blocks.len());
                if selected_num_blocks < missing_ancestors.len() {
                    missing_ancestors = missing_ancestors
                        .choose_multiple(&mut rand::thread_rng(), selected_num_blocks)
                        .copied()
                        .collect::<Vec<_>>();
                }
                let ancestor_blocks = self.dag_state.read().get_blocks(&missing_ancestors);
                blocks.extend(ancestor_blocks.into_iter().flatten());
            } else {
                // Get additional blocks from authorities with missing block, if they are available in cache.
                // Compute the lowest missing round per requested authority.
                let mut lowest_missing_rounds = BTreeMap::<AuthorityIndex, Round>::new();
                for block_ref in blocks.iter().map(|b| b.reference()) {
                    let entry = lowest_missing_rounds
                        .entry(block_ref.author)
                        .or_insert(block_ref.round);
                    *entry = (*entry).min(block_ref.round);
                }

                // Retrieve additional blocks per authority, from peer's highest accepted round + 1 to
                // lowest missing round (exclusive) per requested authority.
                // No block from other authorities are retrieved. It is possible that the requestor is not
                // seeing missing block from another authority, and serving a block would just lead to unnecessary
                // data transfer. Or missing blocks from other authorities are requested from other peers.
                let dag_state = self.dag_state.read();
                for (authority, lowest_missing_round) in lowest_missing_rounds {
                    let highest_accepted_round = highest_accepted_rounds[authority];
                    if highest_accepted_round >= lowest_missing_round {
                        continue;
                    }
                    let missing_blocks = dag_state.get_cached_blocks_in_range(
                        authority,
                        highest_accepted_round + 1,
                        lowest_missing_round,
                        self.context
                            .parameters
                            .max_blocks_per_sync
                            .saturating_sub(blocks.len()),
                    );
                    blocks.extend(missing_blocks);
                    if blocks.len() >= self.context.parameters.max_blocks_per_sync {
                        blocks.truncate(self.context.parameters.max_blocks_per_sync);
                        break;
                    }
                }
            }

            blocks
        } else {
            self.dag_state
                .read()
                .get_blocks(&block_refs)
                .into_iter()
                .flatten()
                .collect()
        };

        // STORE FALLBACK: For blocks not found in dag_state cache
        // First try current epoch's RocksDB store, then legacy stores
        let blocks_found = blocks.len();
        if blocks_found < block_refs.len() {
            // Collect block refs that were not found
            let found_refs: std::collections::HashSet<_> =
                blocks.iter().map(|b| b.reference()).collect();
            let mut missing_refs: Vec<_> = block_refs
                .iter()
                .filter(|r| !found_refs.contains(r))
                .cloned()
                .collect();

            // STEP 1: Try current epoch's RocksDB store (self.store)
            if !missing_refs.is_empty() {
                info!(
                    "🔄 [FETCH-BLOCKS] {} blocks missing from dag_state, searching current store...",
                    missing_refs.len()
                );

                if let Ok(store_blocks) = self.store.read_blocks(&missing_refs) {
                    let found_in_store: Vec<_> = store_blocks.into_iter().flatten().collect();
                    if !found_in_store.is_empty() {
                        info!(
                            "✅ [STORE SYNC] Found {} blocks in current epoch store",
                            found_in_store.len()
                        );
                        // Update missing_refs to exclude found blocks
                        let newly_found: std::collections::HashSet<_> =
                            found_in_store.iter().map(|b| b.reference()).collect();
                        missing_refs.retain(|r| !newly_found.contains(r));
                        blocks.extend(found_in_store);
                    }
                }
            }

            // STEP 2: Try legacy epoch stores for remaining missing blocks
            if !missing_refs.is_empty() {
                if let Some(ref legacy_manager) = self.legacy_store_manager {
                    info!(
                        "🔄 [FETCH-BLOCKS] {} blocks still missing, searching legacy stores...",
                        missing_refs.len()
                    );

                    // Search all legacy stores for missing blocks
                    for (epoch, legacy_store) in legacy_manager.get_all_stores() {
                        if let Ok(legacy_blocks) = legacy_store.read_blocks(&missing_refs) {
                            let found_legacy: Vec<_> =
                                legacy_blocks.into_iter().flatten().collect();
                            if !found_legacy.is_empty() {
                                info!(
                                    "✅ [LEGACY SYNC] Found {} blocks in epoch {} store",
                                    found_legacy.len(),
                                    epoch
                                );
                                blocks.extend(found_legacy);
                            }
                        }
                    }
                }
            }
        }

        // Return the serialized blocks
        let bytes = blocks
            .into_iter()
            .map(|block| block.serialized().clone())
            .collect::<Vec<_>>();
        Ok(bytes)
    }

    async fn handle_fetch_commits(
        &self,
        _peer: AuthorityIndex,
        commit_range: CommitRange,
    ) -> ConsensusResult<(Vec<TrustedCommit>, Vec<VerifiedBlock>)> {
        fail_point_async!("consensus-rpc-response");

        // Compute an inclusive end index and bound the maximum number of commits scanned.
        let inclusive_end = commit_range.end().min(
            commit_range.start() + self.context.parameters.commit_sync_batch_size as CommitIndex
                - 1,
        );

        // First try to get commits from current store
        let mut commits = self
            .store
            .scan_commits((commit_range.start()..=inclusive_end).into())?;

        // CRITICAL LOGGING: Trace commits found for sync debugging
        if !commits.is_empty() {
            let commit_indices: Vec<u32> = commits.iter().map(|c| c.index()).collect();
            info!(
                "📦 [FETCH-COMMITS] Found {} commits in range {:?}: indices={:?}",
                commits.len(),
                commit_range,
                commit_indices
            );
        } else {
            info!(
                "⚠️ [FETCH-COMMITS] No commits found in current store for range {:?}",
                commit_range
            );
        }

        // If current store is empty, try legacy stores (for lagging node sync)
        let mut is_legacy = false;
        let mut legacy_certifier_blocks = vec![];
        if commits.is_empty() {
            if let Some(ref legacy_manager) = self.legacy_store_manager {
                for (epoch, legacy_store) in legacy_manager.get_all_stores() {
                    if let Ok(legacy_commits) =
                        legacy_store.scan_commits((commit_range.start()..=inclusive_end).into())
                    {
                        if !legacy_commits.is_empty() {
                            info!(
                                "📦 [LEGACY SYNC] Found {} commits in epoch {} store for range {:?}",
                                legacy_commits.len(),
                                epoch,
                                commit_range
                            );
                            if let Some(c) = legacy_commits.last() {
                                if let Ok(votes) =
                                    legacy_store.read_commit_votes(c.index(), c.digest())
                                {
                                    if let Ok(blocks) = legacy_store.read_blocks(&votes) {
                                        legacy_certifier_blocks =
                                            blocks.into_iter().flatten().collect();
                                    }
                                }
                            }
                            commits = legacy_commits;
                            is_legacy = true; // Mark as legacy to skip current-committee validation
                            break;
                        }
                    }
                }
            }
        }

        let mut certifier_block_refs = vec![];

        // CRITICAL FIX: Skip quorum validation for Legacy commits
        // Legacy commits were finalized in a previous epoch and verified against THAT epoch's committee.
        // Validating them against the CURRENT committee (self.context.committee) will fail (different keys/weights).
        if !is_legacy {
            'commit: while let Some(c) = commits.last() {
                let index = c.index();
                let votes = self.store.read_commit_votes(index, c.digest())?;
                let mut stake_aggregator = StakeAggregator::<QuorumThreshold>::new();
                for v in &votes {
                    stake_aggregator.add(v.author, &self.context.committee);
                }
                if stake_aggregator.reached_threshold(&self.context.committee) {
                    certifier_block_refs = votes;
                    break 'commit;
                } else {
                    info!(
                        "⚠️ [FETCH-COMMITS] Commit {} votes did not reach quorum ({}/{} stake), skipping from end",
                        index,
                        stake_aggregator.stake(),
                        stake_aggregator.threshold(&self.context.committee)
                    );
                    self.context
                        .metrics
                        .node_metrics
                        .commit_sync_fetch_commits_handler_uncertified_skipped
                        .inc();
                    commits.pop();
                }
            }
        }

        // Log final commits being returned
        if !commits.is_empty() {
            let final_indices: Vec<u32> = commits.iter().map(|c| c.index()).collect();
            info!(
                "✅ [FETCH-COMMITS] Returning {} commits: indices={:?}",
                commits.len(),
                final_indices
            );
        } else {
            info!("⚠️ [FETCH-COMMITS] No commits to return after quorum check");
        }
        let certifier_blocks = if is_legacy {
            legacy_certifier_blocks
        } else {
            self.store
                .read_blocks(&certifier_block_refs)?
                .into_iter()
                .flatten()
                .collect()
        };
        Ok((commits, certifier_blocks))
    }

    /// Handles fetch_commits_by_global_range - searches current epoch
    /// Note: For now, this only supports the current epoch. Legacy epoch support
    /// requires epoch_base metadata which is not yet stored in legacy stores.
    async fn handle_fetch_commits_by_global_range(
        &self,
        _peer: AuthorityIndex,
        start_global_index: u64,
        end_global_index: u64,
    ) -> ConsensusResult<Vec<crate::network::tonic_network::GlobalCommitInfo>> {
        fail_point_async!("consensus-rpc-response");

        use crate::network::tonic_network::GlobalCommitInfo;

        let mut result: Vec<GlobalCommitInfo> = Vec::new();
        let batch_limit = self.context.parameters.commit_sync_batch_size as u64;
        let max_global_index = end_global_index.min(start_global_index + batch_limit - 1);

        let current_epoch = self.context.committee.epoch();

        // Read last commit info to determine epoch_base_index
        // epoch_base_index = global_exec_index_of_last_block_in_previous_epoch
        // For epoch 0, epoch_base = 0
        // For epoch N, epoch_base = boundary_block_of_epoch_N
        let last_commit_index = self.dag_state.read().last_commit_index();

        // For current epoch, we calculate epoch_base from the first commit:
        // epoch_base = first_commit.global_exec_index - first_commit.index
        let current_epoch_base: u64 = if current_epoch == 0 {
            0
        } else {
            // Read the first commit to determine epoch_base
            if let Ok(first_commits) = self.store.scan_commits((1..=1).into()) {
                if let Some(first_commit) = first_commits.first() {
                    let first_global_idx = first_commit.global_exec_index();
                    let first_local_idx = first_commit.index() as u64;
                    let calculated_base = first_global_idx.saturating_sub(first_local_idx);
                    info!(
                        "📊 [FETCH-GLOBAL] Calculated epoch_base from first commit: \
                         first_commit.global_exec_index={}, first_commit.index={}, epoch_base={}",
                        first_global_idx, first_local_idx, calculated_base
                    );
                    calculated_base
                } else {
                    // CRITICAL FIX: Use epoch_base_index from constructor instead of 0!
                    // On snapshot restore / cold start, there are no commits yet but
                    // the epoch_base_index was correctly set during authority startup.
                    warn!(
                        "⚠️ [FETCH-GLOBAL] No commits in store, using epoch_base_index={} from constructor",
                        self.epoch_base_index
                    );
                    self.epoch_base_index
                }
            } else {
                warn!(
                    "⚠️ [FETCH-GLOBAL] Failed to read first commit, using epoch_base_index={} from constructor",
                    self.epoch_base_index
                );
                self.epoch_base_index
            }
        };

        info!(
            "📦 [FETCH-GLOBAL] Searching commits in global range [{}, {}] (epoch={}, base={}, last_commit={})",
            start_global_index, max_global_index, current_epoch, current_epoch_base, last_commit_index
        );

        // Calculate which local commit indices we need
        let local_start = if start_global_index > current_epoch_base {
            (start_global_index - current_epoch_base) as u32
        } else {
            1 // Start from first commit
        };
        let local_end = if max_global_index > current_epoch_base {
            (max_global_index - current_epoch_base) as u32
        } else {
            0 // Range is before this epoch
        };

        if local_end >= local_start && local_end <= last_commit_index {
            if let Ok(commits) = self.store.scan_commits((local_start..=local_end).into()) {
                for commit in commits {
                    let commit_index = commit.index();
                    let global_idx = current_epoch_base + commit_index as u64;

                    // Get block refs for this commit
                    let block_refs: Vec<bytes::Bytes> = commit
                        .blocks()
                        .iter()
                        .filter_map(|r| bcs::to_bytes(r).ok().map(|b| b.into()))
                        .collect();

                    result.push(GlobalCommitInfo {
                        epoch: current_epoch as u64,
                        global_exec_index: global_idx,
                        local_commit_index: commit_index,
                        epoch_boundary_block: current_epoch_base,
                        commit_data: commit.serialized().clone(),
                        block_refs,
                    });
                }
                info!(
                    "✅ [FETCH-GLOBAL] Found {} commits in epoch {} (local range [{}, {}])",
                    result.len(),
                    current_epoch,
                    local_start,
                    local_end
                );
            }
        } else {
            info!(
                "📭 [FETCH-GLOBAL] No commits in range (local=[{}, {}], last={})",
                local_start, local_end, last_commit_index
            );
        }

        // STORE-HOPPING: If the requested range includes blocks before current epoch,
        // we MUST search legacy stores for those blocks (even if current epoch has some results).
        // E.g., request [101, 236] with epoch_base=136 needs blocks 101-136 from previous epoch.
        if start_global_index < current_epoch_base {
            if let Some(ref legacy_manager) = self.legacy_store_manager {
                info!(
                    "🔄 [STORE-HOP] Range [{}, {}] is before current epoch base {}, searching legacy stores...",
                    start_global_index, max_global_index, current_epoch_base
                );

                // Get all legacy stores sorted by epoch (oldest first)
                let mut legacy_stores = legacy_manager.get_all_stores();
                legacy_stores.sort_by_key(|(epoch, _)| *epoch);

                for (legacy_epoch, legacy_store) in legacy_stores {
                    // For legacy stores, we need to derive epoch_base from the first commit
                    let legacy_epoch_base = if legacy_epoch == 0 {
                        0u64
                    } else {
                        // Read first commit to calculate epoch_base
                        if let Ok(first_commits) = legacy_store.scan_commits((1..=1).into()) {
                            if let Some(first_commit) = first_commits.first() {
                                let first_global = first_commit.global_exec_index();
                                let first_local = first_commit.index() as u64;
                                first_global.saturating_sub(first_local)
                            } else {
                                continue; // No commits in this store
                            }
                        } else {
                            continue;
                        }
                    };

                    // Calculate local range for this legacy epoch
                    let leg_local_start = if start_global_index > legacy_epoch_base {
                        (start_global_index - legacy_epoch_base) as u32
                    } else {
                        1
                    };
                    let leg_local_end = if max_global_index > legacy_epoch_base {
                        (max_global_index - legacy_epoch_base) as u32
                    } else {
                        0
                    };

                    if leg_local_end >= leg_local_start {
                        // Also need to check against the last commit in this legacy store
                        // Scan a batch to find commits
                        if let Ok(commits) =
                            legacy_store.scan_commits((leg_local_start..=leg_local_end).into())
                        {
                            for commit in commits {
                                let commit_index = commit.index();
                                let global_idx = commit.global_exec_index();

                                // Verify this commit is within the requested range
                                if global_idx >= start_global_index
                                    && global_idx <= max_global_index
                                {
                                    let block_refs: Vec<bytes::Bytes> = commit
                                        .blocks()
                                        .iter()
                                        .filter_map(|r| bcs::to_bytes(r).ok().map(|b| b.into()))
                                        .collect();

                                    result.push(GlobalCommitInfo {
                                        epoch: legacy_epoch,
                                        global_exec_index: global_idx,
                                        local_commit_index: commit_index,
                                        epoch_boundary_block: legacy_epoch_base,
                                        commit_data: commit.serialized().clone(),
                                        block_refs,
                                    });
                                }
                            }

                            if !result.is_empty() {
                                info!(
                                    "✅ [STORE-HOP] Found {} commits in legacy epoch {} for range [{}, {}]",
                                    result.len(), legacy_epoch, start_global_index, max_global_index
                                );
                                // break; // Found commits, no need to search older epochs
                            }
                        }
                    }
                }
            } else {
                info!(
                    "⚠️ [STORE-HOP] No legacy store manager available, cannot search previous epochs"
                );
            }
        }

        // Sort by global index to ensure order
        result.sort_by_key(|c| c.global_exec_index);

        info!(
            "✅ [FETCH-GLOBAL] Returning {} commits for range [{}, {}]",
            result.len(),
            start_global_index,
            max_global_index
        );

        Ok(result)
    }

    async fn handle_fetch_latest_blocks(
        &self,
        peer: AuthorityIndex,
        authorities: Vec<AuthorityIndex>,
    ) -> ConsensusResult<Vec<Bytes>> {
        fail_point_async!("consensus-rpc-response");

        if authorities.len() > self.context.committee.size() {
            return Err(ConsensusError::TooManyAuthoritiesProvided(peer));
        }

        // Ensure that those are valid authorities
        for authority in &authorities {
            if !self.context.committee.is_valid_index(*authority) {
                return Err(ConsensusError::InvalidAuthorityIndex {
                    index: *authority,
                    max: self.context.committee.size(),
                });
            }
        }

        // Read from the dag state to find the latest blocks.
        // TODO: at the moment we don't look into the block manager for suspended blocks. Ideally we
        // want in the future if we think we would like to tackle the majority of cases.
        let mut blocks = vec![];
        let dag_state = self.dag_state.read();
        for authority in authorities {
            let block = dag_state.get_last_block_for_authority(authority);

            debug!("Latest block for {authority}: {block:?} as requested from {peer}");

            // no reason to serve back the genesis block - it's equal as if it has not received any block
            if block.round() != GENESIS_ROUND {
                blocks.push(block);
            }
        }

        // Return the serialised blocks
        let result = blocks
            .into_iter()
            .map(|block| block.serialized().clone())
            .collect::<Vec<_>>();

        Ok(result)
    }

    async fn handle_get_latest_rounds(
        &self,
        _peer: AuthorityIndex,
    ) -> ConsensusResult<(Vec<Round>, Vec<Round>)> {
        fail_point_async!("consensus-rpc-response");

        let mut highest_received_rounds = self.core_dispatcher.highest_received_rounds();

        let blocks = self
            .dag_state
            .read()
            .get_last_cached_block_per_authority(Round::MAX);
        let highest_accepted_rounds = blocks
            .into_iter()
            .map(|(block, _)| block.round())
            .collect::<Vec<_>>();

        // Own blocks do not go through the core dispatcher, so they need to be set separately.
        highest_received_rounds[self.context.own_index] =
            highest_accepted_rounds[self.context.own_index];

        Ok((highest_received_rounds, highest_accepted_rounds))
    }

    async fn handle_send_epoch_change_proposal(
        &self,
        _peer: AuthorityIndex,
        proposal: EpochChangeProposal,
    ) -> ConsensusResult<()> {
        // Forward to epoch change processor if available
        if let Some(ref processor) = *self.epoch_change_processor.read() {
            let proposal_bytes = bcs::to_bytes(&proposal).map_err(|e| {
                ConsensusError::NetworkRequest(format!("serialize proposal failed: {e:?}"))
            })?;
            processor.process_proposal(&proposal_bytes);
        }

        Ok(())
    }

    async fn handle_send_epoch_change_vote(
        &self,
        _peer: AuthorityIndex,
        vote: EpochChangeVote,
    ) -> ConsensusResult<()> {
        // Forward to epoch change processor if available
        if let Some(ref processor) = *self.epoch_change_processor.read() {
            let vote_bytes = bcs::to_bytes(&vote).map_err(|e| {
                ConsensusError::NetworkRequest(format!("serialize vote failed: {e:?}"))
            })?;
            processor.process_vote(&vote_bytes);
        }

        Ok(())
    }
}

struct Counter {
    count: usize,
    subscriptions_by_authority: Vec<usize>,
}

/// Atomically counts the number of active subscriptions to the block broadcast stream.
struct SubscriptionCounter {
    context: Arc<Context>,
    counter: parking_lot::Mutex<Counter>,
}

impl SubscriptionCounter {
    fn new(context: Arc<Context>) -> Self {
        // Set the subscribed peers by default to 0
        for (_, authority) in context.committee.authorities() {
            context
                .metrics
                .node_metrics
                .subscribed_by
                .with_label_values(&[authority.hostname.as_str()])
                .set(0);
        }

        Self {
            counter: parking_lot::Mutex::new(Counter {
                count: 0,
                subscriptions_by_authority: vec![0; context.committee.size()],
            }),
            context,
        }
    }

    fn increment(&self, peer: AuthorityIndex) -> Result<(), ConsensusError> {
        let mut counter = self.counter.lock();
        counter.count += 1;
        counter.subscriptions_by_authority[peer] += 1;

        let peer_hostname = &self.context.committee.authority(peer).hostname;
        self.context
            .metrics
            .node_metrics
            .subscribed_by
            .with_label_values(&[peer_hostname])
            .set(1);

        Ok(())
    }

    fn decrement(&self, peer: AuthorityIndex) -> Result<(), ConsensusError> {
        let mut counter = self.counter.lock();
        counter.count -= 1;
        counter.subscriptions_by_authority[peer] -= 1;

        if counter.subscriptions_by_authority[peer] == 0 {
            let peer_hostname = &self.context.committee.authority(peer).hostname;
            self.context
                .metrics
                .node_metrics
                .subscribed_by
                .with_label_values(&[peer_hostname])
                .set(0);
        }

        Ok(())
    }
}

/// Each broadcasted block stream wraps a broadcast receiver for blocks.
/// It yields blocks that are broadcasted after the stream is created.
type BroadcastedBlockStream = BroadcastStream<ExtendedBlock>;

/// Adapted from `tokio_stream::wrappers::BroadcastStream`. The main difference is that
/// this tolerates lags with only logging, without yielding errors.
struct BroadcastStream<T> {
    peer: AuthorityIndex,
    // Stores the receiver across poll_next() calls.
    inner: ReusableBoxFuture<
        'static,
        (
            Result<T, broadcast::error::RecvError>,
            broadcast::Receiver<T>,
        ),
    >,
    // Counts total subscriptions / active BroadcastStreams.
    subscription_counter: Arc<SubscriptionCounter>,
}

impl<T: 'static + Clone + Send> BroadcastStream<T> {
    pub fn new(
        peer: AuthorityIndex,
        rx: broadcast::Receiver<T>,
        subscription_counter: Arc<SubscriptionCounter>,
    ) -> Self {
        if let Err(err) = subscription_counter.increment(peer) {
            match err {
                ConsensusError::Shutdown => {}
                _ => panic!("Unexpected error: {err}"),
            }
        }
        Self {
            peer,
            inner: ReusableBoxFuture::new(make_recv_future(rx)),
            subscription_counter,
        }
    }
}

impl<T: 'static + Clone + Send> Stream for BroadcastStream<T> {
    type Item = T;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
    ) -> task::Poll<Option<Self::Item>> {
        let peer = self.peer;
        let maybe_item = loop {
            let (result, rx) = ready!(self.inner.poll(cx));
            self.inner.set(make_recv_future(rx));

            match result {
                Ok(item) => break Some(item),
                Err(broadcast::error::RecvError::Closed) => {
                    info!("Block BroadcastedBlockStream {} closed", peer);
                    break None;
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!(
                        "Block BroadcastedBlockStream {} lagged by {} messages",
                        peer, n
                    );
                    continue;
                }
            }
        };
        task::Poll::Ready(maybe_item)
    }
}

impl<T> Drop for BroadcastStream<T> {
    fn drop(&mut self) {
        if let Err(err) = self.subscription_counter.decrement(self.peer) {
            match err {
                ConsensusError::Shutdown => {}
                _ => panic!("Unexpected error: {err}"),
            }
        }
    }
}

async fn make_recv_future<T: Clone>(
    mut rx: broadcast::Receiver<T>,
) -> (
    Result<T, broadcast::error::RecvError>,
    broadcast::Receiver<T>,
) {
    let result = rx.recv().await;
    (result, rx)
}

// TODO: add a unit test for BroadcastStream.

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        sync::Arc,
        time::Duration,
    };

    use async_trait::async_trait;
    use bytes::Bytes;
    use consensus_config::AuthorityIndex;
    use consensus_types::block::{BlockDigest, BlockRef, Round};
    use mysten_metrics::monitored_mpsc;
    use parking_lot::{Mutex, RwLock};
    use tokio::{sync::broadcast, time::sleep};

    use crate::{
        authority_service::AuthorityService,
        block::{BlockAPI, SignedBlock, TestBlock, VerifiedBlock},
        commit::{CertifiedCommits, CommitRange},
        commit_vote_monitor::CommitVoteMonitor,
        context::Context,
        core_thread::{CoreError, CoreThreadDispatcher},
        dag_state::DagState,
        error::ConsensusResult,
        network::{BlockStream, ExtendedSerializedBlock, NetworkClient, NetworkService},
        round_tracker::PeerRoundTracker,
        storage::mem_store::MemStore,
        synchronizer::Synchronizer,
        test_dag_builder::DagBuilder,
        transaction_certifier::TransactionCertifier,
    };
    struct FakeCoreThreadDispatcher {
        blocks: Mutex<Vec<VerifiedBlock>>,
    }

    impl FakeCoreThreadDispatcher {
        fn new() -> Self {
            Self {
                blocks: Mutex::new(vec![]),
            }
        }

        fn get_blocks(&self) -> Vec<VerifiedBlock> {
            self.blocks.lock().clone()
        }
    }

    #[async_trait]
    impl CoreThreadDispatcher for FakeCoreThreadDispatcher {
        async fn add_blocks(
            &self,
            blocks: Vec<VerifiedBlock>,
        ) -> Result<BTreeSet<BlockRef>, CoreError> {
            let block_refs = blocks.iter().map(|b| b.reference()).collect();
            self.blocks.lock().extend(blocks);
            Ok(block_refs)
        }

        async fn check_block_refs(
            &self,
            _block_refs: Vec<BlockRef>,
        ) -> Result<BTreeSet<BlockRef>, CoreError> {
            Ok(BTreeSet::new())
        }

        async fn add_certified_commits(
            &self,
            _commits: CertifiedCommits,
        ) -> Result<BTreeSet<BlockRef>, CoreError> {
            todo!()
        }

        async fn new_block(&self, _round: Round, _force: bool) -> Result<(), CoreError> {
            Ok(())
        }

        async fn get_missing_blocks(&self) -> Result<BTreeSet<BlockRef>, CoreError> {
            Ok(Default::default())
        }

        fn set_propagation_delay(&self, _propagation_delay: Round) -> Result<(), CoreError> {
            todo!()
        }

        fn set_last_known_proposed_round(&self, _round: Round) -> Result<(), CoreError> {
            todo!()
        }

        fn highest_received_rounds(&self) -> Vec<Round> {
            todo!()
        }
    }

    #[derive(Default)]
    struct FakeNetworkClient {}

    #[async_trait]
    impl NetworkClient for FakeNetworkClient {
        async fn send_block(
            &self,
            _peer: AuthorityIndex,
            _block: &VerifiedBlock,
            _timeout: Duration,
        ) -> ConsensusResult<()> {
            unimplemented!("Unimplemented")
        }

        async fn subscribe_blocks(
            &self,
            _peer: AuthorityIndex,
            _last_received: Round,
            _timeout: Duration,
        ) -> ConsensusResult<BlockStream> {
            unimplemented!("Unimplemented")
        }

        async fn fetch_blocks(
            &self,
            _peer: AuthorityIndex,
            _block_refs: Vec<BlockRef>,
            _highest_accepted_rounds: Vec<Round>,
            _breadth_first: bool,
            _timeout: Duration,
        ) -> ConsensusResult<Vec<Bytes>> {
            unimplemented!("Unimplemented")
        }

        async fn fetch_commits(
            &self,
            _peer: AuthorityIndex,
            _commit_range: CommitRange,
            _timeout: Duration,
        ) -> ConsensusResult<(Vec<Bytes>, Vec<Bytes>)> {
            unimplemented!("Unimplemented")
        }

        async fn fetch_commits_by_global_range(
            &self,
            _peer: AuthorityIndex,
            _start_global_index: u64,
            _max_global_index: u64,
            _timeout: Duration,
        ) -> ConsensusResult<Vec<crate::network::tonic_network::GlobalCommitInfo>> {
            unimplemented!("Unimplemented")
        }

        async fn send_epoch_change_proposal(
            &self,
            _peer: AuthorityIndex,
            _proposal: &crate::epoch_change::EpochChangeProposal,
            _timeout: Duration,
        ) -> ConsensusResult<()> {
            unimplemented!("Unimplemented")
        }

        async fn send_epoch_change_vote(
            &self,
            _peer: AuthorityIndex,
            _vote: &crate::epoch_change::EpochChangeVote,
            _timeout: Duration,
        ) -> ConsensusResult<()> {
            unimplemented!("Unimplemented")
        }

        async fn fetch_latest_blocks(
            &self,
            _peer: AuthorityIndex,
            _authorities: Vec<AuthorityIndex>,
            _timeout: Duration,
        ) -> ConsensusResult<Vec<Bytes>> {
            unimplemented!("Unimplemented")
        }

        async fn get_latest_rounds(
            &self,
            _peer: AuthorityIndex,
            _timeout: Duration,
        ) -> ConsensusResult<(Vec<Round>, Vec<Round>)> {
            unimplemented!("Unimplemented")
        }
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn test_handle_send_block() {
        let (context, _keys) = Context::new_for_test(4);
        let context = Arc::new(context);
        let block_verifier = Arc::new(crate::block_verifier::NoopBlockVerifier {});
        let commit_vote_monitor = Arc::new(CommitVoteMonitor::new(context.clone()));
        let core_dispatcher = Arc::new(FakeCoreThreadDispatcher::new());
        let (_tx_block_broadcast, rx_block_broadcast) = broadcast::channel(100);
        let network_client = Arc::new(FakeNetworkClient::default());
        let (blocks_sender, _blocks_receiver) =
            monitored_mpsc::unbounded_channel("consensus_block_output");
        let store = Arc::new(MemStore::new());
        let dag_state = Arc::new(RwLock::new(DagState::new(context.clone(), store.clone())));
        let transaction_certifier = TransactionCertifier::new(
            context.clone(),
            block_verifier.clone(),
            dag_state.clone(),
            blocks_sender,
        );
        let synchronizer = Synchronizer::start(
            network_client,
            context.clone(),
            core_dispatcher.clone(),
            commit_vote_monitor.clone(),
            block_verifier.clone(),
            transaction_certifier.clone(),
            dag_state.clone(),
            false,
        );
        let round_tracker = Arc::new(RwLock::new(PeerRoundTracker::new(context.clone())));
        let authority_service = Arc::new(AuthorityService::new(
            context.clone(),
            block_verifier,
            commit_vote_monitor,
            round_tracker,
            synchronizer,
            core_dispatcher.clone(),
            rx_block_broadcast,
            transaction_certifier,
            dag_state,
            store,
            None,
            None, // legacy_store_manager
            0,    // epoch_base_index (tests start at epoch 0)
        ));

        // Test delaying blocks with time drift.
        let now = context.clock.timestamp_utc_ms();
        let max_drift = context.parameters.max_forward_time_drift;
        let input_block = VerifiedBlock::new_for_test(
            TestBlock::new(9, 0)
                .set_timestamp_ms(now + max_drift.as_millis() as u64)
                .build(),
        );

        let service = authority_service.clone();
        let serialized = ExtendedSerializedBlock {
            block: input_block.serialized().clone(),
            excluded_ancestors: vec![],
        };

        tokio::spawn({
            let service = service.clone();
            let context = context.clone();
            async move {
                service
                    .handle_send_block(context.committee.to_authority_index(0).unwrap(), serialized)
                    .await
                    .unwrap();
            }
        });

        sleep(max_drift / 2).await;

        let blocks = core_dispatcher.get_blocks();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0], input_block);

        // Test invalid block.
        let invalid_block =
            VerifiedBlock::new_for_test(TestBlock::new(10, 1000).set_timestamp_ms(10).build());
        let extended_block = ExtendedSerializedBlock {
            block: invalid_block.serialized().clone(),
            excluded_ancestors: vec![],
        };
        service
            .handle_send_block(
                context.committee.to_authority_index(0).unwrap(),
                extended_block,
            )
            .await
            .unwrap_err();

        // Test invalid excluded ancestors.
        let invalid_excluded_ancestors = vec![
            bcs::to_bytes(&BlockRef::new(
                10,
                AuthorityIndex::new_for_test(1000),
                BlockDigest::MIN,
            ))
            .unwrap(),
            vec![3u8; 40],
            bcs::to_bytes(&invalid_block.reference()).unwrap(),
        ];
        let extended_block = ExtendedSerializedBlock {
            block: input_block.serialized().clone(),
            excluded_ancestors: invalid_excluded_ancestors,
        };
        service
            .handle_send_block(
                context.committee.to_authority_index(0).unwrap(),
                extended_block,
            )
            .await
            .unwrap_err();
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn test_handle_fetch_blocks() {
        // GIVEN
        // Use NUM_AUTHORITIES and NUM_ROUNDS higher than max_blocks_per_sync to test limits.
        const NUM_AUTHORITIES: usize = 40;
        const NUM_ROUNDS: usize = 40;
        let (context, _keys) = Context::new_for_test(NUM_AUTHORITIES);
        let context = Arc::new(context);
        let block_verifier = Arc::new(crate::block_verifier::NoopBlockVerifier {});
        let commit_vote_monitor = Arc::new(CommitVoteMonitor::new(context.clone()));
        let core_dispatcher = Arc::new(FakeCoreThreadDispatcher::new());
        let (_tx_block_broadcast, rx_block_broadcast) = broadcast::channel(100);
        let network_client = Arc::new(FakeNetworkClient::default());
        let (blocks_sender, _blocks_receiver) =
            monitored_mpsc::unbounded_channel("consensus_block_output");
        let store = Arc::new(MemStore::new());
        let dag_state = Arc::new(RwLock::new(DagState::new(context.clone(), store.clone())));
        let transaction_certifier = TransactionCertifier::new(
            context.clone(),
            block_verifier.clone(),
            dag_state.clone(),
            blocks_sender,
        );
        let synchronizer = Synchronizer::start(
            network_client,
            context.clone(),
            core_dispatcher.clone(),
            commit_vote_monitor.clone(),
            block_verifier.clone(),
            transaction_certifier.clone(),
            dag_state.clone(),
            false,
        );
        let round_tracker = Arc::new(RwLock::new(PeerRoundTracker::new(context.clone())));
        let authority_service = Arc::new(AuthorityService::new(
            context.clone(),
            block_verifier,
            commit_vote_monitor,
            round_tracker,
            synchronizer,
            core_dispatcher.clone(),
            rx_block_broadcast,
            transaction_certifier,
            dag_state.clone(),
            store,
            None,
            None, // legacy_store_manager
            0,    // epoch_base_index (tests start at epoch 0)
        ));

        // GIVEN: 40 rounds of blocks in the dag state.
        let mut dag_builder = DagBuilder::new(context.clone());
        dag_builder
            .layers(1..=(NUM_ROUNDS as u32))
            .build()
            .persist_layers(dag_state.clone());
        let all_blocks = dag_builder.all_blocks();

        // WHEN: Request 2 blocks from round 40, get ancestors breadth first.
        let missing_block_refs: Vec<BlockRef> = all_blocks
            .iter()
            .rev()
            .take(2)
            .map(|b| b.reference())
            .collect();
        let highest_accepted_rounds: Vec<Round> = vec![1; NUM_AUTHORITIES];
        let results = authority_service
            .handle_fetch_blocks(
                AuthorityIndex::new_for_test(0),
                missing_block_refs.clone(),
                highest_accepted_rounds,
                true,
            )
            .await
            .unwrap();

        // THEN: the expected number of unique blocks are returned.
        let blocks: BTreeMap<BlockRef, VerifiedBlock> = results
            .iter()
            .map(|b| {
                let signed = bcs::from_bytes(b).unwrap();
                let block = VerifiedBlock::new_verified(signed, b.clone());
                (block.reference(), block)
            })
            .collect();
        assert_eq!(blocks.len(), context.parameters.max_blocks_per_sync);
        // All missing blocks are returned.
        for b in &missing_block_refs {
            assert!(blocks.contains_key(b));
        }
        let num_missing_ancestors = blocks
            .keys()
            .filter(|b| b.round == NUM_ROUNDS as Round - 1)
            .count();
        assert_eq!(
            num_missing_ancestors,
            context.parameters.max_blocks_per_sync - missing_block_refs.len()
        );

        // WHEN: Request 2 blocks from round 37, get ancestors depth first.
        let missing_round = NUM_ROUNDS as Round - 3;
        let missing_block_refs: Vec<BlockRef> = all_blocks
            .iter()
            .filter(|b| b.reference().round == missing_round)
            .map(|b| b.reference())
            .take(2)
            .collect();
        let mut highest_accepted_rounds: Vec<Round> = vec![1; NUM_AUTHORITIES];
        // Try to fill up the blocks from the 1st authority in missing_block_refs.
        highest_accepted_rounds[missing_block_refs[0].author] = missing_round - 5;
        let results = authority_service
            .handle_fetch_blocks(
                AuthorityIndex::new_for_test(0),
                missing_block_refs.clone(),
                highest_accepted_rounds,
                false,
            )
            .await
            .unwrap();

        // THEN: the expected number of unique blocks are returned.
        let blocks: BTreeMap<BlockRef, VerifiedBlock> = results
            .iter()
            .map(|b| {
                let signed = bcs::from_bytes(b).unwrap();
                let block = VerifiedBlock::new_verified(signed, b.clone());
                (block.reference(), block)
            })
            .collect();
        assert_eq!(blocks.len(), context.parameters.max_blocks_per_sync);
        // All missing blocks are returned.
        for b in &missing_block_refs {
            assert!(blocks.contains_key(b));
        }
        // Ancestor blocks are from the expected rounds and authorities.
        let expected_authors = [missing_block_refs[0].author, missing_block_refs[1].author];
        for b in blocks.keys() {
            assert!(b.round <= missing_round);
            assert!(expected_authors.contains(&b.author));
        }

        // WHEN: Request 5 block from round 40, not getting ancestors.
        let missing_block_refs: Vec<BlockRef> = all_blocks
            .iter()
            .filter(|b| b.reference().round == NUM_ROUNDS as Round - 10)
            .map(|b| b.reference())
            .take(5)
            .collect();
        let results = authority_service
            .handle_fetch_blocks(
                AuthorityIndex::new_for_test(0),
                missing_block_refs.clone(),
                vec![],
                false,
            )
            .await
            .unwrap();

        // THEN: the expected number of unique blocks are returned.
        let blocks: BTreeMap<BlockRef, VerifiedBlock> = results
            .iter()
            .map(|b| {
                let signed = bcs::from_bytes(b).unwrap();
                let block = VerifiedBlock::new_verified(signed, b.clone());
                (block.reference(), block)
            })
            .collect();
        assert_eq!(blocks.len(), 5);
        for b in &missing_block_refs {
            assert!(blocks.contains_key(b));
        }
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn test_handle_fetch_latest_blocks() {
        // GIVEN
        let (context, _keys) = Context::new_for_test(4);
        let context = Arc::new(context);
        let block_verifier = Arc::new(crate::block_verifier::NoopBlockVerifier {});
        let commit_vote_monitor = Arc::new(CommitVoteMonitor::new(context.clone()));
        let core_dispatcher = Arc::new(FakeCoreThreadDispatcher::new());
        let (_tx_block_broadcast, rx_block_broadcast) = broadcast::channel(100);
        let network_client = Arc::new(FakeNetworkClient::default());
        let (blocks_sender, _blocks_receiver) =
            monitored_mpsc::unbounded_channel("consensus_block_output");
        let store = Arc::new(MemStore::new());
        let dag_state = Arc::new(RwLock::new(DagState::new(context.clone(), store.clone())));
        let transaction_certifier = TransactionCertifier::new(
            context.clone(),
            block_verifier.clone(),
            dag_state.clone(),
            blocks_sender,
        );
        let synchronizer = Synchronizer::start(
            network_client,
            context.clone(),
            core_dispatcher.clone(),
            commit_vote_monitor.clone(),
            block_verifier.clone(),
            transaction_certifier.clone(),
            dag_state.clone(),
            true,
        );
        let round_tracker = Arc::new(RwLock::new(PeerRoundTracker::new(context.clone())));
        let authority_service = Arc::new(AuthorityService::new(
            context.clone(),
            block_verifier,
            commit_vote_monitor,
            round_tracker,
            synchronizer,
            core_dispatcher.clone(),
            rx_block_broadcast,
            transaction_certifier,
            dag_state.clone(),
            store,
            None,
            None, // legacy_store_manager
            0,    // epoch_base_index (tests start at epoch 0)
        ));

        // Create some blocks for a few authorities. Create some equivocations as well and store in dag state.
        let mut dag_builder = DagBuilder::new(context.clone());
        dag_builder
            .layers(1..=10)
            .authorities(vec![AuthorityIndex::new_for_test(2)])
            .equivocate(1)
            .build()
            .persist_layers(dag_state);

        // WHEN
        let authorities_to_request = vec![
            AuthorityIndex::new_for_test(1),
            AuthorityIndex::new_for_test(2),
        ];
        let results = authority_service
            .handle_fetch_latest_blocks(AuthorityIndex::new_for_test(1), authorities_to_request)
            .await;

        // THEN
        let serialised_blocks = results.unwrap();
        for serialised_block in serialised_blocks {
            let signed_block: SignedBlock =
                bcs::from_bytes(&serialised_block).expect("Error while deserialising block");
            let verified_block = VerifiedBlock::new_verified(signed_block, serialised_block);

            assert_eq!(verified_block.round(), 10);
        }
    }
}
