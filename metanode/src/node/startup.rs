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

        let rpc_server_handle;
        let uds_server_handle;
        let mut peer_rpc_server_handle = None;

        // Start RPC server for client submissions (HTTP)
        // ALWAY start RPC Server. For SyncOnly nodes that are catching up, they still need the port open
        // so that clients can connect (the transactions will just be rejected/queued until caught up).
        let rpc_port = node_config.metrics_port + 1000;
        let node_for_rpc = node.clone();
        let rpc_server = RpcServer::with_node(rpc_port, node_for_rpc);
        rpc_server_handle = Some(tokio::spawn(async move {
            if let Err(e) = rpc_server.start().await {
                error!("RPC server error: {}", e);
            }
        }));
        info!("RPC server available at http://127.0.0.1:{}", rpc_port);

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
                    // Inject dynamic node reference instead of static transaction submitter
                    peer_server = peer_server.with_node(node.clone());
                    info!("📡 [PEER RPC] Node reference injected for dynamic transaction routing");
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
                            // CRITICAL FIX: Update Rust's internal state to match the synced network state
                            // If Rust started with epoch 0 but Go and the Network are at epoch N,
                            // we must update Rust's state before joining consensus, otherwise it starts at epoch 0!
                            {
                                let mut node = self.node.lock().await;
                                if node.current_epoch != status.go_epoch {
                                    info!(
                                        "🔄 [STARTUP] Updating Rust internal epoch {} -> {} (Network Epoch)",
                                        node.current_epoch, status.go_epoch
                                    );
                                    node.current_epoch = status.go_epoch;
                                }
                                if node.last_global_exec_index < status.network_commit {
                                    info!(
                                        "🔄 [STARTUP] Updating Rust internal exec_index {} -> {} (Network Commit)",
                                        node.last_global_exec_index, status.network_commit
                                    );
                                    node.last_global_exec_index = status.network_commit;
                                }
                            }
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
                    Ok(_status) if matches!(*cm.state.read().await, crate::node::catchup::CatchupState::BehindRustLocal { .. }) => {
                        let state = cm.state.read().await.clone();
                        if let crate::node::catchup::CatchupState::BehindRustLocal { target_block, current_block } = state {
                            warn!("🔄 [STARTUP] Go is behind local Rust storage! Fast-forwarding directly from local DB ({} -> {})...", current_block, target_block);
                            let storage_path = std::path::Path::new(&self.node_config.storage_path);
                            match cm.sync_blocks_from_local_rust(storage_path, current_block, target_block).await {
                                Ok(synced) => {
                                    info!("✅ [STARTUP] Local rust fast-forward complete: {} blocks synced directly.", synced);
                                }
                                Err(e) => {
                                    warn!("⚠️ [STARTUP] Local rust fast-forward failed: {}. Will retry or fallback...", e);
                                }
                            }
                        }
                    }
                    Ok(status) if status.block_gap > 10 => {
                        // ═══════════════════════════════════════════════════════
                        // SNAPSHOT RESTORE BLOCK GAP FIX: Epoch matches, but Go
                        // is behind network by >10 blocks (e.g., snapshot at 3050
                        // but network at 3255). Without syncing these blocks from
                        // peers FIRST, the Rust commit processor will ask Go for
                        // blocks that don't exist yet, and block sync keeps
                        // requesting from Go Master returning 0 blocks forever.
                        // ═══════════════════════════════════════════════════════
                        warn!(
                            "🔄 [STARTUP] Epoch matches but Go is behind by {} blocks (Go={}, Network={}). Syncing gap from peers...",
                            status.block_gap, status.go_last_block, status.network_block_height
                        );
                        let mut current_go_block = status.go_last_block;
                        let target = status.network_block_height;
                        let mut total_synced = 0u64;
                        while current_go_block < target {
                            let fetch_to = std::cmp::min(current_go_block + 50, target);
                            match cm.sync_blocks_from_peers(current_go_block, fetch_to).await {
                                Ok(synced) => {
                                    if synced == 0 { break; }
                                    current_go_block += synced;
                                    total_synced += synced;
                                }
                                Err(e) => {
                                    warn!("⚠️ [STARTUP] Block gap sync failed: {}. Proceeding — commit processor will retry.", e);
                                    break;
                                }
                            }
                        }
                        info!(
                            "✅ [STARTUP] Block gap sync complete: {} blocks synced. Go should be near block {}.",
                            total_synced, current_go_block
                        );
                    }
                    Ok(_) => {
                        info!("✅ [STARTUP] Epoch matches network and block gap is small. Consensus syncing will handle any minor lag.");
                    }
                    Err(e) => {
                        warn!("⚠️ [STARTUP] Epoch check failed: {}. Proceeding anyway.", e);
                    }
                }
            }

            // Restore Validator mode by triggering a mode-only transition
            let was_syncing_up = {
                let node_guard = self.node.lock().await;
                node_guard.node_mode == crate::node::NodeMode::SyncingUp
            };

            if was_syncing_up {
                // Check if this is a cold-start restore
                let is_cold_start = {
                    let node_guard = self.node.lock().await;
                    node_guard.cold_start
                };

                if is_cold_start {
                    // ═══════════════════════════════════════════════════════
                    // COLD-START RESTORE: Sync blocks from peers FIRST.
                    //
                    // CRITICAL: ConsensusAuthority CANNOT start with a fresh
                    // DAG — it creates genesis blocks with incompatible hashes.
                    // All subsequent commits would produce different blocks
                    // than the network (different timestamps → hash fork).
                    //
                    // Flow: Phase 1 (peer sync) → Phase 2 (set exec_index) →
                    //       Phase 3 (start authority) → Phase 4 (DualStream overlap)
                    // ═══════════════════════════════════════════════════════
                    warn!(
                        "🚨 [COLD-START] DAG was wiped (snapshot restore). Syncing blocks from peers \
                         FIRST, then joining consensus with correct exec_index."
                    );

                    let executor_client_opt = self.node.lock().await.executor_client.clone();
                    if let Some(executor_client) = executor_client_opt {
                        let catchup_manager = crate::node::catchup::CatchupManager::new(
                            executor_client.clone(),
                            self.node_config.executor_receive_socket_path.clone(),
                            self.node_config.peer_rpc_addresses.clone(),
                        );

                        let mut last_log_block: u64 = 0;

                        // Phase 1: Sync Go blocks from peers until caught up
                        info!("📋 [COLD-START] Phase 1: Syncing Go blocks from peers...");
                        loop {
                            let go_block = match executor_client.get_last_block_number().await {
                                Ok((b, is_ready)) => {
                                    if !is_ready {
                                        info!("⏳ [COLD-START] Go Master DB is not yet ready. Waiting...");
                                        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                                        continue;
                                    }
                                    b
                                }
                                Err(_) => {
                                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                                    continue;
                                }
                            };

                            match crate::network::peer_rpc::query_peer_epochs_network(
                                &self.node_config.peer_rpc_addresses
                            ).await {
                                Ok((net_epoch, net_block, _peer, net_commit)) => {
                                    if go_block < net_block {
                                        let gap = net_block - go_block;
                                        if go_block != last_log_block {
                                            info!(
                                                "🔄 [COLD-START SYNC] Go={}, Network={}, Gap={} blocks. Fetching...",
                                                go_block, net_block, gap
                                            );
                                            last_log_block = go_block;
                                        }
                                        let fetch_to = std::cmp::min(go_block + 500, net_block);
                                        match catchup_manager.sync_blocks_from_peers(go_block, fetch_to).await {
                                            Ok(synced) if synced > 0 => {
                                                info!(
                                                    "✅ [COLD-START SYNC] Synced {} blocks (Go now at ~{})",
                                                    synced, go_block + synced
                                                );
                                                if gap > 100 { continue; }
                                            }
                                            _ => {}
                                        }

                                        // Caught up when gap is small
                                        if gap <= 5 {
                                            info!(
                                                "✅ [COLD-START] Phase 1 complete! Go={}, Network block={}, commit={}. \
                                                 Joining consensus...",
                                                go_block, net_block, net_commit
                                            );
                                            // Phase 2: Set exec_index to network's current commit
                                            {
                                                let mut node_guard = self.node.lock().await;
                                                let old_exec = node_guard.last_global_exec_index;
                                                node_guard.last_global_exec_index = net_commit;
                                                node_guard.current_epoch = net_epoch;
                                                // NOTE: Do NOT reset cold_start here!
                                                // mode_transition.rs needs cold_start=true to enable
                                                // amnesia recovery (FetchOwnLastBlock). It will be
                                                // reset to false after ConsensusAuthority starts.
                                                info!(
                                                    "📋 [COLD-START] Phase 2: Updated exec_index {} → {} (network commit). \
                                                     Epoch set to {}. cold_start remains true for amnesia recovery.",
                                                    old_exec, net_commit, net_epoch
                                                );
                                            }
                                            break;
                                        }
                                    } else {
                                        info!(
                                            "✅ [COLD-START] Go={} already caught up with Network={}. commit={}",
                                            go_block, net_block, net_commit
                                        );
                                        {
                                            let mut node_guard = self.node.lock().await;
                                            let old_exec = node_guard.last_global_exec_index;
                                            node_guard.last_global_exec_index = net_commit;
                                            node_guard.current_epoch = net_epoch;
                                            // NOTE: Do NOT reset cold_start here!
                                            // mode_transition.rs needs cold_start=true for amnesia recovery.
                                            info!(
                                                "📋 [COLD-START] Updated exec_index {} → {} (network commit). Epoch={}. \
                                                 cold_start remains true for amnesia recovery.",
                                                old_exec, net_commit, net_epoch
                                            );
                                        }
                                        break;
                                    }
                                }
                                Err(e) => {
                                    warn!("⚠️ [COLD-START SYNC] Peer query failed: {}", e);
                                }
                            }
                            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                        }
                    }
                }

                // Phase 3: Start ConsensusAuthority
                info!("✅ [STARTUP] Catch-up complete. Starting ConsensusAuthority for Validator...");
                let (new_epoch, new_exec_index) = {
                    let node_guard = self.node.lock().await;
                    (node_guard.current_epoch, node_guard.last_global_exec_index)
                };
                let mut node_guard = self.node.lock().await;
                
                if let Err(e) = crate::node::transition::mode_transition::transition_mode_only(
                    &mut *node_guard,
                    new_epoch,
                    0,
                    new_exec_index,
                    &self.node_config,
                ).await {
                    error!("❌ [STARTUP] Failed to transition to Validator mode: {}", e);
                    node_guard.node_mode = crate::node::NodeMode::Validator;
                } else {
                    info!("✅ [STARTUP] Node is now a Validator at epoch {}! exec_index={}", new_epoch, new_exec_index);
                }
                
                let executor_client_opt = node_guard.executor_client.clone();
                drop(node_guard);

                // ═══════════════════════════════════════════════════════════════
                // Phase 4: DualStreamController for overlap period
                //
                // ConsensusAuthority just started and will begin delivering
                // blocks via CommitProcessor. Meanwhile, the network may have
                // advanced further during our Phase 1 sync. DualStreamController
                // runs peer sync in background to fill any remaining gap.
                //
                // CRITICAL: Skip Phase 4 when cold_start! After snapshot restore,
                // the commit processor replays ALL old DAG commits in the correct
                // consensus-determined order. Starting peer sync simultaneously
                // creates DUAL DELIVERY → Go receives blocks from two sources →
                // different execution order → stateRoot divergence → FORK!
                //
                // The commit processor alone is the authoritative source.
                // Convergence: When consensus delivers blocks for 10 consecutive
                // rounds without peer sync contributing, peer sync stops.
                // ═══════════════════════════════════════════════════════════════
                if is_cold_start {
                    info!(
                        "⏭️ [STARTUP] Phase 4: SKIPPING DualStreamController (cold_start=true). \
                         Commit processor is the sole block source to prevent fork from dual delivery."
                    );
                } else if let Some(executor_client) = executor_client_opt {
                    let peer_addrs = self.node_config.peer_rpc_addresses.clone();
                    if !peer_addrs.is_empty() {
                        info!("🔄 [STARTUP] Phase 4: Spawning DualStreamController for consensus/peer overlap...");
                        let controller = crate::node::dual_stream::DualStreamController::new(
                            executor_client,
                            peer_addrs,
                        );
                        let _dual_stream_handle = controller.spawn_block_sync_stream();
                    }
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
