// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

//! Consensus infrastructure setup for Validator and SyncOnly modes.

use crate::config::NodeConfig;
use crate::node::executor_client::ExecutorClient;
use crate::node::ConsensusNode;
use anyhow::Result;
use consensus_core::{
    CommitConsumerArgs, ConsensusAuthority, NetworkType, SystemTransactionProvider,
};
use prometheus::Registry;
use std::sync::Arc;
use tracing::{info, warn};

/// Setup Validator consensus infrastructure for a new epoch.
/// Creates CommitProcessor, starts ConsensusAuthority, and updates transaction proxy.
pub(super) async fn setup_validator_consensus(
    node: &mut ConsensusNode,
    new_epoch: u64,
    epoch_boundary_block: u64,
    epoch_timestamp: u64,
    db_path: std::path::PathBuf,
    committee: consensus_config::Committee,
    config: &NodeConfig,
) -> Result<()> {
    let (commit_consumer, commit_receiver, mut block_receiver) = CommitConsumerArgs::new(0, 0);
    let epoch_cb = crate::consensus::commit_callbacks::create_epoch_transition_callback(
        node.epoch_transition_sender.clone(),
    );

    // Use node.last_global_exec_index for epoch_base — this is the correct value for NORMAL startup.
    // For epoch 1 genesis: last_global_exec_index = 0, so GEI = 0 + commit_index = commit_index.
    // NOTE: Do NOT use epoch_boundary_block here — Go returns boundary_block=1 for epoch 1,
    // which causes GEI = 1 + commit_index (off by one → fork).
    // Cold-start restore uses mode_transition.rs which has its own fix.
    let actual_epoch_base = node.last_global_exec_index;
    let initial_next_expected = actual_epoch_base + 1;
    info!(
        "📊 [EXECUTOR INIT] epoch_boundary_block={}, node.last_global_exec_index={}, initial_next_expected={}",
        epoch_boundary_block, actual_epoch_base, initial_next_expected
    );
    let exec_client_proc = if node.executor_commit_enabled {
        Some(Arc::new(ExecutorClient::new_with_initial_index(
            true,
            true,
            config.executor_send_socket_path.clone(),
            config.executor_receive_socket_path.clone(),
            initial_next_expected,
            Some(node.storage_path.clone()),
        )))
    } else {
        None
    };

    // Initialize BlockCoordinator for dual-stream block production
    // Use same initial_next_expected as executor client for consistency
    let coordinator = Arc::new(crate::node::block_coordinator::BlockCoordinator::new(
        initial_next_expected,
        crate::node::block_coordinator::CoordinatorConfig::default(),
    ));
    node.block_coordinator = Some(coordinator.clone());
    info!(
        "📦 [COORDINATOR] BlockCoordinator initialized for epoch {} (next_expected={})",
        new_epoch, initial_next_expected
    );

    let mut processor = crate::consensus::commit_processor::CommitProcessor::new(commit_receiver)
        .with_commit_index_callback(
            crate::consensus::commit_callbacks::create_commit_index_callback(
                node.current_commit_index.clone(),
            ),
        )
        .with_global_exec_index_callback(
            crate::consensus::commit_callbacks::create_global_exec_index_callback(
                node.shared_last_global_exec_index.clone(),
            ),
        )
        .with_shared_last_global_exec_index(node.shared_last_global_exec_index.clone())
        .with_epoch_info(new_epoch, actual_epoch_base)
        .with_is_transitioning(node.is_transitioning.clone())
        .with_pending_transactions_queue(node.pending_transactions_queue.clone())
        .with_epoch_transition_callback(epoch_cb)
        .with_block_coordinator(coordinator.clone());

    processor = processor.with_epoch_eth_addresses(node.epoch_eth_addresses.clone());

    if let Some(c) = exec_client_proc {
        processor = processor.with_executor_client(c);
    }

    tokio::spawn(async move {
        let _ = processor.run().await;
    });
    tokio::spawn(async move { while block_receiver.recv().await.is_some() {} });

    // Start Authority
    let mut params = node.parameters.clone();
    params.db_path = db_path;
    node.boot_counter += 1;

    node.authority = Some(
        ConsensusAuthority::start(
            NetworkType::Tonic,
            epoch_timestamp,
            epoch_boundary_block,
            node.own_index,
            committee,
            params,
            node.protocol_config.clone(),
            node.protocol_keypair.clone(),
            node.network_keypair.clone(),
            node.clock.clone(),
            node.transaction_verifier.clone(),
            commit_consumer,
            Registry::new(),
            node.boot_counter,
            Some(node.system_transaction_provider.clone() as Arc<dyn SystemTransactionProvider>),
            Some(node.legacy_store_manager.clone()),
        )
        .await,
    );

    // Update proxy for Validator mode
    if let Some(auth) = &node.authority {
        if let Some(proxy) = &node.transaction_client_proxy {
            proxy.set_client(auth.transaction_client()).await;
        } else {
            node.transaction_client_proxy = Some(Arc::new(
                crate::node::tx_submitter::TransactionClientProxy::new(auth.transaction_client()),
            ));
        }
    }

    Ok(())
}

/// Setup SyncOnly sync infrastructure for a new epoch.
/// Creates CommitProcessor (for EndOfEpoch detection), stops old sync task,
/// and starts new Rust P2P sync task with fresh committee.
pub(super) async fn setup_synconly_sync(
    node: &mut ConsensusNode,
    new_epoch: u64,
    epoch_boundary_block: u64,
    epoch_timestamp: u64,
    committee: consensus_config::Committee,
    config: &NodeConfig,
) -> Result<()> {
    info!("🔄 [EPOCH TRANSITION] SyncOnly mode - setting up CommitProcessor for epoch detection");

    let (_commit_consumer, commit_receiver, mut block_receiver) = CommitConsumerArgs::new(0, 0);
    let epoch_cb = crate::consensus::commit_callbacks::create_epoch_transition_callback(
        node.epoch_transition_sender.clone(),
    );

    // Use node.last_global_exec_index for epoch_base — same as Validator path.
    // Do NOT use epoch_boundary_block here (Go returns 1 for epoch 1, not 0).
    let actual_epoch_base = node.last_global_exec_index;
    let initial_next_expected = actual_epoch_base + 1;
    info!(
        "📊 [EXECUTOR INIT] SyncOnly: epoch_boundary_block={}, node.last_global_exec_index={}, initial_next_expected={}",
        epoch_boundary_block, actual_epoch_base, initial_next_expected
    );
    let exec_client_proc = if node.executor_commit_enabled {
        Some(Arc::new(ExecutorClient::new_with_initial_index(
            true,
            true,
            config.executor_send_socket_path.clone(),
            config.executor_receive_socket_path.clone(),
            initial_next_expected,
            Some(node.storage_path.clone()),
        )))
    } else {
        None
    };

    // Initialize BlockCoordinator - use same initial_next_expected for consistency
    let coordinator = Arc::new(crate::node::block_coordinator::BlockCoordinator::new(
        initial_next_expected,
        crate::node::block_coordinator::CoordinatorConfig::default(),
    ));
    node.block_coordinator = Some(coordinator.clone());
    info!(
        "📦 [COORDINATOR] BlockCoordinator initialized for SyncOnly epoch {} (next_expected={})",
        new_epoch, initial_next_expected
    );

    let mut processor = crate::consensus::commit_processor::CommitProcessor::new(commit_receiver)
        .with_commit_index_callback(
            crate::consensus::commit_callbacks::create_commit_index_callback(
                node.current_commit_index.clone(),
            ),
        )
        .with_global_exec_index_callback(
            crate::consensus::commit_callbacks::create_global_exec_index_callback(
                node.shared_last_global_exec_index.clone(),
            ),
        )
        .with_shared_last_global_exec_index(node.shared_last_global_exec_index.clone())
        .with_epoch_info(new_epoch, actual_epoch_base)
        .with_is_transitioning(node.is_transitioning.clone())
        .with_pending_transactions_queue(node.pending_transactions_queue.clone())
        .with_epoch_transition_callback(epoch_cb)
        .with_block_coordinator(coordinator.clone());

    processor = processor.with_epoch_eth_addresses(node.epoch_eth_addresses.clone());

    if let Some(c) = exec_client_proc {
        processor = processor.with_executor_client(c);
    }

    tokio::spawn(async move {
        let _ = processor.run().await;
    });
    tokio::spawn(async move { while block_receiver.recv().await.is_some() {} });

    // Clear authority and proxy (SyncOnly doesn't run consensus)
    node.authority = None;
    node.transaction_client_proxy = None;

    // CRITICAL FIX: Stop old sync task FIRST to prevent stale committee from blocking fetch
    if node.sync_task_handle.is_some() {
        info!("🛑 [SYNC ONLY] Stopping old sync task before starting new one with fresh committee");
        if let Err(e) = crate::node::sync::stop_sync_task(node).await {
            warn!(
                "⚠️ [SYNC ONLY] Failed to stop old sync task: {}. Continuing anyway.",
                e
            );
        }
    }

    info!(
        "🔄 [SYNC ONLY] Starting Rust P2P sync task with fresh committee for epoch {}",
        new_epoch
    );

    // CRITICAL FIX: SyncOnly nodes need can_commit=true to send synced blocks to local Go
    let rust_sync_executor = Arc::new(ExecutorClient::new(
        true,
        true,
        config.executor_send_socket_path.clone(),
        config.executor_receive_socket_path.clone(),
        None,
    ));

    // CRITICAL: Initialize ExecutorClient from Go's current state SYNCHRONOUSLY
    rust_sync_executor.initialize_from_go().await;

    // Create Context for P2P networking (SyncOnly uses dummy own_index)
    let sync_metrics = consensus_core::initialise_metrics(Registry::new());
    let sync_context = std::sync::Arc::new(consensus_core::Context::new(
        epoch_timestamp,
        consensus_config::AuthorityIndex::new_for_test(0),
        committee.clone(),
        node.parameters.clone(),
        node.protocol_config.clone(),
        sync_metrics,
        node.clock.clone(),
    ));

    match crate::node::rust_sync_node::start_rust_sync_task_with_network(
        rust_sync_executor,
        node.epoch_transition_sender.clone(),
        new_epoch,
        0,
        sync_context,
        node.network_keypair.clone(),
        committee.clone(),
        config.peer_rpc_addresses.clone(),
    )
    .await
    {
        Ok(handle) => {
            if let Err(e) = node.sync_controller.enable_sync(handle).await {
                warn!("⚠️ [SYNC ONLY] SyncController enable failed: {}", e);
            } else {
                info!("✅ [SYNC ONLY] Rust P2P sync started via SyncController");
            }
        }
        Err(e) => {
            warn!("⚠️ [SYNC ONLY] Failed to start Rust P2P sync: {}", e);
        }
    }

    Ok(())
}
