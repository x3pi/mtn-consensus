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

            // ═══════════════════════════════════════════════════════════════
            // BLOCK SYNC: Proactively sync application blocks from peers.
            // After snapshot restore, Rust consensus may be severely behind
            // (e.g. lag=5249 commits) and unable to push blocks to Go.
            // This ensures Go always has the latest application blocks
            // regardless of Rust consensus state.
            // ═══════════════════════════════════════════════════════════════
            {
                let go_block = client_arc.get_last_block_number().await.unwrap_or(0);
                let peer_rpc = config_clone.peer_rpc_addresses.clone();
                if !peer_rpc.is_empty() {
                    let fetch_from = go_block + 1;
                    let fetch_to = go_block + 100; // Fetch in batches of 100
                    match crate::network::peer_rpc::fetch_blocks_from_peer(
                        &peer_rpc, fetch_from, fetch_to,
                    ).await {
                        Ok(blocks) if !blocks.is_empty() => {
                            let count = blocks.len();
                            match client_arc.sync_blocks(blocks).await {
                                Ok((synced, last_block)) => {
                                    info!(
                                        "✅ [EPOCH MONITOR] Block sync: fetched {} blocks, synced {} to Go (last_block={})",
                                        count, synced, last_block
                                    );
                                }
                                Err(e) => {
                                    warn!("⚠️ [EPOCH MONITOR] Block sync: sync_blocks failed: {}", e);
                                }
                            }
                        }
                        Ok(_) => {
                            // No blocks available from peers — Go is caught up
                        }
                        Err(e) => {
                            debug!("⚠️ [EPOCH MONITOR] Block sync: fetch failed: {}", e);
                        }
                    }
                }
            }

            // 4. Check if transition needed (NETWORK epoch ahead of Rust)
            // Use network_epoch instead of local_go_epoch!
            if network_epoch <= rust_epoch {
                // No transition needed - epochs are in sync with network
                continue;
            }

            // ═══════════════════════════════════════════════════════════════
            // SyncOnly nodes: advance Go Master epoch by fetching blocks + advance_epoch
            // Previously this was a complete `continue` which left Go permanently behind.
            // SyncOnly nodes don't run full transitions, but Go must advance epoch
            // to serve blocks at the correct epoch to other nodes and to itself.
            // ═══════════════════════════════════════════════════════════════
            if matches!(_current_mode, crate::node::NodeMode::SyncOnly) {
                // Skip if Go is already caught up
                if local_go_epoch >= network_epoch {
                    continue;
                }
                info!(
                    "🔄 [EPOCH MONITOR] SyncOnly mode: advancing Go epoch {} → {} (fetching blocks + advance_epoch)",
                    local_go_epoch, network_epoch
                );

                // Fetch boundary data from peers
                let peer_rpc = config_clone.peer_rpc_addresses.clone();
                if peer_rpc.is_empty() {
                    warn!("[EPOCH MONITOR] SyncOnly: no peer_rpc_addresses, cannot advance Go epoch");
                    continue;
                }

                // Advance Go through each intermediate epoch sequentially
                let mut current_go_epoch = local_go_epoch;
                for target_epoch in (local_go_epoch + 1)..=network_epoch {
                    // Get boundary data from peer for this epoch
                    let mut boundary_found = false;
                    for peer_addr in &peer_rpc {
                        match crate::network::peer_rpc::query_peer_epoch_boundary_data(
                            peer_addr, target_epoch,
                        ).await {
                            Ok(data) => {
                                info!(
                                    "📦 [EPOCH MONITOR] SyncOnly: epoch {} boundary={}, timestamp={}ms (from {})",
                                    target_epoch, data.boundary_block, data.timestamp_ms, peer_addr
                                );

                                // Fetch blocks up to boundary from peers
                                let go_block = client_arc.get_last_block_number().await.unwrap_or(0);
                                if go_block < data.boundary_block {
                                    // Fetch missing blocks
                                    match crate::network::peer_rpc::fetch_blocks_from_peer(
                                        &peer_rpc, go_block + 1, data.boundary_block,
                                    ).await {
                                        Ok(blocks) if !blocks.is_empty() => {
                                            match client_arc.sync_blocks(blocks).await {
                                                Ok((synced, last)) => {
                                                    info!(
                                                        "✅ [EPOCH MONITOR] SyncOnly: synced {} blocks to Go (last: {})",
                                                        synced, last
                                                    );
                                                }
                                                Err(e) => {
                                                    warn!("⚠️ [EPOCH MONITOR] SyncOnly: sync_blocks failed: {}", e);
                                                }
                                            }
                                        }
                                        _ => {}
                                    }
                                }

                                // Advance Go epoch
                                if current_go_epoch < target_epoch {
                                    match client_arc.advance_epoch(target_epoch, data.timestamp_ms, data.boundary_block).await {
                                        Ok(_) => {
                                            info!(
                                                "✅ [EPOCH MONITOR] SyncOnly: advanced Go to epoch {} (boundary={})",
                                                target_epoch, data.boundary_block
                                            );
                                            current_go_epoch = target_epoch;
                                        }
                                        Err(e) => {
                                            warn!(
                                                "⚠️ [EPOCH MONITOR] SyncOnly: failed to advance Go to epoch {}: {}",
                                                target_epoch, e
                                            );
                                            break;
                                        }
                                    }
                                }

                                boundary_found = true;
                                break;
                            }
                            Err(e) => {
                                debug!(
                                    "[EPOCH MONITOR] SyncOnly: peer {} failed for epoch {}: {}",
                                    peer_addr, target_epoch, e
                                );
                            }
                        }
                    }

                    if !boundary_found {
                        warn!(
                            "⚠️ [EPOCH MONITOR] SyncOnly: no peer had boundary for epoch {}. Stopping at epoch {}.",
                            target_epoch, current_go_epoch
                        );
                        break;
                    }
                }

                continue;
            }

            let epoch_gap = network_epoch - rust_epoch;
            info!(
                "🔄 [EPOCH MONITOR] Epoch gap detected: Rust={} Network={} (gap={})",
                rust_epoch, network_epoch, epoch_gap
            );

            // ═══════════════════════════════════════════════════════════════
            // MULTI-EPOCH CATCH-UP: Step through each intermediate epoch
            // Transition N→N+2 requires going through N→N+1→N+2 because
            // each epoch needs its own boundary data and committee setup.
            // ═══════════════════════════════════════════════════════════════
            let local_executor_client = client_arc.clone();
            let peer_rpc = config_clone.peer_rpc_addresses.clone();

            let mut current_rust_epoch = rust_epoch;
            for target_epoch in (rust_epoch + 1)..=network_epoch {
                info!(
                    "🔄 [EPOCH MONITOR] Multi-epoch step: {} → {} (target: {})",
                    current_rust_epoch, target_epoch, network_epoch
                );

                // Get boundary data from peer (authoritative) or local Go
                let boundary_data = if local_go_epoch < target_epoch && !peer_rpc.is_empty() {
                    // LOCAL Go is behind — query PEER for authoritative timestamp
                    let mut peer_data: Option<(u64, u64, u64)> = None;
                    for peer_addr in &peer_rpc {
                        match crate::network::peer_rpc::query_peer_epoch_boundary_data(
                            peer_addr,
                            target_epoch,
                        ).await {
                            Ok(data) => {
                                info!(
                                    "✅ [EPOCH MONITOR] Got boundary from PEER {}: epoch={}, timestamp={}ms, boundary={}",
                                    peer_addr, data.epoch, data.timestamp_ms, data.boundary_block
                                );
                                peer_data = Some((data.epoch, data.timestamp_ms, data.boundary_block));
                                break;
                            }
                            Err(e) => {
                                debug!("⚠️ [EPOCH MONITOR] Peer {} failed for epoch {}: {}", peer_addr, target_epoch, e);
                            }
                        }
                    }
                    match peer_data {
                        Some(data) => data,
                        None => {
                            warn!("⚠️ [EPOCH MONITOR] All peers failed for epoch {}. Stopping at epoch {}.", target_epoch, current_rust_epoch);
                            break;
                        }
                    }
                } else {
                    // LOCAL Go has this epoch data
                    match local_executor_client.get_epoch_boundary_data(target_epoch).await {
                        Ok((epoch, timestamp_ms, boundary_block, _validators, _)) => {
                            (epoch, timestamp_ms, boundary_block)
                        }
                        Err(e) => {
                            info!("⏳ [EPOCH MONITOR] Local Go not ready for epoch {}: {}. Trying peer fallback...", target_epoch, e);
                            // Try peer fallback
                            let mut peer_data: Option<(u64, u64, u64)> = None;
                            for peer_addr in &peer_rpc {
                                match crate::network::peer_rpc::query_peer_epoch_boundary_data(peer_addr, target_epoch).await {
                                    Ok(data) => {
                                        peer_data = Some((data.epoch, data.timestamp_ms, data.boundary_block));
                                        break;
                                    }
                                    Err(_) => {}
                                }
                            }
                            match peer_data {
                                Some(data) => data,
                                None => {
                                    warn!("⚠️ [EPOCH MONITOR] No source for epoch {} boundary. Stopping at epoch {}.", target_epoch, current_rust_epoch);
                                    break;
                                }
                            }
                        }
                    }
                };

                let (new_epoch, epoch_timestamp_ms, boundary_block) = boundary_data;

                // First ensure Go has enough blocks for this epoch
                let go_block = client_arc.get_last_block_number().await.unwrap_or(0);
                if go_block < boundary_block && !peer_rpc.is_empty() {
                    match crate::network::peer_rpc::fetch_blocks_from_peer(
                        &peer_rpc, go_block + 1, boundary_block,
                    ).await {
                        Ok(blocks) if !blocks.is_empty() => {
                            let count = blocks.len();
                            if let Ok((synced, last)) = client_arc.sync_blocks(blocks).await {
                                info!("✅ [EPOCH MONITOR] Synced {} blocks to Go for epoch {} boundary (last: {})", synced, target_epoch, last);
                            }
                            let _ = count;
                        }
                        _ => {}
                    }
                }

                // Advance Go epoch if needed
                let current_go_epoch = client_arc.get_current_epoch().await.unwrap_or(0);
                if current_go_epoch < target_epoch {
                    if let Err(e) = client_arc.advance_epoch(target_epoch, epoch_timestamp_ms, boundary_block).await {
                        warn!("⚠️ [EPOCH MONITOR] Failed to advance Go to epoch {}: {}", target_epoch, e);
                    } else {
                        info!("✅ [EPOCH MONITOR] Advanced Go to epoch {}", target_epoch);
                    }
                }

                // Try Rust transition via EpochTransitionManager
                let epoch_manager = match crate::node::epoch_transition_manager::get_epoch_manager() {
                    Some(m) => m,
                    None => {
                        debug!("⏳ [EPOCH MONITOR] Epoch manager not initialized yet");
                        break;
                    }
                };

                if let Err(e) = epoch_manager.try_start_epoch_transition(new_epoch, "epoch_monitor").await {
                    debug!("⏳ [EPOCH MONITOR] Cannot start transition to epoch {}: {}", new_epoch, e);
                    // Still continue — Go epoch was advanced, sync_loop will pick up via auto_epoch_sync
                    continue;
                }

                if let Some(node_arc) = crate::node::get_transition_handler_node().await {
                    let mut node_guard = node_arc.lock().await;
                    let synced_global_exec_index = boundary_block;

                    match node_guard.transition_to_epoch_from_system_tx(
                        new_epoch,
                        epoch_timestamp_ms,
                        synced_global_exec_index,
                        &config_clone,
                    ).await {
                        Ok(()) => {
                            epoch_manager.complete_epoch_transition(new_epoch).await;
                            current_rust_epoch = new_epoch;
                            info!(
                                "✅ [EPOCH MONITOR] Transitioned to epoch {} ({}/{})",
                                new_epoch, new_epoch - rust_epoch, epoch_gap
                            );
                        }
                        Err(e) => {
                            epoch_manager.fail_transition(&e.to_string()).await;
                            warn!(
                                "❌ [EPOCH MONITOR] Failed transition to epoch {}: {}. Stopping at epoch {}.",
                                new_epoch, e, current_rust_epoch
                            );
                            break;
                        }
                    }
                } else {
                    epoch_manager.fail_transition("Node not registered").await;
                    break;
                }

                // Small delay between epoch transitions to let state settle
                tokio::time::sleep(Duration::from_millis(200)).await;
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
