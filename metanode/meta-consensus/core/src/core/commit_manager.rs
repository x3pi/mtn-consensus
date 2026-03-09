use itertools::Itertools;
use std::collections::{BTreeMap, BTreeSet};

use consensus_types::block::{BlockRef, Round};
use meta_macros::fail_point;
use mysten_metrics::monitored_scope;
use tracing::{info, warn};

use crate::{
    block::BlockAPI,
    commit::{CertifiedCommit, CertifiedCommits, CommitAPI, CommittedSubDag, DecidedLeader},
    core::Core,
    error::{ConsensusError, ConsensusResult},
};

impl Core {
    #[tracing::instrument(skip_all)]
    pub(crate) fn add_certified_commits(
        &mut self,
        certified_commits: CertifiedCommits,
    ) -> ConsensusResult<BTreeSet<BlockRef>> {
        let _scope = monitored_scope("Core::add_certified_commits");

        let votes = certified_commits.votes().to_vec();
        let commits = self
            .filter_new_commits(certified_commits.commits().to_vec())
            .expect("Certified commits validation failed");

        // Try to accept the certified commit votes.
        // Even if they may not be part of a future commit, these blocks are useful for certifying
        // commits when helping peers sync commits.
        let (_, missing_block_refs) = self.block_manager.try_accept_blocks(votes);

        // Try to commit the new blocks. Take into account the trusted commit that has been provided.
        self.try_commit(commits)?;

        // Try to propose now since there are new blocks accepted.
        self.try_propose(false)?;

        // Now set up leader timeout if needed.
        // This needs to be called after try_commit() and try_propose(), which may
        // have advanced the threshold clock round.
        self.try_signal_new_round();

        Ok(missing_block_refs)
    }

    /// Runs commit rule to attempt to commit additional blocks from the DAG. If any `certified_commits` are provided, then
    /// it will attempt to commit those first before trying to commit any further leaders.
    pub(crate) fn try_commit(
        &mut self,
        mut certified_commits: Vec<CertifiedCommit>,
    ) -> ConsensusResult<Vec<CommittedSubDag>> {
        let _s = self
            .context
            .metrics
            .node_metrics
            .scope_processing_time
            .with_label_values(&["Core::try_commit"])
            .start_timer();

        let mut certified_commits_map = BTreeMap::new();
        for c in &certified_commits {
            certified_commits_map.insert(c.index(), c.reference());
        }

        if !certified_commits.is_empty() {
            info!(
                "Processing synced commits: {:?}",
                certified_commits
                    .iter()
                    .map(|c| (c.index(), c.leader()))
                    .collect::<Vec<_>>()
            );
        }

        let mut committed_sub_dags = Vec::new();
        // TODO: Add optimization to abort early without quorum for a round.
        loop {
            // LeaderSchedule has a limit to how many sequenced leaders can be committed
            // before a change is triggered. Calling into leader schedule will get you
            // how many commits till next leader change. We will loop back and recalculate
            // any discarded leaders with the new schedule.
            let mut commits_until_update = self
                .leader_schedule
                .commits_until_leader_schedule_update(self.dag_state.clone());

            if commits_until_update == 0 {
                let last_commit_index = self.dag_state.read().last_commit_index();

                tracing::info!(
                    "Leader schedule change triggered at commit index {last_commit_index}"
                );

                self.leader_schedule
                    .update_leader_schedule_v2(&self.dag_state);

                let propagation_scores = self
                    .leader_schedule
                    .leader_swap_table
                    .read()
                    .reputation_scores
                    .clone();
                self.ancestor_state_manager
                    .set_propagation_scores(propagation_scores);

                commits_until_update = self
                    .leader_schedule
                    .commits_until_leader_schedule_update(self.dag_state.clone());

                fail_point!("consensus-after-leader-schedule-change");
            }
            assert!(commits_until_update > 0);

            // If there are certified commits to process, find out which leaders and commits from them
            // are decided and use them as the next commits.
            let (certified_leaders, decided_certified_commits): (
                Vec<DecidedLeader>,
                Vec<CertifiedCommit>,
            ) = self
                .try_select_certified_leaders(&mut certified_commits, commits_until_update)
                .into_iter()
                .unzip();

            // Only accept blocks for the certified commits that we are certain to sequence.
            // This ensures that only blocks corresponding to committed certified commits are flushed to disk.
            // Blocks from non-committed certified commits will not be flushed, preventing issues during crash-recovery.
            // This avoids scenarios where accepting and flushing blocks of non-committed certified commits could lead to
            // premature commit rule execution. Due to GC, this could cause a panic if the commit rule tries to access
            // missing causal history from blocks of certified commits.
            let blocks = decided_certified_commits
                .iter()
                .flat_map(|c| c.blocks())
                .cloned()
                .collect::<Vec<_>>();
            self.block_manager.try_accept_committed_blocks(blocks);

            // If there is no certified commit to process, run the decision rule.
            let (decided_leaders, local) = if certified_leaders.is_empty() {
                // TODO: limit commits by commits_until_update for efficiency, which may be needed when leader schedule length is reduced.
                let mut decided_leaders = self.committer.try_decide(self.last_decided_leader);
                // Truncate the decided leaders to fit the commit schedule limit.
                if decided_leaders.len() >= commits_until_update {
                    let _ = decided_leaders.split_off(commits_until_update);
                }
                (decided_leaders, true)
            } else {
                (certified_leaders, false)
            };

            // If the decided leaders list is empty then just break the loop.
            let Some(last_decided) = decided_leaders.last().cloned() else {
                break;
            };

            self.last_decided_leader = last_decided.slot();
            self.context
                .metrics
                .node_metrics
                .last_decided_leader_round
                .set(self.last_decided_leader.round as i64);

            let sequenced_leaders = decided_leaders
                .into_iter()
                .filter_map(|leader| leader.into_committed_block())
                .collect::<Vec<_>>();
            // It's possible to reach this point as the decided leaders might all of them be "Skip" decisions. In this case there is no
            // leader to commit and we should break the loop.
            if sequenced_leaders.is_empty() {
                break;
            }
            tracing::info!(
                "Committing {} leaders: {}; {} commits before next leader schedule change",
                sequenced_leaders.len(),
                sequenced_leaders
                    .iter()
                    .map(|b| b.reference().to_string())
                    .join(","),
                commits_until_update,
            );

            // TODO: refcount subdags
            let subdags = self
                .commit_observer
                .handle_commit(sequenced_leaders, local)?;

            // Update adaptive delay state with new commit index
            if let Some(adaptive_delay_state) = &self.adaptive_delay_state {
                let new_commit_index = self.dag_state.read().last_commit_index();
                adaptive_delay_state.update_local_commit(new_commit_index);
            }

            // Try to unsuspend blocks if gc_round has advanced.
            self.block_manager
                .try_unsuspend_blocks_for_latest_gc_round();

            committed_sub_dags.extend(subdags);

            fail_point!("consensus-after-handle-commit");
        }

        // Sanity check: for commits that have been linearized using the certified commits, ensure that the same sub dag has been committed.
        for sub_dag in &committed_sub_dags {
            if let Some(commit_ref) = certified_commits_map.remove(&sub_dag.commit_ref.index) {
                assert_eq!(
                    commit_ref, sub_dag.commit_ref,
                    "Certified commit has different reference than the committed sub dag"
                );
            }
        }

        // Notify about our own committed blocks
        let committed_block_refs = committed_sub_dags
            .iter()
            .flat_map(|sub_dag| sub_dag.blocks.iter())
            .filter_map(|block| {
                (block.author() == self.context.own_index).then_some(block.reference())
            })
            .collect::<Vec<_>>();
        self.transaction_consumer
            .notify_own_blocks_status(committed_block_refs, self.dag_state.read().gc_round());

        Ok(committed_sub_dags)
    }

    /// Keeps only the certified commits that have a commit index > last commit index.
    /// It also ensures that the first commit in the list is the next one in line, otherwise it panics.
    pub(crate) fn filter_new_commits(
        &mut self,
        commits: Vec<CertifiedCommit>,
    ) -> ConsensusResult<Vec<CertifiedCommit>> {
        // Filter out the commits that have been already locally committed and keep only anything that is above the last committed index.
        let last_commit_index = self.dag_state.read().last_commit_index();
        let commits = commits
            .iter()
            .filter(|commit| {
                if commit.index() > last_commit_index {
                    true
                } else {
                    tracing::debug!(
                        "Skip commit for index {} as it is already committed with last commit index {}",
                        commit.index(),
                        last_commit_index
                    );
                    false
                }
            })
            .cloned()
            .collect::<Vec<_>>();

        // Make sure that the first commit we find is the next one in line and there is no gap.
        if let Some(commit) = commits.first() {
            if commit.index() != last_commit_index + 1 {
                return Err(ConsensusError::UnexpectedCertifiedCommitIndex {
                    expected_commit_index: last_commit_index + 1,
                    commit_index: commit.index(),
                });
            }
        }

        Ok(commits)
    }

    /// Sets the delay by round for propagating blocks to a quorum.
    pub(crate) fn set_propagation_delay(&mut self, delay: Round) {
        info!("Propagation round delay set to: {delay}");
        self.propagation_delay = delay;
    }

    /// Sets the min propose round for the proposer allowing to propose blocks only for round numbers
    /// `> last_known_proposed_round`. At the moment is allowed to call the method only once leading to a panic
    /// if attempt to do multiple times.
    pub(crate) fn set_last_known_proposed_round(&mut self, round: Round) {
        if self.last_known_proposed_round.is_some() {
            warn!(
                "set_last_known_proposed_round called again (already set to {:?}), ignoring new value {}",
                self.last_known_proposed_round, round
            );
            return;
        }
        self.last_known_proposed_round = Some(round);
        info!("Last known proposed round set to {round}");
    }
}
