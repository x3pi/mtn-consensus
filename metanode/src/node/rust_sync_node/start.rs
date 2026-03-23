// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

//! Helper functions for creating and starting RustSyncNode from ConsensusNode.

use super::{RustSyncHandle, RustSyncNode};
use crate::node::executor_client::ExecutorClient;
use crate::node::sync_metrics::SyncMetrics;
use anyhow::Result;
use consensus_config::{Committee, NetworkKeyPair};
use consensus_core::storage::rocksdb_store::RocksDBStore;
use consensus_core::Context;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{info, warn};

/// Start the Rust P2P sync task for SyncOnly nodes
#[allow(dead_code)]
pub async fn start_rust_sync_task(
    executor_client: Arc<ExecutorClient>,
    epoch_transition_sender: mpsc::UnboundedSender<(u64, u64, u64)>,
    initial_epoch: u64,
    initial_commit_index: u32,
    epoch_base_index: u64,
) -> Result<RustSyncHandle> {
    info!("🚀 [RUST-SYNC] Starting Rust P2P sync task");

    let sync_node = RustSyncNode::new(
        executor_client,
        epoch_transition_sender,
        initial_epoch,
        initial_commit_index,
        epoch_base_index,
    );

    Ok(sync_node.start())
}

/// Start the Rust P2P sync task with full networking support
pub async fn start_rust_sync_task_with_network(
    executor_client: Arc<ExecutorClient>,
    epoch_transition_sender: mpsc::UnboundedSender<(u64, u64, u64)>,
    initial_epoch: u64,
    _initial_commit_index: u32, // Deprecated - we now use go_last_block for queue initialization
    context: Arc<Context>,
    network_keypair: NetworkKeyPair,
    committee: Committee,
    peer_rpc_addresses: Vec<String>, // For epoch boundary data fallback
) -> Result<RustSyncHandle> {
    info!("🚀 [RUST-SYNC] Starting Rust P2P sync task with network");

    // CRITICAL FIX: Get Go's last block number to initialize BlockQueue correctly
    // BlockQueue uses global_exec_index, NOT commit_index, so we need go_last_block + 1
    let go_last_block = executor_client.get_last_block_number().await.map(|(b, _)| b).unwrap_or(0);
    let initial_global_exec_index = go_last_block as u32 + 1;

    // CRITICAL FIX: Get epoch boundary data to calculate epoch_base_index
    // This is needed to convert global_exec_index to epoch-local commit_index when fetching
    let epoch_base_index = match executor_client.get_epoch_boundary_data(initial_epoch).await {
        Ok((_epoch, _timestamp_ms, boundary_block, _validators, _, _)) => {
            // boundary_block is the global_exec_index of the last block in the previous epoch
            // For epoch 0, boundary_block is 0
            // For epoch 1, boundary_block is the last block of epoch 0 (e.g., 3633)
            info!(
                "📊 [RUST-SYNC] Got epoch_base_index={} from GetEpochBoundaryData for epoch {}",
                boundary_block, initial_epoch
            );
            boundary_block
        }
        Err(e) => {
            // Fallback: assume epoch 0, base = 0
            warn!(
                "[RUST-SYNC] Failed to get epoch boundary data for epoch {}: {:?}, using base=0",
                initial_epoch, e
            );
            0
        }
    };

    info!(
        "📊 [RUST-SYNC] Initializing BlockQueue with next_expected={} (go_last_block={}), epoch_base_index={}",
        initial_global_exec_index, go_last_block, epoch_base_index
    );

    let metrics = SyncMetrics::new(prometheus::default_registry());

    let store_path = context
        .parameters
        .db_path
        .as_path()
        .to_str()
        .expect("DB path should be valid UTF-8");
    let store = Arc::new(RocksDBStore::new(store_path));

    let sync_node = RustSyncNode::new(
        executor_client,
        epoch_transition_sender,
        initial_epoch,
        initial_global_exec_index, // Use global_exec_index, not commit_index
        epoch_base_index,
    )
    .with_network(context, network_keypair, committee)
    .with_peer_rpc_addresses(peer_rpc_addresses)
    .with_metrics(metrics)
    .with_store(store);

    Ok(sync_node.start())
}
