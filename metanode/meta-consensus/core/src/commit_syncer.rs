// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

//! CommitSyncer implements efficient synchronization of committed data.
//!
//! During the operation of a committee of authorities for consensus, one or more authorities
//! can fall behind the quorum in their received and accepted blocks. This can happen due to
//! network disruptions, host crash, or other reasons. Authorities fell behind need to catch up to
//! the quorum to be able to vote on the latest leaders. So efficient synchronization is necessary
//! to minimize the impact of temporary disruptions and maintain smooth operations of the network.
//!  
//! CommitSyncer achieves efficient synchronization by relying on the following: when blocks
//! are included in commits with >= 2f+1 certifiers by stake, these blocks must have passed
//! verifications on some honest validators, so re-verifying them is unnecessary. In fact, the
//! quorum certified commits themselves can be trusted to be sent to Sui directly, but for
//! simplicity this is not done. Blocks from trusted commits still go through Core and committer.
//!
//! Another way CommitSyncer improves the efficiency of synchronization is parallel fetching:
//! commits have a simple dependency graph (linear), so it is easy to fetch ranges of commits
//! in parallel.
//!
//! Commit synchronization is an expensive operation, involving transferring large amount of data via
//! the network. And it is not on the critical path of block processing. So the heuristics for
//! synchronization, including triggers and retries, should be chosen to favor throughput and
//! efficient resource usage, over faster reactions.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};

use bytes::Bytes;
use consensus_config::AuthorityIndex;
use consensus_types::block::BlockRef;
use futures::{stream::FuturesOrdered, StreamExt as _};
use itertools::Itertools as _;
use mysten_metrics::spawn_logged_monitored_task;
use parking_lot::RwLock;
use rand::{prelude::SliceRandom as _, rngs::ThreadRng};
use tokio::{
    runtime::Handle,
    sync::oneshot,
    task::{JoinHandle, JoinSet},
    time::{sleep, MissedTickBehavior},
};
use tracing::{debug, info, warn};

use crate::{
    adaptive_delay::AdaptiveDelayState,
    block::{BlockAPI, SignedBlock, VerifiedBlock},
    block_verifier::BlockVerifier,
    commit::{
        CertifiedCommit, CertifiedCommits, Commit, CommitAPI as _, CommitDigest, CommitRange,
        CommitRef, TrustedCommit,
    },
    commit_vote_monitor::CommitVoteMonitor,
    context::Context,
    core_thread::CoreThreadDispatcher,
    dag_state::DagState,
    error::{ConsensusError, ConsensusResult},
    network::NetworkClient,
    stake_aggregator::{QuorumThreshold, StakeAggregator},
    transaction_certifier::TransactionCertifier,
    CommitConsumerMonitor, CommitIndex,
};

// Handle to stop the CommitSyncer loop.
pub(crate) struct CommitSyncerHandle {
    schedule_task: JoinHandle<()>,
    tx_shutdown: oneshot::Sender<()>,
}

impl CommitSyncerHandle {
    pub(crate) async fn stop(self) {
        let _ = self.tx_shutdown.send(());
        // Do not abort schedule task, which waits for fetches to shut down.
        if let Err(e) = self.schedule_task.await {
            if e.is_panic() {
                std::panic::resume_unwind(e.into_panic());
            }
        }
    }
}

pub(crate) struct CommitSyncer<C: NetworkClient> {
    // States shared by scheduler and fetch tasks.

    // Shared components wrapper.
    inner: Arc<Inner<C>>,

    // States only used by the scheduler.

    // Inflight requests to fetch commits from different authorities.
    inflight_fetches: JoinSet<(u32, CertifiedCommits)>,
    // Additional ranges of commits to fetch.
    pending_fetches: BTreeSet<CommitRange>,
    // Fetched commits and blocks by commit range.
    fetched_ranges: BTreeMap<CommitRange, CertifiedCommits>,
    // Highest commit index among inflight and pending fetches.
    // Used to determine the start of new ranges to be fetched.
    highest_scheduled_index: Option<CommitIndex>,
    // Highest index among fetched commits, after commits and blocks are verified.
    // Used for metrics.
    highest_fetched_commit_index: CommitIndex,
    // The commit index that is the max of highest local commit index and commit index inflight to Core.
    // Used to determine if fetched blocks can be sent to Core without gaps.
    synced_commit_index: CommitIndex,

    // --- log throttling ---
    last_schedule_log_at: tokio::time::Instant,
    last_logged_quorum_commit_index: CommitIndex,
    last_logged_local_commit_index: CommitIndex,

    // --- sync mode detection ---
    is_sync_mode: bool, // True when node is in aggressive sync mode due to significant lag
    last_sync_mode_log_at: tokio::time::Instant,

    // --- adaptive delay ---
    adaptive_delay_state: Option<Arc<AdaptiveDelayState>>,
}

impl<C: NetworkClient> CommitSyncer<C> {
    pub(crate) fn new(
        context: Arc<Context>,
        core_thread_dispatcher: Arc<dyn CoreThreadDispatcher>,
        commit_vote_monitor: Arc<CommitVoteMonitor>,
        commit_consumer_monitor: Arc<CommitConsumerMonitor>,
        block_verifier: Arc<dyn BlockVerifier>,
        transaction_certifier: TransactionCertifier,
        network_client: Arc<C>,
        dag_state: Arc<RwLock<DagState>>,
        adaptive_delay_state: Option<Arc<AdaptiveDelayState>>,
    ) -> Self {
        let inner = Arc::new(Inner {
            context,
            core_thread_dispatcher,
            commit_vote_monitor,
            commit_consumer_monitor,
            block_verifier,
            transaction_certifier,
            network_client,
            dag_state,
        });
        let synced_commit_index = inner.dag_state.read().last_commit_index();
        CommitSyncer {
            inner,
            inflight_fetches: JoinSet::new(),
            pending_fetches: BTreeSet::new(),
            fetched_ranges: BTreeMap::new(),
            highest_scheduled_index: None,
            highest_fetched_commit_index: 0,
            synced_commit_index,
            last_schedule_log_at: tokio::time::Instant::now() - Duration::from_secs(300),
            last_logged_quorum_commit_index: 0,
            last_logged_local_commit_index: 0,
            is_sync_mode: false,
            last_sync_mode_log_at: tokio::time::Instant::now() - Duration::from_secs(300),
            adaptive_delay_state,
        }
    }

    pub(crate) fn start(self) -> CommitSyncerHandle {
        let (tx_shutdown, rx_shutdown) = oneshot::channel();
        let schedule_task = spawn_logged_monitored_task!(self.schedule_loop(rx_shutdown,));
        CommitSyncerHandle {
            schedule_task,
            tx_shutdown,
        }
    }

    async fn schedule_loop(mut self, mut rx_shutdown: oneshot::Receiver<()>) {
        // ADAPTIVE SYNC: Use shorter interval when in sync mode for faster response
        let base_interval = Duration::from_secs(2);
        let fast_interval = Duration::from_secs(1);
        let mut current_interval_duration = base_interval;
        let mut interval = tokio::time::interval(base_interval);
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut last_interval_check = tokio::time::Instant::now();

        loop {
            tokio::select! {
                // Periodically, schedule new fetches if the node is falling behind.
                _ = interval.tick() => {
                    // ADAPTIVE SYNC: Adjust interval based on sync mode
                    // In sync mode, check more frequently (every 1 second instead of 2)
                    let now = tokio::time::Instant::now();
                    if now.duration_since(last_interval_check) >= Duration::from_secs(5) {
                        // Check every 5 seconds if we should adjust interval
                        let quorum_commit_index = self.inner.commit_vote_monitor.quorum_commit_index();
                        let local_commit_index = self.inner.dag_state.read().last_commit_index();
                        let lag = quorum_commit_index.saturating_sub(local_commit_index);
                        let should_use_fast_interval = lag > 50 || (quorum_commit_index > 0 &&
                            (lag as f64 / quorum_commit_index as f64) > 0.05);

                        if should_use_fast_interval && current_interval_duration == base_interval {
                            // Switch to faster interval (1 second)
                            current_interval_duration = fast_interval;
                            interval = tokio::time::interval(fast_interval);
                            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
                            info!("⚡ [SYNC-MODE] Switching to fast sync interval (1s) due to lag={}", lag);
                        } else if !should_use_fast_interval && current_interval_duration == fast_interval {
                            // Switch back to normal interval (2 seconds)
                            current_interval_duration = base_interval;
                            interval = tokio::time::interval(base_interval);
                            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
                            info!("✅ [SYNC-MODE] Switching back to normal interval (2s) - lag reduced to {}", lag);
                        }
                        last_interval_check = now;
                    }
                    self.try_schedule_once();
                }
                // Handles results from fetch tasks.
                Some(result) = self.inflight_fetches.join_next(), if !self.inflight_fetches.is_empty() => {
                    if let Err(e) = result {
                        if e.is_panic() {
                            std::panic::resume_unwind(e.into_panic());
                        }
                        warn!("Fetch cancelled. CommitSyncer shutting down: {}", e);
                        // If any fetch is cancelled or panicked, try to shutdown and exit the loop.
                        self.inflight_fetches.shutdown().await;
                        return;
                    }
                    let (target_end, commits) = result.expect("inflight fetch result should be Ok after error handling above");
                    self.handle_fetch_result(target_end, commits).await;
                }
                _ = &mut rx_shutdown => {
                    // Shutdown requested.
                    info!("CommitSyncer shutting down ...");
                    self.inflight_fetches.shutdown().await;
                    return;
                }
            }

            self.try_start_fetches();
        }
    }

    fn try_schedule_once(&mut self) {
        let quorum_commit_index = self.inner.commit_vote_monitor.quorum_commit_index();
        let local_commit_index = self.inner.dag_state.read().last_commit_index();
        let metrics = &self.inner.context.metrics.node_metrics;
        metrics
            .commit_sync_quorum_index
            .set(quorum_commit_index as i64);
        metrics
            .commit_sync_local_index
            .set(local_commit_index as i64);

        // Update quorum commit rate tracker for adaptive delay
        if let Some(adaptive_delay_state) = &self.adaptive_delay_state {
            adaptive_delay_state.update_quorum_commit(quorum_commit_index);
        }
        let highest_handled_index = self.inner.commit_consumer_monitor.highest_handled_commit();
        let highest_scheduled_index = self.highest_scheduled_index.unwrap_or(0);
        // Update synced_commit_index periodically to make sure it is no smaller than
        // local commit index.
        self.synced_commit_index = self.synced_commit_index.max(local_commit_index);

        // ═══════════════════════════════════════════════════════════════════════
        // COLD-START FAST-FORWARD (Snapshot Restore Support)
        // When a node starts fresh from a Go snapshot (local_commit=0) and the
        // network is far ahead, skip historical consensus commits. Go already has
        // the blockchain state from the snapshot — we only need recent commits so
        // the DAG can continue. Without this, verify_commits() fails with
        // "Not enough votes (0)" because the committee from the stale epoch can't
        // validate vote blocks from the current epoch.
        // ═══════════════════════════════════════════════════════════════════════
        if local_commit_index == 0 && quorum_commit_index > 200 {
            // SNAPSHOT RESTORE: Fast-forward to the current quorum. Historical
            // commits can never be verified (DAG was wiped — vote blocks reference
            // digests that don't exist). The node will only process NEW commits 
            // after it joins consensus via the proposer cold-start exemption.
            let fast_forward_to = quorum_commit_index;
            if self.synced_commit_index < fast_forward_to {
                if self.synced_commit_index == 0 {
                    warn!(
                        "🚀 [COLD-START] Fast-forwarding synced_commit_index: 0 → {} (quorum={}). \
                         Go already has historical state from snapshot — skipping ALL historical consensus.",
                        fast_forward_to, quorum_commit_index
                    );
                } else {
                    info!(
                        "🚀 [COLD-START] Continuing fast-forward: {} → {} (quorum={}). \
                         Still no local commits — DAG not synced yet.",
                        self.synced_commit_index, fast_forward_to, quorum_commit_index
                    );
                }
                self.synced_commit_index = fast_forward_to;
                // Cancel any pending/scheduled fetches for historical ranges that will fail
                self.pending_fetches.clear();
                self.highest_scheduled_index = Some(fast_forward_to);
            }
        }

        let unhandled_commits_threshold = self.unhandled_commits_threshold();
        // Throttle noisy logs:
        // - When healthy (no lag): log at most once per 120s.
        // - When lagging (quorum ahead of local): log at most once per 30s, or when lag jumps notably.
        let now = tokio::time::Instant::now();
        let lag = quorum_commit_index.saturating_sub(local_commit_index);
        let last_lag = self
            .last_logged_quorum_commit_index
            .saturating_sub(self.last_logged_local_commit_index);
        let min_interval = if lag == 0 {
            Duration::from_secs(120)
        } else {
            Duration::from_secs(30)
        };
        let lag_jump = lag > last_lag + (unhandled_commits_threshold / 2).max(1);

        // CRITICAL: Detect significant lag and enter sync mode
        // Thresholds for sync mode:
        // - MODERATE_LAG: 50 commits or 5% behind quorum -> enter sync mode
        // - SEVERE_LAG: 200 commits or 10% behind quorum -> aggressive sync mode
        const MODERATE_LAG_THRESHOLD: u32 = 50; // Enter sync mode if lag > 50 commits
        const SEVERE_LAG_THRESHOLD: u32 = 200; // Aggressive sync mode if lag > 200 commits
        const MODERATE_LAG_PERCENTAGE: f64 = 5.0; // Enter sync mode if lag > 5% of quorum
        const SEVERE_LAG_PERCENTAGE: f64 = 10.0; // Aggressive sync mode if lag > 10% of quorum

        let lag_percentage = if quorum_commit_index > 0 {
            (lag as f64 / quorum_commit_index as f64) * 100.0
        } else {
            0.0
        };

        // Determine if we should be in sync mode
        let should_be_in_sync_mode =
            lag > MODERATE_LAG_THRESHOLD || lag_percentage > MODERATE_LAG_PERCENTAGE;
        let is_severe_lag = lag > SEVERE_LAG_THRESHOLD || lag_percentage > SEVERE_LAG_PERCENTAGE;

        // Log sync mode transition
        if should_be_in_sync_mode != self.is_sync_mode {
            self.is_sync_mode = should_be_in_sync_mode;
            if should_be_in_sync_mode {
                tracing::warn!(
                    "🔄 [SYNC-MODE] Entering sync mode: lag={} commits ({}% behind quorum), local_commit={}, quorum_commit={}, synced_commit={}",
                    lag, lag_percentage, local_commit_index, quorum_commit_index, self.synced_commit_index
                );
            } else {
                info!(
                    "✅ [SYNC-MODE] Exiting sync mode: lag={} commits ({}% behind quorum), local_commit={}, quorum_commit={}",
                    lag, lag_percentage, local_commit_index, quorum_commit_index
                );
            }
            self.last_sync_mode_log_at = now;
        }

        // Log significant lag warnings (throttled)
        if lag > MODERATE_LAG_THRESHOLD {
            if now.duration_since(self.last_sync_mode_log_at) >= Duration::from_secs(10) {
                if is_severe_lag {
                    tracing::error!(
                        "🚨 [LAG-DETECTION] Node is severely lagging: lag={} commits ({}% behind quorum), local_commit={}, quorum_commit={}, synced_commit={}",
                        lag, lag_percentage, local_commit_index, quorum_commit_index, self.synced_commit_index
                    );
                } else {
                    tracing::warn!(
                        "⚠️  [LAG-DETECTION] Node is lagging significantly: lag={} commits ({}% behind quorum), local_commit={}, quorum_commit={}, synced_commit={}",
                        lag, lag_percentage, local_commit_index, quorum_commit_index, self.synced_commit_index
                    );
                }
                self.last_sync_mode_log_at = now;
            }
        }

        if now.duration_since(self.last_schedule_log_at) >= min_interval
            || lag_jump
            || quorum_commit_index != self.last_logged_quorum_commit_index
                && lag > 0
                && now.duration_since(self.last_schedule_log_at) >= Duration::from_secs(10)
        {
            info!(
                "Checking to schedule fetches: synced_commit_index={}, highest_handled_index={}, highest_scheduled_index={}, quorum_commit_index={}, unhandled_commits_threshold={}, lag={}",
            self.synced_commit_index,
            highest_handled_index,
            highest_scheduled_index,
            quorum_commit_index,
            unhandled_commits_threshold,
                lag,
        );
            self.last_schedule_log_at = now;
            self.last_logged_quorum_commit_index = quorum_commit_index;
            self.last_logged_local_commit_index = local_commit_index;
        }

        // Cleanup pending fetches whose entire range is already synced.
        self.pending_fetches
            .retain(|range| range.end() > self.synced_commit_index);
        let fetch_after_index = self
            .synced_commit_index
            .max(self.highest_scheduled_index.unwrap_or(0));

        // ADAPTIVE SYNC: Adjust batch size and scheduling based on lag severity
        // When in sync mode, use larger batches and more aggressive scheduling
        let base_batch_size = self.inner.context.parameters.commit_sync_batch_size;
        let lag_percentage_for_batch = if quorum_commit_index > 0 {
            (lag as f64 / quorum_commit_index as f64) * 100.0
        } else {
            0.0
        };
        let effective_batch_size = if self.is_sync_mode {
            // In sync mode: use larger batches for faster catch-up
            // Moderate lag: 1.5x batch size, Severe lag: 2x batch size
            if lag > 200 || lag_percentage_for_batch > 10.0 {
                base_batch_size * 2 // Aggressive: 2x batch size
            } else {
                base_batch_size + base_batch_size / 2 // Moderate: 1.5x batch size
            }
        } else {
            base_batch_size // Normal mode: use configured batch size
        };

        // When the node is falling behind, schedule pending fetches which will be executed on later.
        for prev_end in
            (fetch_after_index..=quorum_commit_index).step_by(effective_batch_size as usize)
        {
            // Create range with inclusive start and end.
            let range_start = prev_end + 1;
            let range_end = prev_end + effective_batch_size;
            // Commit range is not fetched when [range_start, range_end] contains less number of commits
            // than the target batch size. This is to avoid the cost of processing more and smaller batches.
            // Block broadcast, subscription and synchronization will help the node catchup.
            if quorum_commit_index < range_end {
                break;
            }
            // Pause scheduling new fetches when handling of commits is lagging.
            // In sync mode, be more lenient with the threshold to allow more aggressive fetching
            let effective_threshold = if self.is_sync_mode {
                unhandled_commits_threshold * 2 // Allow more unhandled commits in sync mode
            } else {
                unhandled_commits_threshold
            };
            if highest_handled_index + effective_threshold < range_end {
                if !self.is_sync_mode {
                    warn!(
                    "Skip scheduling new commit fetches: consensus handler is lagging. highest_handled_index={}, highest_scheduled_index={}",
                    highest_handled_index, highest_scheduled_index
                );
                }
                break;
            }
            self.pending_fetches
                .insert((range_start..=range_end).into());
            // quorum_commit_index should be non-decreasing, so highest_scheduled_index should not
            // decrease either.
            self.highest_scheduled_index = Some(range_end);
        }
    }

    async fn handle_fetch_result(
        &mut self,
        target_end: CommitIndex,
        certified_commits: CertifiedCommits,
    ) {
        assert!(!certified_commits.commits().is_empty());

        let (total_blocks_fetched, total_blocks_size_bytes) = certified_commits
            .commits()
            .iter()
            .fold((0, 0), |(blocks, bytes), c| {
                (
                    blocks + c.blocks().len(),
                    bytes
                        + c.blocks()
                            .iter()
                            .map(|b| b.serialized().len())
                            .sum::<usize>() as u64,
                )
            });

        let metrics = &self.inner.context.metrics.node_metrics;
        metrics
            .commit_sync_fetched_commits
            .inc_by(certified_commits.commits().len() as u64);
        metrics
            .commit_sync_fetched_blocks
            .inc_by(total_blocks_fetched as u64);
        metrics
            .commit_sync_total_fetched_blocks_size
            .inc_by(total_blocks_size_bytes);

        let (commit_start, commit_end) = (
            certified_commits
                .commits()
                .first()
                .expect("certified_commits checked non-empty above")
                .index(),
            certified_commits
                .commits()
                .last()
                .expect("certified_commits checked non-empty above")
                .index(),
        );
        self.highest_fetched_commit_index = self.highest_fetched_commit_index.max(commit_end);
        metrics
            .commit_sync_highest_fetched_index
            .set(self.highest_fetched_commit_index as i64);

        // Allow returning partial results, and try fetching the rest separately.
        if commit_end < target_end {
            self.pending_fetches
                .insert((commit_end + 1..=target_end).into());
        }
        // Make sure synced_commit_index is up to date.
        self.synced_commit_index = self
            .synced_commit_index
            .max(self.inner.dag_state.read().last_commit_index());
        // Only add new blocks if at least some of them are not already synced.
        if self.synced_commit_index < commit_end {
            self.fetched_ranges
                .insert((commit_start..=commit_end).into(), certified_commits);
        }
        // Try to process as many fetched blocks as possible.
        while let Some((fetched_commit_range, _commits)) = self.fetched_ranges.first_key_value() {
            // Only pop fetched_ranges if there is no gap with blocks already synced.
            // Note: start, end and synced_commit_index are all inclusive.
            let (fetched_commit_range, commits) =
                if fetched_commit_range.start() <= self.synced_commit_index + 1 {
                    self.fetched_ranges
                        .pop_first()
                        .expect("checked first_key_value above")
                } else {
                    // Found gap between earliest fetched block and latest synced block,
                    // so not sending additional blocks to Core.
                    metrics.commit_sync_gap_on_processing.inc();
                    break;
                };
            // Avoid sending to Core a whole batch of already synced blocks.
            if fetched_commit_range.end() <= self.synced_commit_index {
                continue;
            }

            debug!(
                "Fetched blocks for commit range {:?}: {}",
                fetched_commit_range,
                commits
                    .commits()
                    .iter()
                    .flat_map(|c| c.blocks())
                    .map(|b| b.reference().to_string())
                    .join(","),
            );

            // If core thread cannot handle the incoming blocks, it is ok to block here
            // to slow down the commit syncer.
            match self
                .inner
                .core_thread_dispatcher
                .add_certified_commits(commits)
                .await
            {
                // Missing ancestors are possible from certification blocks, but
                // it is unnecessary to try to sync their causal history. If they are required
                // for the progress of the DAG, they will be included in a future commit.
                Ok(missing) => {
                    if !missing.is_empty() {
                        info!(
                            "Certification blocks have missing ancestors: {} for commit range {:?}",
                            missing.iter().map(|b| b.to_string()).join(","),
                            fetched_commit_range,
                        );
                    }
                    for block_ref in missing {
                        let hostname = &self
                            .inner
                            .context
                            .committee
                            .authority(block_ref.author)
                            .hostname;
                        metrics
                            .commit_sync_fetch_missing_blocks
                            .with_label_values(&[hostname])
                            .inc();
                    }
                }
                Err(e) => {
                    info!("Failed to add blocks, shutting down: {}", e);
                    return;
                }
            };

            // Once commits and blocks are sent to Core, ratchet up synced_commit_index
            self.synced_commit_index = self.synced_commit_index.max(fetched_commit_range.end());
        }

        metrics
            .commit_sync_inflight_fetches
            .set(self.inflight_fetches.len() as i64);
        metrics
            .commit_sync_pending_fetches
            .set(self.pending_fetches.len() as i64);
        metrics
            .commit_sync_highest_synced_index
            .set(self.synced_commit_index as i64);
    }

    fn try_start_fetches(&mut self) {
        // Cap parallel fetches based on configured limit and committee size, to avoid overloading the network.
        // Also when there are too many fetched blocks that cannot be sent to Core before an earlier fetch
        // has not finished, reduce parallelism so the earlier fetch can retry on a better host and succeed.
        // ADAPTIVE SYNC: Increase parallelism when in sync mode
        let base_parallel_fetches = self.inner.context.parameters.commit_sync_parallel_fetches;
        let effective_parallel_fetches = if self.is_sync_mode {
            // In sync mode: increase parallelism for faster catch-up
            // Use up to 1.5x the configured parallel fetches, but cap at committee size
            (base_parallel_fetches + base_parallel_fetches / 2)
                .min(self.inner.context.committee.size())
        } else {
            base_parallel_fetches
        };

        let target_parallel_fetches = effective_parallel_fetches
            .min(self.inner.context.committee.size() * 2 / 3)
            .min(
                self.inner
                    .context
                    .parameters
                    .commit_sync_batches_ahead
                    .saturating_sub(self.fetched_ranges.len()),
            )
            .max(1);
        // Start new fetches if there are pending batches and available slots.
        loop {
            if self.inflight_fetches.len() >= target_parallel_fetches {
                break;
            }
            let Some(commit_range) = self.pending_fetches.pop_first() else {
                break;
            };
            self.inflight_fetches
                .spawn(Self::fetch_loop(self.inner.clone(), commit_range));
        }

        let metrics = &self.inner.context.metrics.node_metrics;
        metrics
            .commit_sync_inflight_fetches
            .set(self.inflight_fetches.len() as i64);
        metrics
            .commit_sync_pending_fetches
            .set(self.pending_fetches.len() as i64);
        metrics
            .commit_sync_highest_synced_index
            .set(self.synced_commit_index as i64);
    }

    // Retries fetching commits and blocks from available authorities, until a request succeeds
    // where at least a prefix of the commit range is fetched.
    // Returns the fetched commits and blocks referenced by the commits.
    async fn fetch_loop(
        inner: Arc<Inner<C>>,
        commit_range: CommitRange,
    ) -> (CommitIndex, CertifiedCommits) {
        // Individual request base timeout.
        const TIMEOUT: Duration = Duration::from_secs(10);
        // Max per-request timeout will be base timeout times a multiplier.
        // At the extreme, this means there will be 120s timeout to fetch max_blocks_per_fetch blocks.
        const MAX_TIMEOUT_MULTIPLIER: u32 = 12;
        // timeout * max number of targets should be reasonably small, so the
        // system can adjust to slow network or large data sizes quickly.
        const MAX_NUM_TARGETS: usize = 24;
        let mut timeout_multiplier = 0;
        let _timer = inner
            .context
            .metrics
            .node_metrics
            .commit_sync_fetch_loop_latency
            .start_timer();
        info!("Starting to fetch commits in {commit_range:?} ...",);
        loop {
            // Attempt to fetch commits and blocks through min(committee size, MAX_NUM_TARGETS) peers.
            let mut target_authorities = inner
                .context
                .committee
                .authorities()
                .filter_map(|(i, _)| {
                    if i != inner.context.own_index {
                        Some(i)
                    } else {
                        None
                    }
                })
                .collect_vec();
            target_authorities.shuffle(&mut ThreadRng::default());
            target_authorities.truncate(MAX_NUM_TARGETS);
            // Increase timeout multiplier for each loop until MAX_TIMEOUT_MULTIPLIER.
            timeout_multiplier = (timeout_multiplier + 1).min(MAX_TIMEOUT_MULTIPLIER);
            let request_timeout = TIMEOUT * timeout_multiplier;
            // Give enough overall timeout for fetching commits and blocks.
            // - Timeout for fetching commits and commit certifying blocks.
            // - Timeout for fetching blocks referenced by the commits.
            // - Time spent on pipelining requests to fetch blocks.
            // - Another headroom to allow fetch_once() to timeout gracefully if possible.
            let fetch_timeout = request_timeout * 4;
            // Try fetching from selected target authority.
            for authority in target_authorities {
                match tokio::time::timeout(
                    fetch_timeout,
                    Self::fetch_once(
                        inner.clone(),
                        authority,
                        commit_range.clone(),
                        request_timeout,
                    ),
                )
                .await
                {
                    Ok(Ok(commits)) => {
                        info!("Finished fetching commits in {commit_range:?}",);
                        return (commit_range.end(), commits);
                    }
                    Ok(Err(e)) => {
                        let hostname = inner
                            .context
                            .committee
                            .authority(authority)
                            .hostname
                            .clone();
                        warn!("Failed to fetch {commit_range:?} from {hostname}: {}", e);
                        inner
                            .context
                            .metrics
                            .node_metrics
                            .commit_sync_fetch_once_errors
                            .with_label_values(&[&hostname, e.name()])
                            .inc();
                    }
                    Err(_) => {
                        let hostname = inner
                            .context
                            .committee
                            .authority(authority)
                            .hostname
                            .clone();
                        warn!("Timed out fetching {commit_range:?} from {authority}",);
                        inner
                            .context
                            .metrics
                            .node_metrics
                            .commit_sync_fetch_once_errors
                            .with_label_values(&[&hostname, "FetchTimeout"])
                            .inc();
                    }
                }
            }
            // Avoid busy looping, by waiting for a while before retrying.
            sleep(TIMEOUT).await;
        }
    }

    // Fetches commits and blocks from a single authority. At a high level, first the commits are
    // fetched and verified. After that, blocks referenced in the certified commits are fetched
    // and sent to Core for processing.
    async fn fetch_once(
        inner: Arc<Inner<C>>,
        target_authority: AuthorityIndex,
        commit_range: CommitRange,
        timeout: Duration,
    ) -> ConsensusResult<CertifiedCommits> {
        let _timer = inner
            .context
            .metrics
            .node_metrics
            .commit_sync_fetch_once_latency
            .start_timer();

        // 1. Fetch commits in the commit range from the target authority.
        let (serialized_commits, serialized_blocks) = inner
            .network_client
            .fetch_commits(target_authority, commit_range.clone(), timeout)
            .await?;

        // 2. Verify the response contains blocks that can certify the last returned commit,
        // and the returned commits are chained by digests, so earlier commits are certified
        // as well.
        let (commits, vote_blocks) = Handle::current()
            .spawn_blocking({
                let inner = inner.clone();
                move || {
                    inner.verify_commits(
                        target_authority,
                        commit_range,
                        serialized_commits,
                        serialized_blocks,
                    )
                }
            })
            .await
            .expect("Spawn blocking should not fail")?;

        // 3. Fetch blocks referenced by the commits, from the same peer where commits are fetched.
        let mut block_refs: Vec<_> = commits.iter().flat_map(|c| c.blocks()).cloned().collect();
        block_refs.sort();
        let num_chunks = block_refs
            .len()
            .div_ceil(inner.context.parameters.max_blocks_per_fetch)
            as u32;
        let mut requests: FuturesOrdered<_> = block_refs
            .chunks(inner.context.parameters.max_blocks_per_fetch)
            .enumerate()
            .map(|(i, request_block_refs)| {
                let inner = inner.clone();
                async move {
                    // 4. Send out pipelined fetch requests to avoid overloading the target authority.
                    sleep(timeout * i as u32 / num_chunks).await;
                    // Retry block fetches up to 3 times with backoff before propagating the error.
                    const MAX_BLOCK_FETCH_RETRIES: u32 = 3;
                    let serialized_blocks = {
                        let mut last_err = None;
                        let mut result = None;
                        for attempt in 0..MAX_BLOCK_FETCH_RETRIES {
                            match inner
                                .network_client
                                .fetch_blocks(
                                    target_authority,
                                    request_block_refs.to_vec(),
                                    vec![],
                                    false,
                                    timeout,
                                )
                                .await
                            {
                                Ok(blocks) => {
                                    result = Some(blocks);
                                    break;
                                }
                                Err(e) => {
                                    let hostname = &inner.context.committee.authority(target_authority).hostname;
                                    warn!(
                                        "Commit sync: retry {}/{} fetching blocks from {hostname}: {e}",
                                        attempt + 1,
                                        MAX_BLOCK_FETCH_RETRIES
                                    );
                                    last_err = Some(e);
                                    if attempt + 1 < MAX_BLOCK_FETCH_RETRIES {
                                        sleep(Duration::from_millis(500 * (attempt as u64 + 1))).await;
                                    }
                                }
                            }
                        }
                        match result {
                            Some(blocks) => blocks,
                            None => return Err(last_err.expect("last_err must be set after failed retries")),
                        }
                    };
                    // 5. Verify the same number of blocks are returned as requested.
                    if request_block_refs.len() != serialized_blocks.len() {
                        return Err(ConsensusError::UnexpectedNumberOfBlocksFetched {
                            authority: target_authority,
                            requested: request_block_refs.len(),
                            received: serialized_blocks.len(),
                        });
                    }
                    // 6. Verify returned blocks have valid formats.
                    let signed_blocks = serialized_blocks
                        .iter()
                        .map(|serialized| {
                            let block: SignedBlock = bcs::from_bytes(serialized)
                                .map_err(ConsensusError::MalformedBlock)?;
                            Ok(block)
                        })
                        .collect::<ConsensusResult<Vec<_>>>()?;
                    // 7. Verify the returned blocks match the requested block refs.
                    // If they do match, the returned blocks can be considered verified as well.
                    let mut blocks = Vec::new();
                    for ((requested_block_ref, signed_block), serialized) in request_block_refs
                        .iter()
                        .zip(signed_blocks.into_iter())
                        .zip(serialized_blocks.into_iter())
                    {
                        let signed_block_digest = VerifiedBlock::compute_digest(&serialized);
                        let received_block_ref = BlockRef::new(
                            signed_block.round(),
                            signed_block.author(),
                            signed_block_digest,
                        );
                        if *requested_block_ref != received_block_ref {
                            return Err(ConsensusError::UnexpectedBlockForCommit {
                                peer: target_authority,
                                requested: *requested_block_ref,
                                received: received_block_ref,
                            });
                        }
                        blocks.push(VerifiedBlock::new_verified(signed_block, serialized));
                    }
                    Ok(blocks)
                }
            })
            .collect();

        let mut fetched_blocks = BTreeMap::new();
        while let Some(result) = requests.next().await {
            for block in result? {
                fetched_blocks.insert(block.reference(), block);
            }
        }

        // 8. Check if the block timestamps are lower than current time - this is for metrics only.
        for block in fetched_blocks.values().chain(vote_blocks.iter()) {
            let now_ms = inner.context.clock.timestamp_utc_ms();
            let forward_drift = block.timestamp_ms().saturating_sub(now_ms);
            if forward_drift == 0 {
                continue;
            };
            let peer_hostname = &inner.context.committee.authority(target_authority).hostname;
            inner
                .context
                .metrics
                .node_metrics
                .block_timestamp_drift_ms
                .with_label_values(&[peer_hostname, "commit_syncer"])
                .inc_by(forward_drift);
        }

        // 9. Now create certified commits by assigning the blocks to each commit.
        let mut certified_commits = Vec::new();
        for commit in &commits {
            let blocks = commit
                .blocks()
                .iter()
                .map(|block_ref| {
                    fetched_blocks
                        .remove(block_ref)
                        .expect("Block should exist")
                })
                .collect::<Vec<_>>();
            certified_commits.push(CertifiedCommit::new_certified(commit.clone(), blocks));
        }

        // 10. Add blocks in certified commits to the transaction certifier.
        for commit in &certified_commits {
            for block in commit.blocks() {
                // Only account for reject votes in the block, since they may vote on uncommitted
                // blocks or transactions. It is unnecessary to vote on the committed blocks
                // themselves.
                if inner.context.protocol_config.mysticeti_fastpath() {
                    inner
                        .transaction_certifier
                        .add_voted_blocks(vec![(block.clone(), vec![])]);
                }
            }
        }

        Ok(CertifiedCommits::new(certified_commits, vote_blocks))
    }

    fn unhandled_commits_threshold(&self) -> CommitIndex {
        self.inner.context.parameters.commit_sync_batch_size
            * (self.inner.context.parameters.commit_sync_batches_ahead as u32)
    }

    #[cfg(test)]
    fn pending_fetches(&self) -> BTreeSet<CommitRange> {
        self.pending_fetches.clone()
    }

    #[cfg(test)]
    fn fetched_ranges(&self) -> BTreeMap<CommitRange, CertifiedCommits> {
        self.fetched_ranges.clone()
    }

    #[cfg(test)]
    fn highest_scheduled_index(&self) -> Option<CommitIndex> {
        self.highest_scheduled_index
    }

    #[cfg(test)]
    fn highest_fetched_commit_index(&self) -> CommitIndex {
        self.highest_fetched_commit_index
    }

    #[cfg(test)]
    fn synced_commit_index(&self) -> CommitIndex {
        self.synced_commit_index
    }
}

struct Inner<C: NetworkClient> {
    context: Arc<Context>,
    core_thread_dispatcher: Arc<dyn CoreThreadDispatcher>,
    commit_vote_monitor: Arc<CommitVoteMonitor>,
    commit_consumer_monitor: Arc<CommitConsumerMonitor>,
    block_verifier: Arc<dyn BlockVerifier>,
    transaction_certifier: TransactionCertifier,
    network_client: Arc<C>,
    dag_state: Arc<RwLock<DagState>>,
}

impl<C: NetworkClient> Inner<C> {
    /// Verifies the commits and also certifies them using the provided vote blocks for the last commit. The
    /// method returns the trusted commits and the votes as verified blocks.
    fn verify_commits(
        &self,
        peer: AuthorityIndex,
        commit_range: CommitRange,
        serialized_commits: Vec<Bytes>,
        serialized_vote_blocks: Vec<Bytes>,
    ) -> ConsensusResult<(Vec<TrustedCommit>, Vec<VerifiedBlock>)> {
        // Parse and verify commits.
        let mut commits = Vec::new();
        for serialized in &serialized_commits {
            let commit: Commit =
                bcs::from_bytes(serialized).map_err(ConsensusError::MalformedCommit)?;
            let digest = TrustedCommit::compute_digest(serialized);
            if commits.is_empty() {
                // start is inclusive, so first commit must be at the start index.
                if commit.index() != commit_range.start() {
                    return Err(ConsensusError::UnexpectedStartCommit {
                        peer,
                        start: commit_range.start(),
                        commit: Box::new(commit),
                    });
                }
            } else {
                // Verify next commit increments index and references the previous digest.
                let (last_commit_digest, last_commit): &(CommitDigest, Commit) = commits
                    .last()
                    .expect("commits is non-empty: checked at loop entry");
                if commit.index() != last_commit.index() + 1
                    || &commit.previous_digest() != last_commit_digest
                {
                    return Err(ConsensusError::UnexpectedCommitSequence {
                        peer,
                        prev_commit: Box::new(last_commit.clone()),
                        curr_commit: Box::new(commit),
                    });
                }
            }
            // Do not process more commits past the end index.
            if commit.index() > commit_range.end() {
                break;
            }
            commits.push((digest, commit));
        }
        let Some((end_commit_digest, end_commit)) = commits.last() else {
            return Err(ConsensusError::NoCommitReceived { peer });
        };

        // Parse and verify blocks. Then accumulate votes on the end commit.
        let end_commit_ref = CommitRef::new(end_commit.index(), *end_commit_digest);
        let mut stake_aggregator = StakeAggregator::<QuorumThreshold>::new();
        let mut vote_blocks = Vec::new();
        for serialized in serialized_vote_blocks {
            let block: SignedBlock =
                bcs::from_bytes(&serialized).map_err(ConsensusError::MalformedBlock)?;
            // Only block signatures need to be verified, to verify commit votes.
            // But the blocks will be sent to Core, so they need to be fully verified.
            let (block, reject_transaction_votes) =
                self.block_verifier.verify_and_vote(block, serialized)?;
            if self.context.protocol_config.mysticeti_fastpath() {
                self.transaction_certifier
                    .add_voted_blocks(vec![(block.clone(), reject_transaction_votes)]);
            }
            for vote in block.commit_votes() {
                if *vote == end_commit_ref {
                    stake_aggregator.add(block.author(), &self.context.committee);
                }
            }
            vote_blocks.push(block);
        }

        // Check if the end commit has enough votes.
        if !stake_aggregator.reached_threshold(&self.context.committee) {
            return Err(ConsensusError::NotEnoughCommitVotes {
                stake: stake_aggregator.stake(),
                peer,
                commit: Box::new(end_commit.clone()),
            });
        }

        let trusted_commits = commits
            .into_iter()
            .zip(serialized_commits)
            .map(|((_d, c), s)| TrustedCommit::new_trusted(c, s))
            .collect();
        Ok((trusted_commits, vote_blocks))
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use bytes::Bytes;
    use consensus_config::{AuthorityIndex, Parameters};
    use consensus_types::block::{BlockRef, Round};
    use mysten_metrics::monitored_mpsc;
    use parking_lot::RwLock;

    use crate::{
        block::{TestBlock, VerifiedBlock},
        block_verifier::NoopBlockVerifier,
        commit::CommitRange,
        commit_syncer::CommitSyncer,
        commit_vote_monitor::CommitVoteMonitor,
        context::Context,
        core_thread::MockCoreThreadDispatcher,
        dag_state::DagState,
        error::ConsensusResult,
        network::{BlockStream, NetworkClient},
        storage::mem_store::MemStore,
        transaction_certifier::TransactionCertifier,
        CommitConsumerMonitor, CommitDigest, CommitRef,
    };

    #[derive(Default)]
    struct FakeNetworkClient {}

    #[async_trait::async_trait]
    impl NetworkClient for FakeNetworkClient {
        async fn send_block(
            &self,
            _peer: AuthorityIndex,
            _serialized_block: &VerifiedBlock,
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
    async fn commit_syncer_start_and_pause_scheduling() {
        // SETUP
        let (context, _) = Context::new_for_test(4);
        // Use smaller batches and fetch limits for testing.
        let context = Context {
            own_index: AuthorityIndex::new_for_test(3),
            parameters: Parameters {
                commit_sync_batch_size: 5,
                commit_sync_batches_ahead: 5,
                commit_sync_parallel_fetches: 5,
                max_blocks_per_fetch: 5,
                ..context.parameters
            },
            ..context
        };
        let context = Arc::new(context);
        let block_verifier = Arc::new(NoopBlockVerifier {});
        let core_thread_dispatcher = Arc::new(MockCoreThreadDispatcher::default());
        let network_client = Arc::new(FakeNetworkClient::default());
        let store = Arc::new(MemStore::new());
        let dag_state = Arc::new(RwLock::new(DagState::new(context.clone(), store)));
        let (blocks_sender, _blocks_receiver) =
            monitored_mpsc::unbounded_channel("consensus_block_output");
        let transaction_certifier = TransactionCertifier::new(
            context.clone(),
            block_verifier.clone(),
            dag_state.clone(),
            blocks_sender,
        );
        let commit_vote_monitor = Arc::new(CommitVoteMonitor::new(context.clone()));
        let commit_consumer_monitor = Arc::new(CommitConsumerMonitor::new(0, 0));
        let mut commit_syncer = CommitSyncer::new(
            context,
            core_thread_dispatcher,
            commit_vote_monitor.clone(),
            commit_consumer_monitor.clone(),
            block_verifier,
            transaction_certifier,
            network_client,
            dag_state,
            None,
        );

        // Check initial state.
        assert!(commit_syncer.pending_fetches().is_empty());
        assert!(commit_syncer.fetched_ranges().is_empty());
        assert!(commit_syncer.highest_scheduled_index().is_none());
        assert_eq!(commit_syncer.highest_fetched_commit_index(), 0);
        assert_eq!(commit_syncer.synced_commit_index(), 0);

        // Observe round 15 blocks voting for commit 10 from authorities 0 to 2 in CommitVoteMonitor
        for i in 0..3 {
            let test_block = TestBlock::new(15, i)
                .set_commit_votes(vec![CommitRef::new(10, CommitDigest::MIN)])
                .build();
            let block = VerifiedBlock::new_for_test(test_block);
            commit_vote_monitor.observe_block(&block);
        }

        // Fetches should be scheduled after seeing progress of other validators.
        commit_syncer.try_schedule_once();

        // Verify state.
        assert_eq!(commit_syncer.pending_fetches().len(), 1);
        assert!(commit_syncer.fetched_ranges().is_empty());
        assert_eq!(commit_syncer.highest_scheduled_index(), Some(10));
        assert_eq!(commit_syncer.highest_fetched_commit_index(), 0);
        assert_eq!(commit_syncer.synced_commit_index(), 0);

        // Observe round 40 blocks voting for commit 35 from authorities 0 to 2 in CommitVoteMonitor
        for i in 0..3 {
            let test_block = TestBlock::new(40, i)
                .set_commit_votes(vec![CommitRef::new(35, CommitDigest::MIN)])
                .build();
            let block = VerifiedBlock::new_for_test(test_block);
            commit_vote_monitor.observe_block(&block);
        }

        // Fetches should be scheduled until the unhandled commits threshold.
        commit_syncer.try_schedule_once();

        // Verify commit syncer is paused after scheduling 15 commits to index 25.
        assert_eq!(commit_syncer.unhandled_commits_threshold(), 25);
        assert_eq!(commit_syncer.highest_scheduled_index(), Some(30));
        let pending_fetches = commit_syncer.pending_fetches();
        assert_eq!(pending_fetches.len(), 3);

        // Indicate commit index 25 is consumed, and try to schedule again.
        commit_consumer_monitor.set_highest_handled_commit(25);
        commit_syncer.try_schedule_once();

        // Verify commit syncer schedules fetches up to index 35.
        assert_eq!(commit_syncer.highest_scheduled_index(), Some(30));
        let pending_fetches = commit_syncer.pending_fetches();
        assert_eq!(pending_fetches.len(), 3);

        // Verify contiguous ranges are scheduled.
        for (range, start) in pending_fetches.iter().zip((1..35).step_by(10)) {
            assert_eq!(range.start(), start);
            assert_eq!(range.end(), start + 9);
        }
    }
}
