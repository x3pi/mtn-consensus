// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

//! ConsensusNode methods — transaction handling, mode transitions, sync, shutdown, etc.
//!
//! Extracted from `node/mod.rs` to keep the module file focused on struct/type definitions.

use anyhow::Result;
use consensus_config::Committee;
use consensus_core::ReconfigState;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tracing::{info, warn};

use crate::config::NodeConfig;
use crate::node::tx_submitter::TransactionSubmitter;

use super::{epoch_monitor, epoch_transition_manager, notification_server, queue, sync};
use super::{ConsensusNode, NodeMode};

impl ConsensusNode {
    pub fn transaction_submitter(&self) -> Option<Arc<dyn TransactionSubmitter>> {
        self.transaction_client_proxy
            .as_ref()
            .map(|proxy| proxy.clone() as Arc<dyn TransactionSubmitter>)
    }

    pub async fn check_transaction_acceptance(&self) -> (bool, bool, String) {
        if self.authority.is_none() {
            return (false, false, "Node is still initializing".to_string());
        }
        if self.last_transition_hash.is_some() {
            return (
                false,
                true, // QUEUE TXs during epoch transition (was: false).
                // The UDS tx_socket_server already handles queuing + replay.
                // TXs are buffered and re-submitted after new epoch starts via
                // submit_queued_transactions(). Nonce conflicts are handled by
                // the Go nonce validation layer (tx.GetNonce() == as.Nonce()).
                format!(
                    "Epoch transition in progress: epoch {} -> {}",
                    self.current_epoch,
                    self.current_epoch + 1
                ),
            );
        }
        if self.is_transitioning.load(Ordering::SeqCst) {
            return (false, true, "Epoch transition in progress".to_string());
        }
        if !self.should_accept_tx().await {
            return (false, true, "Reconfiguration in progress".to_string());
        }
        (true, false, "Node is ready".to_string())
    }

    #[allow(dead_code)]
    pub async fn queue_transaction_for_next_epoch(&self, tx_data: Vec<u8>) -> Result<()> {
        queue::queue_transaction(
            &self.pending_transactions_queue,
            &self.storage_path,
            tx_data,
        )
        .await
    }

    pub async fn queue_transactions_for_next_epoch(
        &self,
        tx_data_list: Vec<Vec<u8>>,
    ) -> Result<()> {
        queue::queue_transactions(
            &self.pending_transactions_queue,
            &self.storage_path,
            tx_data_list,
        )
        .await
    }

    pub async fn submit_queued_transactions(&mut self) -> Result<usize> {
        queue::submit_queued_transactions(self).await
    }

    pub async fn shutdown(&mut self) -> Result<()> {
        info!("Shutting down consensus node...");
        let _ = self.stop_sync_task().await;
        if let Some(authority) = self.authority.take() {
            authority.stop().await;
        }
        info!("Consensus node stopped");
        Ok(())
    }

    // Delegated methods
    /// Check if node mode needs to change based on committee membership.
    /// `bypass_lock`: Set to true when calling from within an ongoing epoch transition
    /// to avoid deadlock (the epoch lock is already held by the transition).
    pub async fn check_and_update_node_mode(
        &mut self,
        committee: &Committee,
        config: &NodeConfig,
        bypass_lock: bool,
    ) -> Result<()> {
        // FIX: Use protocol_key matching for consistent identity check
        let own_protocol_pubkey = self.protocol_keypair.public();
        let should_be_validator = committee
            .authorities()
            .any(|(_, authority)| authority.protocol_key == own_protocol_pubkey);

        let new_mode = if should_be_validator {
            NodeMode::Validator
        } else {
            NodeMode::SyncOnly
        };

        if self.node_mode != new_mode {
            // ====================================================================
            // UNIFIED STATE MANAGER: Check with manager before mode transition
            // Skip lock check if bypass_lock is true (called from within epoch transition)
            // ====================================================================
            let state_manager = epoch_transition_manager::get_state_manager();

            // For bootstrap case (manager not initialized), we proceed without lock
            // and just track the transition directly in the mode switch logic below
            if !bypass_lock {
                if let Some(ref manager) = state_manager {
                    // Try to acquire mode transition lock
                    if let Err(e) = manager
                        .try_start_mode_transition(
                            should_be_validator,
                            "check_and_update_node_mode",
                        )
                        .await
                    {
                        info!(
                            "⏳ [NODE MODE] Cannot start mode transition: {} (will retry later)",
                            e
                        );
                        return Ok(()); // Skip this cycle, will be retried
                    }
                } else {
                    warn!("⚠️ [NODE MODE] StateTransitionManager not initialized, proceeding without lock (bootstrap)");
                }
            } else {
                info!("🔓 [NODE MODE] Bypassing lock check (called from epoch transition context)");
            }

            info!(
                "🔄 [NODE MODE] Switching from {:?} to {:?}",
                self.node_mode, new_mode
            );
            match (&self.node_mode, &new_mode) {
                (NodeMode::SyncOnly, NodeMode::Validator) => {
                    // =======================================================================
                    // CENTRALIZED ORDERING FIX: SyncOnly → Validator
                    // ORDER: 1. Stop sync 2. Update mode 3. Notify Go
                    // This prevents race conditions where sync continues after consensus starts
                    // =======================================================================

                    // STEP 1: Stop sync task FIRST (before mode change or Go notification)
                    info!("🛑 [TRANSITION] STEP 1: Stopping sync task before becoming Validator");
                    let _ = self.stop_sync_task().await;

                    // STEP 2: Update mode atomically
                    self.node_mode = new_mode.clone();

                    // MODE TRANSITION STATE LOG
                    info!(
                        "📊 [MODE TRANSITION] SyncOnly → Validator: epoch={}, last_global_exec_index={}, commit_index={}",
                        self.current_epoch,
                        self.last_global_exec_index,
                        self.current_commit_index.load(std::sync::atomic::Ordering::SeqCst)
                    );

                    // STEP 3: Notify Go AFTER sync is fully stopped
                    // Consensus will produce blocks starting from last_global_exec_index + 1
                    let consensus_start_block = self.last_global_exec_index + 1;
                    if let Some(ref executor_client) = self.executor_client {
                        match executor_client
                            .set_consensus_start_block(consensus_start_block)
                            .await
                        {
                            Ok((success, last_sync_block, msg)) => {
                                if success {
                                    info!(
                                        "✅ [TRANSITION HANDOFF] Go confirmed sync complete up to block {} (consensus starts at {})",
                                        last_sync_block, consensus_start_block
                                    );
                                } else {
                                    warn!(
                                        "⚠️ [TRANSITION HANDOFF] Go sync not ready: {} (last_sync_block={}, need={})",
                                        msg, last_sync_block, consensus_start_block - 1
                                    );
                                    // Optionally wait for sync to catch up
                                    if let Ok((reached, current, _)) = executor_client
                                        .wait_for_sync_to_block(consensus_start_block - 1, 30)
                                        .await
                                    {
                                        if reached {
                                            info!("✅ [TRANSITION HANDOFF] Go sync caught up to block {}", current);
                                        } else {
                                            warn!("⚠️ [TRANSITION HANDOFF] Timeout waiting for Go sync (current={})", current);
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                warn!("⚠️ [TRANSITION HANDOFF] Failed to notify Go of consensus start: {}", e);
                            }
                        }
                    }

                    // CRITICAL FIX 2026-02-01: Restart epoch monitor after becoming Validator
                    // The epoch monitor is designed to run continuously for ALL node modes.
                    // Previously, we only take() the handle without restarting, which left
                    // Validators without epoch monitoring backup when they miss EndOfEpoch txs.
                    let _ = self.epoch_monitor_handle.take(); // Take old handle first
                                                              // Start new epoch monitor for Validator mode
                    if let Ok(Some(handle)) =
                        epoch_monitor::start_unified_epoch_monitor(&self.executor_client, config)
                    {
                        info!("🔄 [EPOCH MONITOR] Restarted epoch monitor after promotion to Validator");
                        self.epoch_monitor_handle = Some(handle);
                    }

                    // Mark mode transition as complete in state manager
                    if let Some(ref manager) = state_manager {
                        manager.complete_mode_transition(true).await;
                    }
                }
                (NodeMode::Validator, NodeMode::SyncOnly) => {
                    // =======================================================================
                    // CENTRALIZED ORDERING FIX: Validator → SyncOnly
                    // ORDER: 1. Stop authority 1.5. Verify Go sync 2. Update mode 3. Notify Go 4. Start sync
                    // This prevents race conditions where consensus continues after sync starts
                    // =======================================================================

                    // STEP 1: Stop authority FIRST (before mode change or Go notification)
                    info!("🛑 [TRANSITION] STEP 1: Stopping consensus authority before becoming SyncOnly");
                    if let Some(auth) = self.authority.take() {
                        auth.stop().await;
                        info!("✅ [TRANSITION] Authority stopped successfully");
                    }

                    // STEP 2: Update mode atomically (only after Go has caught up)
                    // Note: Go catch-up is ensured by the caller (stop_authority_and_poll_go in epoch_transition.rs)
                    self.node_mode = new_mode.clone();

                    // MODE TRANSITION STATE LOG
                    info!(
                        "📊 [MODE TRANSITION] Validator → SyncOnly: epoch={}, last_global_exec_index={}, commit_index={}",
                        self.current_epoch,
                        self.last_global_exec_index,
                        self.current_commit_index.load(std::sync::atomic::Ordering::SeqCst)
                    );

                    // STEP 3: Notify Go AFTER authority is fully stopped
                    let last_consensus_block = self.last_global_exec_index;
                    if let Some(ref executor_client) = self.executor_client {
                        match executor_client
                            .set_sync_start_block(last_consensus_block)
                            .await
                        {
                            Ok((success, sync_start_block, msg)) => {
                                if success {
                                    info!(
                                        "✅ [TRANSITION HANDOFF] Go will start sync from block {} (consensus ended at {})",
                                        sync_start_block, last_consensus_block
                                    );
                                } else {
                                    warn!("⚠️ [TRANSITION HANDOFF] Go sync start notification failed: {}", msg);
                                }
                            }
                            Err(e) => {
                                warn!(
                                    "⚠️ [TRANSITION HANDOFF] Failed to notify Go of sync start: {}",
                                    e
                                );
                            }
                        }
                    }

                    // STEP 4: Start sync task (now safe - authority is stopped)
                    let _ = self.start_sync_task(config).await;
                    // Start unified epoch monitor for demotion recovery
                    if let Ok(Some(handle)) =
                        epoch_monitor::start_unified_epoch_monitor(&self.executor_client, config)
                    {
                        info!("🔄 [EPOCH MONITOR] Started unified epoch monitor after demotion to SyncOnly");
                        self.epoch_monitor_handle = Some(handle);
                    }

                    // Mark mode transition as complete in state manager
                    if let Some(ref manager) = state_manager {
                        manager.complete_mode_transition(false).await;
                    }
                }
                _ => {
                    self.node_mode = new_mode.clone();
                }
            }
        }
        Ok(())
    }

    pub async fn start_sync_task(&mut self, config: &NodeConfig) -> Result<()> {
        // Start notification server for event-driven transitions
        if let Err(e) = self.start_notification_server(config).await {
            warn!("⚠️ Failed to start notification server: {}", e);
        }
        sync::start_sync_task(self, config).await
    }

    pub async fn stop_sync_task(&mut self) -> Result<()> {
        // Stop notification server as well when stopping sync
        if let Some(handle) = self.notification_server_handle.take() {
            info!("🛑 [NOTIFICATION SERVER] Stopping...");
            handle.abort();
        }
        sync::stop_sync_task(self).await
    }

    pub async fn start_notification_server(&mut self, config: &NodeConfig) -> Result<()> {
        // Only start if not already running
        if self.notification_server_handle.is_some() {
            return Ok(());
        }

        let socket_path = std::path::PathBuf::from(&config.executor_receive_socket_path)
            .parent()
            .unwrap_or_else(|| std::path::Path::new("/tmp"))
            .join(format!("metanode-notification-{}.sock", config.node_id));
        let sender = self.epoch_transition_sender.clone();

        let server = notification_server::EpochNotificationServer::new(
            socket_path,
            move |epoch, timestamp, boundary| {
                // Forward to transition handler
                if let Err(e) = sender.send((epoch, timestamp, boundary)) {
                    return Err(anyhow::anyhow!(
                        "Failed to forward epoch notification: {}",
                        e
                    ));
                }
                Ok(())
            },
        );

        let handle = tokio::spawn(async move { server.start().await });

        self.notification_server_handle = Some(handle);
        info!("🚀 [NOTIFICATION SERVER] Started background task");
        Ok(())
    }

    /// Flush all buffered blocks to Go Master before shutdown
    /// This ensures no blocks are lost during shutdown
    pub async fn flush_blocks_to_go_master(&self) -> Result<()> {
        if let Some(ref executor_client) = self.executor_client {
            info!("🔄 [SHUTDOWN] Flushing buffered blocks to Go Master...");
            match executor_client.flush_buffer().await {
                Ok(_) => {
                    info!("✅ [SHUTDOWN] Successfully flushed all blocks to Go Master");
                    Ok(())
                }
                Err(e) => {
                    warn!("⚠️  [SHUTDOWN] Failed to flush blocks to Go Master: {}", e);
                    // Don't fail shutdown if flush fails - Go will buffer and process sequentially
                    Ok(())
                }
            }
        } else {
            info!("ℹ️  [SHUTDOWN] No executor client configured, skipping block flush");
            Ok(())
        }
    }

    pub async fn transition_to_epoch_from_system_tx(
        &mut self,
        new_epoch: u64,
        boundary_block_from_tx: u64, // CHANGED: Now boundary_block from EndOfEpoch tx, not timestamp
        synced_global_exec_index: u64, // CHANGED: Use global_exec_index (u64) instead of commit_index (u32)
        config: &NodeConfig,
    ) -> Result<()> {
        super::transition::transition_to_epoch_from_system_tx(
            self,
            new_epoch,
            boundary_block_from_tx,
            synced_global_exec_index,
            config,
        )
        .await
    }

    // Reconfiguration delegators

    pub async fn update_execution_lock_epoch(&self, new_epoch: u64) {
        *self.execution_lock.write().await = new_epoch;
    }

    pub async fn reset_reconfig_state(&self) {
        *self.reconfig_state.write().await = ReconfigState::default();
    }

    pub async fn close_user_certs(&self) {
        self.reconfig_state.write().await.close_user_certs();
    }

    pub async fn should_accept_tx(&self) -> bool {
        self.reconfig_state.read().await.should_accept_tx()
    }
}

// ============================================================================
// DIAGNOSTIC: Drop implementation to track unexpected node drops
// ============================================================================
impl Drop for ConsensusNode {
    fn drop(&mut self) {
        // Log when node is being dropped for debugging race conditions
        tracing::warn!(
            "🔴 [CONSENSUS NODE DROP] Node being dropped! epoch={}, mode={:?}, has_authority={}, is_transitioning={}",
            self.current_epoch,
            self.node_mode,
            self.authority.is_some(),
            self.is_transitioning.load(Ordering::SeqCst)
        );

        // Capture backtrace to identify the source of the drop
        let backtrace = std::backtrace::Backtrace::capture();
        if backtrace.status() == std::backtrace::BacktraceStatus::Captured {
            tracing::warn!("🔴 [CONSENSUS NODE DROP] Backtrace:\n{}", backtrace);
        } else {
            tracing::warn!(
                "🔴 [CONSENSUS NODE DROP] Backtrace not available (set RUST_BACKTRACE=1 to enable)"
            );
        }
    }
}
