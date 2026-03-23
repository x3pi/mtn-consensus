// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

//! Peer Health Tracker - Circuit breaker pattern for peer connections
//!
//! Tracks consecutive failures per peer and applies exponential backoff
//! to prevent overwhelming unhealthy peers with requests.

use std::collections::HashMap;
use std::time::{Duration, Instant};
use tracing::{debug, warn};

/// Peer health state
#[allow(dead_code)]
struct PeerHealth {
    consecutive_failures: u32,
    last_failure: Option<Instant>,
    backoff_until: Option<Instant>,
}

impl Default for PeerHealth {
    fn default() -> Self {
        Self {
            consecutive_failures: 0,
            last_failure: None,
            backoff_until: None,
        }
    }
}

/// Tracks health of peers with circuit breaker backoff
pub struct PeerHealthTracker {
    peers: HashMap<u32, PeerHealth>,
}

/// Backoff thresholds
#[allow(dead_code)]
const BACKOFF_THRESHOLD_1: u32 = 3; // 3 failures → 10s backoff
#[allow(dead_code)]
const BACKOFF_THRESHOLD_2: u32 = 5; // 5 failures → 30s backoff
#[allow(dead_code)]
const BACKOFF_THRESHOLD_3: u32 = 10; // 10 failures → 60s backoff

impl PeerHealthTracker {
    pub fn new() -> Self {
        Self {
            peers: HashMap::new(),
        }
    }

    /// Check if a peer is healthy (not in backoff)
    pub fn is_healthy(&self, peer_index: u32) -> bool {
        match self.peers.get(&peer_index) {
            None => true, // Unknown peers are healthy by default
            Some(health) => {
                match health.backoff_until {
                    None => true,
                    Some(until) => Instant::now() >= until, // Backoff expired
                }
            }
        }
    }

    /// Record a successful interaction with a peer — resets failure count
    #[allow(dead_code)]
    pub fn record_success(&mut self, peer_index: u32) {
        if let Some(health) = self.peers.get_mut(&peer_index) {
            if health.consecutive_failures > 0 {
                debug!(
                    "✅ [PEER-HEALTH] Peer {} recovered after {} failures",
                    peer_index, health.consecutive_failures
                );
            }
            health.consecutive_failures = 0;
            health.last_failure = None;
            health.backoff_until = None;
        }
    }

    /// Record a failed interaction with a peer — increments failure count and may trigger backoff
    #[allow(dead_code)]
    pub fn record_failure(&mut self, peer_index: u32) {
        let health = self.peers.entry(peer_index).or_default();
        health.consecutive_failures += 1;
        health.last_failure = Some(Instant::now());

        let backoff_duration = if health.consecutive_failures >= BACKOFF_THRESHOLD_3 {
            Some(Duration::from_secs(60))
        } else if health.consecutive_failures >= BACKOFF_THRESHOLD_2 {
            Some(Duration::from_secs(30))
        } else if health.consecutive_failures >= BACKOFF_THRESHOLD_1 {
            Some(Duration::from_secs(10))
        } else {
            None
        };

        if let Some(duration) = backoff_duration {
            health.backoff_until = Some(Instant::now() + duration);
            warn!(
                "⏸️ [PEER-HEALTH] Peer {} has {} consecutive failures. Backing off for {}s",
                peer_index,
                health.consecutive_failures,
                duration.as_secs()
            );
        }
    }

    /// Get the number of consecutive failures for a peer
    #[allow(dead_code)]
    pub fn failure_count(&self, peer_index: u32) -> u32 {
        self.peers
            .get(&peer_index)
            .map(|h| h.consecutive_failures)
            .unwrap_or(0)
    }

    /// Get indices of all healthy peers from a list
    #[allow(dead_code)]
    pub fn filter_healthy(&self, peer_indices: &[u32]) -> Vec<u32> {
        peer_indices
            .iter()
            .filter(|&&idx| self.is_healthy(idx))
            .copied()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_healthy_by_default() {
        let tracker = PeerHealthTracker::new();
        assert!(tracker.is_healthy(0));
        assert!(tracker.is_healthy(99));
        assert_eq!(tracker.failure_count(0), 0);
    }

    #[test]
    fn test_backoff_after_failures() {
        let mut tracker = PeerHealthTracker::new();

        // 1-2 failures: still healthy
        tracker.record_failure(1);
        tracker.record_failure(1);
        assert!(tracker.is_healthy(1));
        assert_eq!(tracker.failure_count(1), 2);

        // 3rd failure: triggers backoff
        tracker.record_failure(1);
        assert!(!tracker.is_healthy(1));
        assert_eq!(tracker.failure_count(1), 3);

        // Other peers unaffected
        assert!(tracker.is_healthy(2));
    }

    #[test]
    fn test_reset_on_success() {
        let mut tracker = PeerHealthTracker::new();

        // Accumulate failures until backoff
        for _ in 0..5 {
            tracker.record_failure(1);
        }
        assert!(!tracker.is_healthy(1));
        assert_eq!(tracker.failure_count(1), 5);

        // Success resets everything
        tracker.record_success(1);
        assert!(tracker.is_healthy(1));
        assert_eq!(tracker.failure_count(1), 0);
    }

    #[test]
    fn test_filter_healthy() {
        let mut tracker = PeerHealthTracker::new();

        // Put peer 1 in backoff
        for _ in 0..3 {
            tracker.record_failure(1);
        }

        let all_peers = vec![0, 1, 2, 3];
        let healthy = tracker.filter_healthy(&all_peers);
        assert_eq!(healthy, vec![0, 2, 3]); // Peer 1 filtered out
    }

    #[test]
    fn test_escalating_backoff() {
        let mut tracker = PeerHealthTracker::new();

        // 3 failures → 10s backoff
        for _ in 0..3 {
            tracker.record_failure(1);
        }
        assert!(!tracker.is_healthy(1));

        // Reset and go to 5 failures → 30s backoff
        tracker.record_success(1);
        for _ in 0..5 {
            tracker.record_failure(1);
        }
        assert!(!tracker.is_healthy(1));
        assert_eq!(tracker.failure_count(1), 5);

        // Reset and go to 10 failures → 60s backoff
        tracker.record_success(1);
        for _ in 0..10 {
            tracker.record_failure(1);
        }
        assert!(!tracker.is_healthy(1));
        assert_eq!(tracker.failure_count(1), 10);
    }
}
