// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

//! SyncOnly Block Sync - Rust-Centric Epoch Monitoring
//!
//! Architecture:
//! - Go syncs blocks via network P2P (in network_sync.go)
//! - RustSyncNode handles P2P block sync with full networking
//! - When epoch changes detected, triggers epoch transition via channel
//! - CommitProcessor handles transition callback

use crate::config::NodeConfig;
use crate::node::{ConsensusNode, NodeMode};
use anyhow::Result;
use tracing::{info, warn};

// =======================
// Legacy API (for compatibility with transition.rs)
// =======================

/// Start the sync task for SyncOnly nodes (using RustSyncNode for P2P sync)
pub async fn start_sync_task(node: &mut ConsensusNode, _config: &NodeConfig) -> Result<()> {
    if !matches!(node.node_mode, NodeMode::SyncOnly) || node.sync_task_handle.is_some() {
        return Ok(());
    }

    info!("🚀 [SYNC TASK] Starting RustSyncNode P2P sync for SyncOnly mode");

    // CRITICAL FIX: Reuse the initialized executor_client from ConsensusNode
    // This client has already run initialize_from_go() and has the correct next_expected_index
    // Creating a new one would reset next_expected_index to 1, causing buffering issues
    let executor_client = node
        .executor_client
        .clone()
        .expect("Executor client must be initialized in ConsensusNode");

    // Load committee from Go - with PEER FALLBACK for slow/late starting nodes
    // If Go layer doesn't have epoch data yet (e.g., node started late or syncing slowly),
    // we fetch from peers to ensure the sync task can start
    let (epoch, epoch_timestamp, _boundary_block, validators, _, _boundary_gei) = match executor_client
        .get_epoch_boundary_data(node.current_epoch)
        .await
    {
        Ok(data) => {
            info!(
                "✅ [SYNC TASK] Got epoch {} boundary data from local Go",
                node.current_epoch
            );
            data
        }
        Err(e) => {
            warn!(
                "⚠️ [SYNC TASK] Local Go doesn't have epoch {} data: {}. Trying peers...",
                node.current_epoch, e
            );

            // Fallback: Try to get epoch boundary data from peers
            let peer_addresses = &_config.peer_rpc_addresses;
            if peer_addresses.is_empty() {
                return Err(anyhow::anyhow!(
                    "No local epoch data and no peer addresses configured for fallback"
                ));
            }

            let mut last_error = e;
            let mut found_data = None;

            for peer_addr in peer_addresses {
                info!(
                    "🌐 [SYNC TASK] Trying peer {} for epoch {} boundary data...",
                    peer_addr, node.current_epoch
                );
                match crate::network::peer_rpc::query_peer_epoch_boundary_data(
                    peer_addr,
                    node.current_epoch,
                )
                .await
                {
                    Ok(response) => {
                        if response.error.is_none() && !response.validators.is_empty() {
                            info!(
                                "✅ [SYNC TASK] Got epoch {} data from peer {}: boundary_block={}, validators={}, boundary_gei={}",
                                response.epoch, peer_addr, response.boundary_block, response.validators.len(), response.boundary_gei
                            );

                            // Convert ValidatorInfoSimple to ValidatorInfo
                            let converted_validators: Vec<
                                crate::node::executor_client::proto::ValidatorInfo,
                            > = response
                                .validators
                                .into_iter()
                                .map(|v| crate::node::executor_client::proto::ValidatorInfo {
                                    address: v.address,
                                    stake: v.stake.to_string(),
                                    authority_key: v.authority_key,
                                    protocol_key: v.protocol_key,
                                    network_key: v.network_key,
                                    name: v.name,
                                    description: String::new(),
                                    website: String::new(),
                                    image: String::new(),
                                    commission_rate: 0,
                                    min_self_delegation: String::new(),
                                    accumulated_rewards_per_share: String::new(),
                                    p2p_address: String::new(),
                                })
                                .collect();

                            found_data = Some((
                                response.epoch,
                                response.timestamp_ms,
                                response.boundary_block,
                                converted_validators,
                                900u64, // default epoch_duration_seconds for peer fallback
                                response.boundary_gei,
                            ));
                            break;
                        } else {
                            warn!(
                                "⚠️ [SYNC TASK] Peer {} returned error: {:?}",
                                peer_addr, response.error
                            );
                        }
                    }
                    Err(peer_err) => {
                        warn!(
                            "⚠️ [SYNC TASK] Failed to query peer {}: {}",
                            peer_addr, peer_err
                        );
                        last_error = peer_err;
                    }
                }
            }

            match found_data {
                Some(data) => data,
                None => {
                    return Err(anyhow::anyhow!(
                        "Failed to get epoch {} boundary data from Go or any peer: {}",
                        node.current_epoch,
                        last_error
                    ));
                }
            }
        }
    };

    // Build committee from validators
    let committee = crate::node::committee::build_committee_from_validator_list(validators, epoch)?;

    info!(
        "🌐 [SYNC TASK] Loaded committee for epoch {} with {} validators",
        epoch,
        committee.size()
    );

    // Create Context for P2P networking
    let sync_metrics = consensus_core::initialise_metrics(prometheus::Registry::new());
    let sync_context = std::sync::Arc::new(consensus_core::Context::new(
        epoch_timestamp,
        consensus_config::AuthorityIndex::new_for_test(0), // SyncOnly uses dummy index
        committee.clone(),
        node.parameters.clone(),
        node.protocol_config.clone(),
        sync_metrics,
        node.clock.clone(),
    ));

    // Start RustSyncNode with full networking
    match crate::node::rust_sync_node::start_rust_sync_task_with_network(
        executor_client,
        node.epoch_transition_sender.clone(),
        epoch,
        0, // initial_commit_index
        sync_context,
        node.network_keypair.clone(),
        committee,
        _config.peer_rpc_addresses.clone(), // For epoch boundary data fallback
    )
    .await
    {
        Ok(handle) => {
            node.sync_task_handle = Some(handle);
            info!("✅ [SYNC TASK] RustSyncNode P2P sync started successfully");
        }
        Err(e) => {
            warn!("⚠️ [SYNC TASK] Failed to start RustSyncNode: {}", e);
            return Err(e);
        }
    }

    Ok(())
}

/// Stop the sync task (legacy API)
pub async fn stop_sync_task(node: &mut ConsensusNode) -> Result<()> {
    if let Some(handle) = node.sync_task_handle.take() {
        info!("🛑 [SYNC TASK] Stopping...");
        // CRITICAL FIX: Call stop() which sends shutdown signal before aborting
        // Previously we only called abort() which didn't stop internal loops
        handle.stop().await;
        info!("✅ [SYNC TASK] Stopped successfully");
    }
    Ok(())
}
