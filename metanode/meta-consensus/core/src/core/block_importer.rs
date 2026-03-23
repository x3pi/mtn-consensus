use itertools::Itertools;
use std::collections::BTreeSet;
use tracing::trace;

use consensus_types::block::BlockRef;
use mysten_metrics::monitored_scope;

use crate::{block::VerifiedBlock, core::Core, error::ConsensusResult};

impl Core {
    /// Processes the provided blocks and accepts them if possible when their causal history exists.
    /// The method returns:
    /// - The references of ancestors missing their block
    #[tracing::instrument(skip_all)]
    pub(crate) fn add_blocks(
        &mut self,
        blocks: Vec<VerifiedBlock>,
    ) -> ConsensusResult<BTreeSet<BlockRef>> {
        let _scope = monitored_scope("Core::add_blocks");
        let _s = self
            .context
            .metrics
            .node_metrics
            .scope_processing_time
            .with_label_values(&["Core::add_blocks"])
            .start_timer();
        self.context
            .metrics
            .node_metrics
            .core_add_blocks_batch_size
            .observe(blocks.len() as f64);

        let (accepted_blocks, missing_block_refs) = self.block_manager.try_accept_blocks(blocks);

        if !accepted_blocks.is_empty() {
            trace!(
                "Accepted blocks: {}",
                accepted_blocks
                    .iter()
                    .map(|b| b.reference().to_string())
                    .join(",")
            );

            // Try to commit the new blocks if possible.
            self.try_commit(vec![])?;

            // Try to propose now since there are new blocks accepted.
            self.try_propose(false)?;

            // Now set up leader timeout if needed.
            // This needs to be called after try_commit() and try_propose(), which may
            // have advanced the threshold clock round.
            self.try_signal_new_round();
        };

        if !missing_block_refs.is_empty() {
            trace!(
                "Missing block refs: {}",
                missing_block_refs.iter().map(|b| b.to_string()).join(", ")
            );
        }

        Ok(missing_block_refs)
    }

    /// Checks if provided block refs have been accepted. If not, missing block refs are kept for synchronizations.
    /// Returns the references of missing blocks among the input blocks.
    pub(crate) fn check_block_refs(
        &mut self,
        block_refs: Vec<BlockRef>,
    ) -> ConsensusResult<BTreeSet<BlockRef>> {
        let _scope = monitored_scope("Core::check_block_refs");
        let _s = self
            .context
            .metrics
            .node_metrics
            .scope_processing_time
            .with_label_values(&["Core::check_block_refs"])
            .start_timer();
        self.context
            .metrics
            .node_metrics
            .core_check_block_refs_batch_size
            .observe(block_refs.len() as f64);

        // Try to find them via the block manager
        let missing_block_refs = self.block_manager.try_find_blocks(block_refs);

        if !missing_block_refs.is_empty() {
            trace!(
                "Missing block refs: {}",
                missing_block_refs.iter().map(|b| b.to_string()).join(", ")
            );
        }
        Ok(missing_block_refs)
    }

    pub(crate) fn get_missing_blocks(&self) -> BTreeSet<BlockRef> {
        let _scope = monitored_scope("Core::get_missing_blocks");
        self.block_manager.missing_blocks()
    }
}
