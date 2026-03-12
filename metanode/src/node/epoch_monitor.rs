// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

//! Unified Epoch Monitor
//!
//! A single monitor that handles epoch transitions for BOTH SyncOnly and Validator nodes.
//! This replaces the previous fragmented approach of separate monitors.
//!
//! ## Design Principles
//! 1. **Single Source of Truth**: Go layer epoch is authoritative
//! 2. **Always Running**: Monitor never exits - runs continuously for all node modes
//! 3. **Fork-Safe**: Uses `boundary_block` from `get_epoch_boundary_data()`
//! 4. **Unified Logic**: Same code path for SyncOnly and Validator nodes

use crate::config::NodeConfig;
use anyhow::Result;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

/// Start the unified epoch monitor for ALL node types (SyncOnly and Validator)
///
/// This monitor:
/// 1. Polls Go epoch every N seconds
/// 2. Detects when Rust epoch falls behind Go epoch
/// 3. Fetches epoch boundary data (fork-safe)
/// 4. Triggers appropriate transition (SyncOnly→Validator or epoch update)
///
/// IMPORTANT: This monitor NEVER exits - it runs continuously for the lifetime of the node.
/// This prevents the bug where Validators get stuck when they miss EndOfEpoch transactions.
pub fn start_unified_epoch_monitor(
    executor_client: &Option<Arc<crate::node::executor_client::ExecutorClient>>,
    config: &NodeConfig,
) -> Result<Option<JoinHandle<()>>> {
    let client_arc = match executor_client {
        Some(client) => client.clone(),
        None => {
            warn!("⚠️ [EPOCH MONITOR] Cannot start - no executor client");
            return Ok(None);
        }
    };

    let node_id = config.node_id;
    let config_clone = config.clone();
    // Default poll interval: configurable, default 10 seconds
    let poll_interval_secs = config.epoch_monitor_poll_interval_secs.unwrap_or(10);

    info!(
        "🔄 [EPOCH MONITOR] Starting unified epoch monitor for node-{} (poll_interval={}s)",
        node_id, poll_interval_secs
    );

    let handle = tokio::spawn(async move {
        loop {
            // Wait for poll interval
            tokio::time::sleep(Duration::from_secs(poll_interval_secs)).await;

            // 1. Get LOCAL Go epoch (may be stale for late-joiners!)
            let local_go_epoch = match client_arc.get_current_epoch().await {
                Ok(epoch) => epoch,
                Err(e) => {
                    debug!("⚠️ [EPOCH MONITOR] Failed to get local Go epoch: {}", e);
                    continue;
                }
            };

            // 2. Get NETWORK epoch from peers (critical for late-joiners!)
            // Use peer_rpc_addresses for WAN-based discovery
            let network_epoch = {
                let peer_rpc = config_clone.peer_rpc_addresses.clone();
                let _own_socket = config_clone.executor_receive_socket_path.clone();

                if !peer_rpc.is_empty() {
                    // WAN-based discovery (TCP) - recommended for cross-node sync
                    match crate::network::peer_rpc::query_peer_epochs_network(&peer_rpc).await {
                        Ok((epoch, _block, peer, _global_exec_index)) => {
                            if epoch > local_go_epoch {
                                info!(
                                    "🌐 [EPOCH MONITOR] Network epoch {} from peer {} is AHEAD of local Go epoch {}",
                                    epoch, peer, local_go_epoch
                                );
                            }
                            epoch
                        }
                        Err(_) => local_go_epoch, // Fallback to local
                    }
                } else {
                    // No WAN peers configured - use local Go epoch
                    // NOTE: LAN peer_executor_sockets was removed. For cross-node sync, configure peer_rpc_addresses.
                    local_go_epoch
                }
            };

            // 3. Get current Rust epoch from node
            let (rust_epoch, _current_mode) =
                if let Some(node_arc) = crate::node::get_transition_handler_node().await {
                    let node_guard = node_arc.lock().await;
                    (node_guard.current_epoch, node_guard.node_mode.clone())
                } else {
                    debug!("⚠️ [EPOCH MONITOR] Node not registered yet, waiting...");
                    continue;
                };

            // 4. Check if transition needed (NETWORK epoch ahead of Rust)
            // Use network_epoch instead of local_go_epoch!
            if network_epoch <= rust_epoch {
                // No transition needed - epochs are in sync with network
                continue;
            }

            // ═══════════════════════════════════════════════════════════════
            // CRITICAL FIX: SyncOnly nodes must NOT trigger epoch transitions
            // from epoch_monitor! SyncOnly nodes sync blocks sequentially via
            // sync_loop → blocks reach boundary → check_and_process_pending_epoch_transitions
            // handles it naturally. If epoch_monitor triggers transition early,
            // it causes DEADLOCK: Go GEI=0 but deferred transition waits for GEI>=boundary.
            // ═══════════════════════════════════════════════════════════════
            if matches!(_current_mode, crate::node::NodeMode::SyncOnly) {
                debug!(
                    "📋 [EPOCH MONITOR] SyncOnly mode: skipping epoch transition {} → {} (sync_loop handles transitions via block sync)",
                    rust_epoch, network_epoch
                );
                continue;
            }

            let epoch_gap = network_epoch - rust_epoch;
            info!(
                "🔄 [EPOCH MONITOR] Epoch gap detected: Rust={} Network={} (gap={})",
                rust_epoch, network_epoch, epoch_gap
            );

            // 4. Get epoch boundary data - CRITICAL FIX for timestamp consistency
            // PROBLEM: When LOCAL Go syncs epoch from blocks, it uses block.TimeStamp()*1000
            //          which is rounded to seconds. Validators use exact ms from EndOfEpoch tx.
            //          This causes 906ms discrepancy -> different genesis hashes -> fork!
            // SOLUTION: If LOCAL Go is behind (late-joining node), query PEER for authoritative timestamp
            //           Peers (validators) have the exact timestamp from EndOfEpoch system tx.

            let local_executor_client = client_arc.clone();
            let peer_rpc = config_clone.peer_rpc_addresses.clone();

            let boundary_data = if local_go_epoch < network_epoch && !peer_rpc.is_empty() {
                // LOCAL Go is behind - query PEER for authoritative timestamp
                info!(
                    "🌐 [EPOCH MONITOR] LOCAL Go epoch {} < network epoch {}. Querying PEER for authoritative timestamp...",
                    local_go_epoch, network_epoch
                );

                // Try to get from first responsive peer
                let mut peer_boundary_data: Option<(u64, u64, u64)> = None;
                for peer_addr in &peer_rpc {
                    match crate::network::peer_rpc::query_peer_epoch_boundary_data(
                        peer_addr,
                        network_epoch,
                    )
                    .await
                    {
                        Ok(data) => {
                            info!(
                                "✅ [EPOCH MONITOR] Got AUTHORITATIVE boundary data from PEER {}: epoch={}, timestamp={}ms, boundary={}",
                                peer_addr, data.epoch, data.timestamp_ms, data.boundary_block
                            );
                            peer_boundary_data =
                                Some((data.epoch, data.timestamp_ms, data.boundary_block));
                            break;
                        }
                        Err(e) => {
                            debug!(
                                "⚠️ [EPOCH MONITOR] Peer {} failed for epoch {}: {}",
                                peer_addr, network_epoch, e
                            );
                        }
                    }
                }

                if let Some(data) = peer_boundary_data {
                    data
                } else {
                    // Fallback to LOCAL Go if all peers fail
                    warn!("⚠️ [EPOCH MONITOR] All peers failed, falling back to LOCAL Go (timestamp may be rounded!)");
                    match local_executor_client
                        .get_epoch_boundary_data(network_epoch)
                        .await
                    {
                        Ok((epoch, timestamp_ms, boundary_block, _validators, _)) => {
                            (epoch, timestamp_ms, boundary_block)
                        }
                        Err(e) => {
                            info!("⏳ [EPOCH MONITOR] Local Go not ready: {}. Waiting...", e);
                            continue;
                        }
                    }
                }
            } else {
                // LOCAL Go is in sync - use it as source (it has authoritative timestamp from AdvanceEpoch RPC)
                match local_executor_client
                    .get_epoch_boundary_data(network_epoch)
                    .await
                {
                    Ok((epoch, timestamp_ms, boundary_block, _validators, _)) => {
                        info!(
                            "📊 [EPOCH MONITOR] Got boundary data from LOCAL Go: epoch={}, timestamp={}ms, boundary_block={}",
                            epoch, timestamp_ms, boundary_block
                        );
                        (epoch, timestamp_ms, boundary_block)
                    }
                    Err(e) => {
                        // LOCAL Go not ready - wait and retry
                        info!(
                            "⏳ [EPOCH MONITOR] Local Go not ready for epoch {}: {}. Waiting for sync...",
                            network_epoch, e
                        );
                        continue; // Retry in next poll cycle
                    }
                }
            };

            let (new_epoch, epoch_timestamp_ms, boundary_block) = boundary_data;

            // 5. Check with EpochTransitionManager before proceeding
            // This prevents race conditions with system_tx handler
            let epoch_manager = match crate::node::epoch_transition_manager::get_epoch_manager() {
                Some(m) => m,
                None => {
                    // Manager not initialized yet, skip this cycle
                    debug!("⏳ [EPOCH MONITOR] Epoch manager not initialized yet, skipping");
                    continue;
                }
            };

            // Try to acquire transition lock
            if let Err(e) = epoch_manager
                .try_start_epoch_transition(new_epoch, "epoch_monitor")
                .await
            {
                debug!("⏳ [EPOCH MONITOR] Cannot start transition: {}", e);
                continue;
            }

            // 6. Log with source tracking
            info!(
                "🔄 [EPOCH MONITOR] Triggering transition (source=epoch_monitor): epoch {} → {} | boundary_block={} | mode will be determined by transition.rs",
                rust_epoch, new_epoch, boundary_block
            );

            // 7. Execute transition
            if let Some(node_arc) = crate::node::get_transition_handler_node().await {
                let mut node_guard = node_arc.lock().await;

                // Use boundary_block as synced_global_exec_index (FORK-SAFE!)
                let synced_global_exec_index = boundary_block;

                match node_guard
                    .transition_to_epoch_from_system_tx(
                        new_epoch,
                        epoch_timestamp_ms,
                        synced_global_exec_index,
                        &config_clone,
                    )
                    .await
                {
                    Ok(()) => {
                        // Mark transition as complete in manager
                        epoch_manager.complete_epoch_transition(new_epoch).await;

                        info!(
                            "✅ [EPOCH MONITOR] Successfully triggered transition to epoch {}",
                            new_epoch
                        );

                        // =========================================================
                        // MULTI-EPOCH CATCH-UP: Check if more epochs needed
                        // If still behind network, immediately continue without
                        // waiting for next poll cycle
                        // =========================================================
                        let current_rust_epoch = node_guard.current_epoch;
                        drop(node_guard); // Release lock before continuing

                        if current_rust_epoch < network_epoch {
                            info!(
                                "🔄 [EPOCH MONITOR] Multi-epoch catch-up: still behind (Rust={}, Network={}). Continuing immediately...",
                                current_rust_epoch, network_epoch
                            );
                            // Don't wait for poll interval, continue immediately
                            continue;
                        }
                    }
                    Err(e) => {
                        // Mark transition as failed in manager
                        epoch_manager.fail_transition(&e.to_string()).await;

                        warn!(
                            "❌ [EPOCH MONITOR] Failed to transition to epoch {}: {}",
                            new_epoch, e
                        );
                    }
                }
            } else {
                // No node available, fail the transition
                epoch_manager.fail_transition("Node not registered").await;
            }

            // CRITICAL: Do NOT exit the loop! Monitor continues running
            // This is the key fix - monitor runs for the entire node lifetime
        }
    });

    Ok(Some(handle))
}

/// Stop the epoch monitor task
#[allow(dead_code)]
pub async fn stop_epoch_monitor(handle: Option<JoinHandle<()>>) {
    if let Some(h) = handle {
        h.abort();
        info!("🛑 [EPOCH MONITOR] Stopped unified epoch monitor");
    }
}
