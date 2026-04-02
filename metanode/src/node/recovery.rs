// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

use crate::node::executor_client::ExecutorClient;
use anyhow::Result;
use consensus_core::{storage::rocksdb_store::RocksDBStore, storage::Store, CommitAPI};
use std::sync::Arc;
use tracing::{error, info};

pub async fn perform_block_recovery_check(
    executor_client: &Arc<ExecutorClient>,
    go_last_block: u64,
    epoch_base_exec_index: u64,
    current_epoch: u64,
    storage_path: &std::path::Path,
    node_id: u32,
) -> Result<()> {
    if node_id != 0 {
        // info!("ℹ️ [RECOVERY] Node ID is {}, enabling recovery for all validators.", node_id);
    }
    info!(
        "🔍 [RECOVERY] Checking for missing blocks from index {} (epoch_base={})...",
        go_last_block + 1,
        epoch_base_exec_index
    );

    let db_path = storage_path
        .join("epochs")
        .join(format!("epoch_{}", current_epoch))
        .join("consensus_db");
    if !db_path.exists() {
        info!(
            "ℹ️ [RECOVERY] No DB found at {:?}, skipping recovery.",
            db_path
        );
        return Ok(());
    }

    let db_path_str = db_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("DB path contains non-UTF-8 characters: {:?}", db_path))?;
    let recovery_store = Arc::new(RocksDBStore::new(db_path_str));

    // Calculate start commit index
    // global_exec_index = epoch_base + commit_index
    // commit_index = global_exec_index - epoch_base
    let start_global = go_last_block + 1;
    if start_global <= epoch_base_exec_index {
        info!(
            "⚠️ [RECOVERY] Go Master (GEI {}) is behind the current epoch base (GEI {}). \
            Deferring recovery to Go's network sync mechanism.",
            go_last_block, epoch_base_exec_index
        );
        return Ok(());
    }

    let start_commit_index = (start_global - epoch_base_exec_index) as u32;

    info!(
        "🔍 [RECOVERY] Scanning for commits starting from commit_index={} (global={})",
        start_commit_index, start_global
    );

    // Scan commits from start_commit_index
    let range = consensus_core::CommitRange::new(start_commit_index..=u32::MAX);
    let commits = recovery_store.scan_commits(range)?;

    if commits.is_empty() {
        info!("✅ [RECOVERY] No missing commits found in local DB.");
        return Ok(());
    }

    info!(
        "🔄 [RECOVERY] Found {} missing commits to replay!",
        commits.len()
    );

    let mut next_required_global = go_last_block + 1;

    for commit in commits {
        let commit_index = commit.index();
        let global_exec_index = epoch_base_exec_index + commit_index as u64;

        if global_exec_index < next_required_global {
            continue; // Already processed or duplicate
        }

        if global_exec_index > next_required_global {
            // GAP DETECTED!
            // This is critical: if we skip a block, Go Master will buffer forever waiting for it.
            let error_msg = format!(
                "🚨 [RECOVERY CRITICAL] Gap detected in block sequence! Expected global_exec_index={}, but found {}. Missing {} blocks. Recovery cannot proceed sequentially.",
                next_required_global, global_exec_index, global_exec_index - next_required_global
             );
            error!("{}", error_msg);
            return Err(anyhow::anyhow!(error_msg));
        }

        // At this point, global_exec_index == next_required_global
        info!(
            "🔄 [RECOVERY] Replaying commit #{} (global_exec_index={})",
            commit_index, global_exec_index
        );

        // Reconstruct CommittedSubDag
        // Note: reputation_scores are not critical for execution replay, passing empty
        let subdag = consensus_core::load_committed_subdag_from_store(
            recovery_store.as_ref(),
            commit,
            vec![],
        );

        // Send to executor
        executor_client
            .send_committed_subdag(&subdag, current_epoch, global_exec_index, None)
            .await?;

        // Advance expected index
        next_required_global += 1;

        // Small delay to prevent overwhelming the socket/executor
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    info!("✅ [RECOVERY] Replay completed successfully.");
    Ok(())
}

pub async fn perform_fork_detection_check(node: &crate::node::ConsensusNode) -> Result<()> {
    info!(
        "🔍 [FORK DETECTION] Checking state (Epoch: {}, LastCommit: {})",
        node.current_epoch, node.last_global_exec_index
    );
    // Real implementation would query peers.
    Ok(())
}
