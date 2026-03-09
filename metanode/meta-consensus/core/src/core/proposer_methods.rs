fn try_propose(&mut self, force: bool) -> ConsensusResult<Option<VerifiedBlock>> {
        if !self.should_propose() {
            return Ok(None);
        }
        if let Some(extended_block) = self.try_new_block(force) {
            self.signals.new_block(extended_block.clone())?;

            fail_point!("consensus-after-propose");

            // The new block may help commit.
            self.try_commit(vec![])?;
            return Ok(Some(extended_block.block));
        }
        Ok(None)
    }

    /// Attempts to propose a new block for the next round. If a block has already proposed for latest
    /// or earlier round, then no block is created and None is returned.
    fn try_new_block(&mut self, force: bool) -> Option<ExtendedBlock> {
        let _s = self
            .context
            .metrics
            .node_metrics
            .scope_processing_time
            .with_label_values(&["Core::try_new_block"])
            .start_timer();

        // Ensure the new block has a higher round than the last proposed block.
        let clock_round = {
            let dag_state = self.dag_state.read();
            let clock_round = dag_state.threshold_clock_round();
            if clock_round <= dag_state.get_last_proposed_block().round() {
                debug!(
                    "Skipping block proposal for round {} as it is not higher than the last proposed block {}",
                    clock_round,
                    dag_state.get_last_proposed_block().round()
                );
                return None;
            }
            clock_round
        };

        // There must be a quorum of blocks from the previous round.
        let quorum_round = clock_round.saturating_sub(1);

        // Create a new block either because we want to "forcefully" propose a block due to a leader timeout,
        // or because we are actually ready to produce the block (leader exists and min delay has passed).
        if !force {
            if !self.leaders_exist(quorum_round) {
                return None;
            }

            // Calculate effective delay (base + adaptive delay if enabled)
            let mut effective_delay = self.context.parameters.min_round_delay;
            if let Some(adaptive_delay_state) = &self.adaptive_delay_state {
                let local_commit_index = self.dag_state.read().last_commit_index();
                let quorum_commit_index = self
                    .context
                    .metrics
                    .node_metrics
                    .commit_sync_quorum_index
                    .get() as u32;
                let adaptive_delay = adaptive_delay_state
                    .calculate_adaptive_delay(local_commit_index, quorum_commit_index);
                effective_delay += adaptive_delay;

                // Update metrics
                self.context
                    .metrics
                    .node_metrics
                    .adaptive_delay_ms
                    .set(adaptive_delay.as_millis() as i64);
                let lead = local_commit_index.saturating_sub(quorum_commit_index);
                self.context
                    .metrics
                    .node_metrics
                    .commit_sync_lead
                    .set(lead as i64);
                self.context
                    .metrics
                    .node_metrics
                    .local_commit_rate
                    .set(adaptive_delay_state.local_rate());
                self.context
                    .metrics
                    .node_metrics
                    .quorum_commit_rate
                    .set(adaptive_delay_state.quorum_rate());
            }

            if Duration::from_millis(
                self.context
                    .clock
                    .timestamp_utc_ms()
                    .saturating_sub(self.last_proposed_timestamp_ms()),
            ) < effective_delay
            {
                debug!(
                    "Skipping block proposal for round {} as it is too soon after the last proposed block timestamp {}; effective delay is {}ms (base: {}ms)",
                    clock_round,
                    self.last_proposed_timestamp_ms(),
                    effective_delay.as_millis(),
                    self.context.parameters.min_round_delay.as_millis(),
                );
                return None;
            }
        }

        // Determine the ancestors to be included in proposal.
        let (ancestors, excluded_and_equivocating_ancestors) =
            self.smart_ancestors_to_propose(clock_round, !force);

        // If we did not find enough good ancestors to propose, continue to wait before proposing.
        if ancestors.is_empty() {
            assert!(
                !force,
                "Ancestors should have been returned if force is true!"
            );
            debug!(
                "Skipping block proposal for round {} because no good ancestor is found",
                clock_round,
            );
            return None;
        }

        let excluded_ancestors_limit = self.context.committee.size() * 2;
        if excluded_and_equivocating_ancestors.len() > excluded_ancestors_limit {
            debug!(
                "Dropping {} excluded ancestor(s) during proposal due to size limit",
                excluded_and_equivocating_ancestors.len() - excluded_ancestors_limit,
            );
        }
        let excluded_ancestors = excluded_and_equivocating_ancestors
            .into_iter()
            .take(excluded_ancestors_limit)
            .collect();

        // Update the last included ancestor block refs
        for ancestor in &ancestors {
            self.last_included_ancestors[ancestor.author()] = Some(ancestor.reference());
        }

        let leader_authority = &self
            .context
            .committee
            .authority(self.first_leader(quorum_round))
            .hostname;
        self.context
            .metrics
            .node_metrics
            .block_proposal_leader_wait_ms
            .with_label_values(&[leader_authority])
            .inc_by(
                Instant::now()
                    .saturating_duration_since(self.dag_state.read().threshold_clock_quorum_ts())
                    .as_millis() as u64,
            );
        self.context
            .metrics
            .node_metrics
            .block_proposal_leader_wait_count
            .with_label_values(&[leader_authority])
            .inc();

        self.context
            .metrics
            .node_metrics
            .proposed_block_ancestors
            .observe(ancestors.len() as f64);
        for ancestor in &ancestors {
            let authority = &self.context.committee.authority(ancestor.author()).hostname;
            self.context
                .metrics
                .node_metrics
                .proposed_block_ancestors_depth
                .with_label_values(&[authority])
                .observe(clock_round.saturating_sub(ancestor.round()).into());
        }

        let now = self.context.clock.timestamp_utc_ms();
        ancestors.iter().for_each(|block| {
            if block.timestamp_ms() > now {
                trace!("Ancestor block {:?} has timestamp {}, greater than current timestamp {now}. Proposing for round {}.", block, block.timestamp_ms(), clock_round);
                let authority = &self.context.committee.authority(block.author()).hostname;
                self.context
                    .metrics
                    .node_metrics
                    .proposed_block_ancestors_timestamp_drift_ms
                    .with_label_values(&[authority])
                    .inc_by(block.timestamp_ms().saturating_sub(now));
            }
        });

        // Consume the next transactions to be included. Do not drop the guards yet as this would acknowledge
        // the inclusion of transactions. Just let this be done in the end of the method.
        let (mut transactions, ack_transactions, _limit_reached) = self.transaction_consumer.next();

        // Inject system transactions (e.g., EndOfEpoch) if provider is available
        // FORK-SAFETY: Only inject system transactions when this node is the leader
        // This ensures only one node creates the system transaction, preventing forks
        // from multiple nodes creating different system transactions
        if let Some(provider) = &self.system_transaction_provider {
            // CRITICAL FORK-SAFETY: Only leader should inject system transactions
            // Check if this node is the leader for this round
            let leader_for_round = self.first_leader(clock_round);
            let is_leader = leader_for_round == self.context.own_index;

            if is_leader {
                let current_epoch = self.context.committee.epoch();
                // Get current commit index from dag_state
                let current_commit_index = self.dag_state.read().last_commit_index();

                tracing::debug!(
                    "🔍 Leader checking for system transactions: epoch={}, commit_index={}",
                    current_epoch,
                    current_commit_index
                );

                if let Some(system_txs) =
                    provider.get_system_transactions(current_epoch, current_commit_index)
                {
                    // Convert system transactions to regular transactions
                    let system_transactions: Vec<crate::block::Transaction> = system_txs
                        .into_iter()
                        .filter_map(|stx| {
                            stx.to_bytes()
                                .map(|bytes| crate::block::Transaction::new(bytes))
                                .ok()
                        })
                        .collect();

                    if !system_transactions.is_empty() {
                        tracing::info!(
                            "📝 Injecting {} system transaction(s) into block (epoch={}, commit_index={})",
                            system_transactions.len(),
                            current_epoch,
                            current_commit_index
                        );
                        // Insert system transactions at the beginning
                        let mut all_transactions = system_transactions;
                        all_transactions.extend(transactions);
                        transactions = all_transactions;
                    }
                }
            } else {
                // Not leader - don't inject system transactions
                // Other nodes will receive the system transaction from the leader's block
                tracing::debug!(
                    "⏭️ Skipping system transaction injection: not leader for round {} (leader={})",
                    clock_round,
                    leader_for_round.value()
                );
            }
        }

        self.context
            .metrics
            .node_metrics
            .proposed_block_transactions
            .observe(transactions.len() as f64);

        // Consume the commit votes to be included.
        let commit_votes = self
            .dag_state
            .write()
            .take_commit_votes(MAX_COMMIT_VOTES_PER_BLOCK);

        let transaction_votes = if self.context.protocol_config.mysticeti_fastpath() {
            let new_causal_history = {
                let mut dag_state = self.dag_state.write();
                ancestors
                    .iter()
                    .flat_map(|ancestor| dag_state.link_causal_history(ancestor.reference()))
                    .collect()
            };
            self.transaction_certifier.get_own_votes(new_causal_history)
        } else {
            vec![]
        };

        // Get epoch change data to include in block
        let (epoch_change_proposal, epoch_change_votes) =
            crate::epoch_change_provider::get_epoch_change_data();

        // Create the block and insert to storage.
        let mut block = if self.context.protocol_config.mysticeti_fastpath() {
            Block::V2(BlockV2::new(
                self.context.committee.epoch(),
                clock_round,
                self.context.own_index,
                now,
                ancestors.iter().map(|b| b.reference()).collect(),
                transactions,
                commit_votes,
                transaction_votes,
                vec![],
            ))
        } else {
            Block::V1(BlockV1::new(
                self.context.committee.epoch(),
                clock_round,
                self.context.own_index,
                now,
                ancestors.iter().map(|b| b.reference()).collect(),
                transactions,
                commit_votes,
                vec![],
            ))
        };

        // Include epoch change data in block
        // NOTE: All nodes must have updated code (no backward compatibility)
        // For production, use BlockV2 with versioning or separate epoch change messages
        block.set_epoch_change_proposal(epoch_change_proposal);
        block.set_epoch_change_votes(epoch_change_votes);

        let signed_block =
            SignedBlock::new(block, &self.block_signer).expect("Block signing failed.");
        let serialized = signed_block
            .serialize()
            .expect("Block serialization failed.");
        self.context
            .metrics
            .node_metrics
            .proposed_block_size
            .observe(serialized.len() as f64);
        // Own blocks are assumed to be valid.
        let verified_block = VerifiedBlock::new_verified(signed_block, serialized);

        // Record the interval from last proposal, before accepting the proposed block.
        let last_proposed_block = self.last_proposed_block();
        if last_proposed_block.round() > 0 {
            self.context
                .metrics
                .node_metrics
                .block_proposal_interval
                .observe(
                    Duration::from_millis(
                        verified_block
                            .timestamp_ms()
                            .saturating_sub(last_proposed_block.timestamp_ms()),
                    )
                    .as_secs_f64(),
                );
        }

        // Accept the block into BlockManager and DagState.
        let (accepted_blocks, missing) = self
            .block_manager
            .try_accept_blocks(vec![verified_block.clone()]);
        assert_eq!(accepted_blocks.len(), 1);
        assert!(missing.is_empty());

        // The block must be added to transaction certifier before it is broadcasted or added to DagState.
        // Update proposed state of blocks in local DAG.
        // TODO(fastpath): move this logic and the logic afterwards to proposed block handler.
        if self.context.protocol_config.mysticeti_fastpath() {
            self.transaction_certifier
                .add_voted_blocks(vec![(verified_block.clone(), vec![])]);
            self.dag_state
                .write()
                .link_causal_history(verified_block.reference());
        }

        // Ensure the new block and its ancestors are persisted, before broadcasting it.
        self.dag_state.write().flush();

        // Now acknowledge the transactions for their inclusion to block
        ack_transactions(verified_block.reference());

        info!("Created block {verified_block:?} for round {clock_round}");

        self.context
            .metrics
            .node_metrics
            .proposed_blocks
            .with_label_values(&[&force.to_string()])
            .inc();

        let extended_block = ExtendedBlock {
            block: verified_block,
            excluded_ancestors,
        };

        // Update round tracker with our own highest accepted blocks
        self.round_tracker
            .write()
            .update_from_verified_block(&extended_block);

        Some(extended_block)
    }

    /// Whether the core should propose new blocks.
    pub(crate) fn should_propose(&self) -> bool {
        let clock_round = self.dag_state.read().threshold_clock_round();
        let core_skipped_proposals = &self.context.metrics.node_metrics.core_skipped_proposals;

        // CRITICAL: Check if node is lagging and prioritize sync over consensus
        // When lag is significant, skip proposing new blocks to focus on syncing commits
        let local_commit_index = self.dag_state.read().last_commit_index();
        let quorum_commit_index = self
            .context
            .metrics
            .node_metrics
            .commit_sync_quorum_index
            .get() as u32;

        // Thresholds for skipping consensus when lagging:
        // - MODERATE_LAG: 100 commits or 10% behind quorum -> skip consensus, prioritize sync
        // - SEVERE_LAG: 200 commits or 15% behind quorum -> aggressively skip consensus
        const MODERATE_LAG_THRESHOLD: u32 = 100; // Skip consensus if lag > 100 commits
        const SEVERE_LAG_THRESHOLD: u32 = 200; // Aggressively skip consensus if lag > 200 commits
        const MODERATE_LAG_PERCENTAGE: f64 = 10.0; // Skip consensus if lag > 10% of quorum
        const SEVERE_LAG_PERCENTAGE: f64 = 15.0; // Aggressively skip if lag > 15% of quorum

        // HYSTERESIS: To prevent oscillation, use lower threshold when recovering from sync mode
        // When lag was high and is now decreasing, require lag to drop below 80% of threshold
        // before resuming consensus. This ensures smooth transition back to consensus.
        const HYSTERESIS_FACTOR: f64 = 0.8; // Resume consensus when lag < 80% of threshold

        let lag = quorum_commit_index.saturating_sub(local_commit_index);
        let lag_percentage = if quorum_commit_index > 0 {
            (lag as f64 / quorum_commit_index as f64) * 100.0
        } else {
            0.0
        };

        // Check if we should skip consensus
        // Use hysteresis: if we were in sync mode, require lag to drop below 80% of threshold
        let moderate_lag_threshold_with_hysteresis =
            (MODERATE_LAG_THRESHOLD as f64 * HYSTERESIS_FACTOR) as u32;
        let moderate_lag_percentage_with_hysteresis = MODERATE_LAG_PERCENTAGE * HYSTERESIS_FACTOR;

        // Determine if we should skip consensus
        // If lag is above threshold OR above hysteresis threshold (for smooth recovery)
        let should_skip_consensus =
            lag > MODERATE_LAG_THRESHOLD || lag_percentage > MODERATE_LAG_PERCENTAGE;
        let is_severe_lag = lag > SEVERE_LAG_THRESHOLD || lag_percentage > SEVERE_LAG_PERCENTAGE;

        // Check if we're recovering (lag was high but now decreasing)
        let is_recovering = lag <= moderate_lag_threshold_with_hysteresis
            && lag_percentage <= moderate_lag_percentage_with_hysteresis;

        if should_skip_consensus {
            if is_severe_lag {
                // Severe lag: Skip consensus aggressively
                debug!(
                    "Skip proposing for round {} due to severe lag: lag={} commits ({}% behind quorum), local_commit={}, quorum_commit={}. Prioritizing sync over consensus.",
                    clock_round, lag, lag_percentage, local_commit_index, quorum_commit_index
                );
                core_skipped_proposals
                    .with_label_values(&["severe_lag_prioritize_sync"])
                    .inc();
            } else {
                // Moderate lag: Skip consensus to prioritize sync
                debug!(
                    "Skip proposing for round {} due to moderate lag: lag={} commits ({}% behind quorum), local_commit={}, quorum_commit={}. Prioritizing sync over consensus.",
                    clock_round, lag, lag_percentage, local_commit_index, quorum_commit_index
                );
                core_skipped_proposals
                    .with_label_values(&["moderate_lag_prioritize_sync"])
                    .inc();
            }
            return false;
        }

        // If we're recovering from sync mode (lag dropped below hysteresis threshold), log transition
        if is_recovering && lag > 0 {
            info!(
                "✅ [CONSENSUS-RESUME] Resuming consensus after catch-up: lag={} commits ({}% behind quorum), local_commit={}, quorum_commit={}. Node has caught up, returning to normal consensus mode.",
                lag, lag_percentage, local_commit_index, quorum_commit_index
            );
        }

        if self.propagation_delay
            > self
                .context
                .parameters
                .propagation_delay_stop_proposal_threshold
        {
            debug!(
                "Skip proposing for round {clock_round}, high propagation delay {} > {}.",
                self.propagation_delay,
                self.context
                    .parameters
                    .propagation_delay_stop_proposal_threshold
            );
            core_skipped_proposals
                .with_label_values(&["high_propagation_delay"])
                .inc();
            return false;
        }

        let Some(last_known_proposed_round) = self.last_known_proposed_round else {
            debug!(
                "Skip proposing for round {clock_round}, last known proposed round has not been synced yet."
            );
            core_skipped_proposals
                .with_label_values(&["no_last_known_proposed_round"])
                .inc();
            return false;
        };
        if clock_round <= last_known_proposed_round {
            debug!(
                "Skip proposing for round {clock_round} as last known proposed round is {last_known_proposed_round}"
            );
            core_skipped_proposals
                .with_label_values(&["higher_last_known_proposed_round"])
                .inc();
            return false;
        }

        true
    }

    #[tracing::instrument(skip_all)]
    fn try_select_certified_leaders(
        &mut self,
        certified_commits: &mut Vec<CertifiedCommit>,
        limit: usize,
    ) -> Vec<(DecidedLeader, CertifiedCommit)> {
        assert!(limit > 0, "limit should be greater than 0");
        if certified_commits.is_empty() {
            return vec![];
        }

        let to_commit = if certified_commits.len() >= limit {
            // We keep only the number of leaders as dictated by the `limit`
            certified_commits.drain(..limit).collect::<Vec<_>>()
        } else {
            // Otherwise just take all of them and leave the `synced_commits` empty.
            std::mem::take(certified_commits)
        };

        tracing::debug!(
            "Selected {} certified leaders: {}",
            to_commit.len(),
            to_commit.iter().map(|c| c.leader().to_string()).join(",")
        );

        to_commit
            .into_iter()
            .map(|commit| {
                let leader = commit
                    .blocks()
                    .iter()
                    .find(|b| b.reference() == commit.leader())
                    .expect("Certified commit should contain the leader block");

                // There is no knowledge of direct commit with certified commits, so assuming indirect commit.
                let leader = DecidedLeader::Commit(leader.clone(), /* direct */ false);
                UniversalCommitter::update_metrics(&self.context, &leader, Decision::Certified);
                (leader, commit)
            })
            .collect::<Vec<_>>()
    }

    /// Retrieves the next ancestors to propose to form a block at `clock_round` round.
    /// If smart selection is enabled then this will try to select the best ancestors
    /// based on the propagation scores of the authorities.
    fn smart_ancestors_to_propose(
        &mut self,
        clock_round: Round,
        smart_select: bool,
    ) -> (Vec<VerifiedBlock>, BTreeSet<BlockRef>) {
        let node_metrics = &self.context.metrics.node_metrics;
        let _s = node_metrics
            .scope_processing_time
            .with_label_values(&["Core::smart_ancestors_to_propose"])
            .start_timer();

        // Now take the ancestors before the clock_round (excluded) for each authority.
        let all_ancestors = self
            .dag_state
            .read()
            .get_last_cached_block_per_authority(clock_round);

        assert_eq!(
            all_ancestors.len(),
            self.context.committee.size(),
            "Fatal error, number of returned ancestors don't match committee size."
        );

        // Ensure ancestor state is up to date before selecting for proposal.
        let accepted_quorum_rounds = self.round_tracker.read().compute_accepted_quorum_rounds();

        self.ancestor_state_manager
            .update_all_ancestors_state(&accepted_quorum_rounds);

        let ancestor_state_map = self.ancestor_state_manager.get_ancestor_states();

        let quorum_round = clock_round.saturating_sub(1);

        let mut score_and_pending_excluded_ancestors = Vec::new();
        let mut excluded_and_equivocating_ancestors = BTreeSet::new();

        // Propose only ancestors of higher rounds than what has already been proposed.
        // And always include own last proposed block first among ancestors.
        // Start by only including the high scoring ancestors. Low scoring ancestors
        // will be included in a second pass below.
        let included_ancestors = iter::once(self.last_proposed_block().clone())
            .chain(
                all_ancestors
                    .into_iter()
                    .flat_map(|(ancestor, equivocating_ancestors)| {
                        if ancestor.author() == self.context.own_index {
                            return None;
                        }
                        if let Some(last_block_ref) =
                            self.last_included_ancestors[ancestor.author()]
                        {
                            if last_block_ref.round >= ancestor.round() {
                                return None;
                            }
                        }

                        // We will never include equivocating ancestors so add them immediately
                        excluded_and_equivocating_ancestors.extend(equivocating_ancestors);

                        let ancestor_state = ancestor_state_map[ancestor.author()];
                        match ancestor_state {
                            AncestorState::Include => {
                                trace!("Found ancestor {ancestor} with INCLUDE state for round {clock_round}");
                            }
                            AncestorState::Exclude(score) => {
                                trace!("Added ancestor {ancestor} with EXCLUDE state with score {score} to temporary excluded ancestors for round {clock_round}");
                                score_and_pending_excluded_ancestors.push((score, ancestor));
                                return None;
                            }
                        }

                        Some(ancestor)
                    }),
            )
            .collect::<Vec<_>>();

        let mut parent_round_quorum = StakeAggregator::<QuorumThreshold>::new();

        // Check total stake of high scoring parent round ancestors
        for ancestor in included_ancestors
            .iter()
            .filter(|a| a.round() == quorum_round)
        {
            parent_round_quorum.add(ancestor.author(), &self.context.committee);
        }

        if smart_select && !parent_round_quorum.reached_threshold(&self.context.committee) {
            node_metrics.smart_selection_wait.inc();
            debug!(
                "Only found {} stake of good ancestors to include for round {clock_round}, will wait for more.",
                parent_round_quorum.stake()
            );
            return (vec![], BTreeSet::new());
        }

        // Sort scores descending so we can include the best of the pending excluded
        // ancestors first until we reach the threshold.
        score_and_pending_excluded_ancestors.sort_by(|a, b| b.0.cmp(&a.0));

        let mut ancestors_to_propose = included_ancestors;
        let mut excluded_ancestors = Vec::new();
        for (score, ancestor) in score_and_pending_excluded_ancestors.into_iter() {
            let block_hostname = &self.context.committee.authority(ancestor.author()).hostname;
            if !parent_round_quorum.reached_threshold(&self.context.committee)
                && ancestor.round() == quorum_round
            {
                debug!(
                    "Including temporarily excluded parent round ancestor {ancestor} with score {score} to propose for round {clock_round}"
                );
                parent_round_quorum.add(ancestor.author(), &self.context.committee);
                ancestors_to_propose.push(ancestor);
                node_metrics
                    .included_excluded_proposal_ancestors_count_by_authority
                    .with_label_values(&[block_hostname, "timeout"])
                    .inc();
            } else {
                excluded_ancestors.push((score, ancestor));
            }
        }

        // Iterate through excluded ancestors and include the ancestor or the ancestor's ancestor
        // that has been accepted by a quorum of the network. If the original ancestor itself
        // is not included then it will be part of excluded ancestors that are not
        // included in the block but will still be broadcasted to peers.
        for (score, ancestor) in excluded_ancestors.iter() {
            let excluded_author = ancestor.author();
            let block_hostname = &self.context.committee.authority(excluded_author).hostname;
            // A quorum of validators reported to have accepted blocks from the excluded_author up to the low quorum round.
            let mut accepted_low_quorum_round = accepted_quorum_rounds[excluded_author].0;
            // If the accepted quorum round of this ancestor is greater than or equal
            // to the clock round then we want to make sure to set it to clock_round - 1
            // as that is the max round the new block can include as an ancestor.
            accepted_low_quorum_round = accepted_low_quorum_round.min(quorum_round);

            let last_included_round = self.last_included_ancestors[excluded_author]
                .map(|block_ref| block_ref.round)
                .unwrap_or(GENESIS_ROUND);
            if ancestor.round() <= last_included_round {
                // This should have already been filtered out when filtering all_ancestors.
                // Still, ensure previously included ancestors are filtered out.
                continue;
            }

            if last_included_round >= accepted_low_quorum_round {
                excluded_and_equivocating_ancestors.insert(ancestor.reference());
                trace!(
                    "Excluded low score ancestor {} with score {score} to propose for round {clock_round}: last included round {last_included_round} >= accepted low quorum round {accepted_low_quorum_round}",
                    ancestor.reference()
                );
                node_metrics
                    .excluded_proposal_ancestors_count_by_authority
                    .with_label_values(&[block_hostname])
                    .inc();
                continue;
            }

            let ancestor = if ancestor.round() <= accepted_low_quorum_round {
                // Include the ancestor block as it has been seen & accepted by a strong quorum.
                ancestor.clone()
            } else {
                // Exclude this ancestor since it hasn't been accepted by a strong quorum
                excluded_and_equivocating_ancestors.insert(ancestor.reference());
                trace!(
                    "Excluded low score ancestor {} with score {score} to propose for round {clock_round}: ancestor round {} > accepted low quorum round {accepted_low_quorum_round} ",
                    ancestor.reference(),
                    ancestor.round()
                );
                node_metrics
                    .excluded_proposal_ancestors_count_by_authority
                    .with_label_values(&[block_hostname])
                    .inc();

                // Look for an earlier block in the ancestor chain that we can include as there
                // is a gap between the last included round and the accepted low quorum round.
                //
                // Note: Only cached blocks need to be propagated. Committed and GC'ed blocks
                // do not need to be propagated.
                match self.dag_state.read().get_last_cached_block_in_range(
                    excluded_author,
                    last_included_round + 1,
                    accepted_low_quorum_round + 1,
                ) {
                    Some(earlier_ancestor) => {
                        // Found an earlier block that has been propagated well - include it instead
                        earlier_ancestor
                    }
                    None => {
                        // No suitable earlier block found
                        continue;
                    }
                }
            };
            self.last_included_ancestors[excluded_author] = Some(ancestor.reference());
            ancestors_to_propose.push(ancestor.clone());
            trace!(
                "Included low scoring ancestor {} with score {score} seen at accepted low quorum round {accepted_low_quorum_round} to propose for round {clock_round}",
                ancestor.reference()
            );
            node_metrics
                .included_excluded_proposal_ancestors_count_by_authority
                .with_label_values(&[block_hostname, "quorum"])
                .inc();
        }

        assert!(
            parent_round_quorum.reached_threshold(&self.context.committee),
            "Fatal error, quorum not reached for parent round when proposing for round {clock_round}. Possible mismatch between DagState and Core."
        );

        debug!(
            "Included {} ancestors & excluded {} low performing or equivocating ancestors for proposal in round {clock_round}",
            ancestors_to_propose.len(),
            excluded_and_equivocating_ancestors.len()
        );

        (ancestors_to_propose, excluded_and_equivocating_ancestors)
    }

    /// Checks whether all the leaders of the round exist.
    /// TODO: we can leverage some additional signal here in order to more cleverly manipulate later the leader timeout
    /// Ex if we already have one leader - the first in order - we might don't want to wait as much.
    fn leaders_exist(&self, round: Round) -> bool {
        let dag_state = self.dag_state.read();
        for leader in self.leaders(round) {
            // Search for all the leaders. If at least one is not found, then return false.
            // A linear search should be fine here as the set of elements is not expected to be small enough and more sophisticated
            // data structures might not give us much here.
            if !dag_state.contains_cached_block_at_slot(leader) {
                return false;
            }
        }

        true
    }

    /// Returns the leaders of the provided round.
    fn leaders(&self, round: Round) -> Vec<Slot> {
        self.committer
            .get_leaders(round)
            .into_iter()
            .map(|authority_index| Slot::new(round, authority_index))
            .collect()
    }

    /// Returns the 1st leader of the round.
    fn first_leader(&self, round: Round) -> AuthorityIndex {
        self.leaders(round)
            .first()
            .expect(
                "leaders() returned empty list — committee must have at least one leader per round",
            )
            .authority
    }

fn last_proposed_timestamp_ms(&self) -> BlockTimestampMs {
        self.last_proposed_block().timestamp_ms()
    }

fn last_proposed_round(&self) -> Round {
        self.last_proposed_block().round()
    }

fn last_proposed_block(&self) -> VerifiedBlock {
        self.dag_state.read().get_last_proposed_block()
    }

