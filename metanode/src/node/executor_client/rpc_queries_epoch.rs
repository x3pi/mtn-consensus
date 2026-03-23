// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

//! Epoch-related RPC query methods for ExecutorClient.
//!
//! These methods query Go Master epoch state:
//! - `get_current_epoch`
//! - `get_epoch_start_timestamp`
//! - `get_epoch_boundary_data`

use anyhow::Result;
use prost::Message;
use tokio::io::AsyncWriteExt;
use tracing::info;

use super::proto::{
    self, GetCurrentEpochRequest, GetEpochStartTimestampRequest, Request, Response, ValidatorInfo,
};
use super::ExecutorClient;

impl ExecutorClient {
    /// Get current epoch from Go state
    /// Used to determine which epoch the network is currently in
    pub async fn get_current_epoch(&self) -> Result<u64> {
        if !self.is_enabled() {
            return Err(anyhow::anyhow!("Executor client is not enabled"));
        }

        // Circuit breaker check
        if let Err(reason) = self.rpc_circuit_breaker.check("get_current_epoch") {
            return Err(anyhow::anyhow!("Circuit breaker: {}", reason));
        }

        // Connect to Go request socket if needed
        if let Err(e) = self.connect_request().await {
            return Err(anyhow::anyhow!(
                "Failed to connect to Go request socket: {}",
                e
            ));
        }

        // Create GetCurrentEpochRequest
        let request = Request {
            payload: Some(proto::request::Payload::GetCurrentEpochRequest(
                GetCurrentEpochRequest {},
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

            info!(
                "📤 [EXECUTOR-REQ] Sent GetCurrentEpochRequest to Go (size: {} bytes)",
                request_buf.len()
            );

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
                Some(proto::response::Payload::GetCurrentEpochResponse(
                    get_current_epoch_response,
                )) => {
                    let current_epoch = get_current_epoch_response.epoch;
                    info!(
                        "✅ [EXECUTOR-REQ] Received current epoch from Go: {}",
                        current_epoch
                    );
                    return Ok(current_epoch);
                }
                Some(proto::response::Payload::Error(error_msg)) => {
                    return Err(anyhow::anyhow!("Go returned error: {}", error_msg));
                }
                _ => {
                    return Err(anyhow::anyhow!("Unexpected response payload type"));
                }
            }
        } else {
            return Err(anyhow::anyhow!("Request connection is not available"));
        }
    }

    /// Get epoch start timestamp from Go state
    /// Used to sync timestamp after epoch transitions
    /// NOTE: This endpoint may not be implemented in Go yet - returns error in that case
    pub async fn get_epoch_start_timestamp(&self, epoch: u64) -> Result<u64> {
        if !self.is_enabled() {
            return Err(anyhow::anyhow!("Executor client is not enabled"));
        }

        // Connect to Go request socket if needed
        if let Err(e) = self.connect_request().await {
            return Err(anyhow::anyhow!(
                "Failed to connect to Go request socket: {}",
                e
            ));
        }

        // Create GetEpochStartTimestampRequest
        let request = Request {
            payload: Some(proto::request::Payload::GetEpochStartTimestampRequest(
                GetEpochStartTimestampRequest { epoch },
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

            info!(
                "📤 [EXECUTOR-REQ] Sent GetEpochStartTimestampRequest to Go (size: {} bytes)",
                request_buf.len()
            );

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
            let response = Response::decode(&response_buf[..]).map_err(|e| {
                anyhow::anyhow!(
                    "Failed to decode response: {}. Raw bytes: {:?}",
                    e,
                    &response_buf[..std::cmp::min(100, response_buf.len())]
                )
            })?;

            if let Some(payload) = response.payload {
                match payload {
                    proto::response::Payload::GetEpochStartTimestampResponse(
                        get_epoch_start_timestamp_response,
                    ) => {
                        let epoch_start_timestamp_ms =
                            get_epoch_start_timestamp_response.timestamp_ms;
                        info!(
                            "✅ [EXECUTOR-REQ] Received epoch start timestamp from Go: {}ms",
                            epoch_start_timestamp_ms
                        );
                        return Ok(epoch_start_timestamp_ms);
                    }
                    proto::response::Payload::Error(error_msg) => {
                        return Err(anyhow::anyhow!("Go returned error: {}", error_msg));
                    }
                    _ => {
                        return Err(anyhow::anyhow!("Unexpected response payload type"));
                    }
                }
            } else {
                return Err(anyhow::anyhow!("Request connection is not available"));
            }
        } else {
            return Err(anyhow::anyhow!("Request connection is not available"));
        }
    }

    /// Get unified epoch boundary data from Go Master (NEW: single authoritative source for epoch transitions)
    /// Returns: epoch, epoch_start_timestamp_ms, boundary_block, validators snapshot, epoch_duration_seconds, and boundary_gei
    /// This ensures consistency by getting all epoch transition data in a single atomic request
    pub async fn get_epoch_boundary_data(
        &self,
        epoch: u64,
    ) -> Result<(u64, u64, u64, Vec<ValidatorInfo>, u64, u64)> {
        if !self.is_enabled() {
            return Err(anyhow::anyhow!("Executor client is not enabled"));
        }

        // Circuit breaker check
        if let Err(reason) = self.rpc_circuit_breaker.check("get_epoch_boundary_data") {
            return Err(anyhow::anyhow!("Circuit breaker: {}", reason));
        }

        // Connect to Go request socket if needed
        if let Err(e) = self.connect_request().await {
            return Err(anyhow::anyhow!(
                "Failed to connect to Go request socket: {}",
                e
            ));
        }

        // Create GetEpochBoundaryDataRequest
        let request = Request {
            payload: Some(proto::request::Payload::GetEpochBoundaryDataRequest(
                proto::GetEpochBoundaryDataRequest { epoch },
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

            info!("📤 [EXECUTOR-REQ] Sent GetEpochBoundaryDataRequest to Go for epoch {} (size: {} bytes)",
                epoch, request_buf.len());

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
                "📥 [EXECUTOR-REQ] Received {} bytes from Go (GetEpochBoundaryData), decoding...",
                response_buf.len()
            );

            // Decode response
            let response = Response::decode(&response_buf[..]).map_err(|e| {
                anyhow::anyhow!(
                    "Failed to decode response from Go: {}. Response length: {} bytes",
                    e,
                    response_buf.len()
                )
            })?;

            match response.payload {
                Some(proto::response::Payload::NotifyEpochChangeResponse(_)) => {
                    return Err(anyhow::anyhow!("Unexpected NotifyEpochChangeResponse"));
                }
                Some(proto::response::Payload::EpochBoundaryData(data)) => {
                    info!("✅ [EPOCH BOUNDARY] Received unified epoch boundary data: epoch={}, timestamp_ms={}, boundary_block={}, validator_count={}",
                        data.epoch, data.epoch_start_timestamp_ms, data.boundary_block, data.validators.len());

                    // Log validators for debugging
                    for (idx, validator) in data.validators.iter().enumerate() {
                        let auth_key_preview = if validator.authority_key.len() > 50 {
                            format!("{}...", &validator.authority_key[..50])
                        } else {
                            validator.authority_key.clone()
                        };
                        info!("📥 [RUST←GO] EpochBoundaryData Validator[{}]: address={}, stake={}, name={}, authority_key={}",
                            idx, validator.address, validator.stake, validator.name, auth_key_preview);
                    }

                    // epoch_duration_seconds: 0 means not set by Go, default to 900 (15 min)
                    let epoch_duration = if data.epoch_duration_seconds > 0 {
                        data.epoch_duration_seconds
                    } else {
                        900
                    };
                    return Ok((
                        data.epoch,
                        data.epoch_start_timestamp_ms,
                        data.boundary_block,
                        data.validators,
                        epoch_duration,
                        data.boundary_gei,
                    ));
                }
                Some(proto::response::Payload::Error(error_msg)) => {
                    return Err(anyhow::anyhow!("Go returned error: {}", error_msg));
                }
                _ => {
                    return Err(anyhow::anyhow!(
                        "Unexpected response payload type for GetEpochBoundaryData"
                    ));
                }
            }
        } else {
            return Err(anyhow::anyhow!("Request connection is not available"));
        }
    }
}
