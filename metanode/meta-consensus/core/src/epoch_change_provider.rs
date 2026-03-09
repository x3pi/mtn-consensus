// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use std::sync::OnceLock;

/// Trait for providing epoch change data to Core
/// This allows metanode layer to inject epoch change data into blocks
/// without Core needing direct dependency on metanode code
pub trait EpochChangeProvider: Send + Sync {
    /// Get epoch change proposal to include in next block (serialized bytes)
    fn get_proposal(&self) -> Option<Vec<u8>>;

    /// Get epoch change votes to include in next block (serialized bytes)
    fn get_votes(&self) -> Vec<Vec<u8>>;
}

/// Global epoch change provider (set from metanode layer).
/// Uses `OnceLock` for safe, lock-free concurrent access after initialization.
static EPOCH_CHANGE_PROVIDER: OnceLock<Box<dyn EpochChangeProvider>> = OnceLock::new();

/// Initialize global epoch change provider (called from metanode layer)
pub fn init_epoch_change_provider(provider: Box<dyn EpochChangeProvider>) {
    let _ = EPOCH_CHANGE_PROVIDER.set(provider);
}

/// Get epoch change data for block creation (called from Core)
pub fn get_epoch_change_data() -> (Option<Vec<u8>>, Vec<Vec<u8>>) {
    if let Some(provider) = EPOCH_CHANGE_PROVIDER.get() {
        (provider.get_proposal(), provider.get_votes())
    } else {
        (None, Vec::new())
    }
}

/// Trait for processing epoch change data from received blocks
pub trait EpochChangeProcessor: Send + Sync {
    /// Process epoch change proposal from a received block
    fn process_proposal(&self, proposal_bytes: &[u8]);

    /// Process epoch change vote from a received block
    fn process_vote(&self, vote_bytes: &[u8]);
}

/// Global epoch change processor (set from metanode layer).
/// Uses `OnceLock` for safe, lock-free concurrent access after initialization.
static EPOCH_CHANGE_PROCESSOR: OnceLock<Box<dyn EpochChangeProcessor>> = OnceLock::new();

/// Initialize global epoch change processor (called from metanode layer)
pub fn init_epoch_change_processor(processor: Box<dyn EpochChangeProcessor>) {
    let _ = EPOCH_CHANGE_PROCESSOR.set(processor);
}

/// Process epoch change data from a received block (called from AuthorityService)
pub fn process_block_epoch_change(proposal_bytes: Option<&[u8]>, votes_bytes: &[Vec<u8>]) {
    if let Some(processor) = EPOCH_CHANGE_PROCESSOR.get() {
        if let Some(proposal) = proposal_bytes {
            processor.process_proposal(proposal);
        }
        for vote in votes_bytes {
            processor.process_vote(vote);
        }
    }
}
