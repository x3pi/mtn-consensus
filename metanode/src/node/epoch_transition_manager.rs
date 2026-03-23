// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

//! Unified State Transition Manager
//!
//! This module provides a SINGLE entry point for ALL state transitions:
//! 1. Epoch transitions (epoch N → epoch N+1)
//! 2. Mode transitions (Validator ↔ SyncOnly)
//!
//! By centralizing all transitions, we prevent race conditions and ensure
//! only one transition can occur at a time.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

/// Minimum time between transitions (debounce)
const DEFAULT_DEBOUNCE_DURATION: Duration = Duration::from_secs(3);

/// Type of transition being performed
#[derive(Debug, Clone, PartialEq)]
pub enum TransitionType {
    /// Epoch change (e.g., epoch 5 → epoch 6)
    EpochChange { from_epoch: u64, to_epoch: u64 },
    /// Mode change (e.g., SyncOnly → Validator)
    ModeChange { from_mode: String, to_mode: String },
    /// Combined: Epoch change that also triggers mode change
    #[allow(dead_code)]
    EpochAndModeChange {
        from_epoch: u64,
        to_epoch: u64,
        from_mode: String,
        to_mode: String,
    },
}

impl std::fmt::Display for TransitionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EpochChange {
                from_epoch,
                to_epoch,
            } => {
                write!(f, "Epoch({} → {})", from_epoch, to_epoch)
            }
            Self::ModeChange { from_mode, to_mode } => {
                write!(f, "Mode({} → {})", from_mode, to_mode)
            }
            Self::EpochAndModeChange {
                from_epoch,
                to_epoch,
                from_mode,
                to_mode,
            } => {
                write!(
                    f,
                    "Epoch({} → {}) + Mode({} → {})",
                    from_epoch, to_epoch, from_mode, to_mode
                )
            }
        }
    }
}

/// Unified manager for ALL state transitions.
/// Ensures only one transition can occur at a time.
pub struct StateTransitionManager {
    /// Current epoch known to the manager
    current_epoch: AtomicU64,

    /// Current mode (stored as atomic for fast access)
    /// 0 = SyncOnly, 1 = Validator
    current_mode: AtomicU64,

    /// Flag indicating if ANY transition is currently in progress
    transition_in_progress: AtomicBool,

    /// Type of current transition (for debugging)
    current_transition: Mutex<Option<(TransitionType, String)>>, // (type, source)

    /// Time of last successful transition (for debouncing)
    last_transition_time: Mutex<Option<Instant>>,

    /// Time when the current transition was started (for stale detection)
    transition_started_at: Mutex<Option<Instant>>,

    /// Minimum duration between transitions
    debounce_duration: Duration,
}

impl StateTransitionManager {
    /// Create a new manager
    pub fn new(initial_epoch: u64, is_validator: bool) -> Self {
        let mode = if is_validator { 1 } else { 0 };
        info!(
            "🔧 [STATE MANAGER] Initializing: epoch={}, mode={}",
            initial_epoch,
            if is_validator {
                "Validator"
            } else {
                "SyncOnly"
            }
        );
        Self {
            current_epoch: AtomicU64::new(initial_epoch),
            current_mode: AtomicU64::new(mode),
            transition_in_progress: AtomicBool::new(false),
            current_transition: Mutex::new(None),
            last_transition_time: Mutex::new(None),
            transition_started_at: Mutex::new(None),
            debounce_duration: DEFAULT_DEBOUNCE_DURATION,
        }
    }

    /// Get current epoch
    #[allow(dead_code)]
    pub fn current_epoch(&self) -> u64 {
        self.current_epoch.load(Ordering::SeqCst)
    }

    /// Check if currently in Validator mode
    #[allow(dead_code)]
    pub fn is_validator(&self) -> bool {
        self.current_mode.load(Ordering::SeqCst) == 1
    }

    /// Check if any transition is in progress
    #[allow(dead_code)]
    pub fn is_transition_in_progress(&self) -> bool {
        self.transition_in_progress.load(Ordering::SeqCst)
    }

    /// Try to start an epoch transition
    pub async fn try_start_epoch_transition(
        &self,
        target_epoch: u64,
        source: &str,
    ) -> Result<(), TransitionError> {
        let current = self.current_epoch.load(Ordering::SeqCst);

        if target_epoch <= current {
            debug!(
                "⏭️ [STATE MANAGER] Skipping epoch transition to {} (current={}), source={}",
                target_epoch, current, source
            );
            return Err(TransitionError::EpochAlreadyCurrent {
                current,
                requested: target_epoch,
            });
        }

        self.try_start_transition_internal(
            TransitionType::EpochChange {
                from_epoch: current,
                to_epoch: target_epoch,
            },
            source,
        )
        .await
    }

    /// Try to start a mode transition
    pub async fn try_start_mode_transition(
        &self,
        to_validator: bool,
        source: &str,
    ) -> Result<(), TransitionError> {
        let current_is_validator = self.current_mode.load(Ordering::SeqCst) == 1;

        if current_is_validator == to_validator {
            debug!(
                "⏭️ [STATE MANAGER] Skipping mode transition (already {}), source={}",
                if to_validator {
                    "Validator"
                } else {
                    "SyncOnly"
                },
                source
            );
            return Err(TransitionError::ModeAlreadyCurrent);
        }

        let from_mode = if current_is_validator {
            "Validator"
        } else {
            "SyncOnly"
        };
        let to_mode = if to_validator {
            "Validator"
        } else {
            "SyncOnly"
        };

        self.try_start_transition_internal(
            TransitionType::ModeChange {
                from_mode: from_mode.to_string(),
                to_mode: to_mode.to_string(),
            },
            source,
        )
        .await
    }

    /// Internal: Try to start any transition
    async fn try_start_transition_internal(
        &self,
        transition_type: TransitionType,
        source: &str,
    ) -> Result<(), TransitionError> {
        // Try to atomically set transition_in_progress from false to true
        let was_in_progress = self.transition_in_progress.compare_exchange(
            false,
            true,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );

        if was_in_progress.is_err() {
            let current_transition = self.current_transition.lock().await;
            warn!(
                "⚠️ [STATE MANAGER] Transition already in progress: {:?}, rejecting {} from {}",
                current_transition, transition_type, source
            );
            return Err(TransitionError::TransitionInProgress {
                current: current_transition
                    .as_ref()
                    .map(|(t, s)| format!("{} by {}", t, s)),
                requested: format!("{} by {}", transition_type, source),
            });
        }

        // Check debounce
        {
            let last_time = self.last_transition_time.lock().await;
            if let Some(last) = *last_time {
                let elapsed = last.elapsed();
                if elapsed < self.debounce_duration {
                    let remaining = self.debounce_duration - elapsed;
                    debug!(
                        "⏳ [STATE MANAGER] Debouncing - {}ms remaining, source={}",
                        remaining.as_millis(),
                        source
                    );
                    self.transition_in_progress.store(false, Ordering::SeqCst);
                    return Err(TransitionError::Debouncing {
                        remaining_ms: remaining.as_millis() as u64,
                    });
                }
            }
        }

        // Record current transition
        {
            let mut current = self.current_transition.lock().await;
            *current = Some((transition_type.clone(), source.to_string()));
        }

        // Record transition start time for stale detection
        {
            let mut started = self.transition_started_at.lock().await;
            *started = Some(Instant::now());
        }

        info!(
            "🔒 [STATE MANAGER] Started {}, source={}",
            transition_type, source
        );

        Ok(())
    }

    /// Complete an epoch transition successfully
    pub async fn complete_epoch_transition(&self, new_epoch: u64) {
        let old_epoch = self.current_epoch.swap(new_epoch, Ordering::SeqCst);

        self.complete_transition_internal(&format!("Epoch {} → {}", old_epoch, new_epoch))
            .await;
    }

    /// Complete a mode transition successfully  
    pub async fn complete_mode_transition(&self, is_validator: bool) {
        let mode = if is_validator { 1 } else { 0 };
        let old_mode = self.current_mode.swap(mode, Ordering::SeqCst);

        let from_str = if old_mode == 1 {
            "Validator"
        } else {
            "SyncOnly"
        };
        let to_str = if is_validator {
            "Validator"
        } else {
            "SyncOnly"
        };

        self.complete_transition_internal(&format!("Mode {} → {}", from_str, to_str))
            .await;
    }

    /// Internal: Complete any transition
    async fn complete_transition_internal(&self, description: &str) {
        // Update last transition time
        {
            let mut last_time = self.last_transition_time.lock().await;
            *last_time = Some(Instant::now());
        }

        // Clear transition start time
        {
            let mut started = self.transition_started_at.lock().await;
            *started = None;
        }

        let source = {
            let mut current = self.current_transition.lock().await;
            let source = current.as_ref().map(|(_, s)| s.clone());
            *current = None;
            source
        };

        self.transition_in_progress.store(false, Ordering::SeqCst);

        info!(
            "✅ [STATE MANAGER] Completed {}, source={:?}",
            description, source
        );
    }

    /// Mark any transition as failed
    pub async fn fail_transition(&self, reason: &str) {
        let transition_info = {
            let mut current = self.current_transition.lock().await;
            let info = current.clone();
            *current = None;
            info
        };

        // Clear transition start time
        {
            let mut started = self.transition_started_at.lock().await;
            *started = None;
        }

        self.transition_in_progress.store(false, Ordering::SeqCst);

        warn!(
            "❌ [STATE MANAGER] Failed {:?}: {}",
            transition_info, reason
        );
    }

    /// Force-clear a stale transition that has been running too long.
    /// Returns true if a stale transition was cleared.
    /// This is a safety mechanism for the epoch monitor to recover from
    /// stuck transitions (e.g., when consensus can't form quorum after restore).
    #[allow(dead_code)]
    pub async fn force_clear_stale_transition(&self, max_age: Duration) -> bool {
        if !self.transition_in_progress.load(Ordering::SeqCst) {
            return false;
        }

        let is_stale = {
            let started = self.transition_started_at.lock().await;
            match *started {
                Some(start_time) => start_time.elapsed() > max_age,
                None => {
                    // No start time recorded but transition_in_progress is true
                    // This shouldn't happen but treat as stale
                    true
                }
            }
        };

        if !is_stale {
            return false;
        }

        let transition_info = {
            let mut current = self.current_transition.lock().await;
            let info = current.clone();
            *current = None;
            info
        };

        {
            let mut started = self.transition_started_at.lock().await;
            *started = None;
        }

        self.transition_in_progress.store(false, Ordering::SeqCst);

        warn!(
            "🚨 [STATE MANAGER] FORCE-CLEARED stale transition {:?} (exceeded {:?}). \
             This indicates a previous transition hung (e.g., consensus had no quorum).",
            transition_info, max_age
        );

        true
    }

    /// Force update state (for sync after restart)
    #[allow(dead_code)]
    pub fn set_current_state(&self, epoch: u64, is_validator: bool) {
        let old_epoch = self.current_epoch.swap(epoch, Ordering::SeqCst);
        let mode = if is_validator { 1 } else { 0 };
        let old_mode = self.current_mode.swap(mode, Ordering::SeqCst);

        if old_epoch != epoch || old_mode != mode {
            info!(
                "🔄 [STATE MANAGER] Force-updated: epoch {} → {}, mode {} → {}",
                old_epoch,
                epoch,
                if old_mode == 1 {
                    "Validator"
                } else {
                    "SyncOnly"
                },
                if is_validator {
                    "Validator"
                } else {
                    "SyncOnly"
                }
            );
        }
    }
}

/// Errors that can occur when requesting a transition
#[derive(Debug, Clone)]
pub enum TransitionError {
    /// The requested epoch is already current or in the past
    EpochAlreadyCurrent { current: u64, requested: u64 },

    /// The requested mode is already current
    ModeAlreadyCurrent,

    /// A transition is already in progress
    TransitionInProgress {
        current: Option<String>,
        requested: String,
    },

    /// Debounce period has not elapsed
    Debouncing { remaining_ms: u64 },
}

impl std::fmt::Display for TransitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EpochAlreadyCurrent { current, requested } => {
                write!(
                    f,
                    "Epoch {} already current (requested: {})",
                    current, requested
                )
            }
            Self::ModeAlreadyCurrent => {
                write!(f, "Mode already current")
            }
            Self::TransitionInProgress { current, requested } => {
                write!(
                    f,
                    "Transition in progress {:?}, rejected {}",
                    current, requested
                )
            }
            Self::Debouncing { remaining_ms } => {
                write!(f, "Debouncing - {}ms remaining", remaining_ms)
            }
        }
    }
}

impl std::error::Error for TransitionError {}

/// Global state transition manager instance
static STATE_MANAGER: tokio::sync::OnceCell<Arc<StateTransitionManager>> =
    tokio::sync::OnceCell::const_new();

/// Initialize the global state manager
pub async fn init_state_manager(
    initial_epoch: u64,
    is_validator: bool,
) -> Arc<StateTransitionManager> {
    STATE_MANAGER
        .get_or_init(|| async {
            Arc::new(StateTransitionManager::new(initial_epoch, is_validator))
        })
        .await
        .clone()
}

/// Get the global state manager (must be initialized first)
pub fn get_state_manager() -> Option<Arc<StateTransitionManager>> {
    STATE_MANAGER.get().cloned()
}

/// Alias for backwards compatibility
/// Used by epoch_monitor.rs and epoch_transition.rs
pub fn get_epoch_manager() -> Option<Arc<StateTransitionManager>> {
    get_state_manager()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_epoch_transition() {
        let manager = Arc::new(StateTransitionManager::new(1, false));

        manager.try_start_epoch_transition(2, "test").await.unwrap();
        assert!(manager.is_transition_in_progress());

        manager.complete_epoch_transition(2).await;
        assert!(!manager.is_transition_in_progress());
        assert_eq!(manager.current_epoch(), 2);
    }

    #[tokio::test]
    async fn test_mode_transition() {
        let manager = Arc::new(StateTransitionManager::new(1, false));
        assert!(!manager.is_validator());

        manager
            .try_start_mode_transition(true, "test")
            .await
            .unwrap();
        assert!(manager.is_transition_in_progress());

        manager.complete_mode_transition(true).await;
        assert!(!manager.is_transition_in_progress());
        assert!(manager.is_validator());
    }

    #[tokio::test]
    async fn test_concurrent_transitions_blocked() {
        let manager = Arc::new(StateTransitionManager::new(1, false));

        // Start epoch transition
        manager
            .try_start_epoch_transition(2, "first")
            .await
            .unwrap();

        // Mode transition should be blocked
        let result = manager.try_start_mode_transition(true, "second").await;
        assert!(matches!(
            result,
            Err(TransitionError::TransitionInProgress { .. })
        ));

        // Another epoch transition should also be blocked
        let result = manager.try_start_epoch_transition(3, "third").await;
        assert!(matches!(
            result,
            Err(TransitionError::TransitionInProgress { .. })
        ));

        // Complete first transition
        manager.complete_epoch_transition(2).await;
        assert!(!manager.is_transition_in_progress());
    }
}
