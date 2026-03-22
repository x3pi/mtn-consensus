// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

//! Validator → SyncOnly demotion and role determination helpers.

use crate::config::NodeConfig;
use crate::node::{ConsensusNode, NodeMode};
use anyhow::Result;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{info, warn};

/// CROSS-EPOCH DEMOTION: Validator → SyncOnly with epoch catch-up
///
/// This handles the case where a Validator node:
/// 1. Gets removed from committee at epoch N
/// 2. Needs to demote to SyncOnly AND catch up to current network epoch
///
/// The function:
/// 1. Gracefully stops the authority
/// 2. Waits for Go to sync up
/// 3. Switches mode to SyncOnly
/// 4. Starts sync task to catch up to network
///
/// This is called by check_and_update_node_mode when transitioning Validator→SyncOnly
/// but the existing logic already handles this well. This function provides additional
/// handling for cases where we need to catch up multiple epochs after demotion.
#[allow(dead_code)]
pub async fn demote_to_synconly_and_catchup(
    node: &mut ConsensusNode,
    target_epoch: u64,
    target_block: u64,
    config: &NodeConfig,
) -> Result<()> {
    // Guard against concurrent transitions
    if node.is_transitioning.swap(true, Ordering::SeqCst) {
        warn!("⚠️ Demotion already in progress, skipping.");
        node.is_transitioning.store(false, Ordering::SeqCst);
        return Ok(());
    }

    info!(
        "🔄 [CROSS-EPOCH DEMOTION] Starting Validator → SyncOnly: current_epoch={}, target_epoch={}, target_block={}",
        node.current_epoch, target_epoch, target_block
    );

    // STEP 1: Stop authority gracefully
    if let Some(auth) = node.authority.take() {
        info!("🛑 [DEMOTION] Stopping consensus authority...");
        auth.stop().await;
        info!("✅ [DEMOTION] Authority stopped successfully");
    }

    // STEP 2: Wait for Go to catch up to our last block
    let expected_last_block = node.last_global_exec_index;
    if let Some(ref executor_client) = node.executor_client {
        info!(
            "⏳ [DEMOTION] Waiting for Go to reach block {}...",
            expected_last_block
        );

        let poll_interval = Duration::from_millis(100);
        let mut attempt = 0u64;
        let max_attempts = 6000; // 10 minutes max

        loop {
            attempt += 1;
            match executor_client.get_last_block_number().await {
                Ok((b, _)) => {
                    if b >= expected_last_block {
                        info!(
                            "✅ [DEMOTION] Go reached block {} (expected: {}) after {} polls",
                            b, expected_last_block, attempt
                        );
                        break;
                    }
                    if attempt % 100 == 0 {
                        info!(
                            "⏳ [DEMOTION] Go: {}/{} blocks (waiting {}s)",
                            b,
                            expected_last_block,
                            attempt / 10
                        );
                    }
                }
                Err(e) => {
                    if attempt % 100 == 0 {
                        warn!("⚠️ [DEMOTION] Cannot reach Go: {}. Retrying...", e);
                    }
                }
            }

            if attempt >= max_attempts {
                warn!(
                    "⚠️ [DEMOTION] Timeout waiting for Go sync after {} attempts. Proceeding anyway.",
                    attempt
                );
                break;
            }

            sleep(poll_interval).await;
        }
    }

    // STEP 3: Update mode to SyncOnly
    node.node_mode = NodeMode::SyncOnly;
    node.current_epoch = target_epoch;
    node.last_global_exec_index = target_block;

    info!(
        "📊 [DEMOTION] Mode updated: SyncOnly, epoch={}, last_global_exec_index={}",
        node.current_epoch, node.last_global_exec_index
    );

    // STEP 4: Notify Go of sync mode
    if let Some(ref executor_client) = node.executor_client {
        let _ = executor_client.set_sync_start_block(target_block).await;
    }

    // STEP 5: Start sync task
    info!("🔄 [DEMOTION] Starting sync task for catch-up...");
    if let Err(e) = crate::node::sync::start_sync_task(node, config).await {
        warn!("⚠️ [DEMOTION] Failed to start sync task: {}", e);
    }

    // STEP 6: Start epoch monitor for future transitions
    if let Ok(Some(handle)) =
        crate::node::epoch_monitor::start_unified_epoch_monitor(&node.executor_client, config)
    {
        info!("🔄 [DEMOTION] Started epoch monitor for future transitions");
        node.epoch_monitor_handle = Some(handle);
    }

    node.is_transitioning.store(false, Ordering::SeqCst);

    info!(
        "✅ [CROSS-EPOCH DEMOTION] Successfully demoted to SyncOnly at epoch {}",
        target_epoch
    );

    Ok(())
}

/// Check if node should be promoted from SyncOnly to Validator
///
/// This is a helper function that can be called after sync catches up
/// to verify if promotion is appropriate.
#[allow(dead_code)]
pub async fn check_promotion_eligibility(
    node: &ConsensusNode,
    config: &NodeConfig,
) -> Result<Option<u64>> {
    // Only SyncOnly nodes can be promoted
    if !matches!(node.node_mode, NodeMode::SyncOnly) {
        return Ok(None);
    }

    // Fetch current network epoch and committee
    let committee_source = crate::node::committee_source::CommitteeSource::discover(config).await?;
    let network_epoch = committee_source.epoch;

    // Check if we're at the network epoch
    if node.current_epoch < network_epoch {
        info!(
            "📊 [PROMOTION CHECK] Node epoch {} < network epoch {}. Need to catch up first.",
            node.current_epoch, network_epoch
        );
        return Ok(None);
    }

    // Fetch committee for current epoch
    let committee = committee_source
        .fetch_committee(&config.executor_send_socket_path, network_epoch)
        .await?;

    // Check if we're in the committee
    let own_protocol_pubkey = node.protocol_keypair.public();
    let in_committee = committee
        .authorities()
        .any(|(_, authority)| authority.protocol_key == own_protocol_pubkey);

    if in_committee {
        info!(
            "✅ [PROMOTION CHECK] Node IS in committee for epoch {}. Eligible for promotion!",
            network_epoch
        );
        Ok(Some(network_epoch))
    } else {
        info!(
            "ℹ️ [PROMOTION CHECK] Node NOT in committee for epoch {}. Staying SyncOnly.",
            network_epoch
        );
        Ok(None)
    }
}

// =============================================================================
// ROLE-FIRST DESIGN: Determine node role BEFORE any epoch operations
// =============================================================================

/// Determine the role (Validator or SyncOnly) for a specific epoch.
///
/// # CRITICAL DESIGN PRINCIPLE
/// This function MUST be called FIRST before any epoch-related operations.
/// The node's role determines what infrastructure to start and how to behave.
///
/// # Arguments
/// * `epoch` - The epoch to determine role for
/// * `own_protocol_pubkey` - This node's protocol public key
/// * `config` - Node configuration
///
/// # Returns
/// * `Ok(NodeMode)` - The determined role (Validator or SyncOnly)
/// * `Err` - If committee cannot be fetched
pub async fn determine_role_for_epoch(
    epoch: u64,
    own_protocol_pubkey: &consensus_config::ProtocolPublicKey,
    config: &NodeConfig,
) -> Result<NodeMode> {
    info!("🔍 [ROLE-FIRST] Determining role for epoch {}...", epoch);

    // Step 1: Discover committee source (local Go or peer)
    let committee_source = match crate::node::committee_source::CommitteeSource::discover(config)
        .await
    {
        Ok(source) => source,
        Err(e) => {
            warn!("⚠️ [ROLE-FIRST] Cannot discover committee source for epoch {}: {}. Defaulting to SyncOnly.", epoch, e);
            return Ok(NodeMode::SyncOnly);
        }
    };

    // Step 2: Fetch committee for this epoch
    let committee = match committee_source
        .fetch_committee(&config.executor_send_socket_path, epoch)
        .await
    {
        Ok(c) => c,
        Err(e) => {
            warn!(
                "⚠️ [ROLE-FIRST] Cannot fetch committee for epoch {}: {}. Defaulting to SyncOnly.",
                epoch, e
            );
            return Ok(NodeMode::SyncOnly);
        }
    };

    // Step 3: Check if we're in the committee
    let is_in_committee = committee
        .authorities()
        .any(|(_, authority)| authority.protocol_key == *own_protocol_pubkey);

    // Step 4: Determine and log role
    let role = if is_in_committee {
        info!(
            "✅ [ROLE-FIRST] Epoch {}: Node IS in committee ({} validators) → VALIDATOR",
            epoch,
            committee.size()
        );
        NodeMode::Validator
    } else {
        info!(
            "ℹ️ [ROLE-FIRST] Epoch {}: Node NOT in committee ({} validators) → SYNC_ONLY",
            epoch,
            committee.size()
        );
        NodeMode::SyncOnly
    };

    Ok(role)
}

/// Determine role and handle mode transition if needed.
/// This is a convenience wrapper that:
/// 1. Determines role for epoch
/// 2. Compares with current mode
/// 3. Logs any mode change needed
///
/// Returns (target_role, needs_mode_change)
pub async fn determine_role_and_check_transition(
    epoch: u64,
    current_mode: &NodeMode,
    own_protocol_pubkey: &consensus_config::ProtocolPublicKey,
    config: &NodeConfig,
) -> Result<(NodeMode, bool)> {
    let target_role = determine_role_for_epoch(epoch, own_protocol_pubkey, config).await?;

    let needs_change = *current_mode != target_role;

    if needs_change {
        info!(
            "🔄 [ROLE-FIRST] Epoch {}: Mode change needed: {:?} → {:?}",
            epoch, current_mode, target_role
        );
    } else {
        info!(
            "✓ [ROLE-FIRST] Epoch {}: Mode unchanged ({:?})",
            epoch, current_mode
        );
    }

    Ok((target_role, needs_change))
}
