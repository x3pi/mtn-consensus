// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

//! Dual-Stream Sync Controller (Hybrid: Active Peer Sync + Event-Driven Convergence)
//!
//! Manages two parallel block delivery streams during cold-start/restore:
//! - **Block Sync Stream**: Actively fetches new blocks from peer Go nodes → sends to local Go
//! - **DAG Consensus Stream**: ConsensusAuthority produces committed blocks from DAG
//!
//! ## Key Design Principle
//!
//! The controller actively polls peers for new blocks on every tick (not just when
//! Go block changes). This is critical because on cold-start, the fresh DAG may have
//! incompatible genesis blocks and can never produce commits — so peer sync is the
//! ONLY source of new blocks until the DAG catches up.
//!
//! Convergence is detected when DAG consensus becomes the sole driver of new blocks
//! (peer sync returns 0 new blocks for STABLE_THRESHOLD consecutive events).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::{info, warn};

use crate::node::catchup::CatchupManager;
use crate::node::executor_client::ExecutorClient;

/// Number of consecutive consensus-driven block events before declaring convergence.
const STABLE_THRESHOLD: u32 = 10;

/// Block gap threshold for fast-convergence check.
const CONVERGENCE_GAP: u64 = 5;

/// Active sync interval — how often to poll peers for new blocks (ms).
/// This is the primary sync loop, NOT just a change-detection poll.
const SYNC_INTERVAL_MS: u64 = 500;

/// Maximum batch size for fetching blocks from peers.
const FETCH_BATCH_SIZE: u64 = 500;

/// Controller for dual-stream sync during cold-start/restore.
///
/// Actively fetches blocks from peers AND monitors convergence.
pub struct DualStreamController {
    /// Client to communicate with local Go Master
    executor_client: Arc<ExecutorClient>,
    /// WAN peer RPC addresses for block fetching
    peer_rpc_addresses: Vec<String>,
    /// Signal to stop block sync stream
    shutdown: Arc<AtomicBool>,
}

impl DualStreamController {
    /// Create a new DualStreamController.
    pub fn new(
        executor_client: Arc<ExecutorClient>,
        peer_rpc_addresses: Vec<String>,
    ) -> Self {
        Self {
            executor_client,
            peer_rpc_addresses,
            shutdown: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Get shutdown signal handle (for external stop).
    #[allow(dead_code)]
    pub fn shutdown_signal(&self) -> Arc<AtomicBool> {
        self.shutdown.clone()
    }

    /// Spawn the block sync background task.
    ///
    /// Every SYNC_INTERVAL_MS (500ms):
    /// 1. Query network state from peers
    /// 2. If Go is behind network → actively fetch blocks from peers
    /// 3. Check if Go block changed → evaluate convergence
    ///
    /// Stops when consensus alone drives blocks for STABLE_THRESHOLD rounds.
    pub fn spawn_block_sync_stream(self) -> tokio::task::JoinHandle<()> {
        let executor = self.executor_client.clone();
        let peer_addrs = self.peer_rpc_addresses.clone();
        let shutdown = self.shutdown.clone();

        tokio::spawn(async move {
            info!(
                "🔄 [DUAL-STREAM] Block sync stream started (peers={}, active-sync)",
                peer_addrs.len()
            );

            if peer_addrs.is_empty() {
                warn!("⚠️ [DUAL-STREAM] No peers configured — block sync stream exiting immediately");
                return;
            }

            let catchup = CatchupManager::new(
                executor.clone(),
                String::new(),
                peer_addrs.clone(),
            );

            // State tracking
            let mut consensus_driven_streak: u32 = 0;
            let mut consecutive_errors: u32 = 0;
            let mut total_peer_synced: u64 = 0;
            let mut total_rounds: u64 = 0;
            let mut last_go_block: u64 = 0;

            // Initialize last_go_block
            if let Ok((b, _)) = executor.get_last_block_number().await {
                last_go_block = b;
            }

            loop {
                // Check shutdown signal
                if shutdown.load(Ordering::Relaxed) {
                    info!("🛑 [DUAL-STREAM] Shutdown signal received — stopping block sync");
                    break;
                }

                tokio::time::sleep(std::time::Duration::from_millis(SYNC_INTERVAL_MS)).await;
                total_rounds += 1;

                // 1. Get Go's current block number
                let go_block = match executor.get_last_block_number().await {
                    Ok((b, _)) => {
                        consecutive_errors = 0;
                        b
                    }
                    Err(e) => {
                        consecutive_errors += 1;
                        if consecutive_errors > 60 { // 60 * 500ms = 30s
                            warn!(
                                "⚠️ [DUAL-STREAM] Too many Go query errors ({}): {}. Stopping.",
                                consecutive_errors, e
                            );
                            break;
                        }
                        continue;
                    }
                };

                // 2. Query network state from peers
                let (net_epoch, net_block, _peer, net_commit) =
                    match crate::network::peer_rpc::query_peer_epochs_network(&peer_addrs).await {
                        Ok(res) => {
                            consecutive_errors = 0;
                            res
                        }
                        Err(_) => {
                            consecutive_errors += 1;
                            if consecutive_errors > 60 {
                                warn!("⚠️ [DUAL-STREAM] Too many peer query errors. Stopping.");
                                break;
                            }
                            continue;
                        }
                    };

                // 3. ACTIVE SYNC: Fetch blocks from peers if Go is behind network
                let mut peer_synced: u64 = 0;
                if go_block < net_block {
                    let fetch_to = std::cmp::min(
                        go_block + FETCH_BATCH_SIZE,
                        net_block,
                    );

                    if let Ok(synced) = catchup
                        .sync_blocks_from_peers(go_block, fetch_to)
                        .await
                    {
                        peer_synced = synced;
                        total_peer_synced += synced;
                    }
                }

                let go_block_advanced = go_block != last_go_block;
                let fully_caught_up_and_idle = go_block >= net_block && peer_synced == 0 && !go_block_advanced;

                // 4. CONVERGENCE CHECK: evaluate source of advancement or idle stability
                if go_block_advanced || fully_caught_up_and_idle {
                    if peer_synced == 0 {
                        // Advancement came purely from consensus, OR we are caught up and idle
                        consensus_driven_streak += 1;
                        if consensus_driven_streak == 1
                            || consensus_driven_streak % 5 == 0
                            || consensus_driven_streak >= STABLE_THRESHOLD - 2
                        {
                            let reason = if fully_caught_up_and_idle { "Idle stability" } else { "Consensus driving" };
                            info!(
                                "🏛️ [DUAL-STREAM] {}! streak={}/{} (Go: {} → {})",
                                reason, consensus_driven_streak, STABLE_THRESHOLD,
                                last_go_block, go_block
                            );
                        }
                    } else {
                        // Peer sync contributed — reset streak
                        if consensus_driven_streak > 0 {
                            info!(
                                "🔄 [DUAL-STREAM] Peer sync still needed (synced {}). Reset streak {} → 0",
                                peer_synced, consensus_driven_streak
                            );
                        }
                        consensus_driven_streak = 0;
                    }

                    last_go_block = go_block;
                }

                // 5. Log progress periodically
                if total_rounds % 20 == 0 { // Every 10 seconds (20 * 500ms)
                    let gap = net_block.saturating_sub(go_block);
                    info!(
                        "🔄 [DUAL-STREAM] round={}: Go={}, Net={}, gap={}, epoch={}, commit={}, streak={}, peer_total={}",
                        total_rounds, go_block, net_block, gap,
                        net_epoch, net_commit, consensus_driven_streak, total_peer_synced
                    );
                }

                // 6. Check convergence thresholds
                if consensus_driven_streak >= STABLE_THRESHOLD {
                    info!(
                        "✅ [DUAL-STREAM → CONSENSUS] Convergence! {} consecutive consensus-driven rounds. \
                         Stopping block sync. Stats: peer_synced={}, rounds={}",
                        consensus_driven_streak, total_peer_synced, total_rounds
                    );
                    break;
                }

                // Fast convergence: Go caught up + short streak
                if go_block >= net_block.saturating_sub(CONVERGENCE_GAP)
                    && consensus_driven_streak >= 3
                {
                    info!(
                        "✅ [DUAL-STREAM → CONSENSUS] Fast convergence! Go={} ≈ Net={} (gap≤{}), streak={}. \
                         total_peer_synced={}",
                        go_block, net_block, CONVERGENCE_GAP,
                        consensus_driven_streak, total_peer_synced
                    );
                    break;
                }
            }

            info!(
                "🏁 [DUAL-STREAM] Block sync stream completed. Total peer synced: {}, rounds: {}",
                total_peer_synced, total_rounds
            );
        })
    }
}
