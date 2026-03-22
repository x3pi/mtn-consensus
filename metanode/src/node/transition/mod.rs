// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

//! Epoch and mode transitions for the metanode.
//!
//! This module is split into submodules for maintainability:
//! - `epoch_transition`: Full epoch transitions (epoch N → N+1)
//! - `mode_transition`: Mode-only transitions (SyncOnly → Validator within same epoch)
//! - `consensus_setup`: Validator and SyncOnly infrastructure setup
//! - `verification`: Post-transition verification and readiness checks
//! - `demotion`: Validator → SyncOnly demotion and role determination
//! - `tx_recovery`: Transaction recovery across epoch boundaries

mod consensus_setup;
pub mod demotion;
pub mod epoch_transition;
pub mod mode_transition;
pub mod tx_recovery;
mod verification;

// Re-export public items
pub use epoch_transition::transition_to_epoch_from_system_tx;
pub use tx_recovery::{load_committed_transaction_hashes, save_committed_transaction_hashes_batch};
