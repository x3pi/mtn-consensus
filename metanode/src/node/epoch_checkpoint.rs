// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

//! Epoch Transition Checkpoint Module
//!
//! This module provides crash recovery for epoch transitions by saving
//! checkpoint state at each step. If a node crashes mid-transition,
//! it can resume from the last checkpoint on restart.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::info;

/// State machine for epoch transition
/// Each state represents a checkpoint in the transition process
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TransitionState {
    /// Transition not started
    NotStarted,

    /// Go has been notified about new epoch via advance_epoch
    AdvancedEpochToGo {
        epoch: u64,
        boundary_block: u64,
        boundary_gei: u64,
        timestamp_ms: u64,
    },

    /// Committee has been fetched from Go
    CommitteeFetched {
        epoch: u64,
        committee_hash: [u8; 32],
        validator_count: usize,
    },

    /// Old consensus authority has been stopped
    AuthorityStopped { epoch: u64, previous_epoch: u64 },

    /// New consensus authority started - transition complete
    Completed { epoch: u64, completed_at_ms: u64 },
}

impl TransitionState {
    pub fn epoch(&self) -> Option<u64> {
        match self {
            TransitionState::NotStarted => None,
            TransitionState::AdvancedEpochToGo { epoch, .. } => Some(*epoch),
            TransitionState::CommitteeFetched { epoch, .. } => Some(*epoch),
            TransitionState::AuthorityStopped { epoch, .. } => Some(*epoch),
            TransitionState::Completed { epoch, .. } => Some(*epoch),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            TransitionState::NotStarted => "NotStarted",
            TransitionState::AdvancedEpochToGo { .. } => "AdvancedEpochToGo",
            TransitionState::CommitteeFetched { .. } => "CommitteeFetched",
            TransitionState::AuthorityStopped { .. } => "AuthorityStopped",
            TransitionState::Completed { .. } => "Completed",
        }
    }
}

/// Checkpoint for epoch transition recovery
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransitionCheckpoint {
    /// Current state in the transition
    pub state: TransitionState,
    /// Timestamp when this checkpoint was created
    pub created_at_ms: u64,
    /// Node identifier for debugging
    pub node_id: String,
}

impl TransitionCheckpoint {
    /// Create a new checkpoint
    pub fn new(state: TransitionState, node_id: &str) -> Self {
        let created_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        Self {
            state,
            created_at_ms,
            node_id: node_id.to_string(),
        }
    }

    /// Save checkpoint to file (atomic write)
    pub async fn save(&self, path: &Path) -> Result<()> {
        let data = serde_json::to_vec_pretty(&self)?;

        // Write to temp file first, then rename for atomicity
        let temp_path = path.with_extension("tmp");
        tokio::fs::write(&temp_path, &data).await?;
        tokio::fs::rename(&temp_path, path).await?;

        info!(
            "💾 [CHECKPOINT] Saved transition checkpoint: state={}, epoch={:?}",
            self.state.name(),
            self.state.epoch()
        );

        Ok(())
    }

    /// Load checkpoint from file
    pub async fn load(path: &Path) -> Result<Option<Self>> {
        if !path.exists() {
            return Ok(None);
        }

        let data = tokio::fs::read(path).await?;
        let checkpoint: Self = serde_json::from_slice(&data)?;

        info!(
            "📂 [CHECKPOINT] Loaded transition checkpoint: state={}, epoch={:?}, created={}ms ago",
            checkpoint.state.name(),
            checkpoint.state.epoch(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64
                - checkpoint.created_at_ms
        );

        Ok(Some(checkpoint))
    }

    /// Remove checkpoint file (call after successful transition)
    pub async fn remove(path: &Path) -> Result<()> {
        if path.exists() {
            tokio::fs::remove_file(path).await?;
            info!("🗑️ [CHECKPOINT] Removed transition checkpoint");
        }
        Ok(())
    }
}

/// Manager for epoch transition checkpoints
pub struct CheckpointManager {
    /// Path to checkpoint file
    checkpoint_path: PathBuf,
    /// Node identifier
    node_id: String,
}

#[allow(dead_code)]
impl CheckpointManager {
    /// Create a new checkpoint manager
    pub fn new(data_dir: &Path, node_id: &str) -> Self {
        Self {
            checkpoint_path: data_dir.join("epoch_transition.checkpoint"),
            node_id: node_id.to_string(),
        }
    }

    /// Check if there's an incomplete transition
    pub async fn has_incomplete_transition(&self) -> bool {
        if let Ok(Some(checkpoint)) = TransitionCheckpoint::load(&self.checkpoint_path).await {
            !matches!(
                checkpoint.state,
                TransitionState::Completed { .. } | TransitionState::NotStarted
            )
        } else {
            false
        }
    }

    /// Get the incomplete transition state if any
    pub async fn get_incomplete_transition(&self) -> Result<Option<TransitionCheckpoint>> {
        let checkpoint = TransitionCheckpoint::load(&self.checkpoint_path).await?;

        if let Some(ref cp) = checkpoint {
            if matches!(
                cp.state,
                TransitionState::Completed { .. } | TransitionState::NotStarted
            ) {
                // Transition was completed or never started - clean up
                TransitionCheckpoint::remove(&self.checkpoint_path).await?;
                return Ok(None);
            }
        }

        Ok(checkpoint)
    }

    /// Save checkpoint for advance_epoch step
    pub async fn checkpoint_advance_epoch(
        &self,
        epoch: u64,
        boundary_block: u64,
        boundary_gei: u64,
        timestamp_ms: u64,
    ) -> Result<()> {
        let checkpoint = TransitionCheckpoint::new(
            TransitionState::AdvancedEpochToGo {
                epoch,
                boundary_block,
                boundary_gei,
                timestamp_ms,
            },
            &self.node_id,
        );
        checkpoint.save(&self.checkpoint_path).await
    }

    /// Save checkpoint for committee fetched step
    pub async fn checkpoint_committee_fetched(
        &self,
        epoch: u64,
        committee_hash: [u8; 32],
        validator_count: usize,
    ) -> Result<()> {
        let checkpoint = TransitionCheckpoint::new(
            TransitionState::CommitteeFetched {
                epoch,
                committee_hash,
                validator_count,
            },
            &self.node_id,
        );
        checkpoint.save(&self.checkpoint_path).await
    }

    /// Save checkpoint for authority stopped step
    pub async fn checkpoint_authority_stopped(
        &self,
        epoch: u64,
        previous_epoch: u64,
    ) -> Result<()> {
        let checkpoint = TransitionCheckpoint::new(
            TransitionState::AuthorityStopped {
                epoch,
                previous_epoch,
            },
            &self.node_id,
        );
        checkpoint.save(&self.checkpoint_path).await
    }

    /// Mark transition as complete and remove checkpoint
    pub async fn complete_transition(&self, epoch: u64) -> Result<()> {
        info!(
            "✅ [CHECKPOINT] Epoch {} transition completed successfully",
            epoch
        );
        TransitionCheckpoint::remove(&self.checkpoint_path).await
    }

    /// Get instructions for resuming from a checkpoint
    pub fn get_resume_instructions(state: &TransitionState) -> &'static str {
        match state {
            TransitionState::NotStarted => "No action needed - transition not started",
            TransitionState::AdvancedEpochToGo { .. } => {
                "Resume from: fetch_committee (Go already has epoch data)"
            }
            TransitionState::CommitteeFetched { .. } => {
                "Resume from: stop_authority (committee already loaded)"
            }
            TransitionState::AuthorityStopped { .. } => {
                "Resume from: start_new_epoch (old authority already stopped)"
            }
            TransitionState::Completed { .. } => "No action needed - transition completed",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_checkpoint_save_load() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.checkpoint");

        let checkpoint = TransitionCheckpoint::new(
            TransitionState::CommitteeFetched {
                epoch: 5,
                committee_hash: [0u8; 32],
                validator_count: 4,
            },
            "node-0",
        );

        checkpoint.save(&path).await.unwrap();

        let loaded = TransitionCheckpoint::load(&path).await.unwrap().unwrap();
        assert_eq!(loaded.state, checkpoint.state);
        assert_eq!(loaded.node_id, "node-0");
    }

    #[tokio::test]
    async fn test_checkpoint_manager() {
        let dir = tempdir().unwrap();
        let manager = CheckpointManager::new(dir.path(), "node-0");

        // Initially no incomplete transition
        assert!(!manager.has_incomplete_transition().await);

        // Save a checkpoint
        manager
            .checkpoint_advance_epoch(5, 1000, 1000, 123456789)
            .await
            .unwrap();

        // Now has incomplete transition
        assert!(manager.has_incomplete_transition().await);

        // Complete the transition
        manager.complete_transition(5).await.unwrap();

        // No longer has incomplete transition
        assert!(!manager.has_incomplete_transition().await);
    }
}
