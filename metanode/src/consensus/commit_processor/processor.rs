// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

use anyhow::Result;
use consensus_core::{BlockAPI, CommittedSubDag};
use mysten_metrics::monitored_mpsc::UnboundedReceiver;
use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
// [Added] Import Duration
// [Added] Import sleep for retry mechanism
use tracing::{info, trace, warn};

use crate::consensus::checkpoint::calculate_global_exec_index;
use crate::consensus::tx_recycler::TxRecycler;
use crate::node::block_coordinator::BlockCoordinator;
use crate::node::executor_client::ExecutorClient;
// calculate_transaction_hash_hex removed: was only used for dead logging code

/// Commit processor that ensures commits are executed in order
pub struct CommitProcessor {
    receiver: UnboundedReceiver<CommittedSubDag>,
    next_expected_index: u32, // CommitIndex is u32
    pending_commits: BTreeMap<u32, CommittedSubDag>,
    /// Optional callback to notify commit index updates (for epoch transition)
    commit_index_callback: Option<Arc<dyn Fn(u32) + Send + Sync>>,
    /// Optional callback to update global execution index after successful commit
    global_exec_index_callback: Option<Arc<dyn Fn(u64) + Send + Sync>>,
    /// Current epoch (for deterministic global_exec_index calculation)
    current_epoch: u64,
    /// Epoch base index: GEI at the START of current epoch (from Go epoch boundary).
    /// CRITICAL: This must be the epoch boundary value, NOT the current shared_last_global_exec_index.
    /// After cold start, shared_last_global_exec_index gets set to network commit (e.g., 4364)
    /// but epoch_base_index for epoch 1 must be 0.
    epoch_base_index_override: Option<u64>,
    /// Callback to get current last global execution index
    get_last_global_exec_index: Option<Arc<dyn Fn() -> u64 + Send + Sync>>,
    /// Shared last global exec index for direct updates
    shared_last_global_exec_index: Option<Arc<tokio::sync::Mutex<u64>>>,
    /// Optional executor client to send blocks to Go executor
    executor_client: Option<Arc<ExecutorClient>>,
    /// Flag indicating if epoch transition is in progress
    /// When true, we're transitioning to a new epoch
    is_transitioning: Option<Arc<AtomicBool>>,
    /// Queue for transactions that must be retried in the next epoch
    pending_transactions_queue: Option<Arc<tokio::sync::Mutex<Vec<Vec<u8>>>>>,
    /// Optional callback to handle EndOfEpoch system transactions
    /// Called immediately when an EndOfEpoch system transaction is detected in a committed sub-dag
    /// Uses commit finalization approach (like Sui) - no buffer needed as commits are processed sequentially
    epoch_transition_callback: Option<Arc<dyn Fn(u64, u64, u64) -> Result<()> + Send + Sync>>, // CHANGED: u32 -> u64
    /// Multi-epoch committee cache: ETH addresses keyed by epoch
    /// Supports looking up leaders from previous epochs during transitions
    /// RS-1: Uses RwLock instead of Mutex — reads (every commit) don't block each other,
    /// only writes (epoch transition) take exclusive lock.
    epoch_eth_addresses: Arc<tokio::sync::RwLock<std::collections::HashMap<u64, Vec<Vec<u8>>>>>,
    /// Block Coordinator for dual-stream block production (optional)
    block_coordinator: Option<Arc<BlockCoordinator>>,
    /// TX recycler for confirming committed TXs
    tx_recycler: Option<Arc<TxRecycler>>,
    /// Cold-start flag: set when DAG storage was empty at startup (snapshot restore).
    /// When true, skip stale DAG replay commits and wait for live rounds.
    cold_start: Arc<AtomicBool>,
    /// GEI threshold for cold-start skip: skip ALL commits with GEI ≤ this value.
    /// Set from Go's snapshot GEI (via get_last_global_exec_index) during cold-start.
    /// CRITICAL: Must use snapshot GEI, NOT synced_global_exec_index (network state),
    /// otherwise commits needed for state advancement get skipped → nonce gap → FORK.
    cold_start_skip_gei: u64,

    /// RS-2: Storage path for persisting cumulative_fragment_offset
    storage_path: Option<std::path::PathBuf>,
    /// Channel sender for emitting lag alerts
    lag_alert_sender: Option<
        tokio::sync::mpsc::UnboundedSender<
            crate::consensus::commit_processor::lag_monitor::LagAlert,
        >,
    >,
}

impl CommitProcessor {
    pub fn new(receiver: UnboundedReceiver<CommittedSubDag>) -> Self {
        Self {
            receiver,
            next_expected_index: 1, // First commit has index 1 (consensus doesn't create commit with index 0)
            pending_commits: BTreeMap::new(),
            commit_index_callback: None,
            global_exec_index_callback: None,
            get_last_global_exec_index: None,
            shared_last_global_exec_index: None,
            current_epoch: 0,
            epoch_base_index_override: None,
            executor_client: None,
            is_transitioning: None,
            pending_transactions_queue: None,
            epoch_transition_callback: None,
            epoch_eth_addresses: Arc::new(tokio::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
            block_coordinator: None,
            tx_recycler: None,
            storage_path: None,
            lag_alert_sender: None,
            cold_start: Arc::new(AtomicBool::new(false)),
            cold_start_skip_gei: 0,
        }
    }

    /// Set callback to notify commit index updates
    pub fn with_commit_index_callback<F>(mut self, callback: F) -> Self
    where
        F: Fn(u32) + Send + Sync + 'static,
    {
        self.commit_index_callback = Some(Arc::new(callback));
        self
    }

    /// Set callback to update global execution index after successful commit
    pub fn with_global_exec_index_callback<F>(mut self, callback: F) -> Self
    where
        F: Fn(u64) + Send + Sync + 'static,
    {
        self.global_exec_index_callback = Some(Arc::new(callback));
        self
    }

    /// Set callback to get current last global execution index
    pub fn with_get_last_global_exec_index<F>(self, _callback: F) -> Self
    where
        F: Fn() -> u64 + Send + Sync + 'static,
    {
        // Currently not used, but kept for future extensibility
        // CRITICAL: We now read directly from shared_last_global_exec_index instead of callback
        self
    }

    /// Set shared last global exec index for direct updates
    pub fn with_shared_last_global_exec_index(
        mut self,
        shared_index: Arc<tokio::sync::Mutex<u64>>,
    ) -> Self {
        self.shared_last_global_exec_index = Some(shared_index);
        self
    }

    /// Set epoch and last_global_exec_index for deterministic global_exec_index calculation
    pub fn with_epoch_info(mut self, epoch: u64, last_global_exec_index: u64) -> Self {
        self.current_epoch = epoch;
        // Store the epoch boundary GEI explicitly. This is the CORRECT value for GEI calculations.
        // CRITICAL: Do NOT derive epoch_base_index from shared_last_global_exec_index at runtime,
        // because cold-start updates it to the network commit (causing wrong GEI → hash divergence).
        self.epoch_base_index_override = Some(last_global_exec_index);
        self
    }

    /// Set executor client to send blocks to Go executor
    pub fn with_executor_client(mut self, executor_client: Arc<ExecutorClient>) -> Self {
        self.executor_client = Some(executor_client);
        self
    }

    /// Set is_transitioning flag to track epoch transition state
    pub fn with_is_transitioning(mut self, is_transitioning: Arc<AtomicBool>) -> Self {
        self.is_transitioning = Some(is_transitioning);
        self
    }

    /// Provide a queue to store transactions that must be retried in the next epoch.
    pub fn with_pending_transactions_queue(
        mut self,
        pending_transactions_queue: Arc<tokio::sync::Mutex<Vec<Vec<u8>>>>,
    ) -> Self {
        self.pending_transactions_queue = Some(pending_transactions_queue);
        self
    }

    /// Set callback to handle EndOfEpoch system transactions
    pub fn with_epoch_transition_callback<F>(mut self, callback: F) -> Self
    where
        F: Fn(u64, u64, u64) -> Result<()> + Send + Sync + 'static, // CHANGED: u32 -> u64
    {
        self.epoch_transition_callback = Some(Arc::new(callback));
        self
    }

    /// Set epoch ETH addresses HashMap for multi-epoch leader lookup
    /// Accepts a shared reference to the node's epoch_eth_addresses
    pub fn with_epoch_eth_addresses(
        mut self,
        epoch_eth_addresses: Arc<tokio::sync::RwLock<std::collections::HashMap<u64, Vec<Vec<u8>>>>>,
    ) -> Self {
        self.epoch_eth_addresses = epoch_eth_addresses;
        self
    }

    /// Legacy method for backward compatibility - creates HashMap with epoch 0
    #[allow(dead_code)]
    pub fn with_validator_eth_addresses(mut self, eth_addresses: Vec<Vec<u8>>) -> Self {
        let mut map = std::collections::HashMap::new();
        map.insert(self.current_epoch, eth_addresses);
        self.epoch_eth_addresses = Arc::new(tokio::sync::RwLock::new(map));
        self
    }

    /// Get a clone of the Arc to epoch_eth_addresses for external updates
    #[allow(dead_code)]
    pub fn get_epoch_eth_addresses_arc(
        &self,
    ) -> Arc<tokio::sync::RwLock<std::collections::HashMap<u64, Vec<Vec<u8>>>>> {
        self.epoch_eth_addresses.clone()
    }

    /// Set block coordinator for dual-stream block production
    pub fn with_block_coordinator(mut self, coordinator: Arc<BlockCoordinator>) -> Self {
        self.block_coordinator = Some(coordinator);
        self
    }

    /// Set TX recycler for confirming committed TXs
    pub fn with_tx_recycler(mut self, recycler: Arc<TxRecycler>) -> Self {
        self.tx_recycler = Some(recycler);
        self
    }

    /// RS-2: Set storage path for persisting cumulative_fragment_offset
    pub fn with_storage_path(mut self, path: std::path::PathBuf) -> Self {
        self.storage_path = Some(path);
        self
    }

    /// Set a sender for lag alerts
    pub fn with_lag_alert_sender(
        mut self,
        sender: tokio::sync::mpsc::UnboundedSender<
            crate::consensus::commit_processor::lag_monitor::LagAlert,
        >,
    ) -> Self {
        self.lag_alert_sender = Some(sender);
        self
    }

    /// Set cold-start flag
    pub fn with_cold_start(mut self, cold_start: Arc<AtomicBool>) -> Self {
        self.cold_start = cold_start;
        self
    }

    /// Set GEI threshold for cold-start skip
    pub fn with_cold_start_skip_gei(mut self, skip_gei: u64) -> Self {
        self.cold_start_skip_gei = skip_gei;
        self
    }

    /// Process commits in order
    pub async fn run(self) -> Result<()> {
        let mut receiver = self.receiver;
        let mut next_expected_index = self.next_expected_index;
        let mut pending_commits = self.pending_commits;
        let commit_index_callback = self.commit_index_callback;
        let current_epoch = self.current_epoch;
        let executor_client = self.executor_client;
        let pending_transactions_queue = self.pending_transactions_queue;
        let epoch_transition_callback = self.epoch_transition_callback;

        // --- [FORK SAFETY FIX v3] ---
        // CRITICAL: epoch_base_index must be the GEI at the START of current epoch.
        // USE the value passed via with_epoch_info(), NOT shared_last_global_exec_index.
        // After cold-start restore, shared_last_global_exec_index gets updated to the
        // network commit (e.g., 4364), but the epoch boundary for epoch 1 is 0.
        // Using the wrong base causes: GEI = 4364 + commit_index (WRONG)
        // instead of: GEI = 0 + commit_index (CORRECT) → hash divergence!
        let epoch_base_index = if let Some(override_val) = self.epoch_base_index_override {
            override_val
        } else if let Some(ref shared_index) = self.shared_last_global_exec_index {
            let index_guard = shared_index.lock().await;
            *index_guard
        } else if let Some(ref callback) = self.get_last_global_exec_index {
            callback()
        } else {
            0
        };
        info!("🚀 [COMMIT PROCESSOR] Started processing commits for epoch {} (epoch_base_index={}, next_expected_index={}, override={:?})",
            current_epoch, epoch_base_index, next_expected_index, self.epoch_base_index_override);

        let mut last_heartbeat_commit = 0u32;
        let mut last_heartbeat_time = std::time::Instant::now();
        const HEARTBEAT_INTERVAL: u32 = 1000;
        const HEARTBEAT_TIMEOUT_SECS: u64 = 300;

        // Spawn LagMonitor if configured
        if let (Some(client), Some(shared_gei), Some(sender)) = (
            &executor_client,
            &self.shared_last_global_exec_index,
            self.lag_alert_sender,
        ) {
            let lag_monitor = crate::consensus::commit_processor::lag_monitor::LagMonitor::new(
                client.clone(),
                shared_gei.clone(),
                sender,
            );
            tokio::spawn(async move {
                lag_monitor.run().await;
            });
            info!("🛡️ LagMonitor spawned for CommitProcessor.");
        }

        info!("📡 [COMMIT PROCESSOR] Waiting for commits from consensus...");

        // FRAGMENTATION: Track cumulative extra GEIs consumed by block fragmentation.
        // When a large commit (12K+ TXs) is split into N fragments, the offset
        // accumulates (N-1) extra GEIs. This ensures:
        //   commit_5 (12K TXs, 3 fragments) → GEI = base+5+0, base+6, base+7 → offset += 2
        //   commit_6 (normal)               → GEI = base+6+2 = base+8 ← correct!
        // FORK-SAFETY: All nodes use the same MAX_TXS_PER_GO_BLOCK → identical offsets.
        // RS-2: Load persisted offset from disk for crash recovery.
        let mut cumulative_fragment_offset: u64 = if let Some(ref sp) = self.storage_path {
            let loaded = crate::node::executor_client::persistence::load_fragment_offset(sp);
            if loaded > 0 {
                info!(
                    "📂 [FRAGMENT-OFFSET] Recovered persisted offset={} from disk",
                    loaded
                );
            }
            loaded
        } else {
            0
        };
        let storage_path_for_persist = self.storage_path.clone();

        loop {
            match receiver.recv().await {
                Some(subdag) => {
                    let commit_index: u32 = subdag.commit_ref.index;
                    trace!("📥 [COMMIT PROCESSOR] Received committed subdag: commit_index={}, leader={:?}, blocks={}",
                        commit_index, subdag.leader, subdag.blocks.len());

                    // Heartbeat logic
                    if commit_index >= last_heartbeat_commit + HEARTBEAT_INTERVAL {
                        let elapsed = last_heartbeat_time.elapsed().as_secs();
                        info!("💓 [COMMIT PROCESSOR HEARTBEAT] Processed {} commits (last {} commits in {}s, pending_ooo={})", 
                            commit_index, HEARTBEAT_INTERVAL, elapsed, pending_commits.len());
                        last_heartbeat_commit = commit_index;
                        last_heartbeat_time = std::time::Instant::now();
                    }

                    // Check for stuck processor
                    let time_since_last_heartbeat = last_heartbeat_time.elapsed().as_secs();
                    if time_since_last_heartbeat > HEARTBEAT_TIMEOUT_SECS
                        && commit_index == last_heartbeat_commit
                    {
                        warn!("⚠️  [COMMIT PROCESSOR] Possible stuck detected: No progress for {}s (last commit: {})", 
                            time_since_last_heartbeat, commit_index);
                    }

                    trace!(
                        "📊 [COMMIT CONDITION] Checking commit_index={}, next_expected_index={}",
                        commit_index,
                        next_expected_index
                    );

                    // --- [AUTO-JUMP ON STARTUP] ---
                    // If this is the VERY FIRST commit we receive after restart, and it is > expected,
                    // we assume we are resuming from a higher commit index (provided by reliable Consensus Core).
                    // This avoids reading DB for initial index (which User disallowed).
                    if next_expected_index == 1 && commit_index > 1 {
                        warn!("🚀 [AUTO-JUMP] Initial commit {} > expected 1. Auto-jumping to match stream.", commit_index);
                        next_expected_index = commit_index;
                    }

                    if commit_index == next_expected_index {
                        // --- [FORK SAFETY v2: CONSENSUS-BASED FORMULA + FRAGMENTATION] ---
                        // global_exec_index = epoch_base_index + commit_index + cumulative_fragment_offset
                        // - epoch_base_index: Fixed at epoch start, same for all nodes
                        // - commit_index: From Mysticeti consensus, same for all nodes
                        // - cumulative_fragment_offset: Deterministic offset from previous fragmentations
                        // Result: Deterministic across all nodes, even with fragmentation!
                        let global_exec_index = calculate_global_exec_index(
                            current_epoch,
                            commit_index as u64 + cumulative_fragment_offset,
                            epoch_base_index,
                        );

                        // CC-1: Unified batch_id for end-to-end tracing (Rust → Go)
                        let batch_id =
                            format!("E{}C{}G{}", current_epoch, commit_index, global_exec_index);

                        trace!(
                            "[batch_id={}] 📊 Calculated: epoch_base={}, fragment_offset={}",
                            batch_id,
                            epoch_base_index,
                            cumulative_fragment_offset
                        );
                        info!(
                            "epoch_base_index for epoch {} is set to {}",
                            current_epoch, epoch_base_index
                        );

                        let total_txs_in_commit = subdag
                            .blocks
                            .iter()
                            .map(|b| b.transactions().len())
                            .sum::<usize>();

                        // CRITICAL FIX: Process commit FIRST before triggering epoch transition
                        // This ensures Go receives the EndOfEpoch commit before Rust starts transition
                        let geis_consumed = super::executor::dispatch_commit(
                            &subdag,
                            global_exec_index,
                            current_epoch,
                            executor_client.clone(),
                            pending_transactions_queue.clone(),
                            self.shared_last_global_exec_index.clone(),
                            self.epoch_eth_addresses.clone(), // Multi-epoch committee cache
                        )
                        .await?;

                        // ♻️ TX RECYCLER: Confirm committed TXs so they aren't re-submitted
                        if let Some(ref recycler) = self.tx_recycler {
                            if total_txs_in_commit > 0 {
                                let committed_tx_data: Vec<Vec<u8>> = subdag
                                    .blocks
                                    .iter()
                                    .flat_map(|b| {
                                        b.transactions().iter().map(|tx| tx.data().to_vec())
                                    })
                                    .collect();
                                recycler.confirm_committed(&committed_tx_data).await;
                            }
                        }

                        // NOTE: epoch_base_index is NOT updated after each commit.
                        // It remains constant throughout the epoch.
                        // The shared_last_global_exec_index is updated for monitoring/visibility only.

                        // FRAGMENTATION: Update callback with the LAST GEI of the fragment range
                        let last_gei = global_exec_index + geis_consumed - 1;
                        if let Some(ref callback) = self.global_exec_index_callback {
                            callback(last_gei);
                        }

                        if let Some(ref callback) = commit_index_callback {
                            callback(commit_index);
                        }

                        // FRAGMENTATION: Accumulate extra GEIs consumed by this commit
                        if geis_consumed > 1 {
                            cumulative_fragment_offset += geis_consumed - 1;
                            info!("🔪 [FRAGMENT-OFFSET] Commit {} consumed {} GEIs, cumulative_fragment_offset now {}",
                                commit_index, geis_consumed, cumulative_fragment_offset);
                            // RS-2: Persist after each change for crash recovery
                            if let Some(ref sp) = storage_path_for_persist {
                                let _ = crate::node::executor_client::persistence::persist_fragment_offset(sp, cumulative_fragment_offset).await;
                            }
                        }

                        next_expected_index += 1;

                        // Check for EndOfEpoch system transactions AFTER commit is sent to Go
                        if let Some((_block_ref, system_tx)) =
                            subdag.extract_end_of_epoch_transaction()
                        {
                            // SIMPLIFIED: as_end_of_epoch now returns (new_epoch, boundary_block)
                            // Timestamp is derived from block header at boundary_block (by Go/Rust later)
                            if let Some((new_epoch, boundary_block)) = system_tx.as_end_of_epoch() {
                                info!(
                                    "🎯 [SYSTEM TX] EndOfEpoch transaction detected in commit {}: epoch {} -> {}, boundary_block={}, total_txs_in_commit={}",
                                    commit_index, current_epoch, new_epoch, boundary_block, total_txs_in_commit
                                );

                                if let Some(ref callback) = epoch_transition_callback {
                                    info!(
                                        "🚀 [EPOCH TRANSITION] Triggering epoch transition AFTER commit sent to Go: commit_index={}, new_epoch={}, global_exec_index={}",
                                        commit_index, new_epoch, global_exec_index
                                    );

                                    // CHANGED: Pass boundary_block instead of timestamp_ms
                                    // Timestamp will be derived from boundary_block's block header
                                    if let Err(e) = callback(
                                        new_epoch,
                                        boundary_block, // boundary_block (was timestamp_ms)
                                        global_exec_index, // actual global_exec_index from commit
                                    ) {
                                        warn!("❌ Failed to trigger epoch transition from system transaction: {}", e);
                                    }
                                }

                                // We MUST break here! This epoch is over. Any remaining commits in the channel
                                // belong to the old epoch (empty trailing commits) and must NOT be sent to Go,
                                // otherwise Go will increment LastGlobalExecIndex and cause a hash mismatch for the new epoch.
                                info!("🛑 [COMMIT PROCESSOR] Halting processing for current epoch after EndOfEpoch transaction.");
                                break;
                            }
                        }

                        // Process pending out-of-order commits
                        let mut should_break = false;
                        while let Some(pending) = pending_commits.remove(&next_expected_index) {
                            let pending_commit_index = next_expected_index;

                            // Use epoch_base_index + fragment offset for pending commits (same formula)
                            let global_exec_index = calculate_global_exec_index(
                                current_epoch,
                                pending_commit_index as u64 + cumulative_fragment_offset,
                                epoch_base_index,
                            );

                            let pending_geis = super::executor::dispatch_commit(
                                &pending,
                                global_exec_index,
                                current_epoch,
                                executor_client.clone(),
                                pending_transactions_queue.clone(),
                                self.shared_last_global_exec_index.clone(),
                                self.epoch_eth_addresses.clone(), // Multi-epoch committee cache
                            )
                            .await?;

                            // FRAGMENTATION: Accumulate extra GEIs from pending commits
                            if pending_geis > 1 {
                                cumulative_fragment_offset += pending_geis - 1;
                                info!("🔪 [FRAGMENT-OFFSET] Pending commit {} consumed {} GEIs, cumulative_fragment_offset now {}",
                                    pending_commit_index, pending_geis, cumulative_fragment_offset);
                                // RS-2: Persist after each change for crash recovery
                                if let Some(ref sp) = storage_path_for_persist {
                                    let _ = crate::node::executor_client::persistence::persist_fragment_offset(sp, cumulative_fragment_offset).await;
                                }
                            }

                            if let Some(ref callback) = commit_index_callback {
                                callback(pending_commit_index);
                            }

                            next_expected_index += 1;

                            // Check for EndOfEpoch in pending commits
                            if let Some((_block_ref, system_tx)) =
                                pending.extract_end_of_epoch_transaction()
                            {
                                if let Some((new_epoch, boundary_block)) =
                                    system_tx.as_end_of_epoch()
                                {
                                    if let Some(ref callback) = epoch_transition_callback {
                                        if let Err(e) =
                                            callback(new_epoch, boundary_block, global_exec_index)
                                        {
                                            warn!("❌ Failed to trigger epoch transition from pending system transaction: {}", e);
                                        }
                                    }

                                    info!("🛑 [COMMIT PROCESSOR] Halting processing for current epoch after EndOfEpoch transaction in PENDING commit.");
                                    should_break = true;
                                    break;
                                }
                            }
                        }

                        if should_break {
                            break;
                        }
                    } else if commit_index > next_expected_index {
                        // SAFETY: Limit pending_commits size to prevent OOM
                        const MAX_PENDING_COMMITS: usize = 5000;
                        if pending_commits.len() >= MAX_PENDING_COMMITS {
                            warn!(
                                "🚨 [COMMIT PROCESSOR] pending_commits at capacity ({})! \
                                Dropping out-of-order commit {} (expected {}). \
                                This indicates severe downstream stall.",
                                MAX_PENDING_COMMITS, commit_index, next_expected_index
                            );
                            continue;
                        }
                        warn!(
                            "Received out-of-order commit: index={}, expected={}, pending_count={}, storing for later",
                            commit_index, next_expected_index, pending_commits.len()
                        );
                        pending_commits.insert(commit_index, subdag);
                    } else {
                        warn!(
                            "Received commit with index {} which is less than expected {}",
                            commit_index, next_expected_index
                        );
                    }
                }
                None => {
                    warn!("⚠️  [COMMIT PROCESSOR] Commit receiver closed (commit processor will exit).");
                    break;
                }
            }
        }
        Ok(())
    }
}
