// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

#![allow(dead_code, unused_imports)]

use std::sync::Arc;
use tokio::time::{interval, Duration};
use tracing::{debug, info};

use crate::node::executor_client::ExecutorClient;

/// The types of alerts that the LagMonitor can emit
pub enum LagAlert {
    /// Go is moderately behind Rust — log warning, increase monitoring frequency
    ModerateLag {
        rust_gei: u64,
        go_gei: u64,
        go_block_number: u64,
        gap: u64,
        go_rate: f64,
    },
    /// Go is severely behind Rust — node should consider pausing consensus
    SevereLag {
        rust_gei: u64,
        go_gei: u64,
        go_block_number: u64,
        gap: u64,
        go_rate: f64,
    },
    /// Go has caught up — resume normal operations
    Recovered { rust_gei: u64, go_gei: u64 },
}

/// A background task that monitors the gap between Rust consensus and Go execution.
/// It works by comparing the shared next expected GEI in Rust with the actual GEI from Go.
pub struct LagMonitor {
    executor_client: Arc<ExecutorClient>,
    shared_last_global_exec_index: Arc<tokio::sync::Mutex<u64>>,
    lag_alert_sender: tokio::sync::mpsc::UnboundedSender<LagAlert>,
    moderate_lag_threshold: u64,
    severe_lag_threshold: u64,
}

impl LagMonitor {
    pub fn new(
        executor_client: Arc<ExecutorClient>,
        shared_last_global_exec_index: Arc<tokio::sync::Mutex<u64>>,
        lag_alert_sender: tokio::sync::mpsc::UnboundedSender<LagAlert>,
    ) -> Self {
        Self {
            executor_client,
            shared_last_global_exec_index,
            lag_alert_sender,
            moderate_lag_threshold: 100, // Warn if Go is 100 blocks behind
            severe_lag_threshold: 200,   // Critical alert if 200 blocks behind
        }
    }

    /// Set custom thresholds for moderate and severe lag alerts
    pub fn with_thresholds(mut self, moderate: u64, severe: u64) -> Self {
        self.moderate_lag_threshold = moderate;
        self.severe_lag_threshold = severe;
        self
    }

    /// Run the lag monitor loop. This is intended to be spawned as a tokio task.
    pub async fn run(self) {
        let mut interval = interval(Duration::from_secs(5));

        let mut last_go_gei = self
            .executor_client
            .get_last_global_exec_index()
            .await
            .unwrap_or(0);
        let mut last_check_time = tokio::time::Instant::now();
        let mut currently_lagging = false;

        info!(
            "🛡️ [LAG-MONITOR] Started with moderate_threshold={}, severe_threshold={}",
            self.moderate_lag_threshold, self.severe_lag_threshold
        );

        loop {
            interval.tick().await;

            // 1. Get current Rust GEI (what we've committed)
            let rust_gei = *self.shared_last_global_exec_index.lock().await;

            // 2. Get current Go GEI (what Go has finished executing)
            let go_gei = self
                .executor_client
                .get_last_global_exec_index()
                .await
                .unwrap_or(0);

            // 2.5 Get current Go block number
            let go_block_number = self
                .executor_client
                .get_last_block_number()
                .await
                .map(|(n, _, _)| n)
                .unwrap_or(0);

            // 3. Calculate metrics
            let gap = rust_gei.saturating_sub(go_gei);
            let now = tokio::time::Instant::now();
            let elapsed_secs = now.duration_since(last_check_time).as_secs_f64();

            let go_rate = if elapsed_secs > 0.0 && go_gei >= last_go_gei {
                (go_gei - last_go_gei) as f64 / elapsed_secs
            } else {
                0.0
            };

            // Only analyze if the chain is actually moving (rust_gei > 0)
            if rust_gei > 0 {
                if gap > self.severe_lag_threshold {
                    // SEVERE LAG
                    let _ = self.lag_alert_sender.send(LagAlert::SevereLag {
                        rust_gei,
                        go_gei,
                        go_block_number,
                        gap,
                        go_rate,
                    });
                    currently_lagging = true;
                } else if gap > self.moderate_lag_threshold {
                    // MODERATE LAG
                    let _ = self.lag_alert_sender.send(LagAlert::ModerateLag {
                        rust_gei,
                        go_gei,
                        go_block_number,
                        gap,
                        go_rate,
                    });
                    currently_lagging = true;
                } else if currently_lagging && gap < (self.moderate_lag_threshold / 2) {
                    // RECOVERED (hysteresis: gap must drop to half the moderate threshold)
                    let _ = self
                        .lag_alert_sender
                        .send(LagAlert::Recovered { rust_gei, go_gei });
                    currently_lagging = false;
                } else {
                    // Healthy
                    debug!(
                        "[LAG-MONITOR] Healthy: rust={}, go={}, gap={}, rate={:.1} blk/s",
                        rust_gei, go_gei, gap, go_rate
                    );
                }
            }

            // Update for next tick
            last_check_time = now;
            last_go_gei = go_gei;
        }
    }
}
