// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use std::collections::BTreeSet;
use std::ops::Bound::{Excluded, Included, Unbounded};
use tracing::error;

use consensus_config::AuthorityIndex;
use consensus_types::block::{BlockDigest, BlockRef, Round};

use crate::{
    block::{BlockAPI, Slot, VerifiedBlock, GENESIS_ROUND},
    dag_state::dag_state_impl::DagState,
};

impl DagState {
    /// Gets a block by checking cached recent blocks then storage.
    /// Returns None when the block is not found.
    pub(crate) fn get_block(&self, reference: &BlockRef) -> Option<VerifiedBlock> {
        self.get_blocks(&[*reference])
            .pop()
            .expect("Exactly one element should be returned")
    }

    /// Gets blocks by checking genesis, cached recent blocks in memory, then storage.
    /// An element is None when the corresponding block is not found.
    pub(crate) fn get_blocks(&self, block_refs: &[BlockRef]) -> Vec<Option<VerifiedBlock>> {
        let mut blocks = vec![None; block_refs.len()];
        let mut missing = Vec::new();

        for (index, block_ref) in block_refs.iter().enumerate() {
            if block_ref.round == GENESIS_ROUND {
                // Allow the caller to handle the invalid genesis ancestor error.
                if let Some(block) = self.genesis.get(block_ref) {
                    blocks[index] = Some(block.clone());
                }
                continue;
            }
            if let Some(block_info) = self.recent_blocks.get(block_ref) {
                blocks[index] = Some(block_info.block.clone());
                continue;
            }
            missing.push((index, block_ref));
        }

        if missing.is_empty() {
            return blocks;
        }

        let missing_refs = missing
            .iter()
            .map(|(_, block_ref)| **block_ref)
            .collect::<Vec<_>>();
        let store_results = self.store.read_blocks(&missing_refs).unwrap_or_else(|e| {
            panic!("Failed to read_blocks from storage in get_blocks: {:?}", e)
        });
        self.context
            .metrics
            .node_metrics
            .dag_state_store_read_count
            .with_label_values(&["get_blocks"])
            .inc();

        for ((index, _), result) in missing.into_iter().zip(store_results.into_iter()) {
            blocks[index] = result;
        }

        blocks
    }

    /// Gets all uncommitted blocks in a slot.
    /// Uncommitted blocks must exist in memory, so only in-memory blocks are checked.
    pub(crate) fn get_uncommitted_blocks_at_slot(&self, slot: Slot) -> Vec<VerifiedBlock> {
        // TODO: either panic below when the slot is at or below the last committed round,
        // or support reading from storage while limiting storage reads to edge cases.

        let mut blocks = vec![];
        for (_block_ref, block_info) in self.recent_blocks.range((
            Included(BlockRef::new(slot.round, slot.authority, BlockDigest::MIN)),
            Included(BlockRef::new(slot.round, slot.authority, BlockDigest::MAX)),
        )) {
            blocks.push(block_info.block.clone())
        }
        blocks
    }

    /// Gets all uncommitted blocks in a round.
    /// Uncommitted blocks must exist in memory, so only in-memory blocks are checked.
    pub(crate) fn get_uncommitted_blocks_at_round(&self, round: Round) -> Vec<VerifiedBlock> {
        if round <= self.last_commit_round() {
            error!("get_uncommitted_blocks_at_round called with round {} that has committed blocks (last_commit_round={})", round, self.last_commit_round());
            return vec![];
        }

        let mut blocks = vec![];
        for (_block_ref, block_info) in self.recent_blocks.range((
            Included(BlockRef::new(round, AuthorityIndex::ZERO, BlockDigest::MIN)),
            Excluded(BlockRef::new(
                round + 1,
                AuthorityIndex::ZERO,
                BlockDigest::MIN,
            )),
        )) {
            blocks.push(block_info.block.clone())
        }
        blocks
    }

    /// Gets all ancestors in the history of a block at a certain round.
    pub(crate) fn ancestors_at_round(
        &self,
        later_block: &VerifiedBlock,
        earlier_round: Round,
    ) -> Vec<VerifiedBlock> {
        // Iterate through ancestors of later_block in round descending order.
        let mut linked: BTreeSet<BlockRef> = later_block.ancestors().iter().cloned().collect();
        while !linked.is_empty() {
            let round = linked.last().expect("linked set should not be empty").round;
            // Stop after finishing traversal for ancestors above earlier_round.
            if round <= earlier_round {
                break;
            }
            let block_ref = linked.pop_last().expect("linked set should not be empty");
            let Some(block) = self.get_block(&block_ref) else {
                panic!("Block {:?} should exist in DAG!", block_ref);
            };
            linked.extend(block.ancestors().iter().cloned());
        }
        linked
            .range((
                Included(BlockRef::new(
                    earlier_round,
                    AuthorityIndex::ZERO,
                    BlockDigest::MIN,
                )),
                Unbounded,
            ))
            .map(|r| {
                self.get_block(r)
                    .unwrap_or_else(|| panic!("Block {:?} should exist in DAG!", r))
                    .clone()
            })
            .collect()
    }

    /// Gets the last proposed block from this authority.
    /// If no block is proposed yet, returns the genesis block.
    pub(crate) fn get_last_proposed_block(&self) -> VerifiedBlock {
        self.get_last_block_for_authority(self.context.own_index)
    }

    /// Retrieves the last accepted block from the specified `authority`. If no block is found in cache
    /// then the genesis block is returned as no other block has been received from that authority.
    pub(crate) fn get_last_block_for_authority(&self, authority: AuthorityIndex) -> VerifiedBlock {
        if let Some(last) = self.recent_refs_by_authority[authority].last() {
            return self
                .recent_blocks
                .get(last)
                .expect("Block should be found in recent blocks")
                .block
                .clone();
        }

        // if none exists, then fallback to genesis
        let (_, genesis_block) = self
            .genesis
            .iter()
            .find(|(block_ref, _)| block_ref.author == authority)
            .expect("Genesis should be found for authority {authority_index}");
        genesis_block.clone()
    }

    /// Returns cached recent blocks from the specified authority.
    /// Blocks returned are limited to round >= `start`, and cached.
    /// NOTE: caller should not assume returned blocks are always chained.
    /// "Disconnected" blocks can be returned when there are byzantine blocks,
    /// or a previously evicted block is accepted again.
    pub(crate) fn get_cached_blocks(
        &self,
        authority: AuthorityIndex,
        start: Round,
    ) -> Vec<VerifiedBlock> {
        self.get_cached_blocks_in_range(authority, start, Round::MAX, usize::MAX)
    }

    // Retrieves the cached block within the range [start_round, end_round) from a given authority,
    // limited in total number of blocks.
    pub(crate) fn get_cached_blocks_in_range(
        &self,
        authority: AuthorityIndex,
        start_round: Round,
        end_round: Round,
        limit: usize,
    ) -> Vec<VerifiedBlock> {
        if start_round >= end_round || limit == 0 {
            return vec![];
        }

        let mut blocks = vec![];
        for block_ref in self.recent_refs_by_authority[authority].range((
            Included(BlockRef::new(start_round, authority, BlockDigest::MIN)),
            Excluded(BlockRef::new(
                end_round,
                AuthorityIndex::MIN,
                BlockDigest::MIN,
            )),
        )) {
            let block_info = self
                .recent_blocks
                .get(block_ref)
                .expect("Block should exist in recent blocks");
            blocks.push(block_info.block.clone());
            if blocks.len() >= limit {
                break;
            }
        }
        blocks
    }

    // Retrieves the last cached block within the range [start_round, end_round) from a given authority.
    pub(crate) fn get_last_cached_block_in_range(
        &self,
        authority: AuthorityIndex,
        start_round: Round,
        end_round: Round,
    ) -> Option<VerifiedBlock> {
        if start_round >= end_round {
            return None;
        }

        let block_ref = self.recent_refs_by_authority[authority]
            .range((
                Included(BlockRef::new(start_round, authority, BlockDigest::MIN)),
                Excluded(BlockRef::new(
                    end_round,
                    AuthorityIndex::MIN,
                    BlockDigest::MIN,
                )),
            ))
            .last()?;

        self.recent_blocks
            .get(block_ref)
            .map(|block_info| block_info.block.clone())
    }

    /// Returns the last block proposed per authority with `evicted round < round < end_round`.
    /// The method is guaranteed to return results only when the `end_round` is not earlier of the
    /// available cached data for each authority (evicted round + 1), otherwise the method will panic.
    /// It's the caller's responsibility to ensure that is not requesting for earlier rounds.
    /// In case of equivocation for an authority's last slot, one block will be returned (the last in order)
    /// and the other equivocating blocks will be returned.
    pub(crate) fn get_last_cached_block_per_authority(
        &self,
        end_round: Round,
    ) -> Vec<(VerifiedBlock, Vec<BlockRef>)> {
        // Initialize with the genesis blocks as fallback
        let mut blocks = self.genesis.values().cloned().collect::<Vec<_>>();
        let mut equivocating_blocks = vec![vec![]; self.context.committee.size()];

        if end_round == GENESIS_ROUND {
            error!(
                "Attempted to retrieve blocks earlier than the genesis round which is not possible"
            );
            return blocks.into_iter().map(|b| (b, vec![])).collect();
        }

        if end_round == GENESIS_ROUND + 1 {
            return blocks.into_iter().map(|b| (b, vec![])).collect();
        }

        for (authority_index, block_refs) in self.recent_refs_by_authority.iter().enumerate() {
            let authority_index = self
                .context
                .committee
                .to_authority_index(authority_index)
                .expect("authority_index must be valid for committee");

            let last_evicted_round = self.evicted_rounds[authority_index];
            if end_round.saturating_sub(1) <= last_evicted_round {
                panic!(
                        "Attempted to request for blocks of rounds < {end_round}, when the last evicted round is {last_evicted_round} for authority {authority_index}",
                    );
            }

            let block_ref_iter = block_refs
                .range((
                    Included(BlockRef::new(
                        last_evicted_round + 1,
                        authority_index,
                        BlockDigest::MIN,
                    )),
                    Excluded(BlockRef::new(end_round, authority_index, BlockDigest::MIN)),
                ))
                .rev();

            let mut last_round = 0;
            for block_ref in block_ref_iter {
                if last_round == 0 {
                    last_round = block_ref.round;
                    let block_info = self
                        .recent_blocks
                        .get(block_ref)
                        .expect("Block should exist in recent blocks");
                    blocks[authority_index] = block_info.block.clone();
                    continue;
                }
                if block_ref.round < last_round {
                    break;
                }
                equivocating_blocks[authority_index].push(*block_ref);
            }
        }

        blocks.into_iter().zip(equivocating_blocks).collect()
    }

    /// Checks whether a block exists in the slot. The method checks only against the cached data.
    /// If the user asks for a slot that is not within the cached data then a panic is thrown.
    pub(crate) fn contains_cached_block_at_slot(&self, slot: Slot) -> bool {
        // Always return true for genesis slots.
        if slot.round == GENESIS_ROUND {
            return true;
        }

        let eviction_round = self.evicted_rounds[slot.authority];
        if slot.round <= eviction_round {
            panic!(
                    "{}",
                    format!(
                        "Attempted to check for slot {slot} that is <= the last evicted round {eviction_round}"
                    )
                );
        }

        let mut result = self.recent_refs_by_authority[slot.authority].range((
            Included(BlockRef::new(slot.round, slot.authority, BlockDigest::MIN)),
            Included(BlockRef::new(slot.round, slot.authority, BlockDigest::MAX)),
        ));
        result.next().is_some()
    }

    /// Checks whether the required blocks are in cache, if exist, or otherwise will check in store. The method is not caching
    /// back the results, so its expensive if keep asking for cache missing blocks.
    pub(crate) fn contains_blocks(&self, block_refs: Vec<BlockRef>) -> Vec<bool> {
        let mut exist = vec![false; block_refs.len()];
        let mut missing = Vec::new();

        for (index, block_ref) in block_refs.into_iter().enumerate() {
            let recent_refs = &self.recent_refs_by_authority[block_ref.author];
            if recent_refs.contains(&block_ref) || self.genesis.contains_key(&block_ref) {
                exist[index] = true;
            } else if recent_refs.is_empty()
                || recent_refs
                    .last()
                    .expect("recent_refs checked non-empty")
                    .round
                    < block_ref.round
            {
                // Optimization: recent_refs contain the most recent blocks known to this authority.
                // If a block ref is not found there and has a higher round, it definitely is
                // missing from this authority and there is no need to check disk.
                exist[index] = false;
            } else {
                missing.push((index, block_ref));
            }
        }

        if missing.is_empty() {
            return exist;
        }

        let missing_refs = missing
            .iter()
            .map(|(_, block_ref)| *block_ref)
            .collect::<Vec<_>>();
        let store_results = self
            .store
            .contains_blocks(&missing_refs)
            .unwrap_or_else(|e| panic!("Failed to read from storage: {:?}", e));
        self.context
            .metrics
            .node_metrics
            .dag_state_store_read_count
            .with_label_values(&["contains_blocks"])
            .inc();

        for ((index, _), result) in missing.into_iter().zip(store_results.into_iter()) {
            exist[index] = result;
        }

        exist
    }

    pub(crate) fn contains_block(&self, block_ref: &BlockRef) -> bool {
        let blocks = self.contains_blocks(vec![*block_ref]);
        blocks
            .first()
            .cloned()
            .expect("contains_blocks must return exactly one element")
    }
}
