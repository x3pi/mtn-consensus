// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use serde::{Deserialize, Serialize};
use tracing::info;

/// Status for reconfiguration certificate handling
/// Similar to Sui's ReconfigCertStatus but adapted for metanode's consensus protocol
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ReconfigCertStatus {
    /// Accept all certificates and transactions
    AcceptAll,

    /// Reject user certificates but still accept system transactions from consensus
    /// (e.g., epoch change proposals/votes, randomness generation)
    RejectUserCerts,

    /// Reject all certificates including consensus ones, but still accept system transactions
    /// and process previously deferred transactions
    RejectAllCerts,

    /// Reject all transactions including system transactions
    RejectAllTx,
}

/// Reconfiguration state for smooth epoch transitions
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReconfigState {
    status: ReconfigCertStatus,
}

impl Default for ReconfigState {
    fn default() -> Self {
        Self {
            status: ReconfigCertStatus::AcceptAll,
        }
    }
}

impl ReconfigState {
    /// Close user certificate acceptance, transitioning to RejectUserCerts
    pub fn close_user_certs(&mut self) {
        if matches!(self.status, ReconfigCertStatus::AcceptAll) {
            info!("Closing user certificates during reconfiguration");
            self.status = ReconfigCertStatus::RejectUserCerts;
        }
    }

    /// Close all certificate acceptance, transitioning to RejectAllCerts
    pub fn close_all_certs(&mut self) {
        if !matches!(self.status, ReconfigCertStatus::RejectAllTx) {
            info!("Closing all certificates during reconfiguration");
            self.status = ReconfigCertStatus::RejectAllCerts;
        }
    }

    /// Close all transaction acceptance, transitioning to RejectAllTx
    pub fn close_all_tx(&mut self) {
        info!("Closing all transactions during reconfiguration");
        self.status = ReconfigCertStatus::RejectAllTx;
    }

    /// Check if user certificates should be accepted
    pub fn should_accept_user_certs(&self) -> bool {
        matches!(self.status, ReconfigCertStatus::AcceptAll)
    }

    /// Check if consensus certificates should be accepted
    pub fn should_accept_consensus_certs(&self) -> bool {
        matches!(
            self.status,
            ReconfigCertStatus::AcceptAll | ReconfigCertStatus::RejectUserCerts
        )
    }

    /// Check if any certificates should be accepted
    pub fn should_accept_certs(&self) -> bool {
        !matches!(
            self.status,
            ReconfigCertStatus::RejectAllCerts | ReconfigCertStatus::RejectAllTx
        )
    }

    /// Check if any transactions should be accepted
    pub fn should_accept_tx(&self) -> bool {
        !matches!(self.status, ReconfigCertStatus::RejectAllTx)
    }

    /// Check if in reject user certs state
    pub fn is_reject_user_certs(&self) -> bool {
        matches!(self.status, ReconfigCertStatus::RejectUserCerts)
    }

    /// Check if in reject all certs state
    pub fn is_reject_all_certs(&self) -> bool {
        matches!(self.status, ReconfigCertStatus::RejectAllCerts)
    }

    /// Check if in reject all tx state
    pub fn is_reject_all_tx(&self) -> bool {
        matches!(self.status, ReconfigCertStatus::RejectAllTx)
    }

    /// Get current status
    pub fn status(&self) -> &ReconfigCertStatus {
        &self.status
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reconfig_state_transitions() {
        let mut state = ReconfigState::default();

        // Initial state
        assert!(state.should_accept_user_certs());
        assert!(state.should_accept_consensus_certs());
        assert!(state.should_accept_certs());
        assert!(state.should_accept_tx());

        // Close user certs
        state.close_user_certs();
        assert!(!state.should_accept_user_certs());
        assert!(state.should_accept_consensus_certs());
        assert!(state.should_accept_certs());
        assert!(state.should_accept_tx());
        assert!(state.is_reject_user_certs());

        // Close all certs
        state.close_all_certs();
        assert!(!state.should_accept_user_certs());
        assert!(!state.should_accept_consensus_certs());
        assert!(!state.should_accept_certs());
        assert!(state.should_accept_tx());
        assert!(state.is_reject_all_certs());

        // Close all tx
        state.close_all_tx();
        assert!(!state.should_accept_user_certs());
        assert!(!state.should_accept_consensus_certs());
        assert!(!state.should_accept_certs());
        assert!(!state.should_accept_tx());
        assert!(state.is_reject_all_tx());
    }

    #[test]
    fn test_reconfig_state_no_downgrade() {
        let mut state = ReconfigState::default();

        // Try to close all certs first
        state.close_all_certs();
        assert!(state.is_reject_all_certs());

        // Try to close user certs again (should not change)
        state.close_user_certs();
        assert!(state.is_reject_all_certs());
        assert!(!state.should_accept_user_certs());

        // Try to close all tx then certs (should not change)
        state.close_all_tx();
        state.close_all_certs();
        assert!(state.is_reject_all_tx());
    }

    #[test]
    fn test_reconfig_state_acceptance_checks() {
        let mut state = ReconfigState::default();

        // Initial state
        assert!(state.should_accept_user_certs());
        assert!(state.should_accept_consensus_certs());
        assert!(state.should_accept_certs());
        assert!(state.should_accept_tx());

        // Close user certs
        state.close_user_certs();
        assert!(!state.should_accept_user_certs());
        assert!(state.should_accept_consensus_certs());
        assert!(state.should_accept_certs()); // Still accepts consensus certs
        assert!(state.should_accept_tx());

        // Close all certs
        state.close_all_certs();
        assert!(!state.should_accept_user_certs());
        assert!(!state.should_accept_consensus_certs());
        assert!(!state.should_accept_certs());
        assert!(state.should_accept_tx());

        // Close all tx
        state.close_all_tx();
        assert!(!state.should_accept_user_certs());
        assert!(!state.should_accept_consensus_certs());
        assert!(!state.should_accept_certs());
        assert!(!state.should_accept_tx());
    }

    #[test]
    fn test_reconfig_state_status_queries() {
        let mut state = ReconfigState::default();

        assert!(!state.is_reject_user_certs());
        assert!(!state.is_reject_all_certs());
        assert!(!state.is_reject_all_tx());

        state.close_user_certs();
        assert!(state.is_reject_user_certs());
        assert!(!state.is_reject_all_certs());
        assert!(!state.is_reject_all_tx());

        state.close_all_certs();
        assert!(!state.is_reject_user_certs());
        assert!(state.is_reject_all_certs());
        assert!(!state.is_reject_all_tx());

        state.close_all_tx();
        assert!(!state.is_reject_user_certs());
        assert!(!state.is_reject_all_certs());
        assert!(state.is_reject_all_tx());
    }
}
