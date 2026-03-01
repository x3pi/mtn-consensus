// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

use anyhow::Result;
use consensus_core::{BlockAPI, CommittedSubDag};
use hex;
use mysten_metrics::monitored_mpsc::UnboundedReceiver;
use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration; // [Added] Import Duration
use tokio::time::sleep; // [Added] Import sleep for retry mechanism
use tracing::{error, info, trace, warn};

use crate::consensus::checkpoint::calculate_global_exec_index;
use crate::consensus::tx_recycler::TxRecycler;
use crate::node::block_coordinator::BlockCoordinator;
use crate::node::executor_client::ExecutorClient;
use crate::types::tx_hash::calculate_transaction_hash_hex;

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
    epoch_eth_addresses: Arc<tokio::sync::Mutex<std::collections::HashMap<u64, Vec<Vec<u8>>>>>,
    /// Block Coordinator for dual-stream block production (optional)
    block_coordinator: Option<Arc<BlockCoordinator>>,
    /// TX recycler for confirming committed TXs
    tx_recycler: Option<Arc<TxRecycler>>,
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
            executor_client: None,
            is_transitioning: None,
            pending_transactions_queue: None,
            epoch_transition_callback: None,
            epoch_eth_addresses: Arc::new(
                tokio::sync::Mutex::new(std::collections::HashMap::new()),
            ),
            block_coordinator: None,
            tx_recycler: None,
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
    pub fn with_epoch_info(mut self, epoch: u64, _last_global_exec_index: u64) -> Self {
        self.current_epoch = epoch;
        // last_global_exec_index is now obtained from shared_last_global_exec_index, no need to store locally
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
        epoch_eth_addresses: Arc<tokio::sync::Mutex<std::collections::HashMap<u64, Vec<Vec<u8>>>>>,
    ) -> Self {
        self.epoch_eth_addresses = epoch_eth_addresses;
        self
    }

    /// Legacy method for backward compatibility - creates HashMap with epoch 0
    #[allow(dead_code)]
    pub fn with_validator_eth_addresses(mut self, eth_addresses: Vec<Vec<u8>>) -> Self {
        let mut map = std::collections::HashMap::new();
        map.insert(self.current_epoch, eth_addresses);
        self.epoch_eth_addresses = Arc::new(tokio::sync::Mutex::new(map));
        self
    }

    /// Get a clone of the Arc to epoch_eth_addresses for external updates
    #[allow(dead_code)]
    pub fn get_epoch_eth_addresses_arc(
        &self,
    ) -> Arc<tokio::sync::Mutex<std::collections::HashMap<u64, Vec<Vec<u8>>>>> {
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

        // --- [FORK SAFETY FIX v2] ---
        // CRITICAL: epoch_base_index is set ONCE at epoch start and never changes during epoch.
        // This is the last_global_exec_index at the END of previous epoch (or 0 for epoch 0).
        // All nodes receive this same value from Go via GetEpochBoundaryData API.
        // Formula: global_exec_index = epoch_base_index + commit_index
        // Since commit_index is consensus-agreed (from Mysticeti), all nodes compute same result.
        let epoch_base_index = if let Some(ref shared_index) = self.shared_last_global_exec_index {
            let index_guard = shared_index.lock().await;
            *index_guard
        } else {
            // Fallback: try callback if shared index not available
            if let Some(ref callback) = self.get_last_global_exec_index {
                callback()
            } else {
                0
            }
        };
        info!("🚀 [COMMIT PROCESSOR] Started processing commits for epoch {} (epoch_base_index={}, next_expected_index={})",
            current_epoch, epoch_base_index, next_expected_index);

        let mut last_heartbeat_commit = 0u32;
        let mut last_heartbeat_time = std::time::Instant::now();
        const HEARTBEAT_INTERVAL: u32 = 1000;
        const HEARTBEAT_TIMEOUT_SECS: u64 = 300;

        info!("📡 [COMMIT PROCESSOR] Waiting for commits from consensus...");

        loop {
            match receiver.recv().await {
                Some(subdag) => {
                    let commit_index: u32 = subdag.commit_ref.index;
                    info!("📥 [COMMIT PROCESSOR] Received committed subdag: commit_index={}, leader={:?}, blocks={}",
                        commit_index, subdag.leader, subdag.blocks.len());

                    // Heartbeat logic
                    if commit_index >= last_heartbeat_commit + HEARTBEAT_INTERVAL {
                        let elapsed = last_heartbeat_time.elapsed().as_secs();
                        info!("💓 [COMMIT PROCESSOR HEARTBEAT] Processed {} commits (last {} commits in {}s)", 
                            commit_index, HEARTBEAT_INTERVAL, elapsed);
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

                    info!(
                        "📊 [COMMIT CONDITION] Checking commit_index={}, next_expected_index={}",
                        commit_index, next_expected_index
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
                        // --- [FORK SAFETY v2: CONSENSUS-BASED FORMULA] ---
                        // global_exec_index = epoch_base_index + commit_index
                        // - epoch_base_index: Fixed at epoch start, same for all nodes
                        // - commit_index: From Mysticeti consensus, same for all nodes
                        // Result: Deterministic across all nodes, even late joiners!
                        let global_exec_index = calculate_global_exec_index(
                            current_epoch,
                            commit_index,
                            epoch_base_index,
                        );

                        info!("📊 [GLOBAL_EXEC_INDEX] Calculated: global_exec_index={}, epoch={}, commit_index={}, epoch_base_index={}",
                            global_exec_index, current_epoch, commit_index, epoch_base_index);

                        let total_txs_in_commit = subdag
                            .blocks
                            .iter()
                            .map(|b| b.transactions().len())
                            .sum::<usize>();

                        // CRITICAL FIX: Process commit FIRST before triggering epoch transition
                        // This ensures Go receives the EndOfEpoch commit before Rust starts transition
                        Self::process_commit(
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

                        if let Some(ref callback) = self.global_exec_index_callback {
                            callback(global_exec_index);
                        }

                        if let Some(ref callback) = commit_index_callback {
                            callback(commit_index);
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
                            }
                        }

                        // Process pending out-of-order commits
                        while let Some(pending) = pending_commits.remove(&next_expected_index) {
                            let pending_commit_index = next_expected_index;

                            // Use epoch_base_index for pending commits as well (same formula)
                            let global_exec_index = calculate_global_exec_index(
                                current_epoch,
                                pending_commit_index,
                                epoch_base_index,
                            );

                            Self::process_commit(
                                &pending,
                                global_exec_index,
                                current_epoch,
                                executor_client.clone(),
                                pending_transactions_queue.clone(),
                                self.shared_last_global_exec_index.clone(),
                                self.epoch_eth_addresses.clone(), // Multi-epoch committee cache
                            )
                            .await?;

                            // epoch_base_index is NOT updated (it's constant per epoch)

                            if let Some(ref callback) = commit_index_callback {
                                callback(pending_commit_index);
                            }

                            next_expected_index += 1;
                        }
                    } else if commit_index > next_expected_index {
                        warn!(
                            "Received out-of-order commit: index={}, expected={}, storing for later",
                            commit_index, next_expected_index
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

    /// Queue all user transactions from a failed commit for processing in the next epoch
    async fn queue_commit_transactions_for_next_epoch(
        subdag: &CommittedSubDag,
        pending_transactions_queue: &Arc<tokio::sync::Mutex<Vec<Vec<u8>>>>,
        commit_index: u32,
        global_exec_index: u64,
        epoch: u64,
    ) {
        let has_end_of_epoch = subdag.extract_end_of_epoch_transaction().is_some();

        let mut queued_count = 0;
        let mut skipped_count = 0;

        for (block_idx, block) in subdag.blocks.iter().enumerate() {
            for (tx_idx, tx) in block.transactions().iter().enumerate() {
                let tx_data = tx.data();

                // Skip EndOfEpoch system transactions - they are epoch-specific
                if has_end_of_epoch && Self::is_end_of_epoch_transaction(tx_data) {
                    info!("ℹ️  [TX FLOW] Skipping EndOfEpoch system transaction in failed commit {} (epoch-specific, cannot be queued for next epoch)", commit_index);
                    skipped_count += 1;
                    continue;
                }

                // Queue the transaction for next epoch
                let mut queue = pending_transactions_queue.lock().await;
                queue.push(tx_data.to_vec());
                queued_count += 1;

                let tx_hash = crate::types::tx_hash::calculate_transaction_hash(tx_data);
                let tx_hash_hex = hex::encode(&tx_hash[..8]);

                info!("📦 [TX FLOW] Queued failed transaction from commit {} block {} tx {}: hash={} (size={})",
                    commit_index, block_idx, tx_idx, tx_hash_hex, tx_data.len());
            }
        }

        if queued_count > 0 {
            info!("✅ [TX FLOW] Queued {} transactions from failed commit {} (global_exec_index={}, epoch={}) for next epoch",
                queued_count, commit_index, global_exec_index, epoch);
        }

        if skipped_count > 0 {
            info!("ℹ️  [TX FLOW] Skipped {} system transactions from failed commit {} (not suitable for next epoch)",
                skipped_count, commit_index);
        }
    }

    fn is_end_of_epoch_transaction(tx_data: &[u8]) -> bool {
        if tx_data.len() < 10 {
            return false;
        }
        let data_str = String::from_utf8_lossy(tx_data);
        data_str.contains("EndOfEpoch")
            || data_str.contains("epoch") && data_str.contains("transition")
    }

    async fn process_commit(
        subdag: &CommittedSubDag,
        global_exec_index: u64,
        epoch: u64,
        executor_client: Option<Arc<ExecutorClient>>,
        pending_transactions_queue: Option<Arc<tokio::sync::Mutex<Vec<Vec<u8>>>>>,
        shared_last_global_exec_index: Option<Arc<tokio::sync::Mutex<u64>>>,
        validator_eth_addresses: Arc<
            tokio::sync::Mutex<std::collections::HashMap<u64, Vec<Vec<u8>>>>,
        >, // Multi-epoch committee cache
    ) -> Result<()> {
        let commit_index = subdag.commit_ref.index;
        let mut total_transactions = 0;
        let mut transaction_hashes = Vec::new();
        let mut block_details = Vec::new();

        for (block_idx, block) in subdag.blocks.iter().enumerate() {
            let transactions = block.transactions();
            total_transactions += transactions.len();

            for tx in transactions {
                let tx_data = tx.data();
                let tx_hash_hex = calculate_transaction_hash_hex(tx_data);
                transaction_hashes.push(tx_hash_hex);
            }

            block_details.push(format!(
                "block[{}]={:?} ({}tx)",
                block_idx,
                block.reference(),
                transactions.len()
            ));
        }

        let has_system_tx = subdag.extract_end_of_epoch_transaction().is_some();

        // ═══════════════════════════════════════════════════════════════════════════════
        // 🛡️ RUST-DRIVEN LEADER SELECTION (Critical Fork-Safety)
        // Calculate leader_address for ALL commits (empty or not)
        // NEVER send None - if we can't determine leader, BLOCK/PANIC
        // ═══════════════════════════════════════════════════════════════════════════════
        let leader_address: Option<Vec<u8>> = if executor_client.is_some() {
            let leader_author_index = subdag.leader.author.value() as usize;

            // STEP 1: Validate committee data exists (with retry for startup race condition)
            let mut retry_attempts = 0;
            let max_retries = 10; // 10 * 200ms = 2 seconds max wait

            let resolved_address = loop {
                let epoch_addresses = validator_eth_addresses.lock().await;

                // Check if committee HashMap is loaded
                if epoch_addresses.is_empty() {
                    drop(epoch_addresses);
                    retry_attempts += 1;
                    if retry_attempts > max_retries {
                        error!("🚨 [FATAL] epoch_eth_addresses STILL EMPTY after {} retries! Committee not loaded.", max_retries);
                        error!("🚨 [FATAL] Cannot process commit #{} (global_exec_index={}) without valid committee data!", 
                            commit_index, global_exec_index);
                        panic!(
                            "FORK-SAFETY: Committee data not loaded - refusing to process commits"
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
                            panic!("FORK-SAFETY: No committee data in cache");
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
                        panic!("FORK-SAFETY: No committee data in cache");
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
                        panic!("FORK-SAFETY: Leader index out of range - committee data inconsistent after all retries");
                    }

                    warn!(
                        "⚠️ [LEADER] leader_index {} >= committee_size {} for epoch {}. Refreshing cache... retry {}/{}",
                        leader_author_index, committee_size, epoch, retry_attempts, max_retries
                    );

                    // Try to refresh epoch_eth_addresses from Go
                    if let Some(ref client) = executor_client {
                        match client.get_epoch_boundary_data(epoch).await {
                            Ok((returned_epoch, _ts, _boundary, validators))
                                if returned_epoch == epoch =>
                            {
                                // Sort validators same way as committee builder
                                let mut sorted_validators: Vec<_> =
                                    validators.into_iter().collect();
                                sorted_validators
                                    .sort_by(|a, b| a.authority_key.cmp(&b.authority_key));

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
                                let mut cache = validator_eth_addresses.lock().await;
                                cache.insert(epoch, new_eth_addresses);
                                info!(
                                    "🔄 [LEADER] Refreshed epoch_eth_addresses for epoch {}: now have {} validators (cache size: {})",
                                    epoch, sorted_validators.len(), cache.len()
                                );
                            }
                            Ok((returned_epoch, _, _, _)) => {
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
                        panic!("FORK-SAFETY: Invalid ETH address in committee - cannot determine leader safely");
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
            info!(
                "🔷 [Global Index: {}] Executing commit #{} (epoch={}): {} blocks, {} txs, has_system_tx={}",
                global_exec_index, commit_index, epoch, subdag.blocks.len(), total_transactions, has_system_tx
            );
        } else {
            // Still log empty commits but as trace/debug to avoid spam
            tracing::trace!(
                "⏭️ [TX FLOW] Forwarding empty commit to Go Master (for sequence sync): global_exec_index={}, commit_index={}",
                global_exec_index, commit_index
            );
        }

        if let Some(ref client) = executor_client {
            // leader_address already calculated and validated above

            let mut retry_count = 0;
            loop {
                match client
                    .send_committed_subdag(subdag, epoch, global_exec_index, leader_address.clone())
                    .await
                {
                    Ok(_) => {
                        info!("✅ [TX FLOW] Successfully sent committed subdag: global_exec_index={}, commit_index={}",
                                global_exec_index, commit_index);

                        if let Some(shared_index) = shared_last_global_exec_index.clone() {
                            let mut index_guard = shared_index.lock().await;
                            *index_guard = global_exec_index;
                            info!("📊 [GLOBAL_EXEC_INDEX] Updated shared last_global_exec_index to {} after successful send", global_exec_index);
                        }

                        // Track committed transaction hashes to prevent duplicates during epoch transitions
                        // CRITICAL: Only track when commit is actually processed, not just submitted
                        //
                        // FIX 2026-02-06: Use try_lock() to avoid deadlock with transition handler
                        // The transition handler may hold the node lock while waiting for Go to sync.
                        // If we block here, CommitProcessor stalls and Go never gets more commits = DEADLOCK.
                        // If lock is unavailable, skip tracking - next commit will retry.
                        if let Some(node_arc) = crate::node::get_transition_handler_node().await {
                            // Use try_lock() instead of lock().await to avoid blocking
                            let node_guard = match node_arc.try_lock() {
                                Ok(guard) => guard,
                                Err(_) => {
                                    // Lock held by transition handler - skip TX tracking to avoid deadlock
                                    trace!("⏭️ [TX TRACKING] Skipping tracking for commit {} - node lock held by transition handler", commit_index);
                                    break; // Exit the retry loop, commit was sent successfully
                                }
                            };
                            let mut hashes_guard =
                                node_guard.committed_transaction_hashes.lock().await;

                            let mut tracked_count = 0;
                            let mut batch_hashes = Vec::new();
                            for block in &subdag.blocks {
                                for tx in block.transactions() {
                                    let tx_hash = crate::types::tx_hash::calculate_transaction_hash(
                                        tx.data(),
                                    );
                                    let hash_hex = hex::encode(&tx_hash);

                                    // Special debug logging for the problematic transaction
                                    if hash_hex.starts_with("44a535f2") {
                                        warn!("🔍 [DEBUG] Committing problematic transaction {} in commit #{} (global_exec_index={})",
                                                  hash_hex, commit_index, global_exec_index);
                                    }

                                    hashes_guard.insert(tx_hash.clone());
                                    batch_hashes.push(tx_hash);
                                    tracked_count += 1;
                                }
                            }

                            // Also save to persistent storage for epoch transition recovery (in ONE batch!)
                            if !batch_hashes.is_empty() {
                                if let Err(e) = crate::node::transition::save_committed_transaction_hashes_batch(
                                        &node_guard.storage_path, epoch, &batch_hashes
                                    ).await {
                                        warn!("⚠️ [TX TRACKING] Failed to persist committed hashes after commit: {}", e);
                                    } else {
                                        trace!("💾 [TX TRACKING] Persisted {} committed hashes for epoch {}", batch_hashes.len(), epoch);
                                    }
                            }

                            if tracked_count > 0 {
                                info!("💾 [TX TRACKING] Tracked {} committed transaction hashes after processing commit #{} (global_exec_index={})",
                                          tracked_count, commit_index, global_exec_index);
                            }
                        }

                        break;
                    }
                    Err(e) => {
                        // Case 1: Duplicate index - Critical Fork Safety check
                        if e.to_string().contains("Duplicate global_exec_index") {
                            error!("🚨 [FORK-SAFETY] Duplicate global_exec_index={} detected! Skipping commit {} to prevent fork. Error: {}", 
                                    global_exec_index, commit_index, e);
                            break;
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
                            Self::queue_commit_transactions_for_next_epoch(
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
                        break;
                    }
                }
            }
        } else {
            info!("ℹ️  [TX FLOW] Executor client not enabled, skipping send");
        }

        Ok(())
    }
}
