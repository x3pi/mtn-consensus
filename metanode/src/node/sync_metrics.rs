// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

//! Sync Metrics - Prometheus observability for RustSyncNode
//!
//! Uses a global singleton pattern to ensure metrics are registered exactly once
//! with the default Prometheus registry. This prevents panics when RustSyncNode
//! is restarted during epoch transitions (which previously caused AlreadyReg errors).

use once_cell::sync::Lazy;
use prometheus::{
    register_counter_vec_with_registry, register_counter_with_registry,
    register_gauge_with_registry, register_histogram_with_registry, Counter, CounterVec, Gauge,
    Histogram, Registry,
};

/// Global singleton metrics instance for the default registry.
/// Registered once, cloned by all RustSyncNode instances.
static GLOBAL_SYNC_METRICS: Lazy<SyncMetrics> =
    Lazy::new(|| SyncMetrics::register(prometheus::default_registry()));

/// Prometheus metrics for the sync node
#[derive(Clone)]
pub struct SyncMetrics {
    /// Total blocks received from peers
    pub blocks_received_total: Counter,
    /// Total blocks successfully sent to Go executor
    pub blocks_sent_to_go_total: Counter,
    /// Current BlockQueue pending count
    pub queue_depth: Gauge,
    /// Current global_exec_index being processed
    pub current_block: Gauge,
    /// Current epoch number
    pub current_epoch: Gauge,
    /// Errors per peer (labeled by peer index)
    pub peer_errors_total: CounterVec,
    /// Duration of each sync round in seconds
    pub sync_round_duration_seconds: Histogram,
    /// Duration of epoch transitions in seconds
    pub epoch_transition_duration_seconds: Histogram,
    /// Total sync round errors
    pub sync_errors_total: Counter,
    /// Duration of peer fetch operations in seconds
    pub peer_fetch_duration_seconds: Histogram,
    /// Number of peers currently in circuit breaker backoff
    pub peers_in_backoff: Gauge,

    // ── Pipeline profiling metrics ──────────────────────────────────────
    /// Phase 1: Duration of Go state queries (get_last_block + get_epoch)
    pub go_state_query_seconds: Histogram,
    /// Phase 2: Duration of block deserialization from BCS bytes
    pub deserialize_duration_seconds: Histogram,
    /// Phase 3: Total time spent in process_queue
    pub process_queue_total_seconds: Histogram,
    /// Phase 3: Duration of queue drain_ready() call
    pub queue_drain_duration_seconds: Histogram,
    /// Phase 3: Duration of leader eth-address resolution per commit
    pub leader_resolve_duration_seconds: Histogram,
    /// Phase 3: Duration of each send_committed_subdag call to Go
    pub go_send_per_commit_seconds: Histogram,
    /// Throughput: blocks sent to Go per second (rolling)
    pub blocks_per_second: Gauge,
}

impl SyncMetrics {
    /// Get the global singleton metrics instance (for production use).
    /// Safe to call multiple times — metrics are registered only once.
    pub fn new(_registry: &Registry) -> Self {
        GLOBAL_SYNC_METRICS.clone()
    }

    /// Register all metrics with a specific registry.
    /// This is called exactly once by the global Lazy initializer.
    fn register(registry: &Registry) -> Self {
        Self {
            blocks_received_total: register_counter_with_registry!(
                "sync_blocks_received_total",
                "Total blocks received from peers",
                registry
            )
            .expect("valid metric: sync_blocks_received_total"),

            blocks_sent_to_go_total: register_counter_with_registry!(
                "sync_blocks_sent_to_go_total",
                "Total blocks successfully sent to Go executor",
                registry
            )
            .expect("valid metric: sync_blocks_sent_to_go_total"),

            queue_depth: register_gauge_with_registry!(
                "sync_queue_depth",
                "Current BlockQueue pending count",
                registry
            )
            .expect("valid metric: sync_queue_depth"),

            current_block: register_gauge_with_registry!(
                "sync_current_block",
                "Current global_exec_index being processed",
                registry
            )
            .expect("valid metric: sync_current_block"),

            current_epoch: register_gauge_with_registry!(
                "sync_current_epoch",
                "Current epoch number",
                registry
            )
            .expect("valid metric: sync_current_epoch"),

            peer_errors_total: register_counter_vec_with_registry!(
                "sync_peer_errors_total",
                "Errors per peer during sync",
                &["peer"],
                registry
            )
            .expect("valid metric: sync_peer_errors_total"),

            sync_round_duration_seconds: register_histogram_with_registry!(
                "sync_round_duration_seconds",
                "Duration of each sync round",
                vec![0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0],
                registry
            )
            .expect("valid metric: sync_round_duration_seconds"),

            epoch_transition_duration_seconds: register_histogram_with_registry!(
                "sync_epoch_transition_duration_seconds",
                "Duration of epoch transitions",
                vec![0.1, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0],
                registry
            )
            .expect("valid metric: sync_epoch_transition_duration_seconds"),

            sync_errors_total: register_counter_with_registry!(
                "sync_errors_total",
                "Total sync round errors",
                registry
            )
            .expect("valid metric: sync_errors_total"),

            peer_fetch_duration_seconds: register_histogram_with_registry!(
                "sync_peer_fetch_duration_seconds",
                "Duration of peer fetch operations",
                vec![0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0],
                registry
            )
            .expect("valid metric: sync_peer_fetch_duration_seconds"),

            peers_in_backoff: register_gauge_with_registry!(
                "sync_peers_in_backoff",
                "Number of peers currently in circuit breaker backoff",
                registry
            )
            .expect("valid metric: sync_peers_in_backoff"),

            // ── Pipeline profiling ─────────────────────────────────────
            go_state_query_seconds: register_histogram_with_registry!(
                "sync_go_state_query_seconds",
                "Phase 1: Go RPC calls (get_last_block + get_epoch)",
                vec![0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0],
                registry
            )
            .expect("valid metric: sync_go_state_query_seconds"),

            deserialize_duration_seconds: register_histogram_with_registry!(
                "sync_deserialize_duration_seconds",
                "Phase 2: Block deserialization from BCS",
                vec![0.0001, 0.0005, 0.001, 0.005, 0.01, 0.05, 0.1, 0.5],
                registry
            )
            .expect("valid metric: sync_deserialize_duration_seconds"),

            process_queue_total_seconds: register_histogram_with_registry!(
                "sync_process_queue_total_seconds",
                "Phase 3: Total process_queue duration",
                vec![0.001, 0.005, 0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0],
                registry
            )
            .expect("valid metric: sync_process_queue_total_seconds"),

            queue_drain_duration_seconds: register_histogram_with_registry!(
                "sync_queue_drain_duration_seconds",
                "Phase 3: drain_ready() call duration",
                vec![0.00001, 0.0001, 0.001, 0.01, 0.1],
                registry
            )
            .expect("valid metric: sync_queue_drain_duration_seconds"),

            leader_resolve_duration_seconds: register_histogram_with_registry!(
                "sync_leader_resolve_duration_seconds",
                "Phase 3: Leader eth-address resolution per commit",
                vec![0.0001, 0.001, 0.01, 0.05, 0.1, 0.5, 1.0],
                registry
            )
            .expect("valid metric: sync_leader_resolve_duration_seconds"),

            go_send_per_commit_seconds: register_histogram_with_registry!(
                "sync_go_send_per_commit_seconds",
                "Phase 3: send_committed_subdag per commit",
                vec![0.0001, 0.0005, 0.001, 0.005, 0.01, 0.05, 0.1, 0.5],
                registry
            )
            .expect("valid metric: sync_go_send_per_commit_seconds"),

            blocks_per_second: register_gauge_with_registry!(
                "sync_blocks_per_second",
                "Rolling throughput: blocks sent to Go per second",
                registry
            )
            .expect("valid metric: sync_blocks_per_second"),
        }
    }

    /// Create metrics with a new isolated registry (for testing only).
    /// Each test gets its own registry, so no conflicts.
    pub fn new_for_test() -> Self {
        Self::register(&Registry::new())
    }
}
