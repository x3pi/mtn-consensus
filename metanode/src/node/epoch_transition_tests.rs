// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

//! Integration tests for epoch transition components.
//!
//! Tests cover:
//! 1. StateTransitionManager lifecycle (multi-epoch, rollback, retry)
//! 2. CheckpointManager crash recovery (full lifecycle, incomplete detection)
//! 3. Persistence helpers (roundtrip, legacy compat, uvarint encoding)
//! 4. Cross-module interaction (state manager + checkpoint manager together)

use crate::node::epoch_checkpoint::{CheckpointManager, TransitionCheckpoint, TransitionState};
use crate::node::epoch_transition_manager::{StateTransitionManager, TransitionError};
use crate::node::executor_client::persistence::{
    load_persisted_last_index, persist_last_block_number, persist_last_sent_index,
    read_last_block_number, write_uvarint,
};
use std::sync::Arc;
use tempfile::tempdir;

// ============================================================================
// Group 1: StateTransitionManager Lifecycle
// ============================================================================

#[tokio::test]
async fn test_multi_epoch_sequential() {
    let manager = Arc::new(StateTransitionManager::new(1, true));
    assert_eq!(manager.current_epoch(), 1);
    assert!(manager.is_validator());

    // Advance 1 → 2
    manager.try_start_epoch_transition(2, "test").await.unwrap();
    assert!(manager.is_transition_in_progress());
    manager.complete_epoch_transition(2).await;
    assert_eq!(manager.current_epoch(), 2);
    assert!(!manager.is_transition_in_progress());

    // Advance 2 → 3
    // Need to wait a bit for debounce
    tokio::time::sleep(std::time::Duration::from_secs(4)).await;
    manager.try_start_epoch_transition(3, "test").await.unwrap();
    manager.complete_epoch_transition(3).await;
    assert_eq!(manager.current_epoch(), 3);

    // Advance 3 → 4
    tokio::time::sleep(std::time::Duration::from_secs(4)).await;
    manager.try_start_epoch_transition(4, "test").await.unwrap();
    manager.complete_epoch_transition(4).await;
    assert_eq!(manager.current_epoch(), 4);

    // Mode should still be Validator throughout
    assert!(manager.is_validator());
}

#[tokio::test]
async fn test_epoch_rollback_rejected() {
    let manager = Arc::new(StateTransitionManager::new(5, false));

    // Try to go backwards: 5 → 3 (should fail)
    let result = manager.try_start_epoch_transition(3, "rollback").await;
    assert!(
        matches!(
            result,
            Err(TransitionError::EpochAlreadyCurrent {
                current: 5,
                requested: 3
            })
        ),
        "Expected EpochAlreadyCurrent, got {:?}",
        result
    );

    // Try same epoch: 5 → 5 (should also fail)
    let result = manager.try_start_epoch_transition(5, "same").await;
    assert!(
        matches!(
            result,
            Err(TransitionError::EpochAlreadyCurrent {
                current: 5,
                requested: 5
            })
        ),
        "Expected EpochAlreadyCurrent, got {:?}",
        result
    );

    // Forward should still work
    manager
        .try_start_epoch_transition(6, "forward")
        .await
        .unwrap();
    assert!(manager.is_transition_in_progress());
    manager.complete_epoch_transition(6).await;
    assert_eq!(manager.current_epoch(), 6);
}

#[tokio::test]
async fn test_failed_transition_allows_retry() {
    let manager = Arc::new(StateTransitionManager::new(1, true));

    // Start transition
    manager
        .try_start_epoch_transition(2, "first_attempt")
        .await
        .unwrap();
    assert!(manager.is_transition_in_progress());

    // Fail it
    manager.fail_transition("simulated failure").await;
    assert!(!manager.is_transition_in_progress());
    // Epoch should NOT have advanced
    assert_eq!(manager.current_epoch(), 1);

    // Retry the same transition — should succeed
    manager
        .try_start_epoch_transition(2, "retry")
        .await
        .unwrap();
    assert!(manager.is_transition_in_progress());
    manager.complete_epoch_transition(2).await;
    assert_eq!(manager.current_epoch(), 2);
}

// ============================================================================
// Group 2: CheckpointManager Crash Recovery
// ============================================================================

#[tokio::test]
async fn test_checkpoint_full_lifecycle() {
    let dir = tempdir().unwrap();
    let checkpoint_path = dir.path().join("transition.checkpoint");

    // Step 1: AdvancedEpochToGo
    let cp1 = TransitionCheckpoint::new(
        TransitionState::AdvancedEpochToGo {
            epoch: 5,
            boundary_block: 1000,
            boundary_gei: 0,
            timestamp_ms: 1700000000000,
        },
        "node-0",
    );
    cp1.save(&checkpoint_path).await.unwrap();
    let loaded = TransitionCheckpoint::load(&checkpoint_path)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.state.name(), "AdvancedEpochToGo");
    assert_eq!(loaded.state.epoch(), Some(5));

    // Step 2: CommitteeFetched
    let cp2 = TransitionCheckpoint::new(
        TransitionState::CommitteeFetched {
            epoch: 5,
            committee_hash: [42u8; 32],
            validator_count: 4,
        },
        "node-0",
    );
    cp2.save(&checkpoint_path).await.unwrap();
    let loaded = TransitionCheckpoint::load(&checkpoint_path)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.state.name(), "CommitteeFetched");
    if let TransitionState::CommitteeFetched {
        validator_count, ..
    } = loaded.state
    {
        assert_eq!(validator_count, 4);
    } else {
        panic!("Expected CommitteeFetched state");
    }

    // Step 3: AuthorityStopped
    let cp3 = TransitionCheckpoint::new(
        TransitionState::AuthorityStopped {
            epoch: 5,
            previous_epoch: 4,
        },
        "node-0",
    );
    cp3.save(&checkpoint_path).await.unwrap();
    let loaded = TransitionCheckpoint::load(&checkpoint_path)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.state.name(), "AuthorityStopped");

    // Step 4: Completed
    let cp4 = TransitionCheckpoint::new(
        TransitionState::Completed {
            epoch: 5,
            completed_at_ms: 1700000060000,
        },
        "node-0",
    );
    cp4.save(&checkpoint_path).await.unwrap();
    let loaded = TransitionCheckpoint::load(&checkpoint_path)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.state.name(), "Completed");
    assert_eq!(loaded.state.epoch(), Some(5));
}

#[tokio::test]
async fn test_incomplete_checkpoint_detected() {
    let dir = tempdir().unwrap();
    let manager = CheckpointManager::new(dir.path(), "node-1");

    // Initially no incomplete transition
    assert!(!manager.has_incomplete_transition().await);

    // Save a mid-transition checkpoint
    manager
        .checkpoint_advance_epoch(3, 500, 500, 1700000000000)
        .await
        .unwrap();

    // Should detect incomplete transition
    assert!(manager.has_incomplete_transition().await);

    // Get the incomplete transition and verify resume instructions
    let incomplete = manager.get_incomplete_transition().await.unwrap().unwrap();
    assert_eq!(incomplete.state.epoch(), Some(3));
    let instructions = CheckpointManager::get_resume_instructions(&incomplete.state);
    assert!(
        instructions.contains("fetch_committee"),
        "Expected resume instruction about fetch_committee, got: {}",
        instructions
    );

    // Save committee fetched checkpoint
    manager
        .checkpoint_committee_fetched(3, [0u8; 32], 4)
        .await
        .unwrap();
    let incomplete = manager.get_incomplete_transition().await.unwrap().unwrap();
    let instructions = CheckpointManager::get_resume_instructions(&incomplete.state);
    assert!(
        instructions.contains("stop_authority"),
        "Expected resume instruction about stop_authority, got: {}",
        instructions
    );

    // Complete the transition
    manager.complete_transition(3).await.unwrap();
    assert!(!manager.has_incomplete_transition().await);
}

// ============================================================================
// Group 3: Persistence Helpers
// ============================================================================

#[tokio::test]
async fn test_persist_and_load_sent_index() {
    let dir = tempdir().unwrap();
    let storage_path = dir.path();

    // Persist index and commit
    persist_last_sent_index(storage_path, 12345, 67)
        .await
        .unwrap();

    // Load and verify
    let result = load_persisted_last_index(storage_path);
    assert_eq!(result, Some((12345, 67)));

    // Overwrite with new values
    persist_last_sent_index(storage_path, 99999, 200)
        .await
        .unwrap();
    let result = load_persisted_last_index(storage_path);
    assert_eq!(result, Some((99999, 200)));
}

#[tokio::test]
async fn test_persist_legacy_format_compat() {
    let dir = tempdir().unwrap();
    let storage_path = dir.path();

    // Manually write 8-byte legacy format (u64 only, no commit_index)
    let persist_dir = storage_path.join("executor_state");
    std::fs::create_dir_all(&persist_dir).unwrap();
    let file_path = persist_dir.join("last_sent_index.bin");

    let legacy_index: u64 = 42;
    std::fs::write(&file_path, legacy_index.to_le_bytes()).unwrap();

    // Load should detect legacy format and default commit_index to 0
    let result = load_persisted_last_index(storage_path);
    assert_eq!(
        result,
        Some((42, 0)),
        "Legacy format should return index=42, commit=0"
    );
}

#[tokio::test]
async fn test_persist_last_block_number_roundtrip() {
    let dir = tempdir().unwrap();
    let storage_path = dir.path();

    // Roundtrip various block numbers
    for block_number in [0u64, 1, 1000, u64::MAX] {
        persist_last_block_number(storage_path, block_number)
            .await
            .unwrap();
        let loaded = read_last_block_number(storage_path).await.unwrap();
        assert_eq!(
            loaded, block_number,
            "block_number mismatch for value {}",
            block_number
        );
    }
}

#[tokio::test]
async fn test_write_uvarint_encoding() {
    // Test Go-compatible uvarint encoding for known values
    // Reference: Go's encoding/binary.PutUvarint

    // 0 → [0x00]
    let mut buf = Vec::new();
    write_uvarint(&mut buf, 0).unwrap();
    assert_eq!(buf, vec![0x00]);

    // 127 → [0x7F]
    buf.clear();
    write_uvarint(&mut buf, 127).unwrap();
    assert_eq!(buf, vec![0x7F]);

    // 128 → [0x80, 0x01]
    buf.clear();
    write_uvarint(&mut buf, 128).unwrap();
    assert_eq!(buf, vec![0x80, 0x01]);

    // 300 → [0xAC, 0x02]
    buf.clear();
    write_uvarint(&mut buf, 300).unwrap();
    assert_eq!(buf, vec![0xAC, 0x02]);

    // u64::MAX → 10 bytes
    buf.clear();
    write_uvarint(&mut buf, u64::MAX).unwrap();
    assert_eq!(buf.len(), 10, "u64::MAX should encode to 10 bytes");
    // First 9 bytes should all have high bit set
    for byte in &buf[..9] {
        assert!(
            byte & 0x80 != 0,
            "byte {:#04x} should have high bit set",
            byte
        );
    }
    // Last byte should NOT have high bit set
    assert!(buf[9] & 0x80 == 0, "last byte should not have high bit set");
}

// ============================================================================
// Group 4: Cross-Module Interaction
// ============================================================================

#[tokio::test]
async fn test_epoch_transition_with_checkpoint_and_state() {
    let dir = tempdir().unwrap();
    let state_manager = Arc::new(StateTransitionManager::new(1, true));
    let checkpoint_manager = CheckpointManager::new(dir.path(), "node-0");

    assert_eq!(state_manager.current_epoch(), 1);
    assert!(!checkpoint_manager.has_incomplete_transition().await);

    // --- Simulate epoch 1 → 2 transition ---

    // Step 1: Start transition in state manager
    state_manager
        .try_start_epoch_transition(2, "epoch_monitor")
        .await
        .unwrap();
    assert!(state_manager.is_transition_in_progress());

    // Step 2: Checkpoint advance_epoch
    checkpoint_manager
        .checkpoint_advance_epoch(2, 500, 500, 1700000000000)
        .await
        .unwrap();
    assert!(checkpoint_manager.has_incomplete_transition().await);

    // Step 3: Checkpoint committee_fetched
    checkpoint_manager
        .checkpoint_committee_fetched(2, [1u8; 32], 4)
        .await
        .unwrap();

    // Step 4: Checkpoint authority_stopped
    checkpoint_manager
        .checkpoint_authority_stopped(2, 1)
        .await
        .unwrap();

    // Step 5: Complete both managers
    state_manager.complete_epoch_transition(2).await;
    checkpoint_manager.complete_transition(2).await.unwrap();

    // Verify final state
    assert_eq!(state_manager.current_epoch(), 2);
    assert!(!state_manager.is_transition_in_progress());
    assert!(!checkpoint_manager.has_incomplete_transition().await);

    // --- Simulate crash mid-transition for epoch 2 → 3 ---
    tokio::time::sleep(std::time::Duration::from_secs(4)).await;

    state_manager
        .try_start_epoch_transition(3, "epoch_monitor")
        .await
        .unwrap();

    checkpoint_manager
        .checkpoint_advance_epoch(3, 1000, 1000, 1700000060000)
        .await
        .unwrap();

    // "Crash" — don't complete. Verify checkpoint captures the partial state.
    assert!(state_manager.is_transition_in_progress());
    assert!(checkpoint_manager.has_incomplete_transition().await);

    let incomplete = checkpoint_manager
        .get_incomplete_transition()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(incomplete.state.epoch(), Some(3));
    assert_eq!(incomplete.state.name(), "AdvancedEpochToGo");

    // Recover: fail the state manager, then retry
    state_manager.fail_transition("simulated crash").await;
    assert!(!state_manager.is_transition_in_progress());
    // Epoch should still be 2 (not advanced)
    assert_eq!(state_manager.current_epoch(), 2);

    // Retry the transition
    state_manager
        .try_start_epoch_transition(3, "recovery")
        .await
        .unwrap();
    state_manager.complete_epoch_transition(3).await;
    checkpoint_manager.complete_transition(3).await.unwrap();

    assert_eq!(state_manager.current_epoch(), 3);
    assert!(!checkpoint_manager.has_incomplete_transition().await);
}

// ============================================================================
// Group 5: RPC Circuit Breaker
// ============================================================================

use crate::node::rpc_circuit_breaker::{CircuitBreakerConfig, CircuitState, RpcCircuitBreaker};

#[test]
fn test_circuit_breaker_starts_closed() {
    let cb = RpcCircuitBreaker::new();
    assert_eq!(cb.state("any_method"), CircuitState::Closed);
    assert_eq!(cb.failure_count("any_method"), 0);
    assert_eq!(cb.total_rejections("any_method"), 0);
    // Should allow calls
    assert!(cb.check("any_method").is_ok());
}

#[test]
fn test_circuit_breaker_opens_after_failures() {
    let config = CircuitBreakerConfig {
        failure_threshold: 3,
        cooldown_duration: std::time::Duration::from_secs(60),
        success_threshold: 1,
    };
    let cb = RpcCircuitBreaker::with_config(config);

    // 2 failures: still closed
    cb.record_failure("get_epoch");
    cb.record_failure("get_epoch");
    assert_eq!(cb.state("get_epoch"), CircuitState::Closed);
    assert!(cb.check("get_epoch").is_ok());

    // 3rd failure: opens circuit
    cb.record_failure("get_epoch");
    assert_eq!(cb.state("get_epoch"), CircuitState::Open);
    assert_eq!(cb.failure_count("get_epoch"), 3);

    // Calls should be rejected
    let result = cb.check("get_epoch");
    assert!(result.is_err());
    assert_eq!(cb.total_rejections("get_epoch"), 1);

    // Other methods unaffected
    assert_eq!(cb.state("get_block"), CircuitState::Closed);
    assert!(cb.check("get_block").is_ok());
}

#[test]
fn test_circuit_breaker_half_open_after_cooldown() {
    let config = CircuitBreakerConfig {
        failure_threshold: 2,
        cooldown_duration: std::time::Duration::from_millis(1), // Very short for test
        success_threshold: 1,
    };
    let cb = RpcCircuitBreaker::with_config(config);

    // Open the circuit
    cb.record_failure("rpc_call");
    cb.record_failure("rpc_call");
    assert_eq!(cb.state("rpc_call"), CircuitState::Open);

    // Wait for cooldown to expire
    std::thread::sleep(std::time::Duration::from_millis(10));

    // check() should transition to HalfOpen and allow the call
    assert!(cb.check("rpc_call").is_ok());
    assert_eq!(cb.state("rpc_call"), CircuitState::HalfOpen);
}

#[test]
fn test_circuit_breaker_closes_on_success() {
    let config = CircuitBreakerConfig {
        failure_threshold: 2,
        cooldown_duration: std::time::Duration::from_millis(1),
        success_threshold: 1,
    };
    let cb = RpcCircuitBreaker::with_config(config);

    // Open → HalfOpen
    cb.record_failure("rpc_call");
    cb.record_failure("rpc_call");
    std::thread::sleep(std::time::Duration::from_millis(10));
    cb.check("rpc_call").unwrap(); // Transition to HalfOpen

    // Probe succeeds → Closed
    cb.record_success("rpc_call");
    assert_eq!(cb.state("rpc_call"), CircuitState::Closed);
    assert_eq!(cb.failure_count("rpc_call"), 0);

    // Calls should work normally again
    assert!(cb.check("rpc_call").is_ok());
}

#[test]
fn test_circuit_breaker_reopens_on_probe_failure() {
    let config = CircuitBreakerConfig {
        failure_threshold: 2,
        cooldown_duration: std::time::Duration::from_millis(1),
        success_threshold: 1,
    };
    let cb = RpcCircuitBreaker::with_config(config);

    // Open → HalfOpen
    cb.record_failure("rpc_call");
    cb.record_failure("rpc_call");
    std::thread::sleep(std::time::Duration::from_millis(10));
    cb.check("rpc_call").unwrap(); // Transition to HalfOpen
    assert_eq!(cb.state("rpc_call"), CircuitState::HalfOpen);

    // Probe fails → back to Open
    cb.record_failure("rpc_call");
    assert_eq!(cb.state("rpc_call"), CircuitState::Open);

    // Should be rejected again (until next cooldown)
    let result = cb.check("rpc_call");
    assert!(result.is_err());
}

// ============================================================================
// Group 6: SyncController
// ============================================================================

use crate::node::sync_controller::SyncController;

#[tokio::test]
async fn test_sync_controller_enable_disable_cycle() {
    let controller = SyncController::new();
    assert!(controller.is_disabled());

    // Can't enable without a real handle, but we can test disable from disabled state
    let result = controller.disable_sync().await.unwrap();
    assert!(
        !result,
        "Disabling already-disabled sync should return false"
    );
    assert!(controller.is_disabled());
}

#[tokio::test]
async fn test_sync_controller_double_disable() {
    let controller = SyncController::new();

    // Both disable calls should be safe
    let r1 = controller.disable_sync().await.unwrap();
    let r2 = controller.disable_sync().await.unwrap();
    assert!(!r1);
    assert!(!r2);
    assert!(controller.is_disabled());
}

// ============================================================================
// Group 7: SyncMetrics
// ============================================================================

use crate::node::sync_metrics::SyncMetrics;

#[test]
fn test_sync_metrics_creation() {
    // Verify all metrics register without panicking
    let metrics = SyncMetrics::new_for_test();

    // Verify counters start at 0
    assert_eq!(metrics.blocks_received_total.get(), 0.0);
    assert_eq!(metrics.blocks_sent_to_go_total.get(), 0.0);
    assert_eq!(metrics.sync_errors_total.get(), 0.0);

    // Verify gauges start at 0
    assert_eq!(metrics.queue_depth.get(), 0.0);
    assert_eq!(metrics.current_block.get(), 0.0);
    assert_eq!(metrics.current_epoch.get(), 0.0);
    assert_eq!(metrics.peers_in_backoff.get(), 0.0);

    // Verify they can be used
    metrics.blocks_received_total.inc();
    assert_eq!(metrics.blocks_received_total.get(), 1.0);

    metrics.current_epoch.set(5.0);
    assert_eq!(metrics.current_epoch.get(), 5.0);

    // Verify new profiling metrics register and work
    assert_eq!(metrics.blocks_per_second.get(), 0.0);
    metrics.blocks_per_second.set(42.0);
    assert_eq!(metrics.blocks_per_second.get(), 42.0);

    // Histograms should be observable
    metrics.go_state_query_seconds.observe(0.005);
    metrics.deserialize_duration_seconds.observe(0.001);
    metrics.process_queue_total_seconds.observe(0.05);
    metrics.queue_drain_duration_seconds.observe(0.0001);
    metrics.leader_resolve_duration_seconds.observe(0.002);
    metrics.go_send_per_commit_seconds.observe(0.003);
}

// ============================================================================
// Group 8: StateTransitionManager Edge Cases
// ============================================================================

#[tokio::test]
async fn test_debounce_prevents_rapid_transitions() {
    let manager = Arc::new(StateTransitionManager::new(1, true));

    // First transition succeeds
    manager.try_start_epoch_transition(2, "test").await.unwrap();
    manager.complete_epoch_transition(2).await;
    assert_eq!(manager.current_epoch(), 2);

    // Immediate second transition should be debounced (within 3s window)
    let result = manager.try_start_epoch_transition(3, "test").await;
    assert!(
        matches!(result, Err(TransitionError::Debouncing { .. })),
        "Expected Debouncing error, got {:?}",
        result
    );

    // Epoch should still be 2
    assert_eq!(manager.current_epoch(), 2);
}

#[test]
fn test_set_current_state_force_update() {
    let manager = StateTransitionManager::new(1, true);
    assert_eq!(manager.current_epoch(), 1);
    assert!(manager.is_validator());

    // Force update to epoch 10, SyncOnly
    manager.set_current_state(10, false);
    assert_eq!(manager.current_epoch(), 10);
    assert!(!manager.is_validator());

    // Force update back to Validator
    manager.set_current_state(10, true);
    assert!(manager.is_validator());

    // Force skip to epoch 100
    manager.set_current_state(100, true);
    assert_eq!(manager.current_epoch(), 100);
}

#[tokio::test]
async fn test_concurrent_epoch_and_mode_transitions() {
    let manager = Arc::new(StateTransitionManager::new(1, false));

    // Start epoch transition
    manager
        .try_start_epoch_transition(2, "epoch")
        .await
        .unwrap();
    assert!(manager.is_transition_in_progress());

    // Mode transition should be blocked while epoch in progress
    let result = manager.try_start_mode_transition(true, "mode").await;
    assert!(
        matches!(result, Err(TransitionError::TransitionInProgress { .. })),
        "Expected TransitionInProgress, got {:?}",
        result
    );

    // Complete epoch, then mode should work (after debounce)
    manager.complete_epoch_transition(2).await;
    tokio::time::sleep(std::time::Duration::from_secs(4)).await;
    manager
        .try_start_mode_transition(true, "mode")
        .await
        .unwrap();
    manager.complete_mode_transition(true).await;
    assert!(manager.is_validator());
}

#[tokio::test]
async fn test_epoch_skip_allowed() {
    let manager = Arc::new(StateTransitionManager::new(1, true));

    // Skip from epoch 1 → 5 (should be accepted)
    manager.try_start_epoch_transition(5, "skip").await.unwrap();
    manager.complete_epoch_transition(5).await;
    assert_eq!(manager.current_epoch(), 5);
}

// ============================================================================
// Group 9: CommitteeSource Validation
// ============================================================================

use crate::node::committee_source::CommitteeSource;

#[test]
fn test_committee_source_validate_epoch_match() {
    let source = CommitteeSource {
        socket_path: "/dev/null".to_string(),
        epoch: 5,
        last_block: 1000,
        is_peer: false,
        peer_rpc_addresses: vec![],
    };

    assert!(source.validate_epoch(5));
}

#[test]
fn test_committee_source_validate_epoch_mismatch() {
    let source = CommitteeSource {
        socket_path: "/dev/null".to_string(),
        epoch: 5,
        last_block: 1000,
        is_peer: false,
        peer_rpc_addresses: vec![],
    };

    assert!(!source.validate_epoch(3));
    assert!(!source.validate_epoch(6));
}

#[test]
fn test_committee_source_create_executor_client() {
    let source = CommitteeSource {
        socket_path: "/tmp/test_recv.sock".to_string(),
        epoch: 1,
        last_block: 0,
        is_peer: true,
        peer_rpc_addresses: vec!["10.0.0.1:8080".to_string()],
    };

    let client = source.create_executor_client("/tmp/test_send.sock");
    // Client is created successfully (Arc<ExecutorClient>)
    // ExecutorClient::new passes enabled=true by default
    assert!(client.is_enabled());
}

// ============================================================================
// Group 10: CheckpointManager Resume Instructions
// ============================================================================

#[test]
fn test_resume_instructions_all_states() {
    let cases = vec![
        (TransitionState::NotStarted, "No action needed"),
        (
            TransitionState::AdvancedEpochToGo {
                epoch: 1,
                boundary_block: 0,
                boundary_gei: 0,
                timestamp_ms: 0,
            },
            "fetch_committee",
        ),
        (
            TransitionState::CommitteeFetched {
                epoch: 1,
                committee_hash: [0u8; 32],
                validator_count: 4,
            },
            "stop_authority",
        ),
        (
            TransitionState::AuthorityStopped {
                epoch: 2,
                previous_epoch: 1,
            },
            "start_new_epoch",
        ),
        (
            TransitionState::Completed {
                epoch: 2,
                completed_at_ms: 1700000000000,
            },
            "No action needed",
        ),
    ];

    for (state, expected_substr) in cases {
        let instructions = CheckpointManager::get_resume_instructions(&state);
        assert!(
            instructions.contains(expected_substr),
            "State '{}': expected instruction containing '{}', got: '{}'",
            state.name(),
            expected_substr,
            instructions
        );
    }
}

// ============================================================================
// Group 11: TransitionType Display
// ============================================================================

use crate::node::epoch_transition_manager::TransitionType;

#[test]
fn test_transition_type_display_epoch() {
    let t = TransitionType::EpochChange {
        from_epoch: 1,
        to_epoch: 2,
    };
    assert_eq!(format!("{}", t), "Epoch(1 → 2)");
}

#[test]
fn test_transition_type_display_mode() {
    let t = TransitionType::ModeChange {
        from_mode: "SyncOnly".to_string(),
        to_mode: "Validator".to_string(),
    };
    assert_eq!(format!("{}", t), "Mode(SyncOnly → Validator)");
}

#[test]
fn test_transition_type_display_epoch_and_mode() {
    let t = TransitionType::EpochAndModeChange {
        from_epoch: 3,
        to_epoch: 4,
        from_mode: "Validator".to_string(),
        to_mode: "SyncOnly".to_string(),
    };
    assert_eq!(
        format!("{}", t),
        "Epoch(3 → 4) + Mode(Validator → SyncOnly)"
    );
}

// ============================================================================
// Group 12: TransitionError Display
// ============================================================================

#[test]
fn test_transition_error_display_all_variants() {
    let err1 = TransitionError::EpochAlreadyCurrent {
        current: 5,
        requested: 3,
    };
    assert!(format!("{}", err1).contains("5"));
    assert!(format!("{}", err1).contains("3"));

    let err2 = TransitionError::ModeAlreadyCurrent;
    assert!(format!("{}", err2).contains("Mode already current"));

    let err3 = TransitionError::TransitionInProgress {
        current: Some("Epoch(1 → 2) by monitor".to_string()),
        requested: "Mode(SyncOnly → Validator) by api".to_string(),
    };
    let display = format!("{}", err3);
    assert!(display.contains("in progress"));

    let err4 = TransitionError::Debouncing { remaining_ms: 2500 };
    assert!(format!("{}", err4).contains("2500"));
}

// ============================================================================
// Group 13: TransitionState Helpers
// ============================================================================

#[test]
fn test_transition_state_epoch_and_name() {
    // NotStarted: no epoch, name = "NotStarted"
    let s = TransitionState::NotStarted;
    assert_eq!(s.epoch(), None);
    assert_eq!(s.name(), "NotStarted");

    // AdvancedEpochToGo
    let s = TransitionState::AdvancedEpochToGo {
        epoch: 5,
        boundary_block: 1000,
        boundary_gei: 0,
        timestamp_ms: 1700000000000,
    };
    assert_eq!(s.epoch(), Some(5));
    assert_eq!(s.name(), "AdvancedEpochToGo");

    // CommitteeFetched
    let s = TransitionState::CommitteeFetched {
        epoch: 5,
        committee_hash: [42u8; 32],
        validator_count: 4,
    };
    assert_eq!(s.epoch(), Some(5));
    assert_eq!(s.name(), "CommitteeFetched");

    // AuthorityStopped
    let s = TransitionState::AuthorityStopped {
        epoch: 5,
        previous_epoch: 4,
    };
    assert_eq!(s.epoch(), Some(5));
    assert_eq!(s.name(), "AuthorityStopped");

    // Completed
    let s = TransitionState::Completed {
        epoch: 5,
        completed_at_ms: 1700000060000,
    };
    assert_eq!(s.epoch(), Some(5));
    assert_eq!(s.name(), "Completed");
}
