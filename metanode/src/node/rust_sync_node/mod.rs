// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

//! RustSyncNode - Full Rust P2P Block Sync for SyncOnly Mode
//!
//! Architecture:
//! - Rust fetches commits from validator peers via consensus_core NetworkClient
//! - Rust sends blocks to CommitProcessor for EndOfEpoch detection
//! - Rust sends blocks to Go via ExecutorClient (Go only executes)
//!
//! This replaces Go's network_sync.go for SyncOnly nodes.

mod block_queue;
mod epoch_recovery;
mod fetch;
mod start;
mod sync_loop;

// Re-export public API
pub use start::start_rust_sync_task_with_network;

#[cfg(test)]
mod epoch_recovery_tests;
#[cfg(test)]
mod fetch_tests;
#[cfg(test)]
mod sync_loop_tests;

use crate::node::executor_client::ExecutorClient;
use crate::node::peer_health::PeerHealthTracker;
use crate::node::sync_metrics::SyncMetrics;
use block_queue::BlockQueue;
use consensus_config::{Committee, NetworkKeyPair};
use consensus_core::{Context, TonicClient};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::info;

/// Handle to control the RustSyncNode
pub struct RustSyncHandle {
    shutdown_tx: Option<oneshot::Sender<()>>,
    pub task_handle: tokio::task::JoinHandle<()>,
}

impl RustSyncHandle {
    /// Create a new RustSyncHandle
    pub fn new(shutdown_tx: oneshot::Sender<()>, task_handle: tokio::task::JoinHandle<()>) -> Self {
        Self {
            shutdown_tx: Some(shutdown_tx),
            task_handle,
        }
    }

    /// Stop the RustSyncNode gracefully
    pub async fn stop(mut self) {
        info!("🛑 [RUST-SYNC] Stopping...");
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        self.task_handle.abort();
        let _ = tokio::time::timeout(Duration::from_secs(5), self.task_handle).await;
        info!("✅ [RUST-SYNC] Stopped");
    }
}

/// Configuration for RustSyncNode
pub struct RustSyncConfig {
    pub fetch_interval_secs: u64,
    pub turbo_fetch_interval_ms: u64, // Faster interval when catching up
    pub fetch_batch_size: u32,
    pub turbo_batch_size: u32, // Larger batch when catching up
    #[allow(dead_code)]
    pub fetch_timeout_secs: u64,
    /// Peer RPC addresses for fallback epoch boundary data fetch
    pub peer_rpc_addresses: Vec<String>,
}

impl Default for RustSyncConfig {
    fn default() -> Self {
        Self {
            fetch_interval_secs: 2,
            turbo_fetch_interval_ms: 50, // OPTIMIZED: 50ms (was 200ms) for fast catchup
            fetch_batch_size: 500,       // OPTIMIZED: 500 (was 100) blocks per fetch
            turbo_batch_size: 2000,      // OPTIMIZED: 2000 (was 500) for aggressive catchup
            fetch_timeout_secs: 30,      // OPTIMIZED: 30s (was 10s) for larger batches
            peer_rpc_addresses: vec![],  // Configure for WAN sync
        }
    }
}

/// RustSyncNode - Syncs blocks from peers via P2P and sends to Go
pub struct RustSyncNode {
    pub(crate) executor_client: Arc<ExecutorClient>,
    pub(crate) network_client: Option<Arc<TonicClient>>,
    pub(crate) epoch_transition_sender: mpsc::UnboundedSender<(u64, u64, u64)>,
    pub(crate) current_epoch: Arc<AtomicU64>,
    pub(crate) last_synced_commit_index: Arc<AtomicU32>,
    pub(crate) committee: Arc<RwLock<Option<Committee>>>,
    pub(crate) config: RustSyncConfig,
    /// Queue for buffering and sequential block processing
    pub(crate) block_queue: Arc<Mutex<BlockQueue>>,
    /// Base global_exec_index for current epoch (commits in this epoch are indexed from 1)
    pub(crate) epoch_base_index: Arc<AtomicU64>,
    /// Multi-epoch cache: epoch -> sorted list of validator ETH addresses
    #[allow(dead_code)]
    pub(crate) epoch_eth_addresses: Arc<Mutex<HashMap<u64, Vec<Vec<u8>>>>>,
    // Stored to allow rebuilding TonicClient on committee change
    pub(crate) context: Option<Arc<Context>>,
    pub(crate) network_keypair: Option<NetworkKeyPair>,
    /// Prometheus metrics for sync observability
    pub(crate) metrics: SyncMetrics,
    /// Peer health tracker for circuit breaker pattern
    pub(crate) peer_health: Arc<Mutex<PeerHealthTracker>>,
    /// Optional store to persist fetched blocks and commits, bypassing Validator sync
    pub(crate) store: Option<Arc<dyn consensus_core::storage::Store>>,
}

impl RustSyncNode {
    /// Create a new RustSyncNode
    /// epoch_base_index: the global_exec_index of the last block in the previous epoch
    ///                   (boundary block). Commit index 1 in current epoch = epoch_base_index + 1
    pub fn new(
        executor_client: Arc<ExecutorClient>,
        epoch_transition_sender: mpsc::UnboundedSender<(u64, u64, u64)>,
        initial_epoch: u64,
        initial_global_exec_index: u32,
        epoch_base_index: u64,
    ) -> Self {
        info!(
            "📊 [RUST-SYNC] Creating sync node: epoch={}, global_exec_index={}, epoch_base_index={}",
            initial_epoch, initial_global_exec_index, epoch_base_index
        );
        Self {
            executor_client,
            network_client: None,
            epoch_transition_sender,
            current_epoch: Arc::new(AtomicU64::new(initial_epoch)),
            last_synced_commit_index: Arc::new(AtomicU32::new(initial_global_exec_index)),
            committee: Arc::new(RwLock::new(None)),
            config: RustSyncConfig::default(),
            block_queue: Arc::new(Mutex::new(BlockQueue::new(
                initial_global_exec_index as u64,
            ))),
            epoch_base_index: Arc::new(AtomicU64::new(epoch_base_index)),

            epoch_eth_addresses: Arc::new(Mutex::new(HashMap::new())),
            context: None,
            network_keypair: None,
            metrics: SyncMetrics::new_for_test(),
            peer_health: Arc::new(Mutex::new(PeerHealthTracker::new())),
            store: None,
        }
    }

    /// Initialize P2P networking with Context and NetworkKeyPair
    pub fn with_network(
        mut self,
        context: Arc<Context>,
        network_keypair: NetworkKeyPair,
        committee: Committee,
    ) -> Self {
        let tonic_client = TonicClient::new(context.clone(), network_keypair.clone());
        self.network_client = Some(Arc::new(tonic_client));
        *self.committee.write().expect("committee lock poisoned") = Some(committee);
        self.context = Some(context);
        self.network_keypair = Some(network_keypair);
        self
    }

    /// Set peer RPC addresses for fallback epoch boundary data fetch
    pub fn with_peer_rpc_addresses(mut self, addresses: Vec<String>) -> Self {
        self.config.peer_rpc_addresses = addresses;
        self
    }

    /// Set Prometheus metrics (overrides the default test registry)
    pub fn with_metrics(mut self, metrics: SyncMetrics) -> Self {
        self.metrics = metrics;
        self
    }

    /// Set an optional Store to persist fetched blocks and commits
    pub fn with_store(mut self, store: Arc<dyn consensus_core::storage::Store>) -> Self {
        self.store = Some(store);
        self
    }
}
