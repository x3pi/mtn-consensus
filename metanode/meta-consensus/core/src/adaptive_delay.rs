// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

//! Adaptive delay mechanism to automatically adjust node speed based on network average.
//!
//! When a node is faster than the network average, it will automatically add delay
//! to sync with the network speed. This helps prevent forks and improves network stability.

use parking_lot::RwLock;
use std::{
    collections::VecDeque,
    sync::Arc,
    time::{Duration, Instant},
};
use tracing::debug;

use crate::CommitIndex;

/// Tracks commit rate over a sliding window
pub(crate) struct CommitRateTracker {
    /// History of (timestamp, commit_index) pairs
    commits: VecDeque<(Instant, CommitIndex)>,
    /// Window duration for rate calculation
    window_duration: Duration,
}

impl CommitRateTracker {
    pub(crate) fn new(window_duration: Duration) -> Self {
        Self {
            commits: VecDeque::new(),
            window_duration,
        }
    }

    /// Update tracker with new commit index
    pub(crate) fn update(&mut self, commit_index: CommitIndex) {
        let now = Instant::now();
        self.commits.push_back((now, commit_index));

        // Remove old entries outside window
        while let Some((time, _)) = self.commits.front() {
            if now.duration_since(*time) > self.window_duration {
                self.commits.pop_front();
            } else {
                break;
            }
        }
    }

    /// Calculate commit rate (commits per second)
    pub(crate) fn rate(&self) -> f64 {
        if self.commits.len() < 2 {
            return 0.0;
        }

        let (first_time, first_index) = self
            .commits
            .front()
            .expect("commits deque checked non-empty above");
        let (last_time, last_index) = self
            .commits
            .back()
            .expect("commits deque checked non-empty above");

        let duration = last_time.duration_since(*first_time).as_secs_f64();
        if duration > 0.0 {
            ((*last_index - *first_index) as f64) / duration
        } else {
            0.0
        }
    }
}

/// Shared state for adaptive delay calculation
pub(crate) struct AdaptiveDelayState {
    /// Local commit rate tracker
    local_rate_tracker: Arc<RwLock<CommitRateTracker>>,
    /// Quorum commit rate tracker
    quorum_rate_tracker: Arc<RwLock<CommitRateTracker>>,
    /// Base delay from config
    base_delay_ms: u64,
    /// Whether adaptive delay is enabled
    enabled: bool,
    /// Current adaptive delay being applied
    current_adaptive_delay_ms: Arc<RwLock<u64>>,
}

impl AdaptiveDelayState {
    pub(crate) fn new(base_delay_ms: u64, enabled: bool) -> Self {
        // Use 10 second window for rate calculation
        let window_duration = Duration::from_secs(10);
        Self {
            local_rate_tracker: Arc::new(RwLock::new(CommitRateTracker::new(window_duration))),
            quorum_rate_tracker: Arc::new(RwLock::new(CommitRateTracker::new(window_duration))),
            base_delay_ms,
            enabled,
            current_adaptive_delay_ms: Arc::new(RwLock::new(0)),
        }
    }

    /// Update local commit index
    pub(crate) fn update_local_commit(&self, commit_index: CommitIndex) {
        self.local_rate_tracker.write().update(commit_index);
    }

    /// Update quorum commit index
    pub(crate) fn update_quorum_commit(&self, commit_index: CommitIndex) {
        self.quorum_rate_tracker.write().update(commit_index);
    }

    /// Calculate adaptive delay based on lead and network speed
    pub(crate) fn calculate_adaptive_delay(
        &self,
        local_commit_index: CommitIndex,
        quorum_commit_index: CommitIndex,
    ) -> Duration {
        if !self.enabled {
            return Duration::ZERO;
        }

        // Calculate lead (how many commits ahead)
        let lead = local_commit_index.saturating_sub(quorum_commit_index);

        // Get commit rates
        let local_rate = self.local_rate_tracker.read().rate();
        let quorum_rate = self.quorum_rate_tracker.read().rate();

        // Thresholds for adaptive delay
        const MODERATE_LEAD_THRESHOLD: u32 = 50; // Ahead by 50 commits
        const SEVERE_LEAD_THRESHOLD: u32 = 100; // Ahead by 100 commits
        const MODERATE_LEAD_PERCENTAGE: f64 = 5.0; // Ahead by 5%
        const SEVERE_LEAD_PERCENTAGE: f64 = 10.0; // Ahead by 10%
        const MIN_RATE_DIFF: f64 = 0.1; // Minimum rate difference to trigger delay (10% faster)

        // Calculate lead percentage
        let lead_percentage = if quorum_commit_index > 0 {
            (lead as f64 / quorum_commit_index as f64) * 100.0
        } else {
            0.0
        };

        // Check if node is significantly faster than network
        let rate_diff_ratio = if quorum_rate > 0.0 && local_rate > quorum_rate {
            (local_rate - quorum_rate) / quorum_rate
        } else {
            0.0
        };

        // Calculate adaptive delay
        let adaptive_delay_ms = 0; // Disabled for local testing
        /*
        let adaptive_delay_ms = if lead > SEVERE_LEAD_THRESHOLD
            || lead_percentage > SEVERE_LEAD_PERCENTAGE
            || (rate_diff_ratio > 0.2 && lead > MODERATE_LEAD_THRESHOLD)
        {
            // Severe lead or significantly faster: delay 2x base delay
            self.base_delay_ms * 2
        } else if lead > MODERATE_LEAD_THRESHOLD
            || lead_percentage > MODERATE_LEAD_PERCENTAGE
            || (rate_diff_ratio > MIN_RATE_DIFF && lead > 20)
        {
            // Moderate lead: delay 1.5x base delay
            self.base_delay_ms + self.base_delay_ms / 2
        } else if lead > 20 && rate_diff_ratio > MIN_RATE_DIFF {
            // Small lead but faster rate: delay 1.2x base delay
            self.base_delay_ms + self.base_delay_ms / 5
        } else {
            // Normal: no extra delay
            0
        };
        */

        // Update current adaptive delay
        *self.current_adaptive_delay_ms.write() = adaptive_delay_ms;

        if adaptive_delay_ms > 0 {
            debug!(
                "Adaptive delay: lead={}, lead_pct={:.2}%, local_rate={:.2}, quorum_rate={:.2}, rate_diff={:.2}%, delay={}ms",
                lead, lead_percentage, local_rate, quorum_rate, rate_diff_ratio * 100.0, adaptive_delay_ms
            );
        }

        Duration::from_millis(adaptive_delay_ms)
    }

    /// Get local commit rate
    pub(crate) fn local_rate(&self) -> f64 {
        self.local_rate_tracker.read().rate()
    }

    /// Get quorum commit rate
    pub(crate) fn quorum_rate(&self) -> f64 {
        self.quorum_rate_tracker.read().rate()
    }
}
