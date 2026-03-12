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

        let latest_block_number = match executor_client.get_last_block_number().await {
            Ok(n) => n,
            Err(e) => {
                warn!("⚠️ [STARTUP] Failed to fetch latest block from Go: {}. Attempting to read persisted value.", e);
                super::executor_client::read_last_block_number(&config.storage_path)
                    .await
                    .unwrap_or(0)
            }
        };

        // PEER EPOCH DISCOVERY: Query TCP peers to get correct epoch (with retry)
        // ═══════════════════════════════════════════════════════════════════════
        // CRITICAL FIX: SyncOnly nodes MUST use local Go epoch, NOT peer epoch!
        // Using peer epoch causes DEADLOCK:
        //   1. Peer says epoch=1 → Rust advances internal state to epoch 1
        //   2. Deferred epoch transition waits for Go GEI >= boundary
        //   3. But Go GEI=0 (no blocks synced yet) → 120s timeout → DEADLOCK
        // SyncOnly must sync blocks sequentially: epoch transitions happen naturally
        // when Go processes blocks up to the epoch boundary.
        // ═══════════════════════════════════════════════════════════════════════
        let is_sync_only = matches!(config.initial_node_mode, NodeMode::SyncOnly);
        let (go_epoch, peer_last_block, best_socket) = if is_sync_only {
            // SyncOnly: ALWAYS use local Go epoch to prevent deadlock
            let epoch = executor_client
                .get_current_epoch()
                .await
                .map_err(|e| anyhow::anyhow!("Failed to fetch current epoch from Go: {}", e))?;
            info!(
                "📋 [SYNC-ONLY STARTUP] Using LOCAL Go epoch {} (skipping peer discovery to prevent deadlock)",
                epoch
            );
            (
                epoch,
                latest_block_number,
                config.executor_receive_socket_path.clone(),
            )
        } else if !config.peer_rpc_addresses.is_empty() {
            use crate::network::peer_rpc::query_peer_epochs_network;
            let max_attempts = 3;
            let mut peer_result = None;
            for attempt in 1..=max_attempts {
                match query_peer_epochs_network(&config.peer_rpc_addresses).await {
                    Ok(result) => {
                        info!(
                            "✅ [PEER EPOCH] Using epoch {} from TCP peer discovery (peer: {}, attempt {})",
                            result.0, result.2, attempt
                        );
                        peer_result = Some(result);
                        break;
                    }
                    Err(e) => {
                        if attempt < max_attempts {
                            warn!(
                                "⚠️ [PEER EPOCH] Attempt {}/{} failed: {}. Retrying in 2s...",
                                attempt, max_attempts, e
                            );
                            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                        } else {
                            warn!(
                                "⚠️ [PEER EPOCH] All {} attempts failed. Last error: {}. Falling back to local Go.",
                                max_attempts, e
                            );
                        }
                    }
                }
            }
            if let Some(result) = peer_result {
                (
                    result.0,
                    result.1,
                    config.executor_receive_socket_path.clone(),
                )
            } else {
                let epoch = executor_client
                    .get_current_epoch()
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to fetch epoch: {}", e))?;
                (
                    epoch,
                    latest_block_number,
                    config.executor_receive_socket_path.clone(),
                )
            }
        } else {
            let epoch = executor_client
                .get_current_epoch()
                .await
                .map_err(|e| anyhow::anyhow!("Failed to fetch current epoch from Go: {}", e))?;
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
                let keep_from = if go_epoch >= config.epochs_to_keep as u64 {
                    go_epoch - config.epochs_to_keep as u64
                } else {
                    0
                };
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
            if let Ok(entries) = std::fs::read_dir(&storage_path.join("epochs")) {
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

        // Fetch epoch boundary data with fallback chain
        let (current_epoch, epoch_timestamp_ms, boundary_block, validators, epoch_duration_from_go) =
            match peer_executor_client
                .get_epoch_boundary_data(current_epoch)
                .await
            {
                Ok((epoch, timestamp, boundary, vals, epoch_dur)) => {
                    info!(
                        "✅ [STARTUP] Got epoch boundary data for epoch {} from Go (epoch_duration={}s)",
                        epoch, epoch_dur
                    );
                    (epoch, timestamp, boundary, vals, epoch_dur)
                }
                Err(e) => {
                    warn!(
                        "⚠️ [STARTUP] Failed to get epoch boundary for epoch {}: {}. Falling back to local Go's epoch.",
                        current_epoch, e
                    );
                    let local_epoch = executor_client.get_current_epoch().await.unwrap_or(0);
                    info!(
                        "📊 [STARTUP] Falling back to local Go epoch {} (network epoch was {})",
                        local_epoch, current_epoch
                    );

                    match executor_client.get_epoch_boundary_data(local_epoch).await {
                        Ok((epoch, timestamp, boundary, vals, epoch_dur)) => {
                            info!(
                                "✅ [STARTUP] Got epoch boundary data for local epoch {} (epoch_duration={}s)",
                                epoch, epoch_dur
                            );
                            (epoch, timestamp, boundary, vals, epoch_dur)
                        }
                        Err(e2) => {
                            warn!(
                                "⚠️ [STARTUP] No epoch boundary available (local epoch {} error: {}). Using epoch 0 genesis.",
                                local_epoch, e2
                            );
                            let (genesis_validators, _genesis_epoch, _) = executor_client
                                .get_validators_at_block(0)
                                .await
                                .map_err(|e| {
                                    anyhow::anyhow!("Failed to fetch genesis validators: {}", e)
                                })?;
                            (0u64, 0u64, 0u64, genesis_validators, 900u64)
                        }
                    }
                }
            };

        info!(
            "📊 [STARTUP] Using epoch boundary data: epoch={}, boundary_block={}, epoch_timestamp={}ms, validators={}",
            current_epoch, boundary_block, epoch_timestamp_ms, validators.len()
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

        let epoch_base_exec_index = boundary_block;
        info!(
            "✅ [STARTUP] Using epoch_base={} from Go boundary_block (epoch={})",
            epoch_base_exec_index, current_epoch
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

        let local_go_block = executor_client.get_last_block_number().await.unwrap_or(0);
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
                "📊 [STARTUP] Sync Check: LocalGo={}, Peer={}, Persisted=({}, commit={}) (from {})",
                local_go_block, peer_last_block, persisted_index, persisted_commit, best_socket
            );

            let sources_match =
                local_go_block == peer_last_block || local_go_block.abs_diff(peer_last_block) <= 5;
            if !sources_match {
                warn!("⚠️ [STARTUP] INDEX DISCREPANCY DETECTED:");
                warn!(
                    "   LocalGo={}, Peer={}, Persisted={}",
                    local_go_block, peer_last_block, persisted_index
                );
                warn!("   This may indicate network partition or stale data.");
            }

            if local_go_block > peer_last_block + 5 {
                warn!("🚨 [STARTUP] STALE CHAIN DETECTED: Local ({}) is ahead of Peer ({})! Forcing resync from Peer.", 
                       local_go_block, peer_last_block);
                peer_last_block
            } else if local_go_block < peer_last_block.saturating_sub(5) {
                info!(
                    "ℹ️ [STARTUP] Local Go Master ({}) is behind Peer ({}) by {} blocks. Using Local {} to trigger recovery/backfill.",
                    local_go_block, peer_last_block, peer_last_block - local_go_block, local_go_block
                );
                local_go_block
            } else {
                info!(
                    "✅ [STARTUP] Local and Peer are in sync (Local={}, Peer={}). Using Local Go as authoritative.",
                    local_go_block, peer_last_block
                );
                local_go_block
            }
        } else {
            if persisted_index > local_go_block {
                warn!("⚠️ [STARTUP] Persisted Index {} > Local Go {}. Go is behind (possible rollback/crash). Using Local Go {} to force resync/replay.", 
                    persisted_index, local_go_block, local_go_block);
            }
            info!(
                "📊 [STARTUP] No peer reference, using Local Go Last Block: {}",
                local_go_block
            );
            local_go_block
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
            Arc::new(tokio::sync::Mutex::new(map))
        });

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

        // ♻️ TX Recycler: Create shared instance for tracking and recycling uncommitted TXs
        let tx_recycler = Arc::new(crate::consensus::tx_recycler::TxRecycler::new());
        info!("♻️ [TX RECYCLER] Created shared TxRecycler instance");

        commit_processor = commit_processor
            .with_executor_client(executor_client_for_proc.clone())
            .with_tx_recycler(tx_recycler.clone());

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

        // Start authority or hold commit_consumer for SyncOnly
        let (authority, commit_consumer_holder) = if storage.is_in_committee {
            info!("🚀 Starting consensus authority node...");
            (
                Some(
                    ConsensusAuthority::start(
                        NetworkType::Tonic,
                        storage.epoch_timestamp_ms,
                        storage.epoch_base_exec_index,
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
            info!("🔄 Starting as sync-only node");
            info!("📡 Keeping commit_consumer alive for SyncOnly mode to prevent channel close");
            (None, Some(commit_consumer))
        };

        let transaction_client_proxy = if let Some(ref auth) = authority {
            Some(Arc::new(TransactionClientProxy::new(
                auth.transaction_client(),
            )))
        } else {
            None
        };

        Ok(ConsensusSetup {
            authority,
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
            node_mode: if storage.is_in_committee {
                NodeMode::Validator
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
                Arc::new(tokio::sync::Mutex::new(map))
            },
            block_coordinator: None,
            peer_rpc_addresses: config.peer_rpc_addresses.clone(),
            tx_recycler: Some(consensus.tx_recycler),
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
