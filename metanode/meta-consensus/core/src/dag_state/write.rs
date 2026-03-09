// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use std::{
    cmp::max,
    collections::{BTreeMap, VecDeque},
    time::Duration,
};

use itertools::Itertools as _;
use tracing::{debug, error, trace};

use consensus_types::block::{BlockRef, Round, TransactionIndex};

use crate::{
    block::{BlockAPI, VerifiedBlock},
    commit::{CommitAPI as _, CommitIndex, CommitInfo, CommitRef, CommitVote, TrustedCommit},
    dag_state::{dag_state_impl::DagState, types::BlockInfo},
    leader_scoring::ReputationScores,
    storage::WriteBatch,
    CommittedSubDag,
};

impl DagState {
    /// Accepts a block into DagState and keeps it in memory.
    pub fn accept_block(&mut self, block: VerifiedBlock) {
        assert_ne!(
            block.round(),
            0,
            "Genesis block should not be accepted into DAG."
        );

        let block_ref = block.reference();
        if self.contains_block(&block_ref) {
            return;
        }

        let now = self.context.clock.timestamp_utc_ms();
        if block.timestamp_ms() > now {
            trace!(
                "Block {:?} with timestamp {} is greater than local timestamp {}.",
                block,
                block.timestamp_ms(),
                now,
            );
        }
        let hostname = &self.context.committee.authority(block_ref.author).hostname;
        self.context
            .metrics
            .node_metrics
            .accepted_block_time_drift_ms
            .with_label_values(&[hostname])
            .inc_by(block.timestamp_ms().saturating_sub(now));

        // TODO: Move this check to core
        // Ensure we don't write multiple blocks per slot for our own index
        if block_ref.author == self.context.own_index {
            let existing_blocks = self.get_uncommitted_blocks_at_slot(block_ref.into());
            assert!(
                existing_blocks.is_empty(),
                "Block Rejected! Attempted to add block {block:#?} to own slot where \
                block(s) {existing_blocks:#?} already exists."
            );
        }
        self.update_block_metadata(&block);
        self.blocks_to_write.push(block);
        let source = if self.context.own_index == block_ref.author {
            "own"
        } else {
            "others"
        };
        self.context
            .metrics
            .node_metrics
            .accepted_blocks
            .with_label_values(&[source])
            .inc();
    }

    /// Updates internal metadata for a block.
    pub(crate) fn update_block_metadata(&mut self, block: &VerifiedBlock) {
        let block_ref = block.reference();
        self.recent_blocks
            .insert(block_ref, BlockInfo::new(block.clone()));
        self.recent_refs_by_authority[block_ref.author].insert(block_ref);

        if self.threshold_clock.add_block(block_ref) {
            // Do not measure quorum delay when no local block is proposed in the round.
            let last_proposed_block = self.get_last_proposed_block();
            if last_proposed_block.round() == block_ref.round {
                let quorum_delay_ms = self
                    .context
                    .clock
                    .timestamp_utc_ms()
                    .saturating_sub(self.get_last_proposed_block().timestamp_ms());
                self.context
                    .metrics
                    .node_metrics
                    .quorum_receive_latency
                    .observe(Duration::from_millis(quorum_delay_ms).as_secs_f64());
            }
        }

        self.highest_accepted_round = max(self.highest_accepted_round, block.round());
        self.context
            .metrics
            .node_metrics
            .highest_accepted_round
            .set(self.highest_accepted_round as i64);

        let highest_accepted_round_for_author = self.recent_refs_by_authority[block_ref.author]
            .last()
            .map(|block_ref| block_ref.round)
            .expect("There should be by now at least one block ref");
        let hostname = &self.context.committee.authority(block_ref.author).hostname;
        self.context
            .metrics
            .node_metrics
            .highest_accepted_authority_round
            .with_label_values(&[hostname])
            .set(highest_accepted_round_for_author as i64);
    }

    /// Accepts a blocks into DagState and keeps it in memory.
    pub fn accept_blocks(&mut self, blocks: Vec<VerifiedBlock>) {
        debug!(
            "Accepting blocks: {}",
            blocks.iter().map(|b| b.reference().to_string()).join(",")
        );
        for block in blocks {
            self.accept_block(block);
        }
    }

    // Sets the block as committed in the cache. If the block is set as committed for first time, then true is returned, otherwise false is returned instead.
    // Method will panic if the block is not found in the cache.
    pub fn set_committed(&mut self, block_ref: &BlockRef) -> bool {
        if let Some(block_info) = self.recent_blocks.get_mut(block_ref) {
            if !block_info.committed {
                block_info.committed = true;
                return true;
            }
            false
        } else {
            panic!(
                "Block {:?} not found in cache to set as committed.",
                block_ref
            );
        }
    }

    /// Returns true if the block is committed. Only valid for blocks above the GC round.
    pub fn is_committed(&self, block_ref: &BlockRef) -> bool {
        self.recent_blocks
            .get(block_ref)
            .unwrap_or_else(|| panic!("Attempted to query for commit status for a block not in cached data {block_ref}"))
            .committed
    }

    /// Recursively sets blocks in the causal history of the root block as hard linked, including the root block itself.
    /// Returns the list of blocks that are newly linked.
    /// The returned blocks are guaranteed to be above the GC round.
    /// Transaction votes for the returned blocks are retrieved and carried by the upcoming
    /// proposed block.
    pub fn link_causal_history(&mut self, root_block: BlockRef) -> Vec<BlockRef> {
        let gc_round = self.gc_round();
        let mut linked_blocks = vec![];
        let mut targets = VecDeque::new();
        targets.push_back(root_block);
        while let Some(block_ref) = targets.pop_front() {
            // No need to collect or mark blocks at or below GC round.
            // These blocks and their causal history will not be included in new commits.
            // And their transactions do not need votes to finalize or skip.
            //
            // CommitFinalizer::gced_transaction_votes_for_pending_block() is the counterpart
            // to this logic, when deciding if block A in the causal history of block B gets
            // implicit accept transaction votes from block B.
            if block_ref.round <= gc_round {
                continue;
            }
            let block_info = self
                .recent_blocks
                .get_mut(&block_ref)
                .unwrap_or_else(|| panic!("Block {:?} is not in DAG state", block_ref));
            if block_info.included {
                continue;
            }
            linked_blocks.push(block_ref);
            block_info.included = true;
            targets.extend(block_info.block.ancestors().iter());
        }
        linked_blocks
    }

    /// Returns true if the block has been included in an owned proposed block.
    /// NOTE: caller should make sure only blocks above GC round are queried.
    pub fn has_been_included(&self, block_ref: &BlockRef) -> bool {
        self.recent_blocks
            .get(block_ref)
            .unwrap_or_else(|| {
                panic!(
                    "Attempted to query for inclusion status for a block not in cached data {}",
                    block_ref
                )
            })
            .included
    }

    pub fn threshold_clock_round(&self) -> Round {
        self.threshold_clock.get_round()
    }

    // The timestamp of when quorum threshold was last reached in the threshold clock.
    pub fn threshold_clock_quorum_ts(&self) -> tokio::time::Instant {
        self.threshold_clock.get_quorum_ts()
    }

    pub fn highest_accepted_round(&self) -> Round {
        self.highest_accepted_round
    }

    // Buffers a new commit in memory and updates last committed rounds.
    // REQUIRED: must not skip over any commit index.
    pub fn add_commit(&mut self, commit: TrustedCommit) {
        let time_diff = if let Some(last_commit) = &self.last_commit {
            if commit.index() <= last_commit.index() {
                error!(
                    "New commit index {} <= last commit index {}!",
                    commit.index(),
                    last_commit.index()
                );
                return;
            }
            assert_eq!(commit.index(), last_commit.index() + 1);

            if commit.timestamp_ms() < last_commit.timestamp_ms() {
                panic!(
                    "Commit timestamps do not monotonically increment, prev commit {:?}, new commit {:?}",
                    last_commit, commit
                );
            }
            commit
                .timestamp_ms()
                .saturating_sub(last_commit.timestamp_ms())
        } else {
            assert_eq!(commit.index(), 1);
            0
        };

        self.context
            .metrics
            .node_metrics
            .last_commit_time_diff
            .observe(time_diff as f64);

        let commit_round_advanced = if let Some(previous_commit) = &self.last_commit {
            previous_commit.round() < commit.round()
        } else {
            true
        };

        self.last_commit = Some(commit.clone());

        // Update last_commit_index metric
        self.context
            .metrics
            .node_metrics
            .last_commit_index
            .set(commit.index() as i64);

        if commit_round_advanced {
            let now = std::time::Instant::now();
            if let Some(previous_time) = self.last_commit_round_advancement_time {
                self.context
                    .metrics
                    .node_metrics
                    .commit_round_advancement_interval
                    .observe(now.duration_since(previous_time).as_secs_f64())
            }
            self.last_commit_round_advancement_time = Some(now);
        }

        for block_ref in commit.blocks().iter() {
            self.last_committed_rounds[block_ref.author] = max(
                self.last_committed_rounds[block_ref.author],
                block_ref.round,
            );
        }

        for (i, round) in self.last_committed_rounds.iter().enumerate() {
            let index = self
                .context
                .committee
                .to_authority_index(i)
                .expect("authority index must be valid");
            let hostname = &self.context.committee.authority(index).hostname;
            self.context
                .metrics
                .node_metrics
                .last_committed_authority_round
                .with_label_values(&[hostname])
                .set((*round).into());
        }

        self.pending_commit_votes.push_back(commit.reference());
        self.commits_to_write.push(commit);
    }

    /// Recovers commits to write from storage, at startup.
    pub fn recover_commits_to_write(&mut self, commits: Vec<TrustedCommit>) {
        self.commits_to_write.extend(commits);
    }

    pub fn ensure_commits_to_write_is_empty(&self) {
        assert!(
            self.commits_to_write.is_empty(),
            "Commits to write should be empty. {:?}",
            self.commits_to_write,
        );
    }

    pub fn add_commit_info(&mut self, reputation_scores: ReputationScores) {
        // We create an empty scoring subdag once reputation scores are calculated.
        // Note: It is okay for this to not be gated by protocol config as the
        // scoring_subdag should be empty in either case at this point.
        assert!(self.scoring_subdag.is_empty());

        let commit_info = CommitInfo {
            committed_rounds: self.last_committed_rounds.clone(),
            reputation_scores,
        };
        let last_commit = self
            .last_commit
            .as_ref()
            .expect("Last commit should already be set.");
        self.commit_info_to_write
            .push((last_commit.reference(), commit_info));
    }

    pub fn add_finalized_commit(
        &mut self,
        commit_ref: CommitRef,
        rejected_transactions: BTreeMap<BlockRef, Vec<TransactionIndex>>,
    ) {
        self.finalized_commits_to_write
            .push((commit_ref, rejected_transactions));
    }

    pub fn take_commit_votes(&mut self, limit: usize) -> Vec<CommitVote> {
        let mut votes = Vec::new();
        let mut seen_indices = std::collections::HashSet::new();
        while !self.pending_commit_votes.is_empty() && votes.len() < limit {
            let vote = self
                .pending_commit_votes
                .pop_front()
                .expect("checked non-empty in while condition");
            // Multi-leader dedup: keep only the first vote per commit index.
            // This prevents bloating blocks with redundant votes when multiple
            // leaders produce commits at the same index.
            if seen_indices.insert(vote.index) {
                votes.push(vote);
            }
        }
        votes
    }

    /// Index of the last commit.
    pub fn last_commit_index(&self) -> CommitIndex {
        match &self.last_commit {
            Some(commit) => commit.index(),
            None => 0,
        }
    }

    /// Digest of the last commit.
    pub fn last_commit_digest(&self) -> crate::commit::CommitDigest {
        match &self.last_commit {
            Some(commit) => commit.digest(),
            None => crate::commit::CommitDigest::MIN,
        }
    }

    /// Timestamp of the last commit.
    pub fn last_commit_timestamp_ms(&self) -> consensus_types::block::BlockTimestampMs {
        match &self.last_commit {
            Some(commit) => commit.timestamp_ms(),
            None => 0,
        }
    }

    /// Leader slot of the last commit.
    pub fn last_commit_leader(&self) -> crate::block::Slot {
        match &self.last_commit {
            Some(commit) => commit.leader().into(),
            None => self
                .genesis
                .iter()
                .next()
                .map(|(genesis_ref, _)| *genesis_ref)
                .expect("Genesis blocks should always be available.")
                .into(),
        }
    }

    /// Highest round where a block is committed, which is last commit's leader round.
    pub fn last_commit_round(&self) -> Round {
        match &self.last_commit {
            Some(commit) => commit.leader().round,
            None => 0,
        }
    }

    /// Last committed round per authority.
    pub fn last_committed_rounds(&self) -> Vec<Round> {
        self.last_committed_rounds.clone()
    }

    /// The GC round is the highest round that blocks of equal or lower round are considered obsolete and no longer possible to be committed.
    /// There is no meaning accepting any blocks with round <= gc_round. The Garbage Collection (GC) round is calculated based on the latest
    /// committed leader round. When GC is disabled that will return the genesis round.
    pub fn gc_round(&self) -> Round {
        self.calculate_gc_round(self.last_commit_round())
    }

    /// Calculates the GC round from the input leader round, which can be different
    /// from the last committed leader round.
    pub fn calculate_gc_round(&self, commit_round: Round) -> Round {
        commit_round.saturating_sub(self.context.protocol_config.gc_depth())
    }

    /// Flushes unpersisted blocks, commits and commit info to storage.
    ///
    /// REQUIRED: when buffering a block, all of its ancestors and the latest commit which sets the GC round
    /// must also be buffered.
    /// REQUIRED: when buffering a commit, all of its included blocks and the previous commits must also be buffered.
    /// REQUIRED: when flushing, all of the buffered blocks and commits must be flushed together to ensure consistency.
    ///
    /// After each flush, DagState becomes persisted in storage and it expected to recover
    /// all internal states from storage after restarts.
    pub fn flush(&mut self) {
        let _s = self
            .context
            .metrics
            .node_metrics
            .scope_processing_time
            .with_label_values(&["DagState::flush"])
            .start_timer();

        // Flush buffered data to storage.
        let pending_blocks = std::mem::take(&mut self.blocks_to_write);
        let pending_commits = std::mem::take(&mut self.commits_to_write);
        let pending_commit_info = std::mem::take(&mut self.commit_info_to_write);
        let pending_finalized_commits = std::mem::take(&mut self.finalized_commits_to_write);
        if pending_blocks.is_empty()
            && pending_commits.is_empty()
            && pending_commit_info.is_empty()
            && pending_finalized_commits.is_empty()
        {
            return;
        }

        debug!(
            "Flushing {} blocks ({}), {} commits ({}), {} commit infos ({}), {} finalized commits ({}) to storage.",
            pending_blocks.len(),
            pending_blocks
                .iter()
                .map(|b| b.reference().to_string())
                .join(","),
            pending_commits.len(),
            pending_commits
                .iter()
                .map(|c| c.reference().to_string())
                .join(","),
            pending_commit_info.len(),
            pending_commit_info
                .iter()
                .map(|(commit_ref, _)| commit_ref.to_string())
                .join(","),
            pending_finalized_commits.len(),
            pending_finalized_commits
                .iter()
                .map(|(commit_ref, _)| commit_ref.to_string())
                .join(","),
        );
        self.store
            .write(WriteBatch::new(
                pending_blocks,
                pending_commits,
                pending_commit_info,
                pending_finalized_commits,
            ))
            .unwrap_or_else(|e| panic!("Failed to write to storage: {:?}", e));
        self.context
            .metrics
            .node_metrics
            .dag_state_store_write_count
            .inc();

        // Clean up old cached data. After flushing, all cached blocks are guaranteed to be persisted.
        for (authority_index, _) in self.context.committee.authorities() {
            let eviction_round = self.calculate_authority_eviction_round(authority_index);
            while let Some(block_ref) = self.recent_refs_by_authority[authority_index].first() {
                if block_ref.round <= eviction_round {
                    self.recent_blocks.remove(block_ref);
                    self.recent_refs_by_authority[authority_index].pop_first();
                } else {
                    break;
                }
            }
            self.evicted_rounds[authority_index] = eviction_round;
        }

        let metrics = &self.context.metrics.node_metrics;
        metrics
            .dag_state_recent_blocks
            .set(self.recent_blocks.len() as i64);
        metrics.dag_state_recent_refs.set(
            self.recent_refs_by_authority
                .iter()
                .map(std::collections::BTreeSet::len)
                .sum::<usize>() as i64,
        );
    }

    pub fn recover_last_commit_info(&self) -> Option<(CommitRef, CommitInfo)> {
        self.store
            .read_last_commit_info()
            .unwrap_or_else(|e| panic!("Failed to read from storage: {:?}", e))
    }

    pub fn add_scoring_subdags(&mut self, scoring_subdags: Vec<CommittedSubDag>) {
        self.scoring_subdag.add_subdags(scoring_subdags);
    }

    pub fn clear_scoring_subdag(&mut self) {
        self.scoring_subdag.clear();
    }

    pub fn scoring_subdags_count(&self) -> usize {
        self.scoring_subdag.scored_subdags_count()
    }

    pub fn is_scoring_subdag_empty(&self) -> bool {
        self.scoring_subdag.is_empty()
    }

    pub fn calculate_scoring_subdag_scores(&self) -> ReputationScores {
        self.scoring_subdag.calculate_distributed_vote_scores()
    }

    pub fn scoring_subdag_commit_range(&self) -> CommitIndex {
        self.scoring_subdag
            .commit_range
            .as_ref()
            .expect("commit range should exist for scoring subdag")
            .end()
    }
}
