// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use std::{sync::Arc, vec};

use consensus_config::ProtocolKeyPair;

use consensus_types::block::{BlockRef, Round};

pub mod commit_manager;
pub mod proposer;

pub mod block_importer;

use mysten_metrics::monitored_scope;
use parking_lot::RwLock;
use tracing::info;

use crate::{
    adaptive_delay::AdaptiveDelayState,
    ancestor::AncestorStateManager,
    block::{BlockAPI, ExtendedBlock, Slot, VerifiedBlock, GENESIS_ROUND},
    block_manager::BlockManager,
    commit_observer::CommitObserver,
    context::Context,
    dag_state::DagState,
    error::{ConsensusError, ConsensusResult},
    leader_schedule::LeaderSchedule,
    round_tracker::PeerRoundTracker,
    system_transaction_provider::SystemTransactionProvider,
    transaction::TransactionConsumer,
    transaction_certifier::TransactionCertifier,
    universal_committer::{
        universal_committer_builder::UniversalCommitterBuilder, UniversalCommitter,
    },
};

// Maximum number of commit votes to include in a block.
// TODO: Move to protocol config, and verify in BlockVerifier.
const MAX_COMMIT_VOTES_PER_BLOCK: usize = 100;

pub(crate) struct Core {
    pub(crate) context: Arc<Context>,
    /// The consumer to use in order to pull transactions to be included for the next proposals
    pub(crate) transaction_consumer: TransactionConsumer,
    /// This contains the reject votes on transactions which proposed blocks should include.
    pub(crate) transaction_certifier: TransactionCertifier,
    /// The block manager which is responsible for keeping track of the DAG dependencies when processing new blocks
    /// and accept them or suspend if we are missing their causal history
    pub(crate) block_manager: BlockManager,
    /// Estimated delay by round for propagating blocks to a quorum.
    /// Because of the nature of TCP and block streaming, propagation delay is expected to be
    /// 0 in most cases, even when the actual latency of broadcasting blocks is high.
    /// When this value is higher than the `propagation_delay_stop_proposal_threshold`,
    /// most likely this validator cannot broadcast  blocks to the network at all.
    /// Core stops proposing new blocks in this case.
    pub(crate) propagation_delay: Round,
    /// Used to make commit decisions for leader blocks in the dag.
    pub(crate) committer: UniversalCommitter,
    /// The last new round for which core has sent out a signal.
    pub(crate) last_signaled_round: Round,
    /// The blocks of the last included ancestors per authority. This vector is basically used as a
    /// watermark in order to include in the next block proposal only ancestors of higher rounds.
    /// By default, is initialised with `None` values.
    pub(crate) last_included_ancestors: Vec<Option<BlockRef>>,
    /// The last decided leader returned from the universal committer. Important to note
    /// that this does not signify that the leader has been persisted yet as it still has
    /// to go through CommitObserver and persist the commit in store. On recovery/restart
    /// the last_decided_leader will be set to the last_commit leader in dag state.
    pub(crate) last_decided_leader: Slot,
    /// The consensus leader schedule to be used to resolve the leader for a
    /// given round.
    pub(crate) leader_schedule: Arc<LeaderSchedule>,
    /// The commit observer is responsible for observing the commits and collecting
    /// + sending subdags over the consensus output channel.
    pub(crate) commit_observer: CommitObserver,
    /// Sender of outgoing signals from Core.
    pub(crate) signals: CoreSignals,
    /// The keypair to be used for block signing
    pub(crate) block_signer: ProtocolKeyPair,
    /// Keeping track of state of the DAG, including blocks, commits and last committed rounds.
    pub(crate) dag_state: Arc<RwLock<DagState>>,
    /// The last known round for which the node has proposed. Any proposal should be for a round > of this.
    /// This is currently being used to avoid equivocations during a node recovering from amnesia. When value is None it means that
    /// the last block sync mechanism is enabled, but it hasn't been initialised yet.
    pub(crate) last_known_proposed_round: Option<Round>,
    // The ancestor state manager will keep track of the quality of the authorities
    // based on the distribution of their blocks to the network. It will use this
    // information to decide whether to include that authority block in the next
    // proposal or not.
    pub(crate) ancestor_state_manager: AncestorStateManager,
    // The round tracker will keep track of the highest received and accepted rounds
    // from all authorities. It will use this information to then calculate the
    // quorum rounds periodically which is used across other components to make
    // decisions about block proposals.
    pub(crate) round_tracker: Arc<RwLock<PeerRoundTracker>>,
    /// Adaptive delay state for automatically adjusting node speed based on network average
    pub(crate) adaptive_delay_state: Option<Arc<AdaptiveDelayState>>,
    /// System transaction provider (for EndOfEpoch transactions)
    /// None if using legacy Proposal/Vote/Quorum mechanism
    pub(crate) system_transaction_provider: Option<Arc<dyn SystemTransactionProvider>>,
}

impl Core {
    pub(crate) fn new(
        context: Arc<Context>,
        leader_schedule: Arc<LeaderSchedule>,
        transaction_consumer: TransactionConsumer,
        transaction_certifier: TransactionCertifier,
        block_manager: BlockManager,
        commit_observer: CommitObserver,
        signals: CoreSignals,
        block_signer: ProtocolKeyPair,
        dag_state: Arc<RwLock<DagState>>,
        sync_last_known_own_block: bool,
        round_tracker: Arc<RwLock<PeerRoundTracker>>,
        adaptive_delay_state: Option<Arc<AdaptiveDelayState>>,
        system_transaction_provider: Option<Arc<dyn SystemTransactionProvider>>,
    ) -> Self {
        let last_decided_leader = dag_state.read().last_commit_leader();
        let number_of_leaders = context
            .protocol_config
            .mysticeti_num_leaders_per_round()
            .unwrap_or(1);
        let committer = UniversalCommitterBuilder::new(
            context.clone(),
            leader_schedule.clone(),
            dag_state.clone(),
        )
        .with_number_of_leaders(number_of_leaders)
        .with_pipeline(true)
        .build();

        let last_proposed_block = dag_state.read().get_last_proposed_block();

        let last_signaled_round = last_proposed_block.round();

        // Recover the last included ancestor rounds based on the last proposed block. That will allow
        // to perform the next block proposal by using ancestor blocks of higher rounds and avoid
        // re-including blocks that have been already included in the last (or earlier) block proposal.
        // This is only strongly guaranteed for a quorum of ancestors. It is still possible to re-include
        // a block from an authority which hadn't been added as part of the last proposal hence its
        // latest included ancestor is not accurately captured here. This is considered a small deficiency,
        // and it mostly matters just for this next proposal without any actual penalties in performance
        // or block proposal.
        let mut last_included_ancestors = vec![None; context.committee.size()];
        for ancestor in last_proposed_block.ancestors() {
            last_included_ancestors[ancestor.author] = Some(*ancestor);
        }

        let min_propose_round = if sync_last_known_own_block {
            None
        } else {
            // if the sync is disabled then we practically don't want to impose any restriction.
            Some(0)
        };

        let propagation_scores = leader_schedule
            .leader_swap_table
            .read()
            .reputation_scores
            .clone();
        let mut ancestor_state_manager =
            AncestorStateManager::new(context.clone(), dag_state.clone());
        ancestor_state_manager.set_propagation_scores(propagation_scores);

        Self {
            context,
            last_signaled_round,
            last_included_ancestors,
            last_decided_leader,
            leader_schedule,
            transaction_consumer,
            transaction_certifier,
            block_manager,
            propagation_delay: 0,
            committer,
            commit_observer,
            signals,
            block_signer,
            dag_state,
            last_known_proposed_round: min_propose_round,
            ancestor_state_manager,
            round_tracker,
            adaptive_delay_state,
            system_transaction_provider,
        }
        .recover()
        .expect("Core::recover() failed")
    }

    fn recover(mut self) -> ConsensusResult<Self> {
        let _s = self
            .context
            .metrics
            .node_metrics
            .scope_processing_time
            .with_label_values(&["Core::recover"])
            .start_timer();

        // Try to commit and propose, since they may not have run after the last storage write.
        self.try_commit(vec![])?;

        let last_proposed_block = if let Some(last_proposed_block) = self.try_propose(true)? {
            last_proposed_block
        } else {
            let last_proposed_block = self.dag_state.read().get_last_proposed_block();

            if self.should_propose() {
                assert!(
                    last_proposed_block.round() > GENESIS_ROUND,
                    "At minimum a block of round higher than genesis should have been produced during recovery"
                );
            }

            // if no new block proposed then just re-broadcast the last proposed one to ensure liveness.
            self.signals
                .new_block(ExtendedBlock {
                    block: last_proposed_block.clone(),
                    excluded_ancestors: vec![],
                })
                .map_err(|e| {
                    tracing::warn!("Failed to signal new block during recovery: {e}");
                    ConsensusError::Shutdown
                })?;
            last_proposed_block
        };

        // Try to set up leader timeout if needed.
        // This needs to be called after try_commit() and try_propose(), which may
        // have advanced the threshold clock round.
        self.try_signal_new_round();

        info!(
            "Core recovery completed with last proposed block {:?}",
            last_proposed_block
        );

        Ok(self)
    }

    // Adds the certified commits that have been synced via the commit syncer. We are using the commit info in order to skip running the decision
    // rule and immediately commit the corresponding leaders and sub dags. Pay attention that no block acceptance is happening here, but rather
    // internally in the `try_commit` method which ensures that everytime only the blocks corresponding to the certified commits that are about to
    // be committed are accepted.

    /// If needed, signals a new clock round and sets up leader timeout.
    fn try_signal_new_round(&mut self) {
        // Signal only when the threshold clock round is more advanced than the last signaled round.
        //
        // NOTE: a signal is still sent even when a block has been proposed at the new round.
        // We can consider changing this in the future.
        let new_clock_round = self.dag_state.read().threshold_clock_round();
        if new_clock_round <= self.last_signaled_round {
            return;
        }
        // Then send a signal to set up leader timeout.
        self.signals.new_round(new_clock_round);
        self.last_signaled_round = new_clock_round;

        // Report the threshold clock round
        self.context
            .metrics
            .node_metrics
            .threshold_clock_round
            .set(new_clock_round as i64);
    }

    /// Creating a new block for the dictated round. This is used when a leader timeout occurs, either
    /// when the min timeout expires or max. When `force = true` , then any checks like previous round
    /// leader existence will get skipped.
    pub(crate) fn new_block(
        &mut self,
        round: Round,
        force: bool,
    ) -> ConsensusResult<Option<VerifiedBlock>> {
        let _scope = monitored_scope("Core::new_block");
        if self.last_proposed_round() < round {
            self.context
                .metrics
                .node_metrics
                .leader_timeout_total
                .with_label_values(&[&format!("{force}")])
                .inc();
            let result = self.try_propose(force);
            // The threshold clock round may have advanced, so a signal needs to be sent.
            self.try_signal_new_round();
            return result;
        }
        Ok(None)
    }

    // Attempts to create a new block, persist and propose it to all peers.
    // When force is true, ignore if leader from the last round exists among ancestors and if
    // the minimum round delay has passed.

    // Tries to select a prefix of certified commits to be committed next respecting the `limit`.
    // If provided `limit` is zero, it will panic.
    // The function returns a list of certified leaders and certified commits. If empty vector is returned, it means that
    // there are no certified commits to be committed, as input `certified_commits` is either empty or all of the certified
    // commits have been already committed.
}

// CoreSignals and CoreSignalsReceivers are defined in core_signals.rs
pub(crate) use crate::core_signals::{CoreSignals, CoreSignalsReceivers};

#[cfg(test)]
#[path = "core_tests.rs"]
mod core_tests;
