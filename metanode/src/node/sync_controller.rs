//! Centralized Sync State Controller
//!
//! This module provides centralized management for the sync task lifecycle
//! and mode transitions. It ensures atomic state changes and prevents
//! race conditions between enable/disable operations.

use anyhow::Result;
use std::sync::atomic::{AtomicU8, Ordering};
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::node::rust_sync_node::RustSyncHandle;

/// Sync task state
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncState {
    /// Sync is disabled (Validator mode - gets blocks from DAG)
    Disabled = 0,
    /// Sync is enabled and running (SyncOnly mode - fetches from peers)
    Enabled = 1,
    /// Sync is currently stopping
    Stopping = 2,
    /// Sync is currently starting
    Starting = 3,
}

impl From<u8> for SyncState {
    fn from(v: u8) -> Self {
        match v {
            0 => SyncState::Disabled,
            1 => SyncState::Enabled,
            2 => SyncState::Stopping,
            3 => SyncState::Starting,
            _ => SyncState::Disabled,
        }
    }
}

impl std::fmt::Display for SyncState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SyncState::Disabled => write!(f, "Disabled"),
            SyncState::Enabled => write!(f, "Enabled"),
            SyncState::Stopping => write!(f, "Stopping"),
            SyncState::Starting => write!(f, "Starting"),
        }
    }
}

/// Centralized controller for sync task and mode transitions
///
/// This controller ensures:
/// - Atomic state transitions (no race between stop/start)
/// - Mutex protection for concurrent transition attempts
/// - Clean shutdown signaling via watch channel
pub struct SyncController {
    /// Current sync state (atomic for fast reads)
    state: AtomicU8,

    /// Mutex to prevent concurrent state changes
    transition_lock: Mutex<()>,

    /// Current sync task handle
    handle: Mutex<Option<RustSyncHandle>>,

    /// Shutdown signal sender (watch channel allows multiple receivers)
    shutdown_tx: tokio::sync::watch::Sender<bool>,

    /// Shutdown signal receiver template
    #[allow(dead_code)]
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
}

impl SyncController {
    /// Create a new SyncController
    pub fn new() -> Self {
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

        Self {
            state: AtomicU8::new(SyncState::Disabled as u8),
            transition_lock: Mutex::new(()),
            handle: Mutex::new(None),
            shutdown_tx,
            shutdown_rx,
        }
    }

    /// Get current sync state
    pub fn state(&self) -> SyncState {
        SyncState::from(self.state.load(Ordering::SeqCst))
    }

    /// Check if sync is enabled (running)
    pub fn is_enabled(&self) -> bool {
        self.state() == SyncState::Enabled
    }

    /// Check if sync is disabled
    #[allow(dead_code)]
    pub fn is_disabled(&self) -> bool {
        self.state() == SyncState::Disabled
    }

    /// Check if a transition is in progress
    #[allow(dead_code)]
    pub fn is_transitioning(&self) -> bool {
        matches!(self.state(), SyncState::Starting | SyncState::Stopping)
    }

    /// Get a shutdown receiver for the sync task
    #[allow(dead_code)]
    pub fn get_shutdown_receiver(&self) -> tokio::sync::watch::Receiver<bool> {
        self.shutdown_tx.subscribe()
    }

    /// Enable sync (start sync task)
    ///
    /// Returns Ok(true) if sync was enabled, Ok(false) if already enabled or transitioning
    pub async fn enable_sync(&self, handle: RustSyncHandle) -> Result<bool> {
        // Acquire transition lock to prevent concurrent transitions
        let _lock = self.transition_lock.lock().await;

        let current = self.state();

        // Check if already enabled or transitioning
        if current == SyncState::Enabled {
            info!("🔄 [SYNC-CTRL] Sync already enabled, skipping");
            return Ok(false);
        }

        if current == SyncState::Starting || current == SyncState::Stopping {
            warn!(
                "⚠️ [SYNC-CTRL] Transition in progress ({}), cannot enable",
                current
            );
            return Ok(false);
        }

        // Set state to Starting
        self.state
            .store(SyncState::Starting as u8, Ordering::SeqCst);
        info!("🚀 [SYNC-CTRL] Enabling sync (state: Disabled → Starting)");

        // Reset shutdown signal
        let _ = self.shutdown_tx.send(false);

        // Store the handle
        {
            let mut handle_guard = self.handle.lock().await;
            *handle_guard = Some(handle);
        }

        // Set state to Enabled
        self.state.store(SyncState::Enabled as u8, Ordering::SeqCst);
        info!("✅ [SYNC-CTRL] Sync enabled (state: Starting → Enabled)");

        Ok(true)
    }

    /// Disable sync (stop sync task)
    ///
    /// Returns Ok(true) if sync was disabled, Ok(false) if already disabled or transitioning
    pub async fn disable_sync(&self) -> Result<bool> {
        // Acquire transition lock to prevent concurrent transitions
        let _lock = self.transition_lock.lock().await;

        let current = self.state();

        // Check if already disabled
        if current == SyncState::Disabled {
            info!("🔄 [SYNC-CTRL] Sync already disabled, skipping");
            return Ok(false);
        }

        if current == SyncState::Starting || current == SyncState::Stopping {
            warn!(
                "⚠️ [SYNC-CTRL] Transition in progress ({}), cannot disable",
                current
            );
            return Ok(false);
        }

        // Set state to Stopping
        self.state
            .store(SyncState::Stopping as u8, Ordering::SeqCst);
        info!("🛑 [SYNC-CTRL] Disabling sync (state: Enabled → Stopping)");

        // Send shutdown signal
        let _ = self.shutdown_tx.send(true);

        // Take and stop the handle
        {
            let mut handle_guard = self.handle.lock().await;
            if let Some(handle) = handle_guard.take() {
                handle.stop().await;
                info!("✅ [SYNC-CTRL] Sync task stopped");
            } else {
                warn!("⚠️ [SYNC-CTRL] No sync handle to stop");
            }
        }

        // Set state to Disabled
        self.state
            .store(SyncState::Disabled as u8, Ordering::SeqCst);
        info!("✅ [SYNC-CTRL] Sync disabled (state: Stopping → Disabled)");

        Ok(true)
    }

    /// Force disable sync without checking state
    /// Use this for cleanup during shutdown
    #[allow(dead_code)]
    pub async fn force_disable(&self) {
        let _lock = self.transition_lock.lock().await;

        // Send shutdown signal
        let _ = self.shutdown_tx.send(true);

        // Take and stop the handle
        {
            let mut handle_guard = self.handle.lock().await;
            if let Some(handle) = handle_guard.take() {
                handle.stop().await;
            }
        }

        self.state
            .store(SyncState::Disabled as u8, Ordering::SeqCst);
        info!("🛑 [SYNC-CTRL] Sync force disabled");
    }
}

impl Default for SyncController {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sync_state_conversion() {
        assert_eq!(SyncState::from(0), SyncState::Disabled);
        assert_eq!(SyncState::from(1), SyncState::Enabled);
        assert_eq!(SyncState::from(2), SyncState::Stopping);
        assert_eq!(SyncState::from(3), SyncState::Starting);
        assert_eq!(SyncState::from(255), SyncState::Disabled); // Invalid defaults to Disabled
    }

    #[test]
    fn test_initial_state() {
        let controller = SyncController::new();
        assert!(controller.is_disabled());
        assert!(!controller.is_enabled());
        assert!(!controller.is_transitioning());
    }

    #[test]
    fn test_sync_state_display() {
        assert_eq!(format!("{}", SyncState::Disabled), "Disabled");
        assert_eq!(format!("{}", SyncState::Enabled), "Enabled");
        assert_eq!(format!("{}", SyncState::Stopping), "Stopping");
        assert_eq!(format!("{}", SyncState::Starting), "Starting");
    }

    #[test]
    fn test_shutdown_receiver_initial_value() {
        let controller = SyncController::new();
        let rx = controller.get_shutdown_receiver();
        // Initial shutdown signal should be false
        assert!(!(*rx.borrow()));
    }

    #[test]
    fn test_default_trait() {
        let controller = SyncController::default();
        assert!(controller.is_disabled());
        assert!(!controller.is_enabled());
    }

    #[test]
    fn test_is_transitioning_states() {
        let controller = SyncController::new();

        // Starting state should be transitioning
        controller
            .state
            .store(SyncState::Starting as u8, Ordering::SeqCst);
        assert!(controller.is_transitioning());
        assert!(!controller.is_enabled());
        assert!(!controller.is_disabled());

        // Stopping state should be transitioning
        controller
            .state
            .store(SyncState::Stopping as u8, Ordering::SeqCst);
        assert!(controller.is_transitioning());

        // Enabled state should NOT be transitioning
        controller
            .state
            .store(SyncState::Enabled as u8, Ordering::SeqCst);
        assert!(!controller.is_transitioning());
        assert!(controller.is_enabled());
    }
}
