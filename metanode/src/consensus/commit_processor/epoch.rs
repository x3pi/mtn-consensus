// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

use consensus_core::{BlockAPI, CommittedSubDag};
use std::sync::Arc;
use tracing::info;

/// Queue all user transactions from a failed commit for processing in the next epoch
pub async fn queue_commit_transactions_for_next_epoch(
    subdag: &CommittedSubDag,
    pending_transactions_queue: &Arc<tokio::sync::Mutex<Vec<Vec<u8>>>>,
    commit_index: u32,
    global_exec_index: u64,
    epoch: u64,
) {
    let has_end_of_epoch = subdag.extract_end_of_epoch_transaction().is_some();

    // ═══════════════════════════════════════════════════════════════════
    // PERFORMANCE FIX (H3): Collect all TXs first, then acquire lock ONCE.
    // Previously, the mutex was locked/unlocked per transaction (10K+ cycles
    // for large commits), causing significant overhead under high throughput.
    // ═══════════════════════════════════════════════════════════════════
    let mut tx_batch: Vec<Vec<u8>> = Vec::new();
    let mut skipped_count = 0;

    for block in subdag.blocks.iter() {
        for tx in block.transactions() {
            let tx_data = tx.data();

            // Skip EndOfEpoch system transactions - they are epoch-specific
            if has_end_of_epoch && is_end_of_epoch_transaction(tx_data) {
                skipped_count += 1;
                continue;
            }

            tx_batch.push(tx_data.to_vec());
        }
    }

    // Single lock acquisition for entire batch
    if !tx_batch.is_empty() {
        let queued_count = tx_batch.len();
        let mut queue = pending_transactions_queue.lock().await;
        queue.extend(tx_batch);

        info!("✅ [TX FLOW] Queued {} transactions from failed commit {} (global_exec_index={}, epoch={}) for next epoch",
            queued_count, commit_index, global_exec_index, epoch);
    }

    if skipped_count > 0 {
        info!("ℹ️  [TX FLOW] Skipped {} system transactions from failed commit {} (not suitable for next epoch)",
            skipped_count, commit_index);
    }
}

/// Detect if a transaction is an EndOfEpoch system transaction using
/// proper BCS deserialization (canonical check).
///
/// FORK-SAFETY FIX (C3): Previously used `String::from_utf8_lossy().contains("EndOfEpoch")`
/// which is heuristic and could produce false positives on user TXs containing
/// that substring. Now uses the same BCS deserialization as
/// `CommittedSubDag::extract_end_of_epoch_transaction()` for consistency.
pub fn is_end_of_epoch_transaction(tx_data: &[u8]) -> bool {
    consensus_core::SystemTransaction::from_bytes(tx_data)
        .is_ok_and(|sys_tx| sys_tx.is_end_of_epoch())
}
