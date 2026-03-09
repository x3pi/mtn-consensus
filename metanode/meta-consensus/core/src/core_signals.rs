// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use consensus_types::block::Round;
use tokio::sync::{broadcast, watch};
use tracing::{debug, warn};

use crate::{
    block::{BlockAPI, ExtendedBlock, GENESIS_ROUND},
    context::Context,
    error::{ConsensusError, ConsensusResult},
};

/// Senders of signals from Core, for outputs and events (ex new block produced).
pub(crate) struct CoreSignals {
    tx_block_broadcast: broadcast::Sender<ExtendedBlock>,
    new_round_sender: watch::Sender<Round>,
    context: Arc<Context>,
}

impl CoreSignals {
    pub fn new(context: Arc<Context>) -> (Self, CoreSignalsReceivers) {
        // Blocks buffered in broadcast channel should be roughly equal to thosed cached in dag state,
        // since the underlying blocks are ref counted so a lower buffer here will not reduce memory
        // usage significantly.
        let (tx_block_broadcast, rx_block_broadcast) = broadcast::channel::<ExtendedBlock>(
            context.parameters.dag_state_cached_rounds as usize,
        );
        let (new_round_sender, new_round_receiver) = watch::channel(0);

        // CRITICAL FIX: Clone the sender to create a keeper that will be owned by AuthorityNode.
        // This prevents the broadcast channel from closing prematurely if Core is dropped
        // before CoreThread can start processing. Without this keeper, race conditions
        // during async spawning can cause ProposedBlockHandler to receive "channel closed"
        // immediately after starting.
        let broadcast_sender_keeper = tx_block_broadcast.clone();

        let me = Self {
            tx_block_broadcast,
            new_round_sender,
            context,
        };

        let receivers = CoreSignalsReceivers {
            rx_block_broadcast,
            new_round_receiver,
            broadcast_sender_keeper,
        };

        (me, receivers)
    }

    /// Sends a signal to all the waiters that a new block has been produced. The method will return
    /// true if block has reached even one subscriber, false otherwise.
    pub(crate) fn new_block(&self, extended_block: ExtendedBlock) -> ConsensusResult<()> {
        // When there is only one authority in committee, it is unnecessary to broadcast
        // the block which will fail anyway without subscribers to the signal.
        if self.context.committee.size() > 1 {
            if extended_block.block.round() == GENESIS_ROUND {
                debug!("Ignoring broadcasting genesis block to peers");
                return Ok(());
            }

            if let Err(err) = self.tx_block_broadcast.send(extended_block) {
                warn!("Couldn't broadcast the block to any receiver: {err}");
                return Err(ConsensusError::Shutdown);
            }
        } else {
            debug!(
                "Did not broadcast block {extended_block:?} to receivers as committee size is <= 1"
            );
        }
        Ok(())
    }

    /// Sends a signal that threshold clock has advanced to new round. The `round_number` is the round at which the
    /// threshold clock has advanced to.
    pub(crate) fn new_round(&mut self, round_number: Round) {
        let _ = self.new_round_sender.send_replace(round_number);
    }
}

/// Receivers of signals from Core.
/// Intentionally un-clonable. Comonents should only subscribe to channels they need.
pub(crate) struct CoreSignalsReceivers {
    rx_block_broadcast: broadcast::Receiver<ExtendedBlock>,
    new_round_receiver: watch::Receiver<Round>,
    /// Keeper for broadcast sender to prevent channel from closing during async spawning race conditions.
    /// This sender clone is held by AuthorityNode to ensure the broadcast channel stays open until
    /// all components have been properly initialized and CoreThread has started.
    broadcast_sender_keeper: broadcast::Sender<ExtendedBlock>,
}

impl CoreSignalsReceivers {
    pub(crate) fn block_broadcast_receiver(&self) -> broadcast::Receiver<ExtendedBlock> {
        self.rx_block_broadcast.resubscribe()
    }

    pub(crate) fn new_round_receiver(&self) -> watch::Receiver<Round> {
        self.new_round_receiver.clone()
    }

    /// Returns a clone of the broadcast sender keeper. This should be stored in AuthorityNode
    /// to prevent the broadcast channel from closing prematurely during async initialization.
    /// Without this keeper, race conditions between tokio::spawn scheduling and constructor
    /// return can cause ProposedBlockHandler to receive "Broadcast channel CLOSED" immediately.
    pub(crate) fn broadcast_sender_keeper(&self) -> broadcast::Sender<ExtendedBlock> {
        self.broadcast_sender_keeper.clone()
    }
}
