// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

//! Post-transition verification, readiness checks, and timestamp synchronization.

use crate::node::executor_client::ExecutorClient;
use crate::node::tx_submitter::TransactionSubmitter;
use crate::node::ConsensusNode;
use anyhow::Result;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{info, trace, warn};

/// Post-transition verification: ensure Go and Rust epochs match, sync timestamps.
pub(super) async fn verify_epoch_consistency(
    node: &mut ConsensusNode,
    new_epoch: u64,
    epoch_timestamp: u64,
    executor_client: &ExecutorClient,
) -> Result<()> {
    // FORK-SAFETY: Verify Go and Rust epochs match
    match executor_client.get_current_epoch().await {
        Ok(go_epoch) => {
            if go_epoch != new_epoch {
                warn!(
                    "⚠️ [EPOCH VERIFY] Go-Rust epoch mismatch! Rust: {}, Go: {}. \
                     This could indicate a fork risk. Consider investigating.",
                    new_epoch, go_epoch
                );
            } else {
                info!("✅ [EPOCH VERIFY] Go-Rust epoch consistent: {}", new_epoch);
            }
        }
        Err(e) => {
            warn!("⚠️ [EPOCH VERIFY] Failed to verify epoch with Go: {}", e);
        }
    }

    // FORK-SAFETY: Sync timestamp from Go to ensure consistency
    let go_epoch_timestamp_ms = match sync_epoch_timestamp_from_go(
        executor_client,
        new_epoch,
        epoch_timestamp,
    )
    .await
    {
        Ok(timestamp) => {
            if timestamp != epoch_timestamp {
                warn!(
                    "⚠️ [EPOCH TIMESTAMP SYNC] Timestamp mismatch: Local={}ms, Go={}ms, diff={}ms. Using Go's.",
                    epoch_timestamp, timestamp,
                    (timestamp as i64 - epoch_timestamp as i64).abs()
                );
            } else {
                info!(
                    "✅ [EPOCH TIMESTAMP SYNC] Timestamp consistent: {}ms",
                    epoch_timestamp
                );
            }
            timestamp
        }
        Err(e) => {
            if e.to_string().contains("not found") || e.to_string().contains("Unexpected response")
            {
                info!(
                    "ℹ️ [EPOCH TIMESTAMP SYNC] Go endpoint not implemented yet, using local: {}ms",
                    epoch_timestamp
                );
            } else {
                warn!(
                    "⚠️ [EPOCH TIMESTAMP SYNC] Failed to sync from Go: {}. Using local.",
                    e
                );
            }
            epoch_timestamp
        }
    };

    // Update SystemTransactionProvider with verified timestamp
    node.system_transaction_provider
        .update_epoch(new_epoch, go_epoch_timestamp_ms)
        .await;

    // Cross-epoch transition summary
    info!(
        "✅ [EPOCH TRANSITION COMPLETE] epoch={}, mode={:?}, last_global_exec_index={}, go_sync_complete={}",
        node.current_epoch,
        node.node_mode,
        node.last_global_exec_index,
        executor_client.get_last_block_number().await.map(|(b, _)| b).unwrap_or(0) >= node.last_global_exec_index
    );

    Ok(())
}

pub(super) async fn wait_for_commit_processor_completion(
    node: &ConsensusNode,
    target: u32,
    max_wait: u64,
) -> Result<()> {
    use std::sync::atomic::Ordering;

    let start = std::time::Instant::now();
    loop {
        let current = node.current_commit_index.load(Ordering::SeqCst);
        if current >= target {
            return Ok(());
        }
        if start.elapsed().as_secs() >= max_wait {
            return Err(anyhow::anyhow!("Timeout"));
        }
        // Polling sleep: Wait 100ms before checking commit index again
        // This is acceptable for infrequent epoch transitions where precise timing isn't critical
        sleep(Duration::from_millis(100)).await;
    }
}

/// Wait for consensus to become ready with retries instead of fixed sleep
/// This replaces the unreliable 1000ms sleep with proper synchronization
pub(super) async fn wait_for_consensus_ready(node: &ConsensusNode) -> bool {
    let max_attempts = 20; // Up to 2 seconds with 100ms intervals
    let retry_delay = Duration::from_millis(100);

    for attempt in 1..=max_attempts {
        if test_consensus_readiness(node).await {
            return true;
        }

        if attempt < max_attempts {
            trace!(
                "⏳ Consensus not ready yet (attempt {}/{}), waiting...",
                attempt,
                max_attempts
            );
            sleep(retry_delay).await;
        }
    }

    warn!(
        "⚠️ Consensus failed to become ready after {} attempts",
        max_attempts
    );
    false
}

async fn test_consensus_readiness(node: &ConsensusNode) -> bool {
    if let Some(proxy) = &node.transaction_client_proxy {
        match proxy.submit(vec![vec![0u8; 64]]).await {
            Ok(_) => true,
            Err(_) => false,
        }
    } else {
        false
    }
}

/// Sync epoch timestamp from Go with retry logic to avoid stale timestamps
/// CRITICAL: Prevents using timestamp from old epoch after transition
pub(super) async fn sync_epoch_timestamp_from_go(
    executor_client: &ExecutorClient,
    expected_epoch: u64,
    expected_timestamp: u64,
) -> Result<u64> {
    const MAX_RETRIES: u32 = 5;
    const RETRY_DELAY_MS: u64 = 200;

    for attempt in 1..=MAX_RETRIES {
        // First check if Go has transitioned to expected epoch
        match executor_client.get_current_epoch().await {
            Ok(go_current_epoch) => {
                if go_current_epoch != expected_epoch {
                    if attempt == MAX_RETRIES {
                        return Err(anyhow::anyhow!(
                            "Go still in epoch {} after {} attempts, expected epoch {}",
                            go_current_epoch,
                            MAX_RETRIES,
                            expected_epoch
                        ));
                    }
                    warn!(
                        "⚠️ [EPOCH SYNC] Go still in epoch {} (attempt {}/{}), expected {}. Retrying...",
                        go_current_epoch, attempt, MAX_RETRIES, expected_epoch
                    );
                    sleep(Duration::from_millis(RETRY_DELAY_MS)).await;
                    continue;
                }
            }
            Err(e) => {
                warn!(
                    "⚠️ [EPOCH SYNC] Failed to get current epoch from Go (attempt {}/{}): {}",
                    attempt, MAX_RETRIES, e
                );
                if attempt == MAX_RETRIES {
                    return Err(anyhow::anyhow!(
                        "Failed to verify Go epoch after transition: {}",
                        e
                    ));
                }
                sleep(Duration::from_millis(RETRY_DELAY_MS)).await;
                continue;
            }
        }

        // Now get timestamp and validate it's reasonable
        match executor_client
            .get_epoch_start_timestamp(expected_epoch)
            .await
        {
            Ok(go_timestamp) => {
                // Validate timestamp is not from old epoch (should be close to expected)
                // Timestamp should be within reasonable range of expected timestamp
                let timestamp_diff = (go_timestamp as i64 - expected_timestamp as i64).abs() as u64;

                if timestamp_diff > 10000 {
                    // 10 seconds tolerance
                    warn!(
                        "⚠️ [EPOCH SYNC] Go timestamp {}ms differs from expected {}ms by {}ms (attempt {}/{}). \
                         This may indicate stale timestamp from old epoch.",
                        go_timestamp, expected_timestamp, timestamp_diff, attempt, MAX_RETRIES
                    );

                    if attempt == MAX_RETRIES {
                        // At final attempt, accept the timestamp but log warning
                        warn!(
                            "⚠️ [EPOCH SYNC] Using Go timestamp despite large difference. \
                               This may cause epoch timing issues."
                        );
                        return Ok(go_timestamp);
                    }

                    sleep(Duration::from_millis(RETRY_DELAY_MS)).await;
                    continue;
                }

                info!(
                    "✅ [EPOCH SYNC] Successfully synced timestamp from Go: {}ms (diff: {}ms)",
                    go_timestamp, timestamp_diff
                );
                return Ok(go_timestamp);
            }
            Err(e) => {
                warn!(
                    "⚠️ [EPOCH SYNC] Failed to get timestamp from Go (attempt {}/{}): {}",
                    attempt, MAX_RETRIES, e
                );
                if attempt == MAX_RETRIES {
                    return Err(anyhow::anyhow!(
                        "Failed to get timestamp from Go after transition: {}",
                        e
                    ));
                }
                sleep(Duration::from_millis(RETRY_DELAY_MS)).await;
                continue;
            }
        }
    }

    Err(anyhow::anyhow!(
        "Failed to sync epoch timestamp from Go after {} attempts",
        MAX_RETRIES
    ))
}
