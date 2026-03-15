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
    /// Whether ready to join consensus
    pub ready: bool,
}

/// Manager for node catchup synchronization
pub struct CatchupManager {
    /// Client to communicate with Go Master
    executor_client: Arc<ExecutorClient>,
    /// Own Go Master socket (for fallback/identity)
    _own_socket: String,
    /// WAN peer RPC addresses (e.g., "192.168.1.100:19000")
    peer_rpc_addresses: Vec<String>,
    /// Current catchup state
    state: RwLock<CatchupState>,
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
            Ok(block) => block,
            Err(e) => {
                error!("🚨 [CATCHUP] Failed to get last block from Go: {}", e);
                return Err(anyhow::anyhow!("Failed to get last block from Go: {}", e));
            }
        };

        // 2. Get Network State from Peers (TCP-based only)
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
                    res
                }
                Err(e) => {
                    warn!(
                        "⚠️ [CATCHUP] WAN peer query failed ({}), using local state",
                        e
                    );
                    (
                        local_go_epoch,
                        local_go_last_block,
                        "local".to_string(),
                        local_last_commit,
                    )
                }
            }
        } else {
            // No peers configured (single node?), assume we are the network
            (
                local_go_epoch,
                local_go_last_block,
                "local".to_string(),
                local_last_commit,
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

    /// Actively fetch missing blocks from peers and write them to local Go.
    /// This closes the block gap that passive polling can never close.
    pub async fn sync_blocks_from_peers(
        &self,
        go_last_block: u64,
        network_block: u64,
    ) -> Result<u64> {
        if self.peer_rpc_addresses.is_empty() {
            return Err(anyhow::anyhow!("No peer_rpc_addresses configured"));
        }

        let missing_from = go_last_block + 1;
        if missing_from > network_block {
            info!(
                "✅ [CATCHUP SYNC] No blocks to fetch (go={} >= network={})",
                go_last_block, network_block
            );
            return Ok(0);
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
                Ok(b) => b,
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
}
