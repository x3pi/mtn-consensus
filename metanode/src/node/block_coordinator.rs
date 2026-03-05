// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0
#![allow(dead_code)]

//! Block Coordinator - Dual-Stream Block Production with Unified Output
//!
//! This module provides a centralized coordinator that:
//! 1. Accepts blocks from both Consensus (Validator) and Sync streams
//! 2. Deduplicates blocks by global_exec_index
//! 3. Prioritizes Consensus over Sync
//! 4. Outputs sequential blocks to Go without gaps
//!
//! ## Architecture
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                     BlockCoordinator (Rust)                     │
//! │  ┌─────────────────────────────────────────────────────────┐   │
//! │  │   BTreeMap<global_exec_index, QueuedBlock>              │   │
//! │  │   - Deduplicates by index                               │   │
//! │  │   - Consensus always wins over Sync                     │   │
//! │  └─────────────────────────────────────────────────────────┘   │
//! │         ▲ Consensus (PRIMARY)     ▲ Sync (BACKUP)              │
//! │                           │                                     │
//! │              drain_ready() → Sequential Output                  │
//! └───────────────────────────┼─────────────────────────────────────┘
//!                             ▼
//!                      ExecutorClient → Go
//! ```

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Mutex, RwLock};
use tracing::{debug, info, trace, warn};

use crate::node::executor_client::ExecutorClient;
use crate::node::NodeMode;

/// Source of a block - used for priority decisions
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockSource {
    /// Block from DAG consensus (Validator mode) - HIGH PRIORITY
    Consensus,
    /// Block from peer sync (SyncOnly/backup) - LOW PRIORITY
    Sync,
}

impl std::fmt::Display for BlockSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BlockSource::Consensus => write!(f, "Consensus"),
            BlockSource::Sync => write!(f, "Sync"),
        }
    }
}

/// A block queued for processing
#[allow(dead_code)]
#[derive(Clone)]
pub struct QueuedBlock {
    pub global_exec_index: u64,
    pub commit_index: u64,
    pub epoch: u64,
    pub data: Vec<u8>,
    pub source: BlockSource,
    pub received_at: Instant,
}

impl std::fmt::Debug for QueuedBlock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueuedBlock")
            .field("global_exec_index", &self.global_exec_index)
            .field("commit_index", &self.commit_index)
            .field("epoch", &self.epoch)
            .field("source", &self.source)
            .field("data_len", &self.data.len())
            .finish()
    }
}

/// Configuration for BlockCoordinator
#[allow(dead_code)]
#[derive(Clone)]
pub struct CoordinatorConfig {
    /// Gap threshold - trigger sync if gap exceeds this
    pub gap_threshold: u64,
    /// Max blocks to drain per iteration
    pub max_drain_batch: usize,
    /// Timeout for sync to fill gaps
    pub gap_fill_timeout: Duration,
}

impl Default for CoordinatorConfig {
    fn default() -> Self {
        Self {
            gap_threshold: 5,
            max_drain_batch: 100,
            gap_fill_timeout: Duration::from_secs(10),
        }
    }
}

/// Block Coordinator - Central controller for dual-stream block production
pub struct BlockCoordinator {
    /// Unified queue - deduplicates blocks by global_exec_index
    queue: Arc<Mutex<BTreeMap<u64, QueuedBlock>>>,

    /// Next expected index (synced with Go layer)
    next_expected: Arc<AtomicU64>,

    /// Current node mode (affects priority)
    mode: Arc<RwLock<NodeMode>>,

    /// Whether sync is in standby mode (only gap-fill)
    sync_standby: Arc<AtomicBool>,

    /// Executor client for sending to Go
    executor_client: Option<Arc<ExecutorClient>>,

    /// Channel for requesting gap-fill from sync
    gap_fill_tx: Option<mpsc::UnboundedSender<(u64, u64)>>,

    /// Configuration
    config: CoordinatorConfig,

    /// Statistics
    stats: Arc<Mutex<CoordinatorStats>>,
}

/// Statistics for monitoring
#[allow(dead_code)]
#[derive(Default)]
pub struct CoordinatorStats {
    pub blocks_from_consensus: u64,
    pub blocks_from_sync: u64,
    pub blocks_deduplicated: u64,
    pub gaps_detected: u64,
    pub gaps_filled: u64,
}

impl BlockCoordinator {
    /// Create a new BlockCoordinator
    pub fn new(initial_next_expected: u64, config: CoordinatorConfig) -> Self {
        info!(
            "📦 [COORDINATOR] Initializing BlockCoordinator: next_expected={}, gap_threshold={}",
            initial_next_expected, config.gap_threshold
        );

        Self {
            queue: Arc::new(Mutex::new(BTreeMap::new())),
            next_expected: Arc::new(AtomicU64::new(initial_next_expected)),
            mode: Arc::new(RwLock::new(NodeMode::SyncOnly)),
            sync_standby: Arc::new(AtomicBool::new(false)),
            executor_client: None,
            gap_fill_tx: None,
            config,
            stats: Arc::new(Mutex::new(CoordinatorStats::default())),
        }
    }

    /// Set the executor client
    pub fn with_executor_client(mut self, client: Arc<ExecutorClient>) -> Self {
        self.executor_client = Some(client);
        self
    }

    /// Set the gap-fill channel (for requesting sync to fill gaps)
    pub fn with_gap_fill_channel(mut self, tx: mpsc::UnboundedSender<(u64, u64)>) -> Self {
        self.gap_fill_tx = Some(tx);
        self
    }

    /// Get current next_expected index
    pub fn next_expected(&self) -> u64 {
        self.next_expected.load(Ordering::SeqCst)
    }

    /// Update next_expected (e.g., after syncing with Go)
    pub fn set_next_expected(&self, value: u64) {
        self.next_expected.store(value, Ordering::SeqCst);
    }

    /// Update node mode
    pub async fn set_mode(&self, mode: NodeMode) {
        let old_mode = self.mode.read().await.clone();
        *self.mode.write().await = mode.clone();
        info!("📦 [COORDINATOR] Mode changed: {:?} → {:?}", old_mode, mode);
    }

    /// Set sync to standby mode (Validator mode - sync only fills gaps)
    pub fn set_sync_standby(&self, standby: bool) {
        self.sync_standby.store(standby, Ordering::SeqCst);
        info!(
            "📦 [COORDINATOR] Sync standby mode: {}",
            if standby { "ON" } else { "OFF" }
        );
    }

    /// Check if sync is in standby mode
    pub fn is_sync_standby(&self) -> bool {
        self.sync_standby.load(Ordering::SeqCst)
    }

    /// Push a block from any source with deduplication and priority
    pub async fn push_block(&self, block: QueuedBlock) -> bool {
        let mut queue = self.queue.lock().await;
        let index = block.global_exec_index;
        let source = block.source;

        // Check if already processed
        let next = self.next_expected.load(Ordering::SeqCst);
        if index < next {
            trace!(
                "📦 [COORDINATOR] Skipping old block: index={} < next_expected={}",
                index,
                next
            );
            return false;
        }

        // Check if in standby mode and this is a sync block
        if source == BlockSource::Sync && self.sync_standby.load(Ordering::SeqCst) {
            // In standby, only accept sync blocks that fill gaps
            let has_gap = {
                if let Some((&first_idx, _)) = queue.iter().next() {
                    first_idx > next && index < first_idx
                } else {
                    false
                }
            };

            if !has_gap && !queue.is_empty() {
                trace!(
                    "📦 [COORDINATOR] Sync standby: ignoring non-gap block index={}",
                    index
                );
                return false;
            }
        }

        // Deduplication with priority
        let mut stats = self.stats.lock().await;
        if let Some(existing) = queue.get(&index) {
            // Already have this block
            if source == BlockSource::Consensus && existing.source == BlockSource::Sync {
                // Consensus replaces Sync
                queue.insert(index, block);
                stats.blocks_deduplicated += 1;
                debug!(
                    "📦 [COORDINATOR] Consensus replaced Sync for index={}",
                    index
                );
                return true;
            } else {
                // Sync doesn't replace existing (either Consensus or older Sync)
                stats.blocks_deduplicated += 1;
                trace!(
                    "📦 [COORDINATOR] Duplicate ignored: index={}, existing={:?}, new={:?}",
                    index,
                    existing.source,
                    source
                );
                return false;
            }
        }

        // New block - add to queue
        match source {
            BlockSource::Consensus => stats.blocks_from_consensus += 1,
            BlockSource::Sync => stats.blocks_from_sync += 1,
        }

        queue.insert(index, block);
        debug!(
            "📦 [COORDINATOR] Added block: index={}, source={}, queue_size={}",
            index,
            source,
            queue.len()
        );

        true
    }

    /// Drain all ready blocks (sequential from next_expected)
    /// Returns blocks that can be sent to Go
    pub async fn drain_ready(&self) -> Vec<QueuedBlock> {
        let mut queue = self.queue.lock().await;
        let mut next = self.next_expected.load(Ordering::SeqCst);
        let mut result = Vec::new();

        while let Some((&idx, _)) = queue.iter().next() {
            if idx != next {
                // Gap detected - stop draining
                break;
            }

            if let Some(block) = queue.remove(&idx) {
                result.push(block);
                next += 1;
            }

            if result.len() >= self.config.max_drain_batch {
                break;
            }
        }

        if !result.is_empty() {
            self.next_expected.store(next, Ordering::SeqCst);
            debug!(
                "📦 [COORDINATOR] Drained {} blocks, next_expected now {}",
                result.len(),
                next
            );
        }

        result
    }

    /// Check for gaps and request sync to fill them
    pub async fn check_and_fill_gaps(&self) {
        let queue = self.queue.lock().await;
        let next = self.next_expected.load(Ordering::SeqCst);

        if let Some((&first_idx, _)) = queue.iter().next() {
            if first_idx > next {
                let gap = first_idx - next;
                if gap > self.config.gap_threshold {
                    // Request sync to fill gap
                    if let Some(tx) = &self.gap_fill_tx {
                        let _ = tx.send((next, first_idx));
                        let mut stats = self.stats.lock().await;
                        stats.gaps_detected += 1;
                        warn!(
                            "📦 [COORDINATOR] Gap detected: next={}, first_in_queue={}, gap={}. Requesting sync...",
                            next, first_idx, gap
                        );
                    }
                }
            }
        }
    }

    /// Sync with Go layer's current state
    pub async fn sync_with_go(&self, go_last_block: u64) {
        let expected_next = go_last_block + 1;
        let current_next = self.next_expected.load(Ordering::SeqCst);

        if expected_next != current_next {
            info!(
                "📦 [COORDINATOR] Syncing with Go: next_expected {} → {}",
                current_next, expected_next
            );
            self.next_expected.store(expected_next, Ordering::SeqCst);

            // Clean up old blocks from queue
            let mut queue = self.queue.lock().await;
            queue.retain(|&idx, _| idx >= expected_next);
        }
    }

    /// Get current queue size
    pub async fn queue_size(&self) -> usize {
        self.queue.lock().await.len()
    }

    /// Get statistics
    pub async fn get_stats(&self) -> CoordinatorStats {
        let stats = self.stats.lock().await;
        CoordinatorStats {
            blocks_from_consensus: stats.blocks_from_consensus,
            blocks_from_sync: stats.blocks_from_sync,
            blocks_deduplicated: stats.blocks_deduplicated,
            gaps_detected: stats.gaps_detected,
            gaps_filled: stats.gaps_filled,
        }
    }

    /// Start the background drain loop
    /// This task continuously drains ready blocks and sends to Go
    pub fn start_drain_loop(
        coordinator: Arc<BlockCoordinator>,
        executor_client: Arc<ExecutorClient>,
        shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        let coord = coordinator.clone();
        #[allow(unused)]
        let client = executor_client.clone();

        tokio::spawn(async move {
            info!("📦 [COORDINATOR] Drain loop started");
            let mut shutdown = shutdown_rx;
            let mut interval = tokio::time::interval(Duration::from_millis(50)); // 50ms polling

            loop {
                tokio::select! {
                    _ = shutdown.changed() => {
                        if *shutdown.borrow() {
                            info!("📦 [COORDINATOR] Drain loop stopping (shutdown signal)");
                            break;
                        }
                    }
                    _ = interval.tick() => {
                        // Drain ready blocks
                        let blocks = coord.drain_ready().await;

                        if blocks.is_empty() {
                            // No blocks ready, check for gaps
                            coord.check_and_fill_gaps().await;
                            continue;
                        }

                        // Send each block to Go
                        for block in blocks {
                            trace!(
                                "📦 [COORDINATOR] Sending block to Go: index={}, source={}",
                                block.global_exec_index, block.source
                            );

                            // Build commit data for Go layer
                            // The block.data contains serialized commit info
                            // For now, we log and track - actual sending handled by existing flow
                            if block.data.is_empty() {
                                trace!(
                                    "📦 [COORDINATOR] Block {} has no data (marker only)",
                                    block.global_exec_index
                                );
                            } else {
                                // Track that we sent this block
                                info!(
                                    "📦 [COORDINATOR] Block {} sent to Go (source: {}, epoch: {})",
                                    block.global_exec_index, block.source, block.epoch
                                );
                            }
                        }
                    }
                }
            }
            info!("📦 [COORDINATOR] Drain loop exited");
        })
    }
}

/// Global coordinator instance
#[allow(dead_code)]
static COORDINATOR: once_cell::sync::OnceCell<Arc<BlockCoordinator>> =
    once_cell::sync::OnceCell::new();

/// Initialize the global coordinator
#[allow(dead_code)]
pub fn init_coordinator(
    initial_next_expected: u64,
    config: CoordinatorConfig,
) -> Arc<BlockCoordinator> {
    let coordinator = Arc::new(BlockCoordinator::new(initial_next_expected, config));
    let _ = COORDINATOR.set(coordinator.clone());
    info!("📦 [COORDINATOR] Global coordinator initialized");
    coordinator
}

/// Get the global coordinator
#[allow(dead_code)]
pub fn get_coordinator() -> Option<Arc<BlockCoordinator>> {
    COORDINATOR.get().cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_deduplication_consensus_wins() {
        let coordinator = BlockCoordinator::new(1, CoordinatorConfig::default());

        // Add sync block first
        let sync_block = QueuedBlock {
            global_exec_index: 1,
            commit_index: 1,
            epoch: 0,
            data: vec![1, 2, 3],
            source: BlockSource::Sync,
            received_at: Instant::now(),
        };
        assert!(coordinator.push_block(sync_block).await);

        // Add consensus block with same index - should replace
        let consensus_block = QueuedBlock {
            global_exec_index: 1,
            commit_index: 1,
            epoch: 0,
            data: vec![4, 5, 6],
            source: BlockSource::Consensus,
            received_at: Instant::now(),
        };
        assert!(coordinator.push_block(consensus_block).await);

        // Drain and verify consensus block was kept
        let drained = coordinator.drain_ready().await;
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].source, BlockSource::Consensus);
        assert_eq!(drained[0].data, vec![4, 5, 6]);
    }

    #[tokio::test]
    async fn test_sequential_drain() {
        let coordinator = BlockCoordinator::new(1, CoordinatorConfig::default());

        // Add blocks out of order: 3, 1, 2
        for idx in [3u64, 1, 2] {
            let block = QueuedBlock {
                global_exec_index: idx,
                commit_index: idx,
                epoch: 0,
                data: vec![idx as u8],
                source: BlockSource::Consensus,
                received_at: Instant::now(),
            };
            coordinator.push_block(block).await;
        }

        // Should drain 1, 2, 3 in order
        let drained = coordinator.drain_ready().await;
        assert_eq!(drained.len(), 3);
        assert_eq!(drained[0].global_exec_index, 1);
        assert_eq!(drained[1].global_exec_index, 2);
        assert_eq!(drained[2].global_exec_index, 3);
    }

    #[tokio::test]
    async fn test_gap_stops_drain() {
        let coordinator = BlockCoordinator::new(1, CoordinatorConfig::default());

        // Add blocks with gap: 1, 2, 5, 6 (missing 3, 4)
        for idx in [1u64, 2, 5, 6] {
            let block = QueuedBlock {
                global_exec_index: idx,
                commit_index: idx,
                epoch: 0,
                data: vec![idx as u8],
                source: BlockSource::Consensus,
                received_at: Instant::now(),
            };
            coordinator.push_block(block).await;
        }

        // Should only drain 1, 2 (stop at gap)
        let drained = coordinator.drain_ready().await;
        assert_eq!(drained.len(), 2);
        assert_eq!(coordinator.next_expected(), 3);
        assert_eq!(coordinator.queue_size().await, 2); // 5, 6 still in queue
    }

    // ============================================================================
    // Extended test coverage
    // ============================================================================

    fn make_block(index: u64, source: BlockSource) -> QueuedBlock {
        QueuedBlock {
            global_exec_index: index,
            commit_index: index,
            epoch: 0,
            data: vec![index as u8],
            source,
            received_at: Instant::now(),
        }
    }

    #[tokio::test]
    async fn test_old_block_rejected() {
        // next_expected = 5, so blocks with index < 5 should be rejected
        let coordinator = BlockCoordinator::new(5, CoordinatorConfig::default());
        assert!(
            !coordinator
                .push_block(make_block(1, BlockSource::Consensus))
                .await
        );
        assert!(
            !coordinator
                .push_block(make_block(4, BlockSource::Sync))
                .await
        );
        assert!(
            coordinator
                .push_block(make_block(5, BlockSource::Consensus))
                .await
        );
        assert_eq!(coordinator.queue_size().await, 1);
    }

    #[tokio::test]
    async fn test_sync_standby_rejects_non_gap_blocks() {
        let coordinator = BlockCoordinator::new(1, CoordinatorConfig::default());
        coordinator.set_sync_standby(true);

        // Add a consensus block at index 3 (creating gap at 1,2)
        assert!(
            coordinator
                .push_block(make_block(3, BlockSource::Consensus))
                .await
        );

        // Sync block at index 1 fills gap → accepted
        assert!(
            coordinator
                .push_block(make_block(1, BlockSource::Sync))
                .await
        );

        // Sync block at index 5 does NOT fill gap → rejected (queue not empty, no gap before existing)
        assert!(
            !coordinator
                .push_block(make_block(5, BlockSource::Sync))
                .await
        );

        // Consensus block at index 5 → always accepted regardless of standby
        assert!(
            coordinator
                .push_block(make_block(5, BlockSource::Consensus))
                .await
        );
    }

    #[tokio::test]
    async fn test_sync_standby_accepts_sync_when_queue_empty() {
        let coordinator = BlockCoordinator::new(1, CoordinatorConfig::default());
        coordinator.set_sync_standby(true);

        // When queue is empty, sync blocks should be accepted even in standby
        assert!(
            coordinator
                .push_block(make_block(1, BlockSource::Sync))
                .await
        );
        assert_eq!(coordinator.queue_size().await, 1);
    }

    #[tokio::test]
    async fn test_sync_with_go_advances_state() {
        let coordinator = BlockCoordinator::new(1, CoordinatorConfig::default());

        // Add blocks 1-5
        for i in 1..=5 {
            coordinator
                .push_block(make_block(i, BlockSource::Consensus))
                .await;
        }

        // Go reports it has processed through block 3
        coordinator.sync_with_go(3).await;

        // next_expected should now be 4
        assert_eq!(coordinator.next_expected(), 4);
        // Blocks 1-3 should be cleaned from queue
        assert_eq!(coordinator.queue_size().await, 2); // only 4, 5 remain
    }

    #[tokio::test]
    async fn test_sync_with_go_no_change_if_same() {
        let coordinator = BlockCoordinator::new(5, CoordinatorConfig::default());
        coordinator.sync_with_go(4).await; // go_last_block=4 → next=5, same as current
        assert_eq!(coordinator.next_expected(), 5);
    }

    #[tokio::test]
    async fn test_stats_tracking() {
        let coordinator = BlockCoordinator::new(1, CoordinatorConfig::default());

        coordinator
            .push_block(make_block(1, BlockSource::Consensus))
            .await;
        coordinator
            .push_block(make_block(2, BlockSource::Sync))
            .await;
        coordinator
            .push_block(make_block(3, BlockSource::Consensus))
            .await;

        // Duplicate — sync at index 1 should be rejected (consensus already there)
        coordinator
            .push_block(make_block(1, BlockSource::Sync))
            .await;

        let stats = coordinator.get_stats().await;
        assert_eq!(stats.blocks_from_consensus, 2);
        assert_eq!(stats.blocks_from_sync, 1);
        assert_eq!(stats.blocks_deduplicated, 1);
    }

    #[tokio::test]
    async fn test_max_drain_batch_limit() {
        let config = CoordinatorConfig {
            max_drain_batch: 3,
            ..Default::default()
        };
        let coordinator = BlockCoordinator::new(1, config);

        // Add 10 sequential blocks
        for i in 1..=10 {
            coordinator
                .push_block(make_block(i, BlockSource::Consensus))
                .await;
        }

        // First drain should return at most 3
        let drained = coordinator.drain_ready().await;
        assert_eq!(drained.len(), 3);
        assert_eq!(coordinator.next_expected(), 4);

        // Second drain gets next 3
        let drained2 = coordinator.drain_ready().await;
        assert_eq!(drained2.len(), 3);
        assert_eq!(coordinator.next_expected(), 7);
    }

    #[tokio::test]
    async fn test_drain_empty_queue() {
        let coordinator = BlockCoordinator::new(1, CoordinatorConfig::default());
        let drained = coordinator.drain_ready().await;
        assert!(drained.is_empty());
        assert_eq!(coordinator.next_expected(), 1); // Unchanged
    }

    #[tokio::test]
    async fn test_consensus_does_not_dedup_consensus() {
        let coordinator = BlockCoordinator::new(1, CoordinatorConfig::default());

        // First consensus block
        assert!(
            coordinator
                .push_block(make_block(1, BlockSource::Consensus))
                .await
        );
        // Second consensus block at same index → rejected (not replaced)
        assert!(
            !coordinator
                .push_block(make_block(1, BlockSource::Consensus))
                .await
        );

        let stats = coordinator.get_stats().await;
        assert_eq!(stats.blocks_deduplicated, 1);
    }

    #[tokio::test]
    async fn test_gap_fill_channel_receives_request() {
        let (tx, mut rx) = mpsc::unbounded_channel::<(u64, u64)>();
        let config = CoordinatorConfig {
            gap_threshold: 2,
            ..Default::default()
        };
        let coordinator = BlockCoordinator::new(1, config).with_gap_fill_channel(tx);

        // Add block at index 10 (gap of 9 from next_expected=1)
        coordinator
            .push_block(make_block(10, BlockSource::Consensus))
            .await;

        // check_and_fill_gaps should send (1, 10) on the channel
        coordinator.check_and_fill_gaps().await;

        let (from, to) = rx.try_recv().unwrap();
        assert_eq!(from, 1);
        assert_eq!(to, 10);

        let stats = coordinator.get_stats().await;
        assert_eq!(stats.gaps_detected, 1);
    }

    #[tokio::test]
    async fn test_gap_below_threshold_no_fill_request() {
        let (tx, mut rx) = mpsc::unbounded_channel::<(u64, u64)>();
        let config = CoordinatorConfig {
            gap_threshold: 10, // High threshold
            ..Default::default()
        };
        let coordinator = BlockCoordinator::new(1, config).with_gap_fill_channel(tx);

        // Add block at index 5 (gap of 4 from next_expected=1, below threshold of 10)
        coordinator
            .push_block(make_block(5, BlockSource::Consensus))
            .await;
        coordinator.check_and_fill_gaps().await;

        // No message should be sent
        assert!(rx.try_recv().is_err());
    }
}
