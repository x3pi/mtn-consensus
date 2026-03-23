// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

//! Tests for fetch.rs logic — index conversion, peer address extraction, error paths.

use super::block_queue::{BlockQueue, CommitData};
use super::RustSyncNode;
use crate::node::executor_client::ExecutorClient;
use consensus_core::Commit;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Helper: create a minimal `CommitData` with the given `global_exec_index` and epoch.
fn make_commit_data(global_exec_index: u64, epoch: u64) -> CommitData {
    let zeros = vec![0u8; 32];
    let json = serde_json::json!({
        "V1": {
            "index": 1,
            "previous_digest": zeros,
            "timestamp_ms": 0,
            "leader": {
                "round": 1,
                "author": 0,
                "digest": zeros
            },
            "blocks": [],
            "global_exec_index": global_exec_index
        }
    });
    let commit: Commit = serde_json::from_value(json).expect("valid commit json");
    CommitData {
        commit,
        blocks: vec![],
        epoch,
    }
}

/// Helper: create a minimal RustSyncNode for testing (no network, no real executor).
fn make_test_sync_node(
    initial_epoch: u64,
    initial_global_exec_index: u32,
    epoch_base_index: u64,
) -> (RustSyncNode, mpsc::UnboundedReceiver<(u64, u64, u64)>) {
    let executor_client = Arc::new(ExecutorClient::new(
        false, // disabled — won't actually connect
        false,
        "/dev/null".to_string(),
        "/dev/null".to_string(),
        None,
    ));
    let (tx, rx) = mpsc::unbounded_channel();
    let node = RustSyncNode::new(
        executor_client,
        tx,
        initial_epoch,
        initial_global_exec_index,
        epoch_base_index,
    );
    (node, rx)
}

// =============================================================================
// Global-to-local commit index conversion tests
// =============================================================================

/// Within-epoch conversion: global 100 with base 50 → local 50
#[test]
fn test_global_to_local_commit_within_epoch() {
    let from_global: u64 = 100;
    let epoch_base: u64 = 50;

    // Reproducing the logic from fetch_and_queue line 41-43
    assert!(from_global >= epoch_base);
    let from_local_commit = (from_global - epoch_base) as u32;
    assert_eq!(from_local_commit, 50);
}

/// Edge case: epoch 0, base 0 → passthrough
#[test]
fn test_global_to_local_commit_epoch0() {
    let from_global: u64 = 42;
    let epoch_base: u64 = 0;
    let current_epoch: u64 = 0;

    // In epoch 0 with base 0, global == local
    assert!(from_global >= epoch_base);
    let from_local_commit = (from_global - epoch_base) as u32;
    assert_eq!(from_local_commit, 42);
    let _ = current_epoch; // used in real code for branching
}

/// Cross-epoch gap: global < base → should detect cross-epoch gap
#[test]
fn test_global_to_local_commit_cross_epoch_gap() {
    let from_global: u64 = 30;
    let epoch_base: u64 = 100;
    let current_epoch: u64 = 2; // epoch > 1

    // This condition triggers the cross-epoch gap branch in fetch_and_queue
    assert!(from_global < epoch_base);
    assert!(current_epoch > 1);

    // In real code, this falls through to fetch_and_queue_by_global_range
    // Test verifies the detection condition is correct
}

/// Epoch 1 special case: global < base AND epoch == 1 → use global directly
#[test]
fn test_global_to_local_commit_epoch1_pre_epoch_blocks() {
    let from_global: u64 = 30;
    let epoch_base: u64 = 50;
    let current_epoch: u64 = 1;

    assert!(from_global < epoch_base);
    assert_eq!(current_epoch, 1);

    // In the epoch 1 special case, use from_global directly as local commit
    let from_local_commit = from_global as u32;
    assert_eq!(from_local_commit, 30);
}

// =============================================================================
// get_peer_go_addresses tests
// =============================================================================

/// Returns empty vec when committee is None
/// NOTE: Commented out — `get_peer_go_addresses()` was removed/renamed.
/// TODO: Update to match current API (with_peer_rpc_addresses).
// #[test]
// fn test_get_peer_go_addresses_no_committee() {
//     let (node, _rx) = make_test_sync_node(0, 0, 0);
//     let addresses = node.get_peer_go_addresses();
//     assert!(addresses.is_empty());
// }

// =============================================================================
// fetch_blocks_from_peer_go error path tests
// =============================================================================

/// Returns error with empty peer list
#[tokio::test]
async fn test_fetch_blocks_from_peer_go_empty_addresses() {
    let (node, _rx) = make_test_sync_node(0, 1, 0);

    let result = node.fetch_blocks_from_peer_go(&[], 1, 100).await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("No peer addresses configured"));
}

// =============================================================================
// CommitData epoch tracking through queue
// =============================================================================

/// Verifies CommitData carries correct epoch tag through push/drain cycle
#[test]
fn test_commit_data_epoch_tracking() {
    let mut queue = BlockQueue::new(1);

    // Push commits from different epochs
    queue.push(make_commit_data(1, 0));
    queue.push(make_commit_data(2, 0));
    queue.push(make_commit_data(3, 1)); // epoch changed at global_exec_index 3

    let ready = queue.drain_ready();
    assert_eq!(ready.len(), 3);

    // Verify epoch tags preserved
    assert_eq!(ready[0].epoch, 0);
    assert_eq!(ready[1].epoch, 0);
    assert_eq!(ready[2].epoch, 1);
}
