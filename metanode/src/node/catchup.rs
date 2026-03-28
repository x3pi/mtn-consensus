// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

//! CatchupManager handles node synchronization when restarting behind the network.
//!
//! When a Rust node restarts, it may be behind other nodes:
//! - Different epoch than the network
//! - Missing committed blocks within same epoch
//!
//! CatchupManager coordinates with:
//! - Go Master: Query current epoch and last executed block
//! - CommitSyncer: Fetch missing commits from peer nodes
//! - ConsensusNode: Control when to join consensus

use anyhow::Result;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use crate::network::peer_rpc::query_peer_epochs_network;
use crate::node::executor_client::ExecutorClient;

/// State of catchup process
#[derive(Debug, Clone, PartialEq)]
pub enum CatchupState {
    /// Initial state, checking sync status
    Initializing,
    /// Syncing to a different epoch (need to clear old data)
    SyncingEpoch { target_epoch: u64, local_epoch: u64 },
    /// Syncing commits within same epoch
    SyncingCommits {
        target_commit: u64,
        current_commit: u64,
    },
    /// Go is behind local Rust storage
    BehindRustLocal {
        target_block: u64,
        current_block: u64,
    },
    /// Caught up, ready to participate in consensus
    Ready,
}

/// Result of sync status check
#[derive(Debug)]
pub struct SyncStatus {
    /// Current epoch from Go
    pub go_epoch: u64,
    /// Last executed block from Go
    pub go_last_block: u64,
    /// Whether epoch matches
    pub epoch_match: bool,
    /// Commit gap (how many commits behind)
    pub commit_gap: u64,
    /// Block gap (how many blocks behind)
    pub block_gap: u64,
    /// Network max block height
    pub network_block_height: u64,
    /// Network's current global commit index (global_exec_index)
    pub network_commit: u64,
    /// Whether ready to join consensus
    pub ready: bool,
}

/// Manager for node catchup synchronization
pub struct CatchupManager {
    /// Client to communicate with Go Master
    pub executor_client: Arc<ExecutorClient>,
    /// Own Go Master socket (for fallback/identity)
    _own_socket: String,
    /// WAN peer RPC addresses (e.g., "192.168.1.100:19000")
    peer_rpc_addresses: Vec<String>,
    /// Current catchup state
    pub state: RwLock<CatchupState>,
}

/// Threshold for considering node caught up (within N blocks of network)
const BLOCK_CATCHUP_THRESHOLD: u64 = 5;

impl CatchupManager {
    /// Create new catchup manager
    pub fn new(
        executor_client: Arc<ExecutorClient>,
        own_socket: String,
        peer_rpc_addresses: Vec<String>,
    ) -> Self {
        Self {
            executor_client,
            _own_socket: own_socket,
            peer_rpc_addresses,
            state: RwLock::new(CatchupState::Initializing),
        }
    }

    /// Get peer RPC addresses (for external use in fallback sync loops)
    #[allow(dead_code)]
    pub fn peer_rpc_addresses(&self) -> Vec<String> {
        self.peer_rpc_addresses.clone()
    }

    /// Convenience: sync blocks from peers if local Go is behind network block height.
    /// Returns number of blocks synced.
    #[allow(dead_code)]
    pub async fn sync_blocks_from_peers_if_behind(&self, network_block: u64) -> Result<u64> {
        let go_block = self.executor_client.get_last_block_number().await?.0;
        if go_block >= network_block {
            return Ok(0);
        }
        self.sync_blocks_from_peers(go_block, network_block).await
    }

    /// Check sync status by querying Go Master and Peers
    pub async fn check_sync_status(
        &self,
        local_epoch: u64,
        local_last_commit: u64,
    ) -> Result<SyncStatus> {
        // 1. Get Local State from Go Master
        let local_go_epoch = match self.executor_client.get_current_epoch().await {
            Ok(epoch) => epoch,
            Err(e) => {
                error!("🚨 [CATCHUP] Failed to get current epoch from Go: {}", e);
                return Err(anyhow::anyhow!("Failed to get epoch from Go: {}", e));
            }
        };

        let local_go_last_block = match self.executor_client.get_last_block_number().await {
            Ok((block, _)) => block,
            Err(e) => {
                error!("🚨 [CATCHUP] Failed to get last block from Go: {}", e);
                return Err(anyhow::anyhow!("Failed to get last block from Go: {}", e));
            }
        };

        // 2. Get Network State from Peers (TCP-based only)
        let rust_stored_block = match &self.executor_client.storage_path {
            Some(path) => {
                crate::node::executor_client::block_store::get_max_stored_gei(path)
                    .await
                    .unwrap_or(None)
                    .unwrap_or(local_go_last_block)
            }
            None => local_go_last_block,
        };

        if local_go_last_block < rust_stored_block {
            info!("⚠️ [SYNC STATUS] Go is behind LOCAL Rust DB! (Go: {}, Rust: {}). Fast-forward mandatory.", local_go_last_block, rust_stored_block);
            let state = CatchupState::BehindRustLocal {
                target_block: rust_stored_block,
                current_block: local_go_last_block,
            };
            *self.state.write().await = state;
            
            return Ok(SyncStatus {
                go_epoch: local_go_epoch,
                go_last_block: local_go_last_block,
                epoch_match: true,
                commit_gap: 0,
                block_gap: rust_stored_block - local_go_last_block,
                network_block_height: rust_stored_block,
                network_commit: rust_stored_block,
                ready: false,
            });
        }

        let (network_epoch, network_block, _best_peer, network_commit) = if !self
            .peer_rpc_addresses
            .is_empty()
        {
            info!(
                "🌐 [CATCHUP] Using WAN peer discovery ({} peers configured)",
                self.peer_rpc_addresses.len()
            );
            match query_peer_epochs_network(&self.peer_rpc_addresses).await {
                Ok(res) => {
                    info!(
                        "✅ [CATCHUP] WAN peer query success: epoch={}, block={}, global_exec_index={}, peer={}",
                        res.0, res.1, res.3, res.2
                    );
                    (
                        res.0,
                        std::cmp::max(res.1, rust_stored_block), // Ensure network_block is at least rust_stored_block
                        res.2,
                        std::cmp::max(res.3, rust_stored_block), // Ensure network_commit is at least rust_stored_block
                    )
                }
                Err(e) => {
                    warn!(
                        "⚠️ [CATCHUP] WAN peer query failed ({}), using local state",
                        e
                    );
                    (
                        local_go_epoch,
                        std::cmp::max(local_go_last_block, rust_stored_block),
                        "local".to_string(),
                        std::cmp::max(local_last_commit, rust_stored_block),
                    )
                }
            }
        } else {
            // No peers configured (single node?), assume we are the network
            (
                local_go_epoch,
                std::cmp::max(local_go_last_block, rust_stored_block),
                "local".to_string(),
                std::cmp::max(local_last_commit, rust_stored_block),
            )
        };

        // 3. Compare States
        // FIX: Use Go's epoch (authoritative) for epoch_match, not local_epoch which
        // may be stale from when Rust started at epoch 0.
        let epoch_match = local_go_epoch == network_epoch;

        // Calculate gaps
        let commit_gap = if epoch_match {
            network_commit.saturating_sub(local_last_commit)
        } else {
            u64::MAX
        };

        let block_gap = if epoch_match {
            network_block.saturating_sub(local_go_last_block)
        } else {
            u64::MAX
        };

        // Ready condition: Same Epoch AND Block Gap is small
        // We DO NOT check commit_gap here. Go only increments block_gap for blocks with TXs.
        // Rust increments commit_gap for every consensus block (even empty ones).
        // If we require commit_gap to be small, nodes resuming with large empty commit gaps
        // will get stuck in an infinite loop and never start CommitSyncer.
        let ready = epoch_match && block_gap <= BLOCK_CATCHUP_THRESHOLD;

        let status = SyncStatus {
            go_epoch: network_epoch,
            go_last_block: local_go_last_block,
            epoch_match,
            commit_gap,
            block_gap,
            network_block_height: network_block,
            network_commit,
            ready,
        };

        info!(
            "📊 [CATCHUP] Sync status: Local[Ep={}, GoEp={}, Blk={}, Cmt={}] vs Network[Ep={}, Blk={}, Cmt={}] -> Gap={} commits, {} blocks. Ready={}",
            local_epoch, local_go_epoch, local_go_last_block, local_last_commit,
            network_epoch, network_block, network_commit,
            if commit_gap == u64::MAX { "∞".to_string() } else { commit_gap.to_string() },
            if block_gap == u64::MAX { "∞".to_string() } else { block_gap.to_string() },
            ready
        );

        // Update state based on status
        let new_state = if !epoch_match {
            CatchupState::SyncingEpoch {
                target_epoch: network_epoch,
                local_epoch,
            }
        } else if !ready {
            CatchupState::SyncingCommits {
                target_commit: 0,
                current_commit: local_last_commit,
            }
        } else {
            CatchupState::Ready
        };

        *self.state.write().await = new_state;

        Ok(status)
    }

    /// Actively fetch missing blocks (prefers local Rust DB first, then peers) and write them to local Go.
    /// This closes the block gap that passive polling can never close.
    pub async fn sync_blocks_from_peers(
        &self,
        go_last_block: u64,
        network_block: u64,
    ) -> Result<u64> {
        let missing_from = go_last_block + 1;
        if missing_from > network_block {
            info!(
                "✅ [CATCHUP SYNC] No blocks to fetch (go={} >= network={})",
                go_last_block, network_block
            );
            return Ok(0);
        }

        // 1. Try Local Rust first! This is a fast path and supports offline/single node recovery.
        if let Some(ref path) = self.executor_client.storage_path {
            let rust_max = crate::node::executor_client::block_store::get_max_stored_gei(path)
                .await
                .unwrap_or(None)
                .unwrap_or(0);
                
            if rust_max >= missing_from {
                let fetch_to = std::cmp::min(network_block, rust_max);
                match self.sync_blocks_from_local_rust(path, go_last_block, fetch_to).await {
                    Ok(synced) if synced > 0 => {
                        info!("🚀 [CATCHUP SYNC] Synced {} blocks from FAST local Rust DB", synced);
                        return Ok(synced);
                    }
                    _ => {
                        warn!("⚠️ [CATCHUP SYNC] Local Rust DB fetch yielded 0 blocks or failed. Falling back to WAN peers...");
                    }
                }
            }
        }

        // 2. Fallback to WAN peers if local Rust doesn't have it
        if self.peer_rpc_addresses.is_empty() {
            return Err(anyhow::anyhow!("No local blocks and no peer_rpc_addresses configured"));
        }

        info!(
            "🔄 [CATCHUP SYNC] Fetching blocks {} to {} from {} peer(s)",
            missing_from,
            network_block,
            self.peer_rpc_addresses.len()
        );

        match crate::network::peer_rpc::fetch_blocks_from_peer(
            &self.peer_rpc_addresses,
            missing_from,
            network_block,
        )
        .await
        {
            Ok(blocks) => {
                if blocks.is_empty() {
                    warn!("⚠️ [CATCHUP SYNC] Fetched 0 blocks from peers");
                    return Ok(0);
                }

                let fetched_count = blocks.len() as u64;
                info!(
                    "✅ [CATCHUP SYNC] Fetched {} blocks from peers. Syncing to local Go...",
                    fetched_count
                );

                match self.executor_client.sync_blocks(blocks).await {
                    Ok((synced, last_block)) => {
                        info!(
                            "✅ [CATCHUP SYNC] Synced {} blocks to local Go (last: {})",
                            synced, last_block
                        );
                        Ok(synced)
                    }
                    Err(e) => {
                        warn!("⚠️ [CATCHUP SYNC] Failed to sync blocks to local Go: {}", e);
                        Err(anyhow::anyhow!("Failed to sync blocks: {}", e))
                    }
                }
            }
            Err(e) => {
                warn!("⚠️ [CATCHUP SYNC] Failed to fetch blocks from peers: {}", e);
                Err(anyhow::anyhow!("Failed to fetch from peers: {}", e))
            }
        }
    }

    /// Sync Go Master to the current network epoch by fetching blocks from peers.
    ///
    /// This is used during snapshot restore: Go starts at an old epoch (from snapshot)
    /// and needs to catch up to the current network epoch before Rust consensus can start.
    /// Blocks are fetched from peer Go nodes via peer_rpc and written to local Go.
    ///
    /// Returns total number of blocks synced.
    pub async fn sync_go_to_current_epoch(&self, network_epoch: u64) -> Result<u64> {
        if self.peer_rpc_addresses.is_empty() {
            return Err(anyhow::anyhow!("No peer_rpc_addresses configured for epoch catchup"));
        }

        let mut synced_total: u64 = 0;
        let mut consecutive_empty: u32 = 0;
        let max_consecutive_empty: u32 = 20; // Give up after 20 empty fetches
        let batch_size: u64 = 500;

        info!(
            "🚀 [EPOCH-CATCHUP] Starting Go block sync to reach epoch {} from peers ({} configured)",
            network_epoch, self.peer_rpc_addresses.len()
        );

        loop {
            // Check if Go has reached the target epoch
            let go_epoch = match self.executor_client.get_current_epoch().await {
                Ok(e) => e,
                Err(e) => {
                    warn!("⚠️ [EPOCH-CATCHUP] Failed to get Go epoch: {}", e);
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    continue;
                }
            };

            if go_epoch >= network_epoch {
                info!(
                    "✅ [EPOCH-CATCHUP] Go reached epoch {} (target={}). Synced {} blocks total.",
                    go_epoch, network_epoch, synced_total
                );
                return Ok(synced_total);
            }

            // Get current Go block number
            let go_block = match self.executor_client.get_last_block_number().await {
                Ok((b, _)) => b,
                Err(e) => {
                    warn!("⚠️ [EPOCH-CATCHUP] Failed to get Go block: {}", e);
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    continue;
                }
            };

            // Fetch a batch of blocks from peers
            let fetch_to = go_block + batch_size;
            match self.sync_blocks_from_peers(go_block, fetch_to).await {
                Ok(synced) if synced > 0 => {
                    synced_total += synced;
                    consecutive_empty = 0;

                    if synced_total % 1000 < batch_size {
                        info!(
                            "🔄 [EPOCH-CATCHUP] Progress: synced {} blocks total, Go epoch={}, block={}, target_epoch={}",
                            synced_total, go_epoch, go_block + synced, network_epoch
                        );
                    }
                }
                Ok(_) => {
                    consecutive_empty += 1;
                    if consecutive_empty >= max_consecutive_empty {
                        warn!(
                            "⚠️ [EPOCH-CATCHUP] {} consecutive empty fetches. Go epoch={}, target={}. Giving up.",
                            consecutive_empty, go_epoch, network_epoch
                        );
                        return Ok(synced_total);
                    }
                    // Brief delay before retry when no blocks available
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
                Err(e) => {
                    consecutive_empty += 1;
                    warn!("⚠️ [EPOCH-CATCHUP] Fetch error #{}: {}", consecutive_empty, e);
                    if consecutive_empty >= max_consecutive_empty {
                        warn!(
                            "⚠️ [EPOCH-CATCHUP] Too many errors. Go epoch={}, target={}. Giving up.",
                            go_epoch, network_epoch
                        );
                        return Ok(synced_total);
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            }
        }
    }

    /// Actively fetch missing blocks from LOCAL Rust storage and write them to local Go.
    /// This is critical when a Master node restarts and Go rolls back, but Rust has the commits locally
    /// and there are no peers to fetch them from.
    pub async fn sync_blocks_from_local_rust(
        &self,
        storage_path: &std::path::Path,
        go_last_block: u64,
        rust_last_block: u64,
    ) -> Result<u64> {
        let missing_from = go_last_block + 1;
        if missing_from > rust_last_block {
            info!(
                "✅ [LOCAL CATCHUP] No blocks to fetch (go={} >= rust={})",
                go_last_block, rust_last_block
            );
            return Ok(0);
        }

        info!(
            "🔄 [LOCAL CATCHUP] Fetching local Rust blocks {} to {} for Go executor",
            missing_from,
            rust_last_block
        );

        let blocks = crate::node::executor_client::block_store::load_executable_blocks_range(
            storage_path,
            missing_from,
            rust_last_block,
        ).await?;

        if blocks.is_empty() {
            warn!("⚠️ [LOCAL CATCHUP] Loaded 0 blocks from Rust local DB");
            return Ok(0);
        }

        let fetched_count = blocks.len() as u64;
        info!(
            "✅ [LOCAL CATCHUP] Loaded {} blocks from Rust DB. Syncing to local Go via consensus stream...",
            fetched_count
        );

        use prost::Message;
        let mut synced = 0;
        let mut last_block = 0;

        for (gei, data) in blocks {
            // Decode the ExecutableBlock to extract epoch and commit_index
            match crate::node::executor_client::proto::ExecutableBlock::decode(data.as_slice()) {
                Ok(exec_block) => {
                    let epoch = exec_block.epoch;
                    let commit_index = exec_block.commit_index;
                    
                    match self.executor_client.send_block_data(&data, gei, epoch, commit_index).await {
                        Ok(_) => {
                            synced += 1;
                            last_block = gei;
                        }
                        Err(e) => {
                            warn!("⚠️ [LOCAL CATCHUP] Failed to send local block {} to Go: {}", gei, e);
                            return Err(anyhow::anyhow!("Failed to send local block {}: {}", gei, e));
                        }
                    }
                }
                Err(e) => {
                    warn!("⚠️ [LOCAL CATCHUP] Failed to decode local block {}: {}", gei, e);
                    // Continue to next block instead of failing completely? 
                    // No, if a block is corrupted, we must stop, otherwise we create a gap
                    return Err(anyhow::anyhow!("Failed to decode local block {}: {}", gei, e));
                }
            }
        }

        info!(
            "✅ [LOCAL CATCHUP] Synced {} blocks from local Rust to Go (last: {})",
            synced, last_block
        );
        Ok(synced)
    }
}
