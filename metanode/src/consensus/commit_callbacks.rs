// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tokio::runtime::Handle;
use tracing::{info, warn};

/// Creates a commit index callback that updates the shared commit index
pub fn create_commit_index_callback(
    current_commit_index: Arc<AtomicU32>,
) -> impl Fn(u32) + Send + Sync + 'static {
    move |index| {
        current_commit_index.store(index, Ordering::SeqCst);
    }
}

/// Creates a global execution index callback that updates the shared global exec index
pub fn create_global_exec_index_callback(
    shared_last_global_exec_index: Arc<tokio::sync::Mutex<u64>>,
) -> impl Fn(u64) + Send + Sync + 'static {
    move |global_exec_index| {
        // Try to update the shared index via tokio runtime
        let shared_index = shared_last_global_exec_index.clone();
        let update_result = Handle::try_current().map(|handle| {
            handle.spawn(async move {
                let mut lock = shared_index.lock().await;
                *lock = global_exec_index;
            })
        });

        match update_result {
            Ok(_) => {
                // Successfully spawned task to update shared index
                info!(
                    "✅ [GLOBAL_EXEC_INDEX] Updated shared index to {}",
                    global_exec_index
                );
            }
            Err(_) => {
                // No runtime handle available, log warning
                // The update will happen in process_commit() anyway via shared_last_global_exec_index
                warn!("⚠️  [GLOBAL_EXEC_INDEX] No runtime handle available for callback update. Update will happen in process_commit()");
            }
        }
    }
}

/// Creates an epoch transition callback that sends transition requests via channel
pub fn create_epoch_transition_callback(
    epoch_transition_sender: tokio::sync::mpsc::UnboundedSender<(u64, u64, u64)>,
) -> impl Fn(u64, u64, u64) -> Result<(), anyhow::Error> + Send + Sync + 'static {
    move |new_epoch, boundary_timestamp_ms, synced_global_exec_index| {
        info!("🎯 [SYSTEM TX CALLBACK] EndOfEpoch detected: epoch={}, boundary_timestamp_ms={}, synced_global_exec_index={}",
            new_epoch, boundary_timestamp_ms, synced_global_exec_index);

        if let Err(e) = epoch_transition_sender.send((
            new_epoch,
            boundary_timestamp_ms,
            synced_global_exec_index,
        )) {
            warn!(
                "❌ [SYSTEM TX CALLBACK] Failed to send epoch transition request: {}",
                e
            );
            return Err(anyhow::anyhow!(
                "Failed to send epoch transition request: {}",
                e
            ));
        }
        Ok(())
    }
}
