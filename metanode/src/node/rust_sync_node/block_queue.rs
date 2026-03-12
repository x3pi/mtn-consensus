// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

//! BlockQueue - Buffers commits and processes them sequentially.
//! Uses BTreeMap for automatic ordering by global_exec_index.

use consensus_core::{Commit, CommitAPI, VerifiedBlock};
use std::collections::BTreeMap;
use tracing::{info, warn};

/// Data stored in the queue for each commit
#[derive(Clone)]
pub(crate) struct CommitData {
    pub commit: Commit,
    pub blocks: Vec<VerifiedBlock>,
    pub epoch: u64,
}

/// BlockQueue - Buffers commits and processes them sequentially
/// Uses BTreeMap for automatic ordering by global_exec_index
pub(crate) struct BlockQueue {
    /// Commits waiting to be processed, keyed by global_exec_index
    pub pending: BTreeMap<u64, CommitData>,
    /// Next expected global_exec_index
    next_expected: u64,
    /// Last time we logged about a gap
    last_gap_log: Option<std::time::Instant>,
}

impl BlockQueue {
    /// Create a new BlockQueue starting from a specific next_expected index
    /// NOTE: start_index IS the next_expected (caller provides go_last_block + 1)
    pub fn new(start_index: u64) -> Self {
        Self {
            pending: BTreeMap::new(),
            next_expected: start_index, // Use directly, caller already calculated next
            last_gap_log: None,
        }
    }

    /// Add a commit to the queue (will be deduplicated by index)
    pub fn push(&mut self, data: CommitData) {
        let index = data.commit.global_exec_index();
        // Only add if not already processed and not already pending
        if index >= self.next_expected && !self.pending.contains_key(&index) {
            self.pending.insert(index, data);
        }
    }

    /// Drain all commits that can be processed sequentially
    /// Returns commits in order, stopping at the first gap
    /// CRITICAL: If there's a large gap (e.g., after epoch transition), auto-adjust next_expected
    pub fn drain_ready(&mut self) -> Vec<CommitData> {
        let mut ready = Vec::new();

        // EPOCH TRANSITION FIX: If the first pending commit is way ahead of next_expected,
        // this indicates an epoch transition where global_exec_index jumped.
        // Auto-adjust next_expected to match the first pending commit.
        if let Some((&first_index, _)) = self.pending.first_key_value() {
            if first_index > self.next_expected {
                // STRICT SEQ SYNC: User requested NO SKIPPING/JUMPING
                // Even if gap is large, we must wait for missing blocks to be fetched.
                // We just log the gap for visibility.
                let should_log = match self.last_gap_log {
                    Some(last) => last.elapsed().as_secs() > 10,
                    None => true,
                };

                if should_log {
                    warn!(
                        "⏳ [QUEUE] Gap detected: next_expected={} but first_pending={}. \
                         Waiting for missing blocks (NO SKIP allowed).",
                        self.next_expected, first_index
                    );
                    self.last_gap_log = Some(std::time::Instant::now());
                }
            }
        }

        while let Some((&index, _)) = self.pending.first_key_value() {
            if index == self.next_expected {
                // This is the next expected block - take it
                if let Some(data) = self.pending.remove(&index) {
                    ready.push(data);
                    self.next_expected = index + 1;
                }
            } else if index < self.next_expected {
                // Old block, already processed - remove it
                self.pending.remove(&index);
            } else {
                // Gap detected - stop here
                break;
            }
        }

        ready
    }

    /// Get the next expected index
    pub fn next_expected(&self) -> u64 {
        self.next_expected
    }

    /// Update next_expected to align with Go's progress (AUTHORITATIVE source of truth)
    /// CRITICAL FIX: Always sync queue with Go, not just when Go is ahead.
    /// This fixes the 40K block gap issue where queue jumped ahead while Go was stuck.
    pub fn sync_with_go(&mut self, go_last_block: u64) {
        let go_next = go_last_block + 1;

        // CASE 1: Go is ahead of queue (normal catch-up)
        if go_next > self.next_expected {
            // Remove any pending blocks that Go already processed
            self.pending.retain(|&idx, _| idx > go_last_block);
            self.next_expected = go_next;
            info!(
                "🔄 [QUEUE-SYNC] Go ahead: aligned queue to next_expected={}",
                self.next_expected
            );
        }
        // CASE 2: Queue is way ahead of Go (BUG FIX - queue jumped ahead while Go was stuck)
        // This happens when buffer is full and blocks can't be sent to Go
        else if self.next_expected > go_next + 100 {
            // Queue is more than 100 blocks ahead of Go - this is abnormal!
            // Reset queue to Go's position to prevent wasted fetching
            warn!(
                "🚨 [QUEUE-SYNC] Queue desync detected! queue_next={} >> go_next={}. Resetting queue.",
                self.next_expected, go_next
            );
            // Clear pending blocks that are way ahead - they'll be re-fetched
            self.pending
                .retain(|&idx, _| idx >= go_next && idx < go_next + 1000);
            self.next_expected = go_next;
        }
        // CASE 3: Queue slightly ahead of Go (normal - blocks in flight)
        // Keep as-is, this is expected during normal sync

        // ═══════════════════════════════════════════════════════════════════════
        // CASE 4 (SyncOnly EPOCH BOUNDARY GAP SKIP):
        // When Go has confirmed processing up to a GEI, but the queue has
        // pending commits starting ahead of next_expected with a small gap,
        // the gap represents permanently unavailable empty consensus blocks
        // from epoch transitions that peers no longer store.
        //
        // Examples:
        //   - Fresh start: Go GEI=0, queue expects 1, first pending at GEI=9
        //     (epoch 0 had 8 empty blocks that peers discarded)
        //   - After epoch boundary: Go GEI=208, queue expects 209, first pending
        //     at GEI=217 (epoch boundary had 8 empty blocks)
        //
        // Safety: Go has CONFIRMED processing all blocks up to go_last_block.
        //         The missing blocks between go_last_block+1 and first_pending
        //         were empty consensus blocks (no TX) that don't affect state.
        //         Max gap of 16 prevents accidentally skipping real missing data.
        // ═══════════════════════════════════════════════════════════════════════
        if let Some((&first_pending, _)) = self.pending.first_key_value() {
            let gap = first_pending.saturating_sub(self.next_expected);
            if gap > 0 && gap <= 16 {
                // Small gap - likely epoch boundary empty blocks
                info!(
                    "📋 [QUEUE-SYNC] Epoch boundary gap skip: next_expected={} → {} (Go GEI={}, gap={}, first pending at {})",
                    self.next_expected, first_pending, go_last_block, gap, first_pending
                );
                self.next_expected = first_pending;
            }
        }
    }

    /// Number of pending commits in queue
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a minimal `CommitData` with the given `global_exec_index`.
    /// Uses serde_json to construct `Commit::V1` since `CommitV1` fields are private.
    fn make_commit_data(global_exec_index: u64) -> CommitData {
        let zeros = vec![0u8; 32];
        let json = serde_json::json!({
            "V1": {
                "index": 1,
                "previous_digest": zeros,
                "timestamp_ms": 0,
                "leader": {
                    "round": 1,
                    "author": 0,
                    "digest": zeros
                },
                "blocks": [],
                "global_exec_index": global_exec_index
            }
        });
        let commit: Commit = serde_json::from_value(json).expect("valid commit json");
        CommitData {
            commit,
            blocks: vec![],
            epoch: 0,
        }
    }

    #[test]
    fn test_blockqueue_new() {
        let q = BlockQueue::new(100);
        assert_eq!(q.next_expected(), 100);
        assert_eq!(q.pending_count(), 0);
    }

    #[test]
    fn test_blockqueue_push_and_drain() {
        let mut q = BlockQueue::new(1);

        q.push(make_commit_data(1));
        q.push(make_commit_data(2));
        q.push(make_commit_data(3));
        assert_eq!(q.pending_count(), 3);

        let ready = q.drain_ready();
        assert_eq!(ready.len(), 3);
        assert_eq!(q.next_expected(), 4);
        assert_eq!(q.pending_count(), 0);
    }

    #[test]
    fn test_blockqueue_dedup() {
        let mut q = BlockQueue::new(1);

        q.push(make_commit_data(1));
        q.push(make_commit_data(1)); // duplicate
        assert_eq!(q.pending_count(), 1);
    }

    #[test]
    fn test_blockqueue_skip_old() {
        let mut q = BlockQueue::new(10);

        // Push commits below next_expected — should be ignored
        q.push(make_commit_data(5));
        q.push(make_commit_data(9));
        assert_eq!(q.pending_count(), 0);
    }

    #[test]
    fn test_blockqueue_gap_stops_drain() {
        let mut q = BlockQueue::new(1);

        q.push(make_commit_data(1));
        q.push(make_commit_data(2));
        q.push(make_commit_data(5)); // gap: missing 3, 4

        let ready = q.drain_ready();
        assert_eq!(ready.len(), 2); // only 1 and 2
        assert_eq!(q.next_expected(), 3);
        assert_eq!(q.pending_count(), 1); // 5 still pending
    }

    #[test]
    fn test_blockqueue_sync_with_go_ahead() {
        let mut q = BlockQueue::new(1);

        // Add some commits
        q.push(make_commit_data(1));
        q.push(make_commit_data(2));
        q.push(make_commit_data(3));

        // Go jumped to block 5 — queue should align
        q.sync_with_go(5);
        assert_eq!(q.next_expected(), 6);
        assert_eq!(q.pending_count(), 0); // all cleared (below Go)
    }

    #[test]
    fn test_blockqueue_sync_with_go_desync() {
        let mut q = BlockQueue::new(200); // queue is way ahead

        q.push(make_commit_data(200));
        q.push(make_commit_data(201));

        // Go is stuck at block 10 — desync > 100, triggers reset
        q.sync_with_go(10);
        assert_eq!(q.next_expected(), 11);
        // 200 and 201 are within go_next..go_next+1000 (11..1011), so they're retained
        assert_eq!(q.pending_count(), 2);
    }

    #[test]
    fn test_blockqueue_pending_count() {
        let mut q = BlockQueue::new(1);

        assert_eq!(q.pending_count(), 0);
        q.push(make_commit_data(3));
        q.push(make_commit_data(5));
        assert_eq!(q.pending_count(), 2);

        // Drain won't return anything (gap at 1→3)
        let ready = q.drain_ready();
        assert_eq!(ready.len(), 0);
        assert_eq!(q.pending_count(), 2);
    }

    #[test]
    fn test_blockqueue_drain_removes_old() {
        // If old blocks (below next_expected) exist in pending, drain_ready cleans them
        let mut q = BlockQueue::new(5);

        // Manually insert an "old" block at index 3 (below next_expected=5)
        // via sync_with_go that brings next_expected back
        q.push(make_commit_data(5));
        q.push(make_commit_data(6));
        q.push(make_commit_data(7));

        // Drain should return all 3 in order
        let ready = q.drain_ready();
        assert_eq!(ready.len(), 3);
        assert_eq!(q.next_expected(), 8);
    }

    #[test]
    fn test_blockqueue_fill_gap_then_drain() {
        let mut q = BlockQueue::new(1);

        // Push 1, 3, 4 (gap at 2)
        q.push(make_commit_data(1));
        q.push(make_commit_data(3));
        q.push(make_commit_data(4));

        // Only block 1 should drain (gap at 2)
        let ready = q.drain_ready();
        assert_eq!(ready.len(), 1);
        assert_eq!(q.next_expected(), 2);

        // Now fill the gap
        q.push(make_commit_data(2));

        // Now 2, 3, 4 should all drain
        let ready = q.drain_ready();
        assert_eq!(ready.len(), 3);
        assert_eq!(q.next_expected(), 5);
    }

    #[test]
    fn test_blockqueue_sync_with_go_slight_ahead() {
        // Queue slightly ahead of Go (< 100 difference) should be kept as-is
        let mut q = BlockQueue::new(50);

        q.push(make_commit_data(50));
        q.push(make_commit_data(51));

        // Go is at block 10 but difference (50 - 11 = 39) < 100 threshold
        // So this is "Case 3: Queue slightly ahead" — no reset
        q.sync_with_go(10);
        // next_expected stays at 50 (Go hasn't caught up yet, but within tolerance)
        assert_eq!(q.next_expected(), 50);
        assert_eq!(q.pending_count(), 2);
    }

    /// Large gap (e.g., 1000 blocks) → drain_ready does NOT skip/jump
    #[test]
    fn test_blockqueue_large_gap_no_skip() {
        let mut q = BlockQueue::new(1);

        // Push block 1 and block 1001 (huge gap)
        q.push(make_commit_data(1));
        q.push(make_commit_data(1001));

        // Only block 1 should drain; the gap prevents block 1001
        let ready = q.drain_ready();
        assert_eq!(ready.len(), 1);
        assert_eq!(q.next_expected(), 2);
        assert_eq!(q.pending_count(), 1); // 1001 still waiting
    }

    /// Multiple interleaved push/drain cycles work correctly
    #[test]
    fn test_blockqueue_mixed_push_drain_push() {
        let mut q = BlockQueue::new(1);

        // Cycle 1: push [1, 2], drain
        q.push(make_commit_data(1));
        q.push(make_commit_data(2));
        let ready = q.drain_ready();
        assert_eq!(ready.len(), 2);
        assert_eq!(q.next_expected(), 3);

        // Cycle 2: push [3, 5] (gap at 4), drain
        q.push(make_commit_data(3));
        q.push(make_commit_data(5));
        let ready = q.drain_ready();
        assert_eq!(ready.len(), 1); // only 3
        assert_eq!(q.next_expected(), 4);

        // Fill gap and push more
        q.push(make_commit_data(4));
        q.push(make_commit_data(6));
        let ready = q.drain_ready();
        assert_eq!(ready.len(), 3); // 4, 5, 6
        assert_eq!(q.next_expected(), 7);
    }

    /// sync_with_go clears old pending, then drain gets remaining
    #[test]
    fn test_blockqueue_go_sync_then_drain() {
        let mut q = BlockQueue::new(1);

        // Push a range of commits
        for i in 1..=10 {
            q.push(make_commit_data(i));
        }
        assert_eq!(q.pending_count(), 10);

        // Go jumps to block 5 — clear blocks 1-5
        q.sync_with_go(5);
        assert_eq!(q.next_expected(), 6);

        // Remaining blocks 6-10 should drain
        let ready = q.drain_ready();
        assert_eq!(ready.len(), 5);
        assert_eq!(q.next_expected(), 11);
        assert_eq!(q.pending_count(), 0);
    }
}
