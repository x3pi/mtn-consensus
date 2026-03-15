// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

use anyhow::Result;
use mysten_metrics::start_prometheus_server;
use prometheus::Registry;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

use crate::config::NodeConfig;
use crate::network::peer_discovery::PeerDiscoveryService;
use crate::network::peer_rpc::PeerRpcServer;
use crate::network::rpc::RpcServer;
use crate::network::tx_socket_server::TxSocketServer;
use crate::node::ConsensusNode;

/// Startup configuration and initialization
pub struct StartupConfig {
    pub node_config: NodeConfig,
    pub registry: Registry,
    pub registry_service: Option<Arc<mysten_metrics::RegistryService>>,
}

impl StartupConfig {
    pub fn new(
        node_config: NodeConfig,
        registry: Registry,
        registry_service: Option<Arc<mysten_metrics::RegistryService>>,
    ) -> Self {
        Self {
            node_config,
            registry,
            registry_service,
        }
    }
}

/// Represents the initialized node and its servers
pub struct InitializedNode {
    pub node: Arc<Mutex<ConsensusNode>>,
    pub rpc_server_handle: Option<tokio::task::JoinHandle<()>>,
    pub uds_server_handle: Option<tokio::task::JoinHandle<()>>,
    #[allow(dead_code)]
    pub peer_rpc_server_handle: Option<tokio::task::JoinHandle<()>>,
    pub node_config: NodeConfig,
}

impl InitializedNode {
    /// Initialize and start all node components
    pub async fn initialize(config: StartupConfig) -> Result<Self> {
        let StartupConfig {
            node_config,
            registry,
            registry_service,
        } = config;

        // Start metrics server if enabled
        let _metrics_addr = if node_config.enable_metrics {
            let metrics_addr = SocketAddr::from(([127, 0, 0, 1], node_config.metrics_port));
            let _registry_service = start_prometheus_server(metrics_addr);
            info!(
                "Metrics server started at http://127.0.0.1:{}/metrics",
                node_config.metrics_port
            );
            Some(metrics_addr)
        } else {
            info!("Metrics server is disabled (enable_metrics = false)");
            None
        };

        // Get registry from RegistryService if metrics is enabled, otherwise create a new one
        let registry = if let Some(ref rs) = registry_service {
            rs.default_registry()
        } else {
            registry
        };

        // Create the ConsensusNode wrapped in a Mutex for safe concurrent access
        let node = Arc::new(Mutex::new(
            ConsensusNode::new_with_registry_and_service(node_config.clone(), registry).await?,
        ));

        // Register node in global registry for transition handler access
        crate::node::set_transition_handler_node(node.clone()).await;

        // Get transaction submitter for servers
        let tx_client = { node.lock().await.transaction_submitter() };

        let mut rpc_server_handle = None;
        let uds_server_handle;
        let mut peer_rpc_server_handle = None;

        if let Some(ref tx_client) = tx_client {
            // Start RPC server for client submissions (HTTP) - only for validator nodes
            let rpc_port = node_config.metrics_port + 1000;
            let node_for_rpc = node.clone();
            let rpc_server =
                RpcServer::with_node(tx_client.clone(), rpc_port, node_for_rpc.clone());
            rpc_server_handle = Some(tokio::spawn(async move {
                if let Err(e) = rpc_server.start().await {
                    error!("RPC server error: {}", e);
                }
            }));
            info!("Consensus node started successfully (validator mode)");
            info!("RPC server available at http://127.0.0.1:{}", rpc_port);
        } else {
            info!("Sync-only node started (no RPC server, UDS forwarding enabled)");
        }

        // Start Unix Domain Socket server for ALL node types (validator + SyncOnly)
        // Validators submit directly to consensus; SyncOnly forwards to validators via peer RPC
        {
            let tx_client_for_uds: Arc<dyn crate::node::tx_submitter::TransactionSubmitter> =
                match tx_client {
                    Some(ref tc) => tc.clone(),
                    None => Arc::new(crate::node::tx_submitter::NoOpTransactionSubmitter),
                };

            let socket_path = node_config
                .rust_tx_socket_path
                .clone()
                .unwrap_or_else(|| format!("/tmp/metanode-tx-{}.sock", node_config.node_id));
            let node_for_uds = node.clone();
            let (is_transitioning_for_uds, pending_tx_queue, storage_path) = {
                let node_guard = node.lock().await;
                (
                    node_guard.is_transitioning.clone(),
                    node_guard.pending_transactions_queue.clone(),
                    node_guard.storage_path.clone(),
                )
            };

            // Start PeerDiscoveryService if enabled
            let peer_discovery_addresses = if node_config.enable_peer_discovery {
                if let Some(ref go_rpc_url) = node_config.go_rpc_url {
                    let peer_port = node_config.peer_rpc_port.unwrap_or(6090);
                    let refresh_interval =
                        std::time::Duration::from_secs(node_config.peer_discovery_refresh_secs);
                    let service = Arc::new(
                        PeerDiscoveryService::new(go_rpc_url.clone(), peer_port)
                            .with_refresh_interval(refresh_interval),
                    );
                    let addresses_handle = service.get_addresses_handle();

                    // Start background refresh task
                    let _discovery_handle = service.start();
                    info!(
                        "🔍 [PEER DISCOVERY] Service started (refresh every {}s)",
                        node_config.peer_discovery_refresh_secs
                    );

                    Some(addresses_handle)
                } else {
                    warn!("⚠️ [PEER DISCOVERY] Enabled but go_rpc_url is not set, skipping");
                    None
                }
            } else {
                None
            };

            let mut uds_server = TxSocketServer::with_node(
                socket_path.clone(),
                tx_client_for_uds,
                node_for_uds,
                is_transitioning_for_uds,
                pending_tx_queue,
                storage_path,
                node_config.peer_rpc_addresses.clone(),
            );

            // ♻️ TX RECYCLER: Inject into UDS server for tracking submitted TXs
            {
                let node_guard = node.lock().await;
                if let Some(ref recycler) = node_guard.tx_recycler {
                    uds_server = uds_server.with_tx_recycler(recycler.clone());
                    info!("♻️ [TX RECYCLER] Injected into UDS server");
                }
            }

            // Inject dynamic peer addresses if discovery is enabled
            if let Some(addrs) = peer_discovery_addresses {
                uds_server = uds_server.with_peer_discovery(addrs);
            }

            uds_server_handle = Some(tokio::spawn(async move {
                if let Err(e) = uds_server.start().await {
                    error!("UDS server error: {}", e);
                }
            }));
            info!("Unix Domain Socket server available at {}", socket_path);
        }

        if let Some(peer_port) = node_config.peer_rpc_port {
            if peer_port > 0 {
                let (executor_client_for_peer, shared_index_for_peer) = {
                    let node_guard = node.lock().await;
                    (
                        node_guard.executor_client.clone(),
                        Some(node_guard.shared_last_global_exec_index.clone()),
                    )
                };
                if let Some(exc) = executor_client_for_peer {
                    let mut peer_server = PeerRpcServer::new(
                        node_config.node_id,
                        peer_port,
                        node_config.network_address.clone(),
                        exc,
                        shared_index_for_peer
                            .unwrap_or_else(|| std::sync::Arc::new(tokio::sync::Mutex::new(0))),
                    );
                    // Inject transaction submitter so validators can accept forwarded TXs from SyncOnly nodes
                    if let Some(ref submitter) = tx_client {
                        peer_server = peer_server.with_transaction_submitter(submitter.clone());
                        info!("📡 [PEER RPC] Transaction submitter injected for /submit_transaction endpoint");
                    }
                    peer_rpc_server_handle = Some(tokio::spawn(async move {
                        if let Err(e) = peer_server.start().await {
                            error!("Peer RPC server error: {}", e);
                        }
                    }));
                    info!(
                        "📡 [PEER RPC] Server started on 0.0.0.0:{} for WAN sync",
                        peer_port
                    );
                } else {
                    warn!("⚠️ [PEER RPC] No executor client available, skipping peer RPC server");
                }
            }
        }

        Ok(Self {
            node,
            rpc_server_handle,
            uds_server_handle,
            peer_rpc_server_handle,
            node_config,
        })
    }

    /// Run the main event loop
    pub async fn run_main_loop(self) -> Result<()> {
        // --- [SYNC-BEFORE-CONSENSUS] ---
        // Block startup until we catch up with the network
        // This prevents the node from proposing blocks on a fork or when behind
        let catchup_manager = {
            let node_guard = self.node.lock().await;
            if let Some(client) = node_guard.executor_client.clone() {
                Some(crate::node::catchup::CatchupManager::new(
                    client,
                    self.node_config.executor_receive_socket_path.clone(),
                    self.node_config.peer_rpc_addresses.clone(),
                ))
            } else {
                warn!("⚠️ [STARTUP] No executor client available, skipping catchup check");
                None
            }
        };

        // ═══════════════════════════════════════════════════════════════════════
        // SNAPSHOT RESTORE SUPPORT for SyncOnly nodes:
        // Previously, SyncOnly skipped catchup entirely, causing nodes restored
        // from Go snapshot to get stuck (commit_syncer at stale epoch = "Not
        // enough votes"). Now we run a LIGHTWEIGHT epoch-catchup: sync Go blocks
        // from peers until Go reaches the current network epoch. After Go catches
        // up, the existing epoch_monitor detects the epoch change and triggers
        // consensus restart at the correct epoch. Combined with the cold-start
        // fast-forward in commit_syncer, the node does NOT need Rust consensus
        // storage copied from another node.
        //
        // The old deadlock (can't match epoch without blocks, can't get blocks
        // without matching epoch) is broken by sync_go_to_current_epoch() which
        // fetches blocks from peer Go nodes regardless of epoch match.
        // ═══════════════════════════════════════════════════════════════════════
        let is_sync_only_mode = {
            let node_guard = self.node.lock().await;
            matches!(node_guard.node_mode, crate::node::NodeMode::SyncOnly)
        };

        if is_sync_only_mode {
            // SyncOnly: Run lightweight epoch-catchup only (no consensus join wait)
            if let Some(ref cm) = catchup_manager {
                info!("📋 [STARTUP] SyncOnly mode: running epoch-catchup before sync_loop...");
                let local_epoch = {
                    let node_guard = self.node.lock().await;
                    node_guard.current_epoch
                };
                match cm.check_sync_status(local_epoch, 0).await {
                    Ok(status) if !status.epoch_match => {
                        info!(
                            "🔄 [STARTUP] SyncOnly epoch mismatch: Local={}, Network={}. Syncing Go blocks...",
                            local_epoch, status.go_epoch
                        );
                        match cm.sync_go_to_current_epoch(status.go_epoch).await {
                            Ok(synced) => {
                                info!(
                                    "✅ [STARTUP] SyncOnly epoch-catchup complete: {} blocks synced. \
                                     epoch_monitor will handle consensus restart at correct epoch.",
                                    synced
                                );
                            }
                            Err(e) => {
                                warn!(
                                    "⚠️ [STARTUP] SyncOnly epoch-catchup failed: {}. \
                                     sync_loop will handle catchup at runtime.",
                                    e
                                );
                            }
                        }
                    }
                    Ok(_) => {
                        info!("✅ [STARTUP] SyncOnly: epoch matches network, no catchup needed.");
                    }
                    Err(e) => {
                        warn!("⚠️ [STARTUP] SyncOnly sync status check failed: {}. Proceeding.", e);
                    }
                }
            } else {
                info!("📋 [STARTUP] SyncOnly mode: no executor client, skipping catchup.");
            }
        } else if let Some(cm) = catchup_manager {
            let is_syncing_up_mode = {
                let node_guard = self.node.lock().await;
                matches!(node_guard.node_mode, crate::node::NodeMode::SyncingUp)
            };

            if is_syncing_up_mode {
                info!("⏳ [STARTUP] Verifying sync status before joining consensus...");
                let _check_interval = std::time::Duration::from_secs(2); // kept for reference
                let timeout = std::time::Duration::from_secs(600); // 10 minutes timeout
            let timeout = std::time::Duration::from_secs(600); // 10 minutes timeout
            let start = std::time::Instant::now();

                // Node is already in SyncingUp mode, we just wait.

            loop {
                // Check timeout
                if start.elapsed() > timeout {
                    warn!("⚠️ [STARTUP] Catchup timed out after 600s. Forcing start (risky).");
                    break;
                }

                // Get current local state
                let (local_epoch, local_commit) = {
                    let node = self.node.lock().await;
                    // RocksDBStore read is expensive? No, we use in-memory counters if available?
                    // ConsensusNode has current_commit_index (AtomicU32) but we need u64 mapping?
                    // Let's use current_epoch.
                    // Commit index is trickier. Let's assume passed 0 for now as catchup checks Epoch primarily.
                    // But for Commit sync, we need local commit.
                    // Use commit_processor's tracked index?
                    // Node has `current_commit_index` (AtomicU32).
                    (
                        node.current_epoch,
                        node.current_commit_index
                            .load(std::sync::atomic::Ordering::Relaxed)
                            as u64,
                    )
                };

                match cm.check_sync_status(local_epoch, local_commit).await {
                    Ok(status) => {
                        if status.ready {
                            info!(
                                "✅ [STARTUP] Node is synced (gap={}). Joining consensus!",
                                status.commit_gap
                            );
                            break;
                        }

                        if status.epoch_match {
                            info!(
                                "🔄 [CATCHUP] Syncing blocks: LocalExec={}, Network={}, Gap={}",
                                status.go_last_block, status.network_block_height, status.block_gap
                            );

                            // FAST SYNC: For large gaps, loop continuously fetching batches
                            if status.block_gap > 100 {
                                let mut remaining = status.block_gap;
                                let mut current_go_block = status.go_last_block;
                                let max_blocks_per_cycle = 2000u64;
                                let mut fetched_total = 0u64;

                                while remaining > 0 && fetched_total < max_blocks_per_cycle {
                                    let fetch_to = std::cmp::min(
                                        current_go_block + 50,
                                        status.network_block_height,
                                    );
                                    match cm
                                        .sync_blocks_from_peers(current_go_block, fetch_to)
                                        .await
                                    {
                                        Ok(synced) => {
                                            if synced == 0 {
                                                break;
                                            }
                                            current_go_block += synced;
                                            remaining = remaining.saturating_sub(synced);
                                            fetched_total += synced;
                                        }
                                        Err(e) => {
                                            warn!("⚠️ [CATCHUP] Fast sync batch failed: {}", e);
                                            break;
                                        }
                                    }
                                }
                                info!(
                                    "🚀 [CATCHUP] Fast sync cycle: fetched {} blocks total",
                                    fetched_total
                                );
                                continue; // No delay - immediately re-check
                            }

                            // Normal sync: small gap, single fetch
                            if let Err(e) = cm
                                .sync_blocks_from_peers(
                                    status.go_last_block,
                                    status.network_block_height,
                                )
                                .await
                            {
                                warn!("⚠️ [CATCHUP] Block sync from peers failed: {}", e);
                            }
                        } else {
                            // ═══════════════════════════════════════════════════
                            // CROSS-EPOCH BLOCK SYNC (Snapshot Restore Support)
                            // Go is at a different epoch than the network.
                            // Fetch blocks from peer Go nodes until Go catches up
                            // to the current network epoch. This enables nodes to
                            // start from only a Go snapshot without Rust data.
                            // ═══════════════════════════════════════════════════
                            info!(
                                "🔄 [CATCHUP] Epoch mismatch: GoLocal={}, Network={}. Syncing Go blocks to reach target epoch...",
                                local_epoch, status.go_epoch
                            );
                            match cm.sync_go_to_current_epoch(status.go_epoch).await {
                                Ok(synced) => {
                                    info!(
                                        "✅ [CATCHUP] Cross-epoch sync complete: {} blocks synced. Go should now be at epoch {}.",
                                        synced, status.go_epoch
                                    );
                                    // Don't delay — immediately re-check sync status
                                    continue;
                                }
                                Err(e) => {
                                    warn!(
                                        "⚠️ [CATCHUP] Cross-epoch sync failed: {}. Will retry...",
                                        e
                                    );
                                }
                            }
                        }
                    }
                    Err(e) => {
                        warn!("⚠️ [STARTUP] Failed to check sync status: {}", e);
                    }
                }

                    // Dynamic delay: 200ms for near-caught-up (much faster than original 2s)
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }
            } else {
                // ═══════════════════════════════════════════════════════════════
                // SNAPSHOT RESTORE: Even when "not lagging" (no peer reference),
                // the node may be behind by entire epochs after snapshot restore.
                // Check if Go epoch matches network epoch — if not, sync Go
                // blocks from peers until Go catches up to the current epoch.
                // Without this, the node joins consensus at a stale epoch and
                // commit_syncer fails with "wrong epoch" on every fetch.
                // ═══════════════════════════════════════════════════════════════
                info!("✅ [STARTUP] Node is starting directly as Validator (lag is below threshold). Checking epoch match before proceeding...");
                let local_epoch = {
                    let node_guard = self.node.lock().await;
                    node_guard.current_epoch
                };
                match cm.check_sync_status(local_epoch, 0).await {
                    Ok(status) if !status.epoch_match => {
                        warn!(
                            "🔄 [STARTUP] Epoch mismatch detected at Validator startup: Local epoch={}, Network epoch={}. Running cross-epoch sync...",
                            local_epoch, status.go_epoch
                        );
                        match cm.sync_go_to_current_epoch(status.go_epoch).await {
                            Ok(synced) => {
                                info!(
                                    "✅ [STARTUP] Cross-epoch sync complete: {} blocks synced. epoch_monitor will handle transition.",
                                    synced
                                );
                            }
                            Err(e) => {
                                warn!(
                                    "⚠️ [STARTUP] Cross-epoch sync failed: {}. Proceeding anyway — epoch_monitor may handle it later.",
                                    e
                                );
                            }
                        }
                    }
                    Ok(_) => {
                        info!("✅ [STARTUP] Epoch matches network. Consensus syncing will handle any minor lag.");
                    }
                    Err(e) => {
                        warn!("⚠️ [STARTUP] Epoch check failed: {}. Proceeding anyway.", e);
                    }
                }
            }

            // Restore Validator mode by triggering a mode-only transition
            let (was_syncing_up, epoch, exec_index) = {
                let node_guard = self.node.lock().await;
                (
                    node_guard.node_mode == crate::node::NodeMode::SyncingUp,
                    node_guard.current_epoch,
                    node_guard.last_global_exec_index,
                )
            };

            if was_syncing_up {
                info!("✅ [STARTUP] Catch-up complete. Starting ConsensusAuthority for Validator...");
                let mut node_guard = self.node.lock().await;
                
                // Mode transition logic handles starting the authority
                // We pass 0 for boundary_block since transition_mode_only fetches authoritative timestamp from Go
                if let Err(e) = crate::node::transition::mode_transition::transition_mode_only(
                    &mut *node_guard,
                    epoch,
                    0, // Unused
                    exec_index,
                    &self.node_config,
                ).await {
                    error!("❌ [STARTUP] Failed to transition to Validator mode: {}", e);
                    // Fallback: just set the flag so at least we don't stay stuck
                    node_guard.node_mode = crate::node::NodeMode::Validator;
                }
            }
        }

        info!("Press Ctrl+C to stop the node");
        tokio::signal::ctrl_c().await?;
        info!("Received Ctrl+C, initiating shutdown...");
        self.shutdown().await
    }

    /// Shutdown the node and all servers
    /// Thứ tự tắt được tối ưu để đảm bảo data integrity:
    /// 1. Shutdown consensus connections/tasks (để tránh new blocks)
    /// 2. Flush remaining blocks to Go Master (đảm bảo không mất blocks)
    /// 3. Shutdown servers (dừng accept new requests)
    pub async fn shutdown(self) -> Result<()> {
        info!("Shutting down node...");

        // 1. Shutdown servers FIRST (stop accepting new requests)
        if let Some(handle) = self.rpc_server_handle {
            handle.abort();
            // Optional: wait for it to finish
            let _ = handle.await;
        }
        if let Some(handle) = self.uds_server_handle {
            handle.abort();
            let _ = handle.await;
        }

        // 2. Lock node and perform shutdown sequence
        // We use lock() instead of try_unwrap() because the node is shared (e.g. global registry)
        let mut node = self.node.lock().await;

        // 3. Flush remaining blocks to Go Master
        if let Err(e) = node.flush_blocks_to_go_master().await {
            warn!("⚠️ [SHUTDOWN] Failed to flush blocks: {}", e);
        }

        // 4. Shutdown consensus connections/tasks
        if let Err(e) = node.shutdown().await {
            error!("❌ [SHUTDOWN] Error during consensus shutdown: {}", e);
        }

        info!("Node stopped gracefully");
        Ok(())
    }
}
