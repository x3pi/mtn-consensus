// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

//! Node module — top-level orchestration for MetaNode consensus participants.
//!
//! Submodules:
//! - `consensus_node` — ConsensusNode constructors
//! - `node_methods` — ConsensusNode methods (transactions, mode switches, sync, shutdown)
//! - `epoch_store` — free functions for local epoch detection and legacy store loading

use crate::consensus::clock_sync::ClockSyncManager;
use crate::types::transaction::NoopTransactionVerifier;
use consensus_config::AuthorityIndex;
use consensus_core::{
    Clock, CommitConsumerArgs, ConsensusAuthority, DefaultSystemTransactionProvider,
};
use meta_protocol_config::ProtocolConfig;
use std::sync::atomic::{AtomicBool, AtomicU32};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

use crate::node::executor_client::ExecutorClient;
use crate::node::tx_submitter::TransactionClientProxy;

// Declare submodules
pub mod block_coordinator;
pub mod catchup;
pub mod committee;
pub mod dual_stream;
pub mod committee_source;
pub mod epoch_checkpoint;
pub mod epoch_monitor;
pub mod epoch_transition_manager;
pub mod executor_client;
pub mod notification_listener;
pub mod notification_server;
pub mod peer_go_client;
pub mod peer_health;
pub mod queue;
pub mod recovery;
pub mod rpc_circuit_breaker;
pub mod rust_sync_node;
pub mod startup;
pub mod sync;
pub mod sync_controller;
pub mod sync_metrics;
pub mod transition;
pub mod tx_submitter;

// Extracted submodules
mod consensus_node;
mod epoch_store;
mod node_methods;

// Re-export from epoch_store for use in consensus_node
use epoch_store::detect_local_epoch;
#[allow(unused_imports)]
use epoch_store::load_legacy_epoch_stores;

#[cfg(test)]
mod epoch_transition_tests;

/// Node operation modes
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum NodeMode {
    /// Node only syncs data, does not participate in voting
    SyncOnly,
    /// Node participates in consensus and voting
    Validator,
    /// Node is catching up with the network (syncing epoch/commits)
    SyncingUp,
}

/// Pending epoch transition that is deferred until sync is complete
/// Used by SyncOnly nodes to ensure they don't advance epoch before syncing all blocks
#[derive(Clone, Debug)]
pub struct PendingEpochTransition {
    pub epoch: u64,
    pub timestamp_ms: u64,
    pub boundary_block: u64,
    pub boundary_gei: u64,
}

// Global registry for transition handler to access node
static TRANSITION_HANDLER_REGISTRY: tokio::sync::OnceCell<
    Arc<tokio::sync::Mutex<Option<Arc<tokio::sync::Mutex<ConsensusNode>>>>>,
> = tokio::sync::OnceCell::const_new();

pub async fn get_transition_handler_node() -> Option<Arc<tokio::sync::Mutex<ConsensusNode>>> {
    if let Some(registry) = TRANSITION_HANDLER_REGISTRY.get() {
        let registry_guard = registry.lock().await;
        registry_guard.clone()
    } else {
        None
    }
}

pub async fn set_transition_handler_node(node: Arc<tokio::sync::Mutex<ConsensusNode>>) {
    if TRANSITION_HANDLER_REGISTRY.get().is_none() {
        let _ = TRANSITION_HANDLER_REGISTRY.set(Arc::new(tokio::sync::Mutex::new(None)));
    }

    if let Some(registry) = TRANSITION_HANDLER_REGISTRY.get() {
        let mut registry_guard = registry.lock().await;
        *registry_guard = Some(node);
        drop(registry_guard);
        info!("✅ Registered node in global transition handler registry");
    }
}

pub struct ConsensusNode {
    // Made fields pub(crate) so submodules can access them
    pub(crate) authority: Option<ConsensusAuthority>,
    /// Legacy epoch store manager - keeps RocksDB stores from previous epochs
    /// open for read-only sync access. Only stores (not full authorities) are kept.
    pub(crate) legacy_store_manager: Arc<consensus_core::LegacyEpochStoreManager>,
    pub(crate) node_mode: NodeMode,
    pub(crate) execution_lock: Arc<tokio::sync::RwLock<u64>>,
    pub(crate) reconfig_state: Arc<tokio::sync::RwLock<consensus_core::ReconfigState>>,
    pub(crate) transaction_client_proxy: Option<Arc<TransactionClientProxy>>,
    #[allow(dead_code)]
    pub(crate) clock_sync_manager: Arc<RwLock<ClockSyncManager>>,
    pub(crate) current_commit_index: Arc<AtomicU32>,

    pub(crate) storage_path: std::path::PathBuf,
    pub(crate) current_epoch: u64,
    pub(crate) last_global_exec_index: u64,
    pub(crate) shared_last_global_exec_index: Arc<tokio::sync::Mutex<u64>>,

    pub(crate) protocol_keypair: consensus_config::ProtocolKeyPair,
    pub(crate) network_keypair: consensus_config::NetworkKeyPair,
    pub(crate) protocol_config: ProtocolConfig,
    pub(crate) clock: Arc<Clock>,
    pub(crate) transaction_verifier: Arc<NoopTransactionVerifier>,
    pub(crate) parameters: consensus_config::Parameters,
    pub(crate) own_index: AuthorityIndex,
    pub(crate) boot_counter: u64,
    pub(crate) last_transition_hash: Option<Vec<u8>>,
    #[allow(dead_code)]
    pub(crate) current_registry_id: Option<mysten_metrics::RegistryID>,
    pub(crate) executor_commit_enabled: bool,
    pub(crate) is_transitioning: Arc<AtomicBool>,
    pub(crate) pending_transactions_queue: Arc<tokio::sync::Mutex<Vec<Vec<u8>>>>,
    pub(crate) system_transaction_provider: Arc<DefaultSystemTransactionProvider>,
    pub(crate) epoch_transition_sender: tokio::sync::mpsc::UnboundedSender<(u64, u64, u64)>,

    // Handles for background tasks
    pub(crate) sync_task_handle: Option<crate::node::rust_sync_node::RustSyncHandle>,
    /// Centralized controller for sync task lifecycle
    pub(crate) sync_controller: Arc<crate::node::sync_controller::SyncController>,
    pub(crate) epoch_monitor_handle: Option<tokio::task::JoinHandle<()>>,
    pub(crate) notification_server_handle: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
    pub(crate) executor_client: Option<Arc<ExecutorClient>>,
    /// Transactions submitted in current epoch that may need recovery during epoch transition
    pub(crate) epoch_pending_transactions: Arc<tokio::sync::Mutex<Vec<Vec<u8>>>>,
    /// Transaction hashes that have been committed in current epoch (for duplicate prevention)
    pub(crate) committed_transaction_hashes:
        Arc<tokio::sync::Mutex<std::collections::HashSet<Vec<u8>>>>,

    /// Queued epoch transitions waiting for sync to complete (for SyncOnly nodes)
    /// When a SyncOnly node receives AdvanceEpoch but hasn't synced to the boundary yet,
    /// the transition is queued here and processed after sync catches up
    pub(crate) pending_epoch_transitions: Arc<tokio::sync::Mutex<Vec<PendingEpochTransition>>>,

    /// Holds commit_consumer to prevent channel close for SyncOnly nodes
    /// When authority is None (SyncOnly), commit_consumer would be dropped causing
    /// commit_receiver to close immediately. This field keeps it alive.
    #[allow(dead_code)]
    pub(crate) _commit_consumer_holder: Option<CommitConsumerArgs>,

    /// Multi-epoch committee cache: ETH addresses keyed by epoch
    /// Keeps last 3 epochs to support lookups during epoch transitions
    /// Updated when committee is loaded, used by CommitProcessor to send leader_address to Go
    pub(crate) epoch_eth_addresses:
        Arc<tokio::sync::Mutex<std::collections::HashMap<u64, Vec<Vec<u8>>>>>,

    /// Block Coordinator for dual-stream block production
    /// Handles both Consensus and Sync streams with deduplication and priority
    pub(crate) block_coordinator: Option<Arc<block_coordinator::BlockCoordinator>>,

    /// Peer RPC addresses for cross-node block fetching during epoch transitions
    pub(crate) peer_rpc_addresses: Vec<String>,

    /// TX recycler for tracking and re-submitting stale TXs
    pub(crate) tx_recycler: Option<Arc<crate::consensus::tx_recycler::TxRecycler>>,

    /// Cold-start flag: set to true when DAG was wiped (snapshot restore).
    /// When true, the node stays in SyncingUp mode and runs blocking Phase 1
    /// peer sync first. After sync completes, ConsensusAuthority starts and
    /// DualStreamController handles the overlap period (Phase 4).
    pub(crate) cold_start: bool,
}

// ConsensusNode constructors are in consensus_node.rs
// ConsensusNode methods are in node_methods.rs
// Free functions (detect_local_epoch, load_legacy_epoch_stores) are in epoch_store.rs
