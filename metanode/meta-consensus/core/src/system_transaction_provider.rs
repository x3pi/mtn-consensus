// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

use crate::system_transaction::SystemTransaction;
use consensus_config::Epoch;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use tracing::{info, warn};

/// Provider for system transactions (similar to Sui's EndOfEpochTransaction provider)
/// This replaces the Proposal/Vote/Quorum mechanism with system transactions
pub trait SystemTransactionProvider: Send + Sync {
    /// Get system transactions to include in the next block
    /// Returns None if no system transaction should be included
    fn get_system_transactions(
        &self,
        current_epoch: Epoch,
        current_commit_index: u32,
    ) -> Option<Vec<SystemTransaction>>;
}

/// Default implementation that checks if epoch transition is needed
pub struct DefaultSystemTransactionProvider {
    /// Current epoch
    current_epoch: Arc<RwLock<Epoch>>,
    /// Epoch duration in seconds
    epoch_duration_seconds: u64,
    /// Epoch start timestamp in milliseconds
    epoch_start_timestamp_ms: Arc<RwLock<u64>>,
    /// Time-based epoch change enabled
    time_based_enabled: bool,
    /// Last commit index where we checked for epoch change
    last_checked_commit_index: Arc<RwLock<u32>>,
    /// Commit index buffer (number of commits to wait after detecting system transaction)
    /// OPTIMIZED: Default reduced to 50 commits for faster epoch transitions (was 100)
    /// With commit rate 200 commits/s, 50 commits = 250ms (faster than 100 commits = 500ms)
    commit_index_buffer: u32,
    /// BACKPRESSURE: Go executor lag (in blocks). When Go is lagging behind Rust,
    /// EndOfEpoch emission is suppressed to prevent epoch desynchronization.
    /// Updated by CommitProcessor after each flush_buffer() cycle.
    go_lag: Arc<AtomicU64>,
    /// Maximum lag threshold: EndOfEpoch is suppressed when go_lag >= this value
    go_lag_threshold: u64,
}

impl DefaultSystemTransactionProvider {
    /// Create a new provider with default buffer (50 commits for faster transition)
    pub fn new(
        current_epoch: Epoch,
        epoch_duration_seconds: u64,
        epoch_start_timestamp_ms: u64,
        time_based_enabled: bool,
    ) -> Self {
        Self::new_with_buffer(
            current_epoch,
            epoch_duration_seconds,
            epoch_start_timestamp_ms,
            time_based_enabled,
            50, // OPTIMIZED: Reduced from 100 to 50 commits for faster epoch transition
        )
    }

    /// Create a new provider with custom commit index buffer
    ///
    /// # Arguments
    /// * `commit_index_buffer` - Number of commits to wait after detecting system transaction
    ///   before triggering epoch transition.
    ///   - For low commit rate (<10 commits/s): 10-20 commits is sufficient
    ///   - For medium commit rate (10-100 commits/s): 20-50 commits recommended
    ///   - For high commit rate (>100 commits/s): 50-100 commits recommended
    ///   - OPTIMIZED: Default reduced from 100 to 50 commits for faster transitions
    ///   - With 200 commits/s, 50 commits = 250ms (faster than 100 commits = 500ms)
    pub fn new_with_buffer(
        current_epoch: Epoch,
        epoch_duration_seconds: u64,
        epoch_start_timestamp_ms: u64,
        time_based_enabled: bool,
        commit_index_buffer: u32,
    ) -> Self {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("SystemTime before UNIX_EPOCH — clock is misconfigured")
            .as_millis() as u64;
        let elapsed_seconds = (now_ms.saturating_sub(epoch_start_timestamp_ms)) / 1000;

        info!(
            "📅 SystemTransactionProvider initialized: epoch={}, epoch_start={}ms, now={}ms, elapsed={}s, duration={}s, time_based_enabled={}",
            current_epoch,
            epoch_start_timestamp_ms,
            now_ms,
            elapsed_seconds,
            epoch_duration_seconds,
            time_based_enabled
        );

        // RESTART FIX: If epoch_start_timestamp is significantly in the past (elapsed >= duration),
        // reset to now() to prevent immediate EndOfEpoch trigger on restart.
        // Without this fix, restarting after downtime > epoch_duration causes:
        // 1. Immediate EndOfEpoch system tx creation
        // 2. A few blocks committed before Go catches up
        // 3. Deferred transition deadlock (Go behind, no consensus to send blocks)
        // This mirrors the same logic in update_epoch() (line 122-130).
        let effective_epoch_start = if time_based_enabled
            && elapsed_seconds >= epoch_duration_seconds
        {
            warn!(
                "⚠️  SystemTransactionProvider: Epoch start timestamp is {}s old (>= duration {}s). \
                 RESETTING to now() to prevent immediate EndOfEpoch on restart. \
                 Next epoch change will trigger after {}s from now.",
                elapsed_seconds,
                epoch_duration_seconds,
                epoch_duration_seconds
            );
            now_ms
        } else {
            epoch_start_timestamp_ms
        };

        Self {
            current_epoch: Arc::new(RwLock::new(current_epoch)),
            epoch_duration_seconds,
            epoch_start_timestamp_ms: Arc::new(RwLock::new(effective_epoch_start)),
            time_based_enabled,
            last_checked_commit_index: Arc::new(RwLock::new(0)),
            commit_index_buffer,
            go_lag: Arc::new(AtomicU64::new(0)),
            go_lag_threshold: 50, // Suppress EndOfEpoch when Go is >= 50 blocks behind
        }
    }

    /// Update current epoch (called after epoch transition)
    /// FORK-SAFETY: When syncing from Go, prioritize Go's timestamp over local calculation
    /// Only override if timestamp is significantly in the past (consensus delay scenario)
    pub async fn update_epoch(&self, new_epoch: Epoch, new_timestamp_ms: u64) {
        // Use blocking write from async context (safe - we're not blocking the runtime thread)
        *self
            .current_epoch
            .write()
            .unwrap_or_else(|p| p.into_inner()) = new_epoch;

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("SystemTime before UNIX_EPOCH — clock is misconfigured")
            .as_millis() as u64;

        // FORK-SAFETY: More lenient check for Go-synced timestamps
        // Only override if timestamp is more than 5 seconds in the past
        // This allows for reasonable clock differences between Go and Rust
        let adjusted_timestamp_ms = if new_timestamp_ms < now_ms.saturating_sub(5000) {
            warn!(
                "⚠️  [EPOCH TIMING] SystemTransactionProvider::update_epoch: new_timestamp_ms {}ms is significantly in the past (now={}ms, diff={}ms). \
                 This may indicate clock sync issues. Using current time to prevent rapid transitions.",
                new_timestamp_ms,
                now_ms,
                now_ms.saturating_sub(new_timestamp_ms)
            );
            now_ms
        } else {
            new_timestamp_ms
        };

        *self
            .epoch_start_timestamp_ms
            .write()
            .unwrap_or_else(|p| p.into_inner()) = adjusted_timestamp_ms;
        *self
            .last_checked_commit_index
            .write()
            .unwrap_or_else(|p| p.into_inner()) = 0;

        info!(
            "📅 SystemTransactionProvider::update_epoch: epoch={}, epoch_start_timestamp_ms={}ms (from new_timestamp_ms={}ms, now={}ms)",
            new_epoch,
            adjusted_timestamp_ms,
            new_timestamp_ms,
            now_ms
        );
    }

    /// Get a clone of the go_lag Arc for sharing with CommitProcessor
    pub fn go_lag_handle(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.go_lag)
    }

    /// Update the Go lag value (called by CommitProcessor after flush_buffer)
    pub fn set_go_lag(&self, lag: u64) {
        self.go_lag.store(lag, Ordering::Relaxed);
    }

    /// Check if epoch transition should be triggered
    fn should_trigger_epoch_change(&self, current_commit_index: u32) -> bool {
        if !self.time_based_enabled {
            tracing::debug!("⏰ SystemTransactionProvider: time_based_enabled=false, skipping epoch change check");
            return false;
        }

        // Only check once per commit index to avoid spam
        let last_checked = *self
            .last_checked_commit_index
            .read()
            .unwrap_or_else(|p| p.into_inner());
        if current_commit_index <= last_checked {
            tracing::debug!(
                "⏰ SystemTransactionProvider: Already checked commit_index {} (last_checked={}), skipping",
                current_commit_index,
                last_checked
            );
            return false;
        }

        // Check if enough time has elapsed
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("SystemTime before UNIX_EPOCH — clock is misconfigured")
            .as_millis() as u64;

        let epoch_start = *self
            .epoch_start_timestamp_ms
            .read()
            .unwrap_or_else(|p| p.into_inner());
        let elapsed_seconds = (now_ms - epoch_start) / 1000;

        // Log epoch start timestamp for debugging
        tracing::debug!(
            "⏰ SystemTransactionProvider: Time check - epoch_start={}ms, now={}ms, elapsed={}s, duration={}s, remaining={}s",
            epoch_start,
            now_ms,
            elapsed_seconds,
            self.epoch_duration_seconds,
            self.epoch_duration_seconds.saturating_sub(elapsed_seconds)
        );

        let time_elapsed = elapsed_seconds >= self.epoch_duration_seconds;

        // ═══════════════════════════════════════════════════════════════════════
        // BACKPRESSURE REMOVED (2026-03-19)
        //
        // PROBLEM: The old backpressure mechanism delayed EndOfEpoch when Go lagged
        // behind (go_lag >= 50 blocks). This caused a FATAL DEADLOCK:
        //   1. Node X delays EndOfEpoch due to backpressure
        //   2. Other nodes (no backpressure) emit EndOfEpoch → transition to epoch N+1
        //   3. Consensus halts for node X (epoch mismatch rejects all blocks)
        //   4. No new commits → should_trigger_epoch_change() never called again
        //   5. Force timeout (checked per-commit) NEVER FIRES → node X stuck FOREVER
        //
        // SOLUTION: Emit EndOfEpoch purely based on time elapsed. All nodes emit at
        // roughly the same time → consensus agrees on a single EndOfEpoch commit.
        // Go lag is already handled by stop_authority_and_poll_go() during the actual
        // epoch transition, which waits up to 5 minutes for Go to catch up.
        // ═══════════════════════════════════════════════════════════════════════
        let should_trigger = time_elapsed;

        if should_trigger {
            let current_go_lag = self.go_lag.load(Ordering::Relaxed);
            info!(
                "⏰ SystemTransactionProvider: Epoch change triggered - epoch={}, elapsed={}s, duration={}s, commit_index={}, go_lag={} (backpressure disabled — Go lag handled during transition)",
                *self.current_epoch.read().unwrap_or_else(|p| p.into_inner()),
                elapsed_seconds,
                self.epoch_duration_seconds,
                current_commit_index,
                current_go_lag
            );
        } else {
            // Log periodically when close to threshold
            if elapsed_seconds % 10 == 0
                || elapsed_seconds >= self.epoch_duration_seconds.saturating_sub(30)
            {
                tracing::debug!(
                    "⏰ SystemTransactionProvider: Epoch change check - epoch={}, elapsed={}s, duration={}s, remaining={}s, commit_index={}",
                    *self.current_epoch.read().unwrap_or_else(|p| p.into_inner()),
                    elapsed_seconds,
                    self.epoch_duration_seconds,
                    self.epoch_duration_seconds.saturating_sub(elapsed_seconds),
                    current_commit_index
                );
            }
        }

        should_trigger
    }
}

impl SystemTransactionProvider for DefaultSystemTransactionProvider {
    fn get_system_transactions(
        &self,
        current_epoch: Epoch,
        current_commit_index: u32,
    ) -> Option<Vec<SystemTransaction>> {
        tracing::debug!(
            "🔍 SystemTransactionProvider::get_system_transactions called: epoch={}, commit_index={}, time_based_enabled={}",
            current_epoch,
            current_commit_index,
            self.time_based_enabled
        );

        // Check if epoch transition should be triggered FIRST (before updating last_checked)
        // This ensures we don't skip the check if commit_index hasn't increased
        let should_trigger = self.should_trigger_epoch_change(current_commit_index);

        // Only update last_checked if we actually checked (not skipped due to already checked)
        // This allows re-checking if commit_index increases
        {
            let mut last_checked = self
                .last_checked_commit_index
                .write()
                .unwrap_or_else(|p| p.into_inner());
            if current_commit_index > *last_checked {
                *last_checked = current_commit_index;
            }
        }

        if !should_trigger {
            return None;
        }

        // Create EndOfEpoch system transaction
        // FORK-SAFETY: Simplified design - EndOfEpoch contains only:
        // - new_epoch: current_epoch + 1 (deterministic)
        // - boundary_block: global_exec_index at which to transition (deterministic)
        // Timestamp is derived from the block header at boundary_block by the Go layer,
        // ensuring all nodes derive the same timestamp deterministically.
        let new_epoch = current_epoch + 1;

        // FORK-SAFETY WARNING: transition_commit_index may differ between nodes
        // if they have different current_commit_index values.
        // This is acceptable because:
        // 1. System transaction will be included in a committed block
        // 2. All nodes will see the same system transaction in the committed block
        // 3. Transition happens when commit_index >= transition_commit_index (from the committed block)
        // However, to be extra safe, we should use the commit_index from the committed block
        // that contains the system transaction, not the current_commit_index when creating it.
        //
        // For now, we use current_commit_index + 10, but the actual transition_commit_index
        // should be read from the committed block that contains this system transaction.
        //
        // SAFETY: Use checked_add to handle overflow explicitly
        // If commit_index is too large (near u32::MAX), we use u32::MAX - 1 to ensure
        // transition can still be triggered, but log a warning.
        //
        // BUFFER SAFETY: Increased from 10 to configurable buffer (default 100) for high commit rate systems.
        // With commit rate 200 commits/s:
        // - 10 commits = 50ms (not safe for network propagation)
        // - 100 commits = 500ms (safer, allows network delay and processing time)
        let transition_commit_index = current_commit_index
            .checked_add(self.commit_index_buffer)
            .unwrap_or_else(|| {
                warn!(
                    "⚠️ [FORK-SAFETY] commit_index {} quá lớn (gần u32::MAX), không thể cộng buffer {}. \
                     Sử dụng u32::MAX - 1 làm transition_commit_index. \
                     Điều này có thể gây vấn đề nếu commit_index tiếp tục tăng. \
                     Cân nhắc reset commit_index hoặc tăng epoch duration.",
                    current_commit_index, self.commit_index_buffer
                );
                u32::MAX - 1
            });

        info!(
            "📝 SystemTransactionProvider: Creating EndOfEpoch transaction - epoch {} -> {}, current_commit_index={}, boundary_block={}",
            current_epoch,
            new_epoch,
            current_commit_index,
            transition_commit_index
        );

        // SIMPLIFIED: EndOfEpoch only contains new_epoch and boundary_block
        // Timestamp will be derived from block header at boundary_block (deterministic)
        let system_tx = SystemTransaction::end_of_epoch(
            new_epoch,
            transition_commit_index as u64, // boundary_block is the last block before transition
        );

        Some(vec![system_tx])
    }
}
