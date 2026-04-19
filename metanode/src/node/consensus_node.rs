// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

//! ConsensusNode constructors.
//!
//! Contains the struct definition and all `new*` constructors for ConsensusNode.
//! The main constructor delegates to 4 helper functions:
//! - `setup_storage()` — executor, epoch discovery, committee, execution index, identity
//! - `setup_consensus()` — commit processor, params, authority start
//! - `setup_networking()` — NTP/clock sync
//! - `setup_epoch_management()` — transition manager, monitor, sync task

use crate::consensus::clock_sync::ClockSyncManager;
use crate::types::transaction::NoopTransactionVerifier;
use anyhow::Result;
use consensus_config::AuthorityIndex;
use consensus_core::{
    Clock, CommitConsumerArgs, ConsensusAuthority, DefaultSystemTransactionProvider, NetworkType,
    ReconfigState, SystemTransactionProvider,
};
use meta_protocol_config::ProtocolConfig;
use prometheus::Registry;
use std::sync::atomic::{AtomicBool, AtomicU32};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::config::NodeConfig;
use crate::node::epoch_store::load_legacy_epoch_stores;
use crate::node::executor_client::ExecutorClient;
use crate::node::tx_submitter::TransactionClientProxy;

use super::{detect_local_epoch, ConsensusNode, NodeMode};
use super::{epoch_monitor, epoch_transition_manager, recovery};

// ---------------------------------------------------------------------------
// Intermediate structs for passing data between setup phases
// ---------------------------------------------------------------------------

/// Results from storage/epoch initialization phase.
struct StorageSetup {
    current_epoch: u64,
    epoch_timestamp_ms: u64,
    committee: consensus_config::Committee,
    validator_eth_addresses: Vec<Vec<u8>>,
    own_index: AuthorityIndex,
    is_in_committee: bool,
    last_global_exec_index: u64,
    epoch_base_exec_index: u64,
    storage_path: std::path::PathBuf,
    protocol_keypair: consensus_config::ProtocolKeyPair,
    network_keypair: consensus_config::NetworkKeyPair,
    /// Epoch duration in seconds, loaded from Go via protobuf (from genesis.json)
    epoch_duration_from_go: u64,
}

/// Results from consensus setup phase.
struct ConsensusSetup {
    authority: Option<ConsensusAuthority>,
    /// Whether DAG storage has prior history. False after snapshot restore (DAG deleted).
    #[allow(dead_code)]
    dag_has_history: bool,
    commit_consumer_holder: Option<CommitConsumerArgs>,
    transaction_client_proxy: Option<Arc<TransactionClientProxy>>,
    executor_client_for_proc: Arc<ExecutorClient>,
    current_commit_index: Arc<AtomicU32>,
    is_transitioning: Arc<AtomicBool>,
    pending_transactions_queue: Arc<tokio::sync::Mutex<Vec<Vec<u8>>>>,
    committed_transaction_hashes: Arc<tokio::sync::Mutex<std::collections::HashSet<Vec<u8>>>>,
    epoch_tx_sender: tokio::sync::mpsc::UnboundedSender<(u64, u64, u64)>,
    epoch_tx_receiver: tokio::sync::mpsc::UnboundedReceiver<(u64, u64, u64)>,
    shared_last_global_exec_index: Arc<tokio::sync::Mutex<u64>>,
    system_transaction_provider: Arc<DefaultSystemTransactionProvider>,
    protocol_config: ProtocolConfig,
    parameters: consensus_config::Parameters,
    clock: Arc<Clock>,
    transaction_verifier: Arc<NoopTransactionVerifier>,
    /// TX recycler for tracking and re-submitting uncommitted TXs
    tx_recycler: Arc<crate::consensus::tx_recycler::TxRecycler>,
    /// Whether the node should start as a validator immediately.
    start_as_validator: bool,
}

// ---------------------------------------------------------------------------
// Constructor entry points
// ---------------------------------------------------------------------------

impl ConsensusNode {
    #[allow(dead_code)]
    pub async fn new(config: NodeConfig) -> Result<Self> {
        Self::new_with_registry(config, Registry::new()).await
    }

    #[allow(dead_code)]
    pub async fn new_with_registry(config: NodeConfig, registry: Registry) -> Result<Self> {
        Self::new_with_registry_and_service(config, registry).await
    }

    /// Main constructor — delegates to 4 setup helpers.
    pub async fn new_with_registry_and_service(
        config: NodeConfig,
        registry: Registry,
    ) -> Result<Self> {
        info!("Initializing consensus node {}...", config.node_id);

        // Phase 1: Storage, epoch discovery, committee, execution index, identity
        let storage = Self::setup_storage(&config).await?;

        // Phase 2: Consensus params, commit processor, authority start
        let consensus = Self::setup_consensus(&config, &storage, &registry).await?;

        // Phase 3: Clock/NTP sync
        let clock_sync_manager = Self::setup_networking(&config);

        // Phase 4: Assemble node + epoch management (transition manager, monitor, sync)
        let node =
            Self::setup_epoch_management(config, storage, consensus, clock_sync_manager, &registry)
                .await?;

        Ok(node)
    }

    // -----------------------------------------------------------------------
    // Phase 1: setup_storage
    // -----------------------------------------------------------------------

    /// Initializes executor client, discovers epoch from peers/Go, loads committee,
    /// calculates execution index, finds own identity in the committee.
    async fn setup_storage(config: &NodeConfig) -> Result<StorageSetup> {
        info!("🚀 [STARTUP] Loading latest block, epoch and committee from Go state...");

        let executor_client = Arc::new(ExecutorClient::new(
            true,
            false,
            config.executor_send_socket_path.clone(),
            config.executor_receive_socket_path.clone(),
            Some(config.storage_path.clone()),
        ));

        // SNAPSHOT RESTORE FIX: Go Master needs time to load DB after snapshot restore.
        // Without retry, Rust gets block=0/epoch=0 and initializes consensus at epoch 0,
        // making it permanently stuck and unable to catch up with the cluster.
        // Retry up to 30 times (2s intervals = 60s max) waiting for Go to report block > 0.
        let latest_block_number = {
            let max_retries = 30;
            let retry_interval = std::time::Duration::from_secs(2);
            let mut block_num = 0u64;

            for attempt in 1..=max_retries {
                match executor_client.get_last_block_number().await {
                    Ok((n, _, _is_ready)) => {
                        block_num = n;
                        if n > 0 || _is_ready {
                            info!(
                                "✅ [STARTUP] Got block number {} from Go (is_ready={}) (attempt {})",
                                n, _is_ready, attempt
                            );
                            break;
                        } else if attempt < max_retries {
                            info!(
                                "⏳ [STARTUP] Go returned block=0 (attempt {}/{}). Go may still be loading DB after snapshot restore. Retrying in {}s...",
                                attempt, max_retries, retry_interval.as_secs()
                            );
                            tokio::time::sleep(retry_interval).await;
                        }
                    }
                    Err(e) => {
                        if attempt < max_retries {
                            warn!(
                                "⚠️ [STARTUP] Failed to fetch block from Go (attempt {}/{}): {}. Retrying...",
                                attempt, max_retries, e
                            );
                            tokio::time::sleep(retry_interval).await;
                        } else {
                            warn!("⚠️ [STARTUP] Failed to fetch latest block from Go after {} attempts: {}. Attempting to read persisted value.", max_retries, e);
                            block_num = super::executor_client::read_last_block_number(
                                &config.storage_path,
                            )
                            .await
                            .unwrap_or(0);
                        }
                    }
                }
            }

            if block_num == 0 {
                warn!("⚠️ [STARTUP] Go still reporting block=0 after {} retries. This may be a fresh node or Go failed to load snapshot data.", max_retries);
            }

            block_num
        };

        // ═══════════════════════════════════════════════════════════════════════
        // CRITICAL FIX: ALL nodes MUST use local Go epoch, NOT peer epoch!
        // Using peer epoch causes DEADLOCK for nodes recovering from snapshot:
        //   1. Peer says epoch=100 → Rust advances internal state to epoch 100
        //   2. Deferred epoch transition waits for Go GEI >= boundary
        //   3. But Go GEI=0 (snapshot restore) → 120s timeout → DEADLOCK
        // All nodes must sync blocks sequentially: epoch transitions happen naturally
        // when Go processes blocks up to the epoch boundary.
        // ═══════════════════════════════════════════════════════════════════════
        let (go_epoch, peer_last_block, best_socket) = {
            // SNAPSHOT RESTORE FIX: If we already know block > 0 (from retry above),
            // Go should also report epoch > 0. Retry until epoch matches.
            let epoch = if latest_block_number > 0 {
                let max_epoch_retries = 30;
                let retry_interval = std::time::Duration::from_secs(2);
                let mut final_epoch = 0u64;

                for attempt in 1..=max_epoch_retries {
                    match executor_client.get_current_epoch().await {
                        Ok(e) => {
                            final_epoch = e;
                            if e > 0 {
                                info!(
                                    "✅ [STARTUP] Got epoch {} from Go (attempt {}, block={})",
                                    e, attempt, latest_block_number
                                );
                                break;
                            } else if attempt < max_epoch_retries {
                                info!(
                                    "⏳ [STARTUP] Go returned epoch=0 but block={}. DB still loading. Retrying in {}s... (attempt {}/{})",
                                    latest_block_number, retry_interval.as_secs(), attempt, max_epoch_retries
                                );
                                tokio::time::sleep(retry_interval).await;
                            }
                        }
                        Err(e) => {
                            if attempt < max_epoch_retries {
                                warn!(
                                    "⚠️ [STARTUP] Failed to get epoch (attempt {}/{}): {}. Retrying...",
                                    attempt, max_epoch_retries, e
                                );
                                tokio::time::sleep(retry_interval).await;
                            } else {
                                return Err(anyhow::anyhow!(
                                    "Failed to fetch epoch from Go after {} attempts: {}",
                                    max_epoch_retries,
                                    e
                                ));
                            }
                        }
                    }
                }

                if final_epoch == 0 && latest_block_number > 0 {
                    warn!(
                        "⚠️ [STARTUP] Go still reporting epoch=0 despite block={}. Snapshot data may not have loaded correctly.",
                        latest_block_number
                    );
                }
                final_epoch
            } else {
                executor_client
                    .get_current_epoch()
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to fetch current epoch from Go: {}", e))?
            };

            info!(
                "📋 [STARTUP] Using LOCAL Go epoch {} (skipping peer discovery to prevent deadlock)",
                epoch
            );
            (
                epoch,
                latest_block_number,
                config.executor_receive_socket_path.clone(),
            )
        };

        info!(
            "📊 [STARTUP] Go State: Block {}, Epoch {} (peer_block={})",
            latest_block_number, go_epoch, peer_last_block
        );

        // CATCHUP: Check if we need to sync epoch from local storage
        let storage_path = config.storage_path.clone();
        let local_epoch = detect_local_epoch(&storage_path);

        let current_epoch = if local_epoch < go_epoch {
            warn!(
                "🔄 [CATCHUP] Epoch mismatch detected: local={}, go={}. Syncing to epoch {}.",
                local_epoch, go_epoch, go_epoch
            );
            if config.epochs_to_keep > 0 {
                // Smart cleanup: only delete epochs older than epochs_to_keep
                // Keep recent epochs so THIS node can serve historical data to lagging peers
                let keep_from = go_epoch.saturating_sub(config.epochs_to_keep as u64);
                let epochs_dir = storage_path.join("epochs");
                if epochs_dir.exists() {
                    if let Ok(entries) = std::fs::read_dir(&epochs_dir) {
                        for entry in entries.flatten() {
                            if let Some(name) = entry.file_name().to_str() {
                                if let Some(epoch_str) = name.strip_prefix("epoch_") {
                                    if let Ok(epoch) = epoch_str.parse::<u64>() {
                                        if epoch < keep_from {
                                            info!("🗑️ [CATCHUP] Removing old epoch {} data (older than keep_from={})", epoch, keep_from);
                                            let _ = std::fs::remove_dir_all(entry.path());
                                        } else {
                                            info!("📦 [CATCHUP] Keeping epoch {} data for sync support", epoch);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            } else {
                info!("📦 [CATCHUP] Archive mode (epochs_to_keep=0): keeping all epoch data");
            }
            go_epoch
        } else if local_epoch > go_epoch {
            warn!(
                "🚨 [CATCHUP] Local epoch {} is AHEAD of network epoch {}! Detect stale chain.",
                local_epoch, go_epoch
            );
            warn!("🗑️ [CATCHUP] Clearing ALL local epochs to resync with network.");
            if let Ok(entries) = std::fs::read_dir(storage_path.join("epochs")) {
                for entry in entries.flatten() {
                    if let Ok(path) = entry.path().canonicalize() {
                        info!("🗑️ [CATCHUP] Removing {:?}", path);
                        let _ = std::fs::remove_dir_all(path);
                    }
                }
            }
            go_epoch
        } else {
            go_epoch
        };

        info!(
            "📊 [STARTUP] Using epoch {} (synced with Go)",
            current_epoch
        );

        // Fetch committee from the best Go Master source
        let peer_executor_client = if best_socket != config.executor_receive_socket_path {
            info!(
                "🔄 [PEER SYNC] Using peer Go Master {} for validators (has correct epoch {})",
                best_socket, go_epoch
            );
            Arc::new(ExecutorClient::new(
                true,
                false,
                String::new(),
                best_socket.clone(),
                None,
            ))
        } else {
            executor_client.clone()
        };

        let (
            current_epoch,
            epoch_timestamp_ms,
            boundary_block,
            validators,
            epoch_duration_from_go,
            boundary_gei,
        ) = match peer_executor_client
            .get_epoch_boundary_data(current_epoch)
            .await
        {
            Ok((epoch, timestamp, boundary_blk, vals, epoch_dur, boundary_gei_val)) => {
                info!(
                        "✅ [STARTUP] Got epoch boundary data for epoch {} from Go (epoch_duration={}s, boundary_gei={})",
                        epoch, epoch_dur, boundary_gei_val
                    );
                (
                    epoch,
                    timestamp,
                    boundary_blk,
                    vals,
                    epoch_dur,
                    boundary_gei_val,
                )
            }
            Err(e) => {
                warn!(
                        "⚠️ [STARTUP] Failed to get epoch boundary for epoch {}: {}. Trying fallbacks...",
                        current_epoch, e
                    );

                // SNAPSHOT RESTORE FIX (2026-03-19):
                // After snapshot restore, Go may have stale epoch data (epoch=0) while
                // peers are at epoch N. Instead of falling back to local Go's stale epoch,
                // query peers FIRST for epoch boundary data.
                let local_epoch = executor_client.get_current_epoch().await.unwrap_or(0);

                if local_epoch < current_epoch && !config.peer_rpc_addresses.is_empty() {
                    warn!(
                            "🔄 [STARTUP] Local Go epoch {} < peer epoch {}. Go may have stale data (snapshot restore?). Querying peers for epoch boundary...",
                            local_epoch, current_epoch
                        );

                    // Try each peer for epoch boundary data at the CORRECT (peer) epoch
                    let mut peer_boundary = None;
                    for peer_addr in &config.peer_rpc_addresses {
                        match crate::network::peer_rpc::query_peer_epoch_boundary_data(
                            peer_addr,
                            current_epoch,
                        )
                        .await
                        {
                            Ok(boundary) => {
                                info!(
                                        "✅ [STARTUP] Got epoch {} boundary from peer {}: {} validators, boundary_block={}, boundary_gei={}",
                                        current_epoch, peer_addr, boundary.validators.len(), boundary.boundary_block, boundary.boundary_gei
                                    );
                                peer_boundary = Some(boundary);
                                break;
                            }
                            Err(pe) => {
                                warn!(
                                    "⚠️ [STARTUP] Peer {} epoch {} boundary failed: {}",
                                    peer_addr, current_epoch, pe
                                );
                            }
                        }
                    }

                    if let Some(boundary) = peer_boundary {
                        use super::executor_client::proto::ValidatorInfo as ProtoVI;
                        let validators: Vec<ProtoVI> = boundary
                            .validators
                            .into_iter()
                            .map(|v| ProtoVI {
                                address: v.address,
                                stake: v.stake.to_string(),
                                name: v.name,
                                authority_key: v.authority_key,
                                protocol_key: v.protocol_key,
                                network_key: v.network_key,
                                description: String::new(),
                                website: String::new(),
                                image: String::new(),
                                commission_rate: 0,
                                min_self_delegation: String::new(),
                                accumulated_rewards_per_share: String::new(),
                                p2p_address: String::new(),
                            })
                            .collect();
                        (
                            current_epoch,
                            boundary.timestamp_ms,
                            boundary.boundary_block,
                            validators,
                            900u64,
                            boundary.boundary_gei,
                        )
                    } else {
                        warn!("⚠️ [STARTUP] No peers returned epoch {} boundary. Falling back to local Go epoch {}.", current_epoch, local_epoch);
                        // Fall through to local Go fallback below
                        match executor_client.get_epoch_boundary_data(local_epoch).await {
                            Ok((
                                epoch,
                                timestamp,
                                boundary_blk,
                                vals,
                                epoch_dur,
                                boundary_gei_val,
                            )) => (
                                epoch,
                                timestamp,
                                boundary_blk,
                                vals,
                                epoch_dur,
                                boundary_gei_val,
                            ),
                            Err(e2) => {
                                return Err(anyhow::anyhow!(
                                        "Failed to get epoch boundary from peers AND local Go. Peer epoch={} error: {}, Local epoch={} error: {}",
                                        current_epoch, e, local_epoch, e2
                                    ));
                            }
                        }
                    }
                } else {
                    // Local epoch matches or no peers — use local Go
                    info!(
                        "📊 [STARTUP] Using local Go epoch {} for boundary data",
                        local_epoch
                    );

                    match executor_client.get_epoch_boundary_data(local_epoch).await {
                        Ok((epoch, timestamp, boundary_blk, vals, epoch_dur, boundary_gei_val)) => {
                            info!(
                                    "✅ [STARTUP] Got epoch boundary data for local epoch {} (epoch_duration={}s, boundary_gei={})",
                                    epoch, epoch_dur, boundary_gei_val
                                );
                            (
                                epoch,
                                timestamp,
                                boundary_blk,
                                vals,
                                epoch_dur,
                                boundary_gei_val,
                            )
                        }
                        Err(e2) => {
                            warn!(
                                    "⚠️ [STARTUP] No epoch boundary available (local epoch {} error: {}). Trying genesis validators...",
                                    local_epoch, e2
                                );
                            // Try local Go first
                            match executor_client.get_validators_at_block(0).await {
                                Ok((genesis_validators, _genesis_epoch, _)) => {
                                    (0u64, 0u64, 0u64, genesis_validators, 900u64, 0u64)
                                }
                                Err(e3) => {
                                    // LOCAL GO FAILED — query peers for epoch 0
                                    warn!(
                                            "⚠️ [STARTUP] Local Go genesis validators failed: {}. Querying peers...",
                                            e3
                                        );
                                    if !config.peer_rpc_addresses.is_empty() {
                                        let mut peer_validators = None;
                                        for peer_addr in &config.peer_rpc_addresses {
                                            match crate::network::peer_rpc::query_peer_epoch_boundary_data(
                                                    peer_addr, 0,
                                                ).await {
                                                    Ok(boundary) => {
                                                        info!(
                                                            "✅ [STARTUP] Got epoch 0 boundary from peer {}: {} validators",
                                                            peer_addr, boundary.validators.len()
                                                        );
                                                        peer_validators = Some(boundary);
                                                        break;
                                                    }
                                                    Err(pe) => {
                                                        warn!("⚠️ [STARTUP] Peer {} epoch 0 boundary failed: {}", peer_addr, pe);
                                                    }
                                                }
                                        }
                                        if let Some(boundary) = peer_validators {
                                            use super::executor_client::proto::ValidatorInfo as ProtoVI;
                                            let validators: Vec<ProtoVI> = boundary
                                                .validators
                                                .into_iter()
                                                .map(|v| ProtoVI {
                                                    address: v.address,
                                                    stake: v.stake.to_string(),
                                                    name: v.name,
                                                    authority_key: v.authority_key,
                                                    protocol_key: v.protocol_key,
                                                    network_key: v.network_key,
                                                    description: String::new(),
                                                    website: String::new(),
                                                    image: String::new(),
                                                    commission_rate: 0,
                                                    min_self_delegation: String::new(),
                                                    accumulated_rewards_per_share: String::new(),
                                                    p2p_address: String::new(),
                                                })
                                                .collect();
                                            (
                                                0u64,
                                                boundary.timestamp_ms,
                                                boundary.boundary_block,
                                                validators,
                                                900u64,
                                                boundary.boundary_gei,
                                            )
                                        } else {
                                            return Err(anyhow::anyhow!(
                                                    "Failed to fetch genesis validators from both local Go and peers. Local: {}, No peers returned data.",
                                                    e3
                                                ));
                                        }
                                    } else {
                                        return Err(anyhow::anyhow!(
                                                "Failed to fetch genesis validators: {} (no peers configured for fallback)",
                                                e3
                                            ));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        };

        info!(
            "📊 [STARTUP] Using epoch boundary data: epoch={}, boundary_block={}, epoch_timestamp={}ms, validators={}, boundary_gei={}",
            current_epoch, boundary_block, epoch_timestamp_ms, validators.len(), boundary_gei
        );

        if validators.is_empty() {
            anyhow::bail!("Go state returned empty validators list at epoch boundary");
        }

        // Filter validators for single node debug if needed
        let validators_to_use = if std::env::var("SINGLE_NODE_DEBUG").is_ok() {
            info!("🔧 SINGLE_NODE_DEBUG: Using only node 0");
            validators
                .into_iter()
                .filter(|v| v.name == "node-0")
                .collect::<Vec<_>>()
        } else {
            validators
        };

        let (committee, validator_eth_addresses) =
            super::committee::build_committee_with_eth_addresses(validators_to_use, current_epoch)?;
        info!(
            "✅ Loaded committee with {} authorities and {} eth_addresses (from epoch boundary)",
            committee.size(),
            validator_eth_addresses.len()
        );

        // EXECUTION INDEX SYNC
        let last_global_exec_index = Self::calculate_last_global_exec_index(
            config,
            &executor_client,
            &best_socket,
            peer_last_block,
        )
        .await;

        if last_global_exec_index > 100000 {
            warn!(
                "⚠️ [STARTUP] Very high last_global_exec_index={} - this is normal for long-running chains. Trusting Go's value.",
                last_global_exec_index
            );
        }

        // ═══════════════════════════════════════════════════════════════════════════
        // FORK-SAFETY: Validate boundary_gei before using it as epoch_base_index.
        // After snapshot restore, Go's epoch_data_backup.json may have boundary_gei=0
        // for epoch>0 because the epoch transition wasn't captured in the snapshot.
        // Using boundary_gei=0 causes wrong GEI calculation → block hash divergence.
        //
        // CRITICAL FIX (2026-04-15): Also validate when DAG storage is empty (cold_start).
        // After snapshot restore, Go may have non-zero but STALE boundary_gei that doesn't
        // match the network. This causes epoch_base_index to be wrong → global_exec_index
        // calculation diverges → FORK.
        //
        // Fix: Validate from peers if:
        //   1. boundary_gei == 0 && epoch > 0 (original fix), OR
        //   2. DAG storage is empty (cold_start) - indicates snapshot restore
        // ═══════════════════════════════════════════════════════════════════════════
        let epoch_db_path = config
            .storage_path
            .join("epochs")
            .join(format!("epoch_{}", current_epoch))
            .join("consensus_db");
        let dag_has_history = epoch_db_path.exists()
            && std::fs::read_dir(&epoch_db_path)
                .map(|mut entries| entries.next().is_some())
                .unwrap_or(false);

        let epoch_base_exec_index = if (boundary_gei == 0 && current_epoch > 0)
            || (!dag_has_history && current_epoch > 0)
        {
            let force_peer_check = !dag_has_history && current_epoch > 0;
            if force_peer_check {
                warn!(
                    "⚠️ [FORK-SAFETY] Cold-start detected (empty DAG). Validating boundary_gei={} from peers for epoch {}...",
                    boundary_gei, current_epoch
                );
            }
            let (_, _, _, _, _, safe_gei) = executor_client
                .get_safe_epoch_boundary_data_with_force(
                    current_epoch,
                    &config.peer_rpc_addresses,
                    force_peer_check,
                )
                .await?;
            if safe_gei != boundary_gei {
                warn!(
                    "🔄 [FORK-SAFETY] Corrected boundary_gei: {} → {} (from peers)",
                    boundary_gei, safe_gei
                );
            }
            safe_gei
        } else {
            boundary_gei
        };
        info!(
            "✅ [STARTUP] Using epoch_base={} from Go boundary_gei (epoch={}, boundary_block={})",
            epoch_base_exec_index, current_epoch, boundary_block
        );

        // Recovery check
        if config.executor_read_enabled && last_global_exec_index > 0 {
            recovery::perform_block_recovery_check(
                &executor_client,
                last_global_exec_index,
                epoch_base_exec_index,
                current_epoch,
                &config.storage_path,
                config.node_id as u32,
            )
            .await?;
        }

        let protocol_keypair = config.load_protocol_keypair()?;
        let network_keypair = config.load_network_keypair()?;

        // Identity: find own index in committee
        let own_protocol_pubkey = protocol_keypair.public();
        let own_index_opt = committee.authorities().find_map(|(idx, auth)| {
            if auth.protocol_key == own_protocol_pubkey {
                Some(idx)
            } else {
                None
            }
        });

        let is_in_committee = own_index_opt.is_some();
        let own_index = own_index_opt.unwrap_or(AuthorityIndex::ZERO);

        if is_in_committee {
            info!(
                "✅ [IDENTITY] Found self in committee at index {} using protocol_key match",
                own_index
            );
        } else {
            info!(
                "ℹ️ [IDENTITY] Not in committee (protocol_key not found in {} authorities)",
                committee.size()
            );
        }

        std::fs::create_dir_all(&config.storage_path)?;

        Ok(StorageSetup {
            current_epoch,
            epoch_timestamp_ms,
            committee,
            validator_eth_addresses,
            own_index,
            is_in_committee,
            last_global_exec_index,
            epoch_base_exec_index,
            storage_path,
            protocol_keypair,
            network_keypair,
            epoch_duration_from_go,
        })
    }

    /// Determines the effective last global execution index from local Go, peers, and persisted state.
    async fn calculate_last_global_exec_index(
        config: &NodeConfig,
        executor_client: &Arc<ExecutorClient>,
        best_socket: &str,
        peer_last_block: u64,
    ) -> u64 {
        if !config.executor_read_enabled {
            return 0;
        }

        let (local_go_block, _, _go_ready) = executor_client
            .get_last_block_number()
            .await
            .unwrap_or((0, 0, false));
        let local_go_gei = executor_client
            .get_last_global_exec_index()
            .await
            .unwrap_or(0);
        let storage_path = &config.storage_path;

        let (persisted_index, persisted_commit) =
            super::executor_client::load_persisted_last_index(storage_path).unwrap_or((0, 0));

        let peer_last_block =
            if best_socket != config.executor_receive_socket_path && peer_last_block > 0 {
                peer_last_block
            } else {
                0
            };

        if peer_last_block > 0 {
            info!(
                "📊 [STARTUP] Sync Check: LocalGoBlock={}, PeerBlock={}, PersistedGEI=({}, commit={}) (from {})",
                local_go_block, peer_last_block, persisted_index, persisted_commit, best_socket
            );

            let sources_match =
                local_go_block == peer_last_block || local_go_block.abs_diff(peer_last_block) <= 5;
            if !sources_match {
                warn!("⚠️ [STARTUP] INDEX DISCREPANCY DETECTED:");
                warn!(
                    "   LocalGoBlock={}, PeerBlock={}, PersistedGEI={}, LocalGEI={}",
                    local_go_block, peer_last_block, persisted_index, local_go_gei
                );
                warn!("   This may indicate network partition or stale data.");
            }

            if local_go_block > peer_last_block + 5 {
                warn!("🚨 [STARTUP] STALE CHAIN DETECTED: Local ({}) is ahead of Peer ({})! Forcing resync from Peer.", 
                       local_go_block, peer_last_block);
                // In recovery we just use the local GEI anyway because Go Master blocks handles actual rollback if needed
                local_go_gei
            } else if local_go_block < peer_last_block.saturating_sub(5) {
                let lag = peer_last_block - local_go_block;
                info!(
                    "ℹ️ [STARTUP] Local Go Master ({}) is behind Peer ({}) by {} blocks. Using Local {} to trigger recovery/backfill.",
                    local_go_block, peer_last_block, lag, local_go_block
                );
                // Flag as lagging if behind by more than 50 blocks
                local_go_gei
            } else {
                info!(
                    "✅ [STARTUP] Local and Peer are in sync (LocalBlock={}, PeerBlock={}). Using Local Go GEI: {} as authoritative.",
                    local_go_block, peer_last_block, local_go_gei
                );
                local_go_gei
            }
        } else {
            if persisted_index > local_go_gei {
                warn!("⚠️ [STARTUP] Persisted Index (GEI) {} > Local Go GEI {}. Go is behind (possible rollback/crash). Using Local Go GEI {} to force resync/replay.", 
                    persisted_index, local_go_gei, local_go_gei);
            }
            info!(
                "📊 [STARTUP] No peer reference, using Local Go Last GEI: {} (Block: {})",
                local_go_gei, local_go_block
            );
            local_go_gei
        }
    }

    // -----------------------------------------------------------------------
    // Phase 2: setup_consensus
    // -----------------------------------------------------------------------

    /// Builds the commit processor, consensus parameters, starts authority (or SyncOnly holder),
    /// and wires up all the shared state.
    async fn setup_consensus(
        config: &NodeConfig,
        storage: &StorageSetup,
        registry: &Registry,
    ) -> Result<ConsensusSetup> {
        let clock = Arc::new(Clock::default());
        let transaction_verifier = Arc::new(NoopTransactionVerifier);
        let (commit_consumer, commit_receiver, mut block_receiver) = CommitConsumerArgs::new(0, 0);
        let current_commit_index = Arc::new(AtomicU32::new(0));
        let is_transitioning = Arc::new(AtomicBool::new(false));

        // Load persisted transaction queue
        let persisted_queue = super::queue::load_transaction_queue_static(&storage.storage_path)
            .await
            .unwrap_or_default();
        if !persisted_queue.is_empty() {
            info!("💾 Loaded {} persisted transactions", persisted_queue.len());
        }
        let pending_transactions_queue = Arc::new(tokio::sync::Mutex::new(persisted_queue));

        // Load committed transaction hashes from current epoch for duplicate prevention
        let committed_hashes = crate::node::transition::load_committed_transaction_hashes(
            &storage.storage_path,
            storage.current_epoch,
        )
        .await;
        if !committed_hashes.is_empty() {
            info!(
                "💾 Loaded {} committed transaction hashes from epoch {}",
                committed_hashes.len(),
                storage.current_epoch
            );
        }
        let committed_transaction_hashes = Arc::new(tokio::sync::Mutex::new(committed_hashes));

        let (epoch_tx_sender, epoch_tx_receiver) =
            tokio::sync::mpsc::unbounded_channel::<(u64, u64, u64)>();
        let epoch_transition_callback =
            crate::consensus::commit_callbacks::create_epoch_transition_callback(
                epoch_tx_sender.clone(),
            );

        let shared_last_global_exec_index =
            Arc::new(tokio::sync::Mutex::new(storage.epoch_base_exec_index));

        // ═══════════════════════════════════════════════════════════════════
        // Detect empty DAG (snapshot restore) for startup logging and
        // boundary GEI validation. No longer used for cold_start guard
        // since Phase 1 ensures Go's GEI is always accurate.
        // ═══════════════════════════════════════════════════════════════════
        let dag_has_history = {
            let epoch_db = config
                .storage_path
                .join("epochs")
                .join(format!("epoch_{}", storage.current_epoch))
                .join("consensus_db");
            epoch_db.exists()
                && std::fs::read_dir(&epoch_db)
                    .map(|mut entries| entries.next().is_some())
                    .unwrap_or(false)
        };
        if !dag_has_history && storage.is_in_committee && storage.current_epoch > 0 {
            warn!(
                "⚠️ [FORK-SAFETY] DAG storage empty for epoch {} — snapshot restore detected. \
                 GEI guard in executor will skip commits Go has already executed.",
                storage.current_epoch
            );
        }

        let mut commit_processor = crate::consensus::commit_processor::CommitProcessor::new(
            commit_receiver,
        )
        .with_commit_index_callback(
            crate::consensus::commit_callbacks::create_commit_index_callback(
                current_commit_index.clone(),
            ),
        )
        .with_global_exec_index_callback(
            crate::consensus::commit_callbacks::create_global_exec_index_callback(
                shared_last_global_exec_index.clone(),
            ),
        )
        .with_get_last_global_exec_index({
            let shared_index = shared_last_global_exec_index.clone();
            move || {
                if let Ok(_rt) = tokio::runtime::Handle::try_current() {
                    warn!("⚠️ get_last_global_exec_index called from async context, returning 0.");
                    0
                } else {
                    let shared_index_clone = shared_index.clone();
                    futures::executor::block_on(async { *shared_index_clone.lock().await })
                }
            }
        })
        .with_shared_last_global_exec_index(shared_last_global_exec_index.clone())
        .with_epoch_info(storage.current_epoch, storage.epoch_base_exec_index)
        .with_is_transitioning(is_transitioning.clone())
        .with_pending_transactions_queue(pending_transactions_queue.clone())
        .with_epoch_transition_callback(epoch_transition_callback)
        .with_epoch_eth_addresses({
            let mut map = std::collections::HashMap::new();
            map.insert(
                storage.current_epoch,
                storage.validator_eth_addresses.clone(),
            );
            Arc::new(tokio::sync::RwLock::new(map))
        })
        .with_storage_path(config.storage_path.clone());

        // ExecutorClient for commit processing
        let initial_next_expected = if config.executor_read_enabled {
            storage.last_global_exec_index + 1
        } else {
            1
        };

        // MOVED UP: Create system_transaction_provider BEFORE executor_client_for_proc
        // so we can wire the go_lag_handle for backpressure
        let epoch_duration_seconds = storage.epoch_duration_from_go;
        let system_transaction_provider = Arc::new(DefaultSystemTransactionProvider::new(
            storage.current_epoch,
            epoch_duration_seconds,
            storage.epoch_timestamp_ms,
            config.time_based_epoch_change,
        ));

        let executor_client_for_proc = if config.executor_read_enabled {
            let mut client = ExecutorClient::new_with_initial_index(
                true,
                config.executor_commit_enabled,
                config.executor_send_socket_path.clone(),
                config.executor_receive_socket_path.clone(),
                initial_next_expected,
                Some(config.storage_path.clone()),
            );
            // BACKPRESSURE: Wire go_lag_handle from SystemTransactionProvider into ExecutorClient
            // This creates the feedback loop: flush_buffer() computes lag → updates go_lag
            // → SystemTransactionProvider checks go_lag before emitting EndOfEpoch
            client.set_go_lag_handle(system_transaction_provider.go_lag_handle());
            Arc::new(client)
        } else {
            Arc::new(ExecutorClient::new(
                false,
                false,
                "".to_string(),
                "".to_string(),
                None,
            ))
        };

        let executor_client_for_init = executor_client_for_proc.clone();
        tokio::spawn(async move {
            executor_client_for_init.initialize_from_go().await;
        });

        // ═══════════════════════════════════════════════════════════════════════════
        // SYNC-FIRST BARRIER (Phase 1 — Snapshot Restore Redesign)
        //
        // PROBLEM: After snapshot restore, Go is at block N (e.g., 50) but the cluster
        // is at block M (e.g., 300). Without this barrier, Rust starts consensus and
        // sends live commits to Go — but these commits contain transactions from round
        // 3000+ of the DAG, NOT the original transactions for blocks 51-300.
        // Go creates block 51 with WRONG transactions → stateRoot diverges → FORK.
        //
        // FIX: Before spawning CommitProcessor, fetch the gap blocks from peers and
        // send them to Go for NOMT execution. This ensures Go has the EXACT same state
        // as the cluster before receiving any new consensus commits.
        //
        // SAFETY: This runs BEFORE CommitProcessor::run() and ConsensusAuthority::start(),
        // so there are no race conditions between sync blocks and live commits.
        // ═══════════════════════════════════════════════════════════════════════════
        if config.executor_read_enabled && !config.peer_rpc_addresses.is_empty() {
            info!(
                "🔄 [SYNC-FIRST] Checking if Go Master needs catch-up before starting consensus..."
            );

            // Wait a moment for Go Master socket to be fully ready
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;

            // Step 1: Query Go Master's current block height
            let go_block = match executor_client_for_proc.get_last_block_number().await {
                Ok((block, _, _)) => block,
                Err(e) => {
                    warn!("⚠️ [SYNC-FIRST] Could not query Go block height: {}. Proceeding without sync.", e);
                    0
                }
            };

            // Step 2: Query peers for cluster's current block height
            let peer_block = match crate::network::peer_rpc::query_peer_epochs_network(
                &config.peer_rpc_addresses,
            )
            .await
            {
                Ok((_epoch, block, _addr, _gei)) => block,
                Err(e) => {
                    warn!("⚠️ [SYNC-FIRST] Could not query peer block height: {}. Proceeding without sync.", e);
                    0
                }
            };

            let gap = peer_block.saturating_sub(go_block);

            if peer_block > 0 && gap > 5 {
                info!("╔══════════════════════════════════════════════════════════════╗");
                info!(
                    "║  🔄 SYNC-FIRST: Go Master is {} blocks behind cluster      ║",
                    gap
                );
                info!(
                    "║  Go block: {}, Cluster block: {}                            ║",
                    go_block, peer_block
                );
                info!("║  Fetching + executing blocks from peers BEFORE consensus... ║");
                info!("╚══════════════════════════════════════════════════════════════╝");

                // Step 3: Fetch blocks from peers in batches of 200 and send to Go for execution
                let batch_size = 200u64;
                let mut current_from = go_block + 1;
                let mut total_synced = 0u64;

                while current_from <= peer_block {
                    let current_to = std::cmp::min(current_from + batch_size - 1, peer_block);

                    match crate::network::peer_rpc::fetch_blocks_from_peer(
                        &config.peer_rpc_addresses,
                        current_from,
                        current_to,
                    )
                    .await
                    {
                        Ok(blocks) => {
                            if blocks.is_empty() {
                                warn!("⚠️ [SYNC-FIRST] Got 0 blocks for range {}-{}. Peers may not have these blocks yet.", current_from, current_to);
                                break;
                            }
                            let fetched_count = blocks.len();
                            let max_fetched = blocks
                                .iter()
                                .map(|b| b.block_number)
                                .max()
                                .unwrap_or(current_from);

                            match executor_client_for_proc
                                .sync_and_execute_blocks(blocks)
                                .await
                            {
                                Ok((synced, last_block, _gei)) => {
                                    total_synced += synced;
                                    info!("✅ [SYNC-FIRST] Synced+Executed {fetched_count} blocks ({current_from}→{last_block}). Total: {total_synced}/{gap}");
                                    current_from = std::cmp::max(current_from + 1, last_block + 1);
                                }
                                Err(e) => {
                                    tracing::error!("❌ [SYNC-FIRST] Failed to execute blocks {current_from}-{current_to} via UDS: {e}");
                                    break;
                                }
                            }

                            // Advance past what we actually fetched (handle sparse blocks)
                            current_from = std::cmp::max(current_from, max_fetched + 1);
                        }
                        Err(e) => {
                            tracing::error!("❌ [SYNC-FIRST] Failed to fetch blocks {current_from}-{current_to} from peers: {e}");
                            break;
                        }
                    }

                    // Yield to let other tasks (socket listeners) run
                    tokio::task::yield_now().await;
                }

                // Step 4: Verify Go caught up (blocks)
                let new_go_block = executor_client_for_proc
                    .get_last_block_number()
                    .await
                    .map(|(b, _, _)| b)
                    .unwrap_or(go_block);
                let remaining_gap = peer_block.saturating_sub(new_go_block);

                if remaining_gap <= 5 {
                    info!("╔══════════════════════════════════════════════════════════════╗");
                    info!(
                        "║  ✅ SYNC-FIRST COMPLETE: Go caught up {} → {}              ║",
                        go_block, new_go_block
                    );
                    info!(
                        "║  Total synced: {} blocks. Ready for consensus.              ║",
                        total_synced
                    );
                    info!("╚══════════════════════════════════════════════════════════════╝");
                } else {
                    warn!("⚠️ [SYNC-FIRST] Go partially synced: {} → {} (still {} blocks behind). Consensus will start with gap.",
                        go_block, new_go_block, remaining_gap);
                }

                // ═══════════════════════════════════════════════════════════════
                // Step 5: GEI GAP SYNC — Fill empty commits between Go and cluster
                //
                // After syncing BLOCKS, Go may have block=144 but GEI=1575, while
                // the cluster has block=144 but GEI=2342. The 767 empty GEIs are
                // consensus rounds with 0 transactions — they advance the GEI but
                // don't create blocks. Without filling this gap, when consensus
                // starts, the first commit would have GEI=2342 but Go expects 1576,
                // causing massive out-of-order processing.
                //
                // FIX: Fetch executable blocks (including empty commits) from peer
                // Rust nodes and pipe them to Go via UDS. Go's batch-drain will
                // quickly absorb them.
                // ═══════════════════════════════════════════════════════════════
                let go_gei = executor_client_for_proc
                    .get_last_global_exec_index()
                    .await
                    .unwrap_or(0);
                let peer_gei = match crate::network::peer_rpc::query_peer_epochs_network(
                    &config.peer_rpc_addresses,
                )
                .await
                {
                    Ok((_ep, _bl, _addr, gei)) => gei,
                    Err(_) => 0,
                };
                let gei_gap = peer_gei.saturating_sub(go_gei);
                if gei_gap > 10 {
                    info!("╔══════════════════════════════════════════════════════════════╗");
                    info!(
                        "║  🔄 SYNC-FIRST GEI: Go GEI={} behind cluster GEI={}       ║",
                        go_gei, peer_gei
                    );
                    info!(
                        "║  Gap: {} empty commits to fill                              ║",
                        gei_gap
                    );
                    info!("╚══════════════════════════════════════════════════════════════╝");

                    // Fetch executable blocks (empty commits) from peer Rust nodes
                    let fetch_from = go_gei + 1;
                    let fetch_to = peer_gei;
                    let batch_gei_size = 500u64;
                    let mut gei_from = fetch_from;
                    let mut total_gei_synced = 0u64;

                    while gei_from <= fetch_to {
                        let gei_to = std::cmp::min(gei_from + batch_gei_size - 1, fetch_to);

                        match crate::network::peer_rpc::fetch_executable_blocks_from_peer(
                            &config.peer_rpc_addresses,
                            gei_from,
                            gei_to,
                        )
                        .await
                        {
                            Ok(blocks) => {
                                if blocks.is_empty() {
                                    info!("⚠️ [SYNC-FIRST GEI] No executable blocks from peers for GEI {}-{}. Peers may not store these.", gei_from, gei_to);
                                    break;
                                }
                                let fetched_count = blocks.len();
                                // Send each executable block to Go via UDS
                                let mut sent_ok = 0u64;
                                for (gei_num, block_bytes) in blocks {
                                    match executor_client_for_proc
                                        .send_block_data(
                                            &block_bytes,
                                            gei_num,
                                            storage.current_epoch,
                                            0,
                                        )
                                        .await
                                    {
                                        Ok(_) => sent_ok += 1,
                                        Err(e) => {
                                            warn!("⚠️ [SYNC-FIRST GEI] Failed to send GEI {} to Go: {}", gei_num, e);
                                            break;
                                        }
                                    }
                                }
                                total_gei_synced += sent_ok;
                                info!("✅ [SYNC-FIRST GEI] Sent {}/{} executable blocks (GEI {}-{}). Total: {}/{}",
                                    sent_ok, fetched_count, gei_from, gei_to, total_gei_synced, gei_gap);
                                gei_from = gei_to + 1;
                            }
                            Err(e) => {
                                warn!("⚠️ [SYNC-FIRST GEI] Failed to fetch GEI {}-{} from peers: {}. Go TRANSITION SYNC will handle remaining gap.", gei_from, gei_to, e);
                                break;
                            }
                        }

                        tokio::task::yield_now().await;
                    }

                    // Wait briefly for Go to process the batch-drained empty commits
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    let final_go_gei = executor_client_for_proc
                        .get_last_global_exec_index()
                        .await
                        .unwrap_or(0);
                    let final_gap = peer_gei.saturating_sub(final_go_gei);
                    info!("📊 [SYNC-FIRST GEI] After GEI sync: Go GEI={}, cluster GEI={}, remaining gap={}",
                        final_go_gei, peer_gei, final_gap);
                } else if peer_gei > 0 {
                    info!("✅ [SYNC-FIRST GEI] Go GEI={} matches cluster GEI={} (gap={}). No GEI sync needed.",
                        go_gei, peer_gei, gei_gap);
                }
            } else if peer_block > 0 {
                info!("✅ [SYNC-FIRST] Go Master is up-to-date (go_block={}, peer_block={}, gap={}). No sync needed.",
                    go_block, peer_block, gap);
            } else {
                info!("ℹ️ [SYNC-FIRST] Could not determine peer block height. Skipping sync-first check.");
            }
        }

        // ♻️ TX Recycler: Create shared instance for tracking and recycling uncommitted TXs
        let tx_recycler = Arc::new(crate::consensus::tx_recycler::TxRecycler::new());
        info!("♻️ [TX RECYCLER] Created shared TxRecycler instance");

        commit_processor = commit_processor
            .with_executor_client(executor_client_for_proc.clone())
            .with_tx_recycler(tx_recycler.clone());

        let (lag_alert_sender, mut lag_alert_receiver) = tokio::sync::mpsc::unbounded_channel::<
            crate::consensus::commit_processor::lag_monitor::LagAlert,
        >();

        commit_processor = commit_processor.with_lag_alert_sender(lag_alert_sender);

        let lag_executor_client = executor_client_for_proc.clone();
        let lag_peer_addresses = config.peer_rpc_addresses.clone();

        tokio::spawn(async move {
            while let Some(alert) = lag_alert_receiver.recv().await {
                match alert {
                    crate::consensus::commit_processor::lag_monitor::LagAlert::ModerateLag {
                        gap,
                        go_rate,
                        go_block_number,
                        ..
                    } => {
                        tracing::warn!("⚠️ [LAG-MONITOR] Go is {} blocks behind Rust (rate: {:.1} blk/s), go_block_number={}. Monitoring...", gap, go_rate, go_block_number);
                    }
                    crate::consensus::commit_processor::lag_monitor::LagAlert::SevereLag {
                        rust_gei,
                        go_gei,
                        go_block_number,
                        gap,
                        go_rate,
                    } => {
                        tracing::error!("🚨 [LAG-MONITOR] SEVERE: Go is {} blocks behind Rust! (rust={}, go_gei={}, go_block={}, rate={:.1} blk/s).",
                            gap, rust_gei, go_gei, go_block_number, go_rate);

                        if lag_peer_addresses.is_empty() {
                            tracing::warn!("⚠️ [LAG-RECOVERY] No peer_rpc_addresses configured! Cannot fetch missing blocks from P2P.");
                            continue;
                        }

                        // Use go_block_number (actual block index in Go DB) rather than GEI.
                        // We safely ask for blocks up to go_block_number + gap (Go peer caps nicely to its own max).
                        let missing_from = go_block_number + 1;
                        let missing_to = go_block_number + gap;
                        tracing::info!("🔄 [LAG-RECOVERY] Triggering P2P block fetch: blocks {} -> {} (assuming worst-case dense gap)", missing_from, missing_to);

                        match crate::network::peer_rpc::fetch_blocks_from_peer(
                            &lag_peer_addresses,
                            missing_from,
                            missing_to,
                        )
                        .await
                        {
                            Ok(blocks) => {
                                if blocks.is_empty() {
                                    tracing::warn!(
                                        "⚠️ [LAG-RECOVERY] Fetched 0 blocks from peers."
                                    );
                                } else {
                                    tracing::info!("✅ [LAG-RECOVERY] Fetched {} blocks. Sending to Go for execution...", blocks.len());
                                    match lag_executor_client.sync_and_execute_blocks(blocks).await
                                    {
                                        Ok((synced, last_block, _gei)) => {
                                            tracing::info!("✅ [LAG-RECOVERY] Successfully executed {} P2P blocks (last_block={})", synced, last_block);
                                        }
                                        Err(e) => {
                                            tracing::error!("❌ [LAG-RECOVERY] Failed to execute blocks via UDS: {}", e);
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::error!("❌ [LAG-RECOVERY] P2P block fetch failed: {}", e);
                            }
                        }
                    }
                    crate::consensus::commit_processor::lag_monitor::LagAlert::Recovered {
                        ..
                    } => {
                        tracing::info!("✅ [LAG-MONITOR] Go has caught up with Rust. Normal operations resumed.");
                    }
                }
            }
        });

        // Spawn background recycler is done in setup_epoch_management where tx_client is accessible

        tokio::spawn(async move {
            if let Err(e) = commit_processor.run().await {
                tracing::error!("❌ [COMMIT PROCESSOR] Error: {}", e);
            }
        });

        tokio::spawn(async move {
            while let Some(output) = block_receiver.recv().await {
                tracing::debug!("Received {} certified blocks", output.blocks.len());
            }
        });

        // Consensus parameters
        let protocol_config = ProtocolConfig::get_for_max_version_UNSAFE();
        let mut parameters = consensus_config::Parameters::default();
        parameters.commit_sync_batch_size = config.commit_sync_batch_size;
        parameters.commit_sync_parallel_fetches = config.commit_sync_parallel_fetches;
        parameters.commit_sync_batches_ahead = config.commit_sync_batches_ahead;

        if let Some(ms) = config.min_round_delay_ms {
            parameters.min_round_delay = Duration::from_millis(ms);
        }

        parameters.adaptive_delay_enabled = config.adaptive_delay_enabled;

        if let Some(ms) = config.leader_timeout_ms {
            parameters.leader_timeout = Duration::from_millis(ms);
        } else if config.speed_multiplier != 1.0 {
            info!("Applying speed multiplier: {}x", config.speed_multiplier);
            parameters.leader_timeout =
                Duration::from_millis((200.0 / config.speed_multiplier) as u64);
        }

        let db_path = config
            .storage_path
            .join("epochs")
            .join(format!("epoch_{}", storage.current_epoch))
            .join("consensus_db");
        std::fs::create_dir_all(&db_path)?;
        parameters.db_path = db_path;

        // system_transaction_provider and epoch_duration_seconds are created earlier
        // (before executor_client_for_proc) for backpressure wiring

        // Conditional logic: Start as Validator immediately IF:
        // 1. Node is designated as Validator in config
        // 2. AND we have DAG history for this epoch (meaning we didn't amnesia-crash)
        // OR
        // 3. We are exactly at the start of the epoch (no blocks produced yet, so no gap)
        let is_designated_validator = storage.is_in_committee;
        let start_as_validator = is_designated_validator
            && (dag_has_history || storage.last_global_exec_index <= storage.epoch_base_exec_index);

        let (authority, commit_consumer_holder) = if start_as_validator {
            info!("🚀 Starting consensus authority node immediately (in-sync confirmed)...");
            (
                Some(
                    ConsensusAuthority::start(
                        NetworkType::Tonic,
                        storage.epoch_timestamp_ms,
                        storage.epoch_base_exec_index,
                        storage.last_global_exec_index,
                        storage.own_index,
                        storage.committee.clone(),
                        parameters.clone(),
                        protocol_config.clone(),
                        storage.protocol_keypair.clone(),
                        storage.network_keypair.clone(),
                        clock.clone(),
                        transaction_verifier.clone(),
                        commit_consumer,
                        registry.clone(),
                        0,
                        Some(system_transaction_provider.clone()
                            as Arc<dyn SystemTransactionProvider>),
                        None,
                    )
                    .await,
                ),
                None,
            )
        } else {
            info!("🔄 Starting as sync-only node (not in committee)");
            info!("📡 Keeping commit_consumer alive for SyncOnly mode to prevent channel close");
            (None, Some(commit_consumer))
        };

        let transaction_client_proxy = authority
            .as_ref()
            .map(|auth| Arc::new(TransactionClientProxy::new(auth.transaction_client())));

        Ok(ConsensusSetup {
            authority,
            dag_has_history,
            commit_consumer_holder,
            transaction_client_proxy,
            executor_client_for_proc,
            current_commit_index,
            is_transitioning,
            pending_transactions_queue,
            committed_transaction_hashes,
            epoch_tx_sender,
            epoch_tx_receiver,
            shared_last_global_exec_index,
            system_transaction_provider,
            protocol_config,
            parameters,
            clock,
            transaction_verifier,
            tx_recycler,
            start_as_validator,
        })
    }

    // -----------------------------------------------------------------------
    // Phase 3: setup_networking
    // -----------------------------------------------------------------------

    /// Initializes clock sync manager and starts NTP sync tasks.
    fn setup_networking(config: &NodeConfig) -> Arc<RwLock<ClockSyncManager>> {
        let clock_sync_manager = Arc::new(RwLock::new(ClockSyncManager::new(
            config.ntp_servers.clone(),
            config.max_clock_drift_seconds * 1000,
            config.ntp_sync_interval_seconds,
            config.enable_ntp_sync,
        )));

        if config.enable_ntp_sync {
            let sync_manager_clone = clock_sync_manager.clone();
            let monitor_manager_clone = clock_sync_manager.clone();
            tokio::spawn(async move {
                let mut manager = sync_manager_clone.write().await;
                let _ = manager.sync_with_ntp().await;
            });
            ClockSyncManager::start_sync_task(clock_sync_manager.clone());
            ClockSyncManager::start_drift_monitor(monitor_manager_clone);
        }

        clock_sync_manager
    }

    // -----------------------------------------------------------------------
    // Phase 4: setup_epoch_management
    // -----------------------------------------------------------------------

    /// Assembles the ConsensusNode, initializes epoch management, starts sync tasks and monitors.
    async fn setup_epoch_management(
        config: NodeConfig,
        storage: StorageSetup,
        consensus: ConsensusSetup,
        clock_sync_manager: Arc<RwLock<ClockSyncManager>>,
        _registry: &Registry,
    ) -> Result<ConsensusNode> {
        // Initialize no-op epoch change handlers (required by core)
        use consensus_core::epoch_change_provider::{EpochChangeProcessor, EpochChangeProvider};
        struct NoOpProvider;
        impl EpochChangeProvider for NoOpProvider {
            fn get_proposal(&self) -> Option<Vec<u8>> {
                None
            }
            fn get_votes(&self) -> Vec<Vec<u8>> {
                Vec::new()
            }
        }
        struct NoOpProcessor;
        impl EpochChangeProcessor for NoOpProcessor {
            fn process_proposal(&self, _: &[u8]) {}
            fn process_vote(&self, _: &[u8]) {}
        }
        consensus_core::epoch_change_provider::init_epoch_change_provider(Box::new(NoOpProvider));
        consensus_core::epoch_change_provider::init_epoch_change_processor(Box::new(NoOpProcessor));

        // ♻️ TX RECYCLER: Background recycler DISABLED PERMANENTLY.
        // ARCHITECTURAL REASON: Go broadcasts its mempool to all nodes via LibP2P.
        // Therefore, MULTIPLE Rust validators will propose the EXACT SAME transactions in their blocks.
        // Only one of those blocks might get sequenced quickly, while the others might be GarbageCollected.
        // If a losing validator auto-resubmits its dropped/GC'd transactions (either via timeout or GC events),
        // it introduces massive duplicate transaction bloat into the DAG, causing State Root forks or performance collapse.
        // Dropped transactions should only be retried by the original Client/Wallet, NOT the consensus layer.
        // Tracking + confirming still active solely for metrics and observability.

        let mut node = ConsensusNode {
            authority: consensus.authority,
            legacy_store_manager: Arc::new(consensus_core::LegacyEpochStoreManager::new(
                config.epochs_to_keep,
            )),
            // Phase 2: In-committee nodes start as SyncingUp. They will explicitly catch up
            // via startup.rs and then reliably transition to Validator mode.
            node_mode: if consensus.start_as_validator {
                NodeMode::Validator
            } else if storage.is_in_committee {
                NodeMode::SyncingUp
            } else {
                NodeMode::SyncOnly
            },
            execution_lock: Arc::new(tokio::sync::RwLock::new(storage.current_epoch)),
            reconfig_state: Arc::new(tokio::sync::RwLock::new(ReconfigState::default())),
            transaction_client_proxy: consensus.transaction_client_proxy,
            clock_sync_manager,
            current_commit_index: consensus.current_commit_index,
            storage_path: storage.storage_path,
            current_epoch: storage.current_epoch,
            last_global_exec_index: storage.last_global_exec_index,
            shared_last_global_exec_index: consensus.shared_last_global_exec_index,
            protocol_keypair: storage.protocol_keypair,
            network_keypair: storage.network_keypair,
            protocol_config: consensus.protocol_config,
            clock: consensus.clock,
            transaction_verifier: consensus.transaction_verifier,
            parameters: consensus.parameters,
            own_index: storage.own_index,
            boot_counter: 0,
            last_transition_hash: None,
            current_registry_id: None,
            executor_commit_enabled: config.executor_commit_enabled,
            is_transitioning: consensus.is_transitioning,
            pending_transactions_queue: consensus.pending_transactions_queue,
            system_transaction_provider: consensus.system_transaction_provider,
            epoch_transition_sender: consensus.epoch_tx_sender,
            sync_task_handle: None,
            sync_controller: Arc::new(crate::node::sync_controller::SyncController::new()),
            epoch_monitor_handle: None,
            notification_server_handle: None,
            executor_client: Some(consensus.executor_client_for_proc),
            epoch_pending_transactions: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            committed_transaction_hashes: consensus.committed_transaction_hashes,
            pending_epoch_transitions: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            _commit_consumer_holder: consensus.commit_consumer_holder,
            epoch_eth_addresses: {
                let mut map = std::collections::HashMap::new();
                map.insert(
                    storage.current_epoch,
                    storage.validator_eth_addresses.clone(),
                );
                Arc::new(tokio::sync::RwLock::new(map))
            },
            block_coordinator: None,
            peer_rpc_addresses: config.peer_rpc_addresses.clone(),
            tx_recycler: Some(consensus.tx_recycler),
            cold_start: !consensus.dag_has_history
                && storage.is_in_committee
                && storage.current_epoch > 0,
            cold_start_snapshot_gei: 0, // Will be set during cold-start in startup.rs
        };

        // Initialize the global StateTransitionManager
        epoch_transition_manager::init_state_manager(
            storage.current_epoch,
            storage.is_in_committee,
        )
        .await;
        info!(
            "🔧 [STARTUP] Initialized StateTransitionManager: epoch={}, mode={}",
            storage.current_epoch,
            if storage.is_in_committee {
                "Validator"
            } else {
                "SyncOnly"
            }
        );

        // Load previous epoch stores for cross-epoch sync support
        // This enables THIS node to serve historical commits to lagging peers
        load_legacy_epoch_stores(
            &node.legacy_store_manager,
            &config.storage_path,
            storage.current_epoch,
            config.epochs_to_keep,
        );

        crate::consensus::epoch_transition::start_epoch_transition_handler(
            consensus.epoch_tx_receiver,
            node.system_transaction_provider.clone(),
            config.clone(),
        );

        node.check_and_update_node_mode(&storage.committee, &config, false)
            .await?;

        // Start sync task for SyncOnly nodes
        if matches!(node.node_mode, NodeMode::SyncOnly) {
            let _ = node.start_sync_task(&config).await;
        }

        // UNIFIED EPOCH MONITOR
        if let Ok(Some(handle)) =
            epoch_monitor::start_unified_epoch_monitor(&node.executor_client, &config)
        {
            node.epoch_monitor_handle = Some(handle);
            info!(
                "🔄 Started unified epoch monitor for {:?} mode at epoch={}",
                node.node_mode, node.current_epoch
            );
        }

        recovery::perform_fork_detection_check(&node).await?;

        Ok(node)
    }
}
