// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

//! Transition handoff APIs for ExecutorClient.
//!
//! These APIs ensure no gaps or overlaps between sync and consensus modes:
//! - `advance_epoch` — advance epoch in Go state
//! - `set_consensus_start_block` — SyncOnly → Validator transition
//! - `set_sync_start_block` — Validator → SyncOnly transition
//! - `wait_for_sync_to_block` — wait for Go sync to reach target

use anyhow::Result;
use prost::Message;
use tokio::io::AsyncWriteExt;
use tracing::info;

use super::proto::{self, AdvanceEpochRequest, Request, Response};
use super::ExecutorClient;

impl ExecutorClient {
    /// Advance epoch in Go state (Sui-style epoch transition)
    /// boundary_block is the global_exec_index of the last block of the ending epoch
    pub async fn advance_epoch(
        &self,
        new_epoch: u64,
        epoch_start_timestamp_ms: u64,
        boundary_block: u64,
        boundary_gei: u64,
    ) -> Result<()> {
        if !self.is_enabled() {
            return Err(anyhow::anyhow!("Executor client is not enabled"));
        }

        let max_retries = 3;
        let mut retry_count = 0;
        let mut last_err = anyhow::anyhow!("Unknown error");

        while retry_count < max_retries {
            match self
                .try_advance_epoch(new_epoch, epoch_start_timestamp_ms, boundary_block, boundary_gei)
                .await
            {
                Ok(()) => return Ok(()),
                Err(e) => {
                    tracing::warn!(
                        "⚠️ [EXECUTOR-REQ] Failed to advance epoch to {} (attempt {}/{}): {}",
                        new_epoch,
                        retry_count + 1,
                        max_retries,
                        e
                    );
                    last_err = e;
                    retry_count += 1;
                    if retry_count < max_retries {
                        tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
                    }
                }
            }
        }

        Err(anyhow::anyhow!(
            "Failed to advance epoch after {} retries. Last error: {}",
            max_retries,
            last_err
        ))
    }

    async fn try_advance_epoch(
        &self,
        new_epoch: u64,
        epoch_start_timestamp_ms: u64,
        boundary_block: u64,
        boundary_gei: u64,
    ) -> Result<()> {
        // Connect to Go request socket if needed
        if let Err(e) = self.connect_request().await {
            return Err(anyhow::anyhow!(
                "Failed to connect to Go request socket: {}",
                e
            ));
        }

        // Create AdvanceEpochRequest with boundary_block for deterministic epoch transition
        let request = Request {
            payload: Some(proto::request::Payload::AdvanceEpochRequest(
                AdvanceEpochRequest {
                    new_epoch,
                    epoch_start_timestamp_ms,
                    boundary_block,
                    boundary_gei,
                },
            )),
        };

        // Encode request to protobuf bytes
        let mut request_buf = Vec::new();
        request.encode(&mut request_buf)?;

        // Send request via UDS
        let mut conn_guard = self.request_connection.lock().await;
        if let Some(ref mut stream) = *conn_guard {
            // Write 4-byte length prefix (big-endian)
            let len = request_buf.len() as u32;
            let len_bytes = len.to_be_bytes();
            stream.write_all(&len_bytes).await?;

            // Write request data
            stream.write_all(&request_buf).await?;
            stream.flush().await?;

            info!("📤 [EXECUTOR-REQ] Sent AdvanceEpochRequest to Go (new_epoch={}, timestamp_ms={}, size: {} bytes)",
                new_epoch, epoch_start_timestamp_ms, request_buf.len());

            // Read response (4-byte length prefix + response data)
            use tokio::io::AsyncReadExt;
            use tokio::time::{timeout, Duration};

            // Set timeout for reading response (5 seconds)
            let read_timeout = Duration::from_secs(5);

            let mut len_buf = [0u8; 4];
            timeout(read_timeout, stream.read_exact(&mut len_buf))
                .await
                .map_err(|e| anyhow::anyhow!("Timeout reading response length: {}", e))??;
            let response_len = u32::from_be_bytes(len_buf) as usize;

            if response_len == 0 {
                return Err(anyhow::anyhow!("Received zero-length response from Go"));
            }
            if response_len > 10_000_000 {
                // 10MB limit
                return Err(anyhow::anyhow!(
                    "Response too large: {} bytes",
                    response_len
                ));
            }

            let mut response_buf = vec![0u8; response_len];
            timeout(read_timeout, stream.read_exact(&mut response_buf))
                .await
                .map_err(|e| anyhow::anyhow!("Timeout reading response data: {}", e))??;

            info!(
                "📥 [EXECUTOR-REQ] Received {} bytes from Go, decoding...",
                response_buf.len()
            );

            // Decode response
            let response = Response::decode(&response_buf[..])
                .map_err(|e| {
                    anyhow::anyhow!(
                        "Failed to decode response from Go: {}. Response length: {} bytes. Response bytes (hex): {}. Response bytes (first 100): {:?}",
                        e,
                        response_buf.len(),
                        hex::encode(&response_buf),
                        &response_buf[..response_buf.len().min(100)]
                    )
                })?;

            info!("🔍 [EXECUTOR-REQ] Decoded response successfully");
            info!(
                "🔍 [EXECUTOR-REQ] Response payload type: {:?}",
                response.payload
            );

            match response.payload {
                Some(proto::response::Payload::NotifyEpochChangeResponse(_)) => {
                    return Err(anyhow::anyhow!("Unexpected NotifyEpochChangeResponse"));
                }
                Some(proto::response::Payload::AdvanceEpochResponse(_advance_epoch_response)) => {
                    info!(
                        "✅ [EXECUTOR-REQ] Go successfully advanced to epoch {}",
                        new_epoch
                    );
                    return Ok(());
                }
                Some(proto::response::Payload::Error(error_msg)) => {
                    return Err(anyhow::anyhow!(
                        "Go returned error during epoch advance: {}",
                        error_msg
                    ));
                }
                _ => {
                    return Err(anyhow::anyhow!(
                        "Unexpected response payload type for AdvanceEpoch"
                    ));
                }
            }
        } else {
            return Err(anyhow::anyhow!("Request connection is not available"));
        }
    }

    /// Set consensus start block - called before transitioning to Validator mode
    /// Tells Go that consensus will produce blocks starting from `block_number`
    /// This is used during SyncOnly -> Validator transition
    pub async fn set_consensus_start_block(
        &self,
        block_number: u64,
    ) -> Result<(bool, u64, String)> {
        if !self.is_enabled() {
            return Err(anyhow::anyhow!("Executor client is not enabled"));
        }

        if let Err(e) = self.connect_request().await {
            return Err(anyhow::anyhow!(
                "Failed to connect to Go request socket: {}",
                e
            ));
        }

        let request = Request {
            payload: Some(proto::request::Payload::SetConsensusStartBlockRequest(
                proto::SetConsensusStartBlockRequest { block_number },
            )),
        };

        let mut request_buf = Vec::new();
        request.encode(&mut request_buf)?;

        let mut conn_guard = self.request_connection.lock().await;
        if let Some(ref mut stream) = *conn_guard {
            let len = request_buf.len() as u32;
            stream.write_all(&len.to_be_bytes()).await?;
            stream.write_all(&request_buf).await?;
            stream.flush().await?;

            info!(
                "📤 [TRANSITION] Sent SetConsensusStartBlockRequest: block_number={}",
                block_number
            );

            use tokio::io::AsyncReadExt;
            use tokio::time::{timeout, Duration};
            let read_timeout = Duration::from_secs(35); // Longer timeout for potential waiting

            let mut len_buf = [0u8; 4];
            timeout(read_timeout, stream.read_exact(&mut len_buf))
                .await
                .map_err(|e| anyhow::anyhow!("Timeout reading response length: {}", e))??;
            let response_len = u32::from_be_bytes(len_buf) as usize;

            if response_len == 0 || response_len > 10_000_000 {
                return Err(anyhow::anyhow!("Invalid response length: {}", response_len));
            }

            let mut response_buf = vec![0u8; response_len];
            timeout(read_timeout, stream.read_exact(&mut response_buf))
                .await
                .map_err(|e| anyhow::anyhow!("Timeout reading response data: {}", e))??;

            let response = Response::decode(&response_buf[..])?;
            match response.payload {
                Some(proto::response::Payload::NotifyEpochChangeResponse(_)) => {
                    return Err(anyhow::anyhow!("Unexpected NotifyEpochChangeResponse"));
                }
                Some(proto::response::Payload::SetConsensusStartBlockResponse(res)) => {
                    info!(
                        "✅ [TRANSITION] SetConsensusStartBlock response: success={}, last_sync_block={}, message={}",
                        res.success, res.last_sync_block, res.message
                    );
                    Ok((res.success, res.last_sync_block, res.message))
                }
                Some(proto::response::Payload::Error(error_msg)) => {
                    Err(anyhow::anyhow!("Go returned error: {}", error_msg))
                }
                _ => Err(anyhow::anyhow!("Unexpected response type from Go")),
            }
        } else {
            Err(anyhow::anyhow!("Request connection is not available"))
        }
    }

    /// Set sync start block - called when transitioning from Validator to SyncOnly mode
    /// Tells Go that consensus ended at `last_consensus_block`, sync should start from `last_consensus_block + 1`
    pub async fn set_sync_start_block(
        &self,
        last_consensus_block: u64,
    ) -> Result<(bool, u64, String)> {
        if !self.is_enabled() {
            return Err(anyhow::anyhow!("Executor client is not enabled"));
        }

        if let Err(e) = self.connect_request().await {
            return Err(anyhow::anyhow!(
                "Failed to connect to Go request socket: {}",
                e
            ));
        }

        let request = Request {
            payload: Some(proto::request::Payload::SetSyncStartBlockRequest(
                proto::SetSyncStartBlockRequest {
                    last_consensus_block,
                },
            )),
        };

        let mut request_buf = Vec::new();
        request.encode(&mut request_buf)?;

        let mut conn_guard = self.request_connection.lock().await;
        if let Some(ref mut stream) = *conn_guard {
            let len = request_buf.len() as u32;
            stream.write_all(&len.to_be_bytes()).await?;
            stream.write_all(&request_buf).await?;
            stream.flush().await?;

            info!(
                "📤 [TRANSITION] Sent SetSyncStartBlockRequest: last_consensus_block={}",
                last_consensus_block
            );

            use tokio::io::AsyncReadExt;
            use tokio::time::{timeout, Duration};
            let read_timeout = Duration::from_secs(5);

            let mut len_buf = [0u8; 4];
            timeout(read_timeout, stream.read_exact(&mut len_buf))
                .await
                .map_err(|e| anyhow::anyhow!("Timeout reading response length: {}", e))??;
            let response_len = u32::from_be_bytes(len_buf) as usize;

            if response_len == 0 || response_len > 10_000_000 {
                return Err(anyhow::anyhow!("Invalid response length: {}", response_len));
            }

            let mut response_buf = vec![0u8; response_len];
            timeout(read_timeout, stream.read_exact(&mut response_buf))
                .await
                .map_err(|e| anyhow::anyhow!("Timeout reading response data: {}", e))??;

            let response = Response::decode(&response_buf[..])?;
            match response.payload {
                Some(proto::response::Payload::NotifyEpochChangeResponse(_)) => {
                    return Err(anyhow::anyhow!("Unexpected NotifyEpochChangeResponse"));
                }
                Some(proto::response::Payload::SetSyncStartBlockResponse(res)) => {
                    info!(
                        "✅ [TRANSITION] SetSyncStartBlock response: success={}, sync_start_block={}, message={}",
                        res.success, res.sync_start_block, res.message
                    );
                    Ok((res.success, res.sync_start_block, res.message))
                }
                Some(proto::response::Payload::Error(error_msg)) => {
                    Err(anyhow::anyhow!("Go returned error: {}", error_msg))
                }
                _ => Err(anyhow::anyhow!("Unexpected response type from Go")),
            }
        } else {
            Err(anyhow::anyhow!("Request connection is not available"))
        }
    }

    /// Wait for Go sync to reach a specific block
    /// Used during SyncOnly -> Validator transition to ensure sync is complete before consensus starts
    pub async fn wait_for_sync_to_block(
        &self,
        target_block: u64,
        timeout_seconds: u64,
    ) -> Result<(bool, u64, String)> {
        if !self.is_enabled() {
            return Err(anyhow::anyhow!("Executor client is not enabled"));
        }

        if let Err(e) = self.connect_request().await {
            return Err(anyhow::anyhow!(
                "Failed to connect to Go request socket: {}",
                e
            ));
        }

        let request = Request {
            payload: Some(proto::request::Payload::WaitForSyncToBlockRequest(
                proto::WaitForSyncToBlockRequest {
                    target_block,
                    timeout_seconds,
                },
            )),
        };

        let mut request_buf = Vec::new();
        request.encode(&mut request_buf)?;

        let mut conn_guard = self.request_connection.lock().await;
        if let Some(ref mut stream) = *conn_guard {
            let len = request_buf.len() as u32;
            stream.write_all(&len.to_be_bytes()).await?;
            stream.write_all(&request_buf).await?;
            stream.flush().await?;

            info!(
                "📤 [TRANSITION] Sent WaitForSyncToBlockRequest: target_block={}, timeout={}s",
                target_block, timeout_seconds
            );

            use tokio::io::AsyncReadExt;
            use tokio::time::{timeout, Duration};
            // Timeout needs to be longer than the Go-side timeout
            let read_timeout = Duration::from_secs(timeout_seconds + 10);

            let mut len_buf = [0u8; 4];
            timeout(read_timeout, stream.read_exact(&mut len_buf))
                .await
                .map_err(|e| anyhow::anyhow!("Timeout reading response length: {}", e))??;
            let response_len = u32::from_be_bytes(len_buf) as usize;

            if response_len == 0 || response_len > 10_000_000 {
                return Err(anyhow::anyhow!("Invalid response length: {}", response_len));
            }

            let mut response_buf = vec![0u8; response_len];
            timeout(read_timeout, stream.read_exact(&mut response_buf))
                .await
                .map_err(|e| anyhow::anyhow!("Timeout reading response data: {}", e))??;

            let response = Response::decode(&response_buf[..])?;
            match response.payload {
                Some(proto::response::Payload::NotifyEpochChangeResponse(_)) => {
                    return Err(anyhow::anyhow!("Unexpected NotifyEpochChangeResponse"));
                }
                Some(proto::response::Payload::WaitForSyncToBlockResponse(res)) => {
                    info!(
                        "✅ [TRANSITION] WaitForSyncToBlock response: reached={}, current_block={}, message={}",
                        res.reached, res.current_block, res.message
                    );
                    Ok((res.reached, res.current_block, res.message))
                }
                Some(proto::response::Payload::Error(error_msg)) => {
                    Err(anyhow::anyhow!("Go returned error: {}", error_msg))
                }
                _ => Err(anyhow::anyhow!("Unexpected response type from Go")),
            }
        } else {
            Err(anyhow::anyhow!("Request connection is not available"))
        }
    }
}
