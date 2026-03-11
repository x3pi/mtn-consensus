// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

//! RPC query methods for ExecutorClient — state queries.
//!
//! These methods query Go Master state via the request/response socket:
//! - `get_validators_at_block`
//! - `get_last_block_number`
//!
//! Epoch-related queries are in `rpc_queries_epoch.rs`.

use anyhow::Result;
use prost::Message;
use tokio::io::AsyncWriteExt;
use tracing::{info, warn};

use super::persistence::persist_last_block_number;
use super::proto::{self, GetValidatorsAtBlockRequest, Request, Response, ValidatorInfo};
use super::ExecutorClient;

impl ExecutorClient {
    /// Get validators at a specific block number from Go state
    /// Used for startup (block 0) and epoch transition (last_global_exec_index)
    pub async fn get_validators_at_block(
        &self,
        block_number: u64,
    ) -> Result<(Vec<ValidatorInfo>, u64, u64)> {
        if !self.is_enabled() {
            return Err(anyhow::anyhow!("Executor client is not enabled"));
        }

        // Circuit breaker check
        if let Err(reason) = self.rpc_circuit_breaker.check("get_validators_at_block") {
            return Err(anyhow::anyhow!("Circuit breaker: {}", reason));
        }

        // Connect to Go request socket if needed
        if let Err(e) = self.connect_request().await {
            return Err(anyhow::anyhow!(
                "Failed to connect to Go request socket: {}",
                e
            ));
        }

        // Create GetValidatorsAtBlockRequest
        let request = Request {
            payload: Some(proto::request::Payload::GetValidatorsAtBlockRequest(
                GetValidatorsAtBlockRequest { block_number },
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

            info!("📤 [EXECUTOR-REQ] Sent GetValidatorsAtBlockRequest to Go for block {} (size: {} bytes)", 
                block_number, request_buf.len());

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
            info!(
                "🔍 [EXECUTOR-REQ] Raw response bytes (hex): {}",
                hex::encode(&response_buf)
            );
            info!(
                "🔍 [EXECUTOR-REQ] Raw response bytes (first 50): {:?}",
                &response_buf[..response_buf.len().min(50)]
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

            // Debug: Check all possible payload types
            match &response.payload {
                Some(proto::response::Payload::NotifyEpochChangeResponse(_)) => {
                    warn!("🔍 [EXECUTOR-REQ] Payload is NotifyEpochChangeResponse (ignored in debug match)");
                }
                Some(proto::response::Payload::ValidatorInfoList(v)) => {
                    info!(
                        "🔍 [EXECUTOR-REQ] Payload is ValidatorInfoList with {} validators",
                        v.validators.len()
                    );
                    // CRITICAL: Log each ValidatorInfo exactly as received from Go
                    for (idx, validator) in v.validators.iter().enumerate() {
                        let auth_key_preview = if validator.authority_key.len() > 50 {
                            format!("{}...", &validator.authority_key[..50])
                        } else {
                            validator.authority_key.clone()
                        };
                        info!("📥 [RUST←GO] ValidatorInfo[{}]: address={}, stake={}, name={}, authority_key={}, protocol_key={}, network_key={}",
                            idx, validator.address, validator.stake, validator.name, 
                            auth_key_preview, validator.protocol_key, validator.network_key);
                    }
                }
                Some(proto::response::Payload::Error(e)) => {
                    info!("🔍 [EXECUTOR-REQ] Payload is Error: {}", e);
                }
                Some(proto::response::Payload::ValidatorList(_)) => {
                    warn!("🔍 [EXECUTOR-REQ] Payload is ValidatorList (not expected for this request)");
                }
                Some(proto::response::Payload::ServerStatus(_)) => {
                    warn!(
                        "🔍 [EXECUTOR-REQ] Payload is ServerStatus (not expected for this request)"
                    );
                }
                Some(proto::response::Payload::LastBlockNumberResponse(_)) => {
                    warn!("🔍 [EXECUTOR-REQ] Payload is LastBlockNumberResponse (not expected for GetValidatorsAtBlockRequest)");
                }
                Some(proto::response::Payload::GetCurrentEpochResponse(_)) => {
                    info!("🔍 [EXECUTOR-REQ] Payload is GetCurrentEpochResponse (handled below)");
                }
                Some(proto::response::Payload::GetEpochStartTimestampResponse(_)) => {
                    warn!("🔍 [EXECUTOR-REQ] Payload is GetEpochStartTimestampResponse (not expected for this request)");
                }
                Some(proto::response::Payload::AdvanceEpochResponse(_)) => {
                    warn!("🔍 [EXECUTOR-REQ] Payload is AdvanceEpochResponse (not expected for this request)");
                }
                Some(proto::response::Payload::EpochBoundaryData(_)) => {
                    warn!("🔍 [EXECUTOR-REQ] Payload is EpochBoundaryData (not expected for this request)");
                }
                Some(proto::response::Payload::SetConsensusStartBlockResponse(_))
                | Some(proto::response::Payload::SetSyncStartBlockResponse(_))
                | Some(proto::response::Payload::WaitForSyncToBlockResponse(_)) => {
                    warn!("🔍 [EXECUTOR-REQ] Payload is Transition Handoff response (not expected for this request)");
                }
                Some(proto::response::Payload::GetBlocksRangeResponse(_))
                | Some(proto::response::Payload::SyncBlocksResponse(_)) => {
                    warn!("🔍 [EXECUTOR-REQ] Payload is Block Sync response (not expected for this request)");
                }
                None => {
                    warn!(
                        "🔍 [EXECUTOR-REQ] Payload is None - response structure may be incorrect"
                    );
                    warn!("🔍 [EXECUTOR-REQ] Full response debug: {:?}", response);
                }
            }

            match response.payload {
                Some(proto::response::Payload::NotifyEpochChangeResponse(_)) => {
                    return Err(anyhow::anyhow!("Unexpected NotifyEpochChangeResponse"));
                }
                Some(proto::response::Payload::ValidatorInfoList(validator_info_list)) => {
                    info!("✅ [EXECUTOR-REQ] Received ValidatorInfoList from Go at block {} with {} validators, epoch_timestamp_ms={}, last_global_exec_index={}",
                        block_number, validator_info_list.validators.len(),
                        validator_info_list.epoch_timestamp_ms,
                        validator_info_list.last_global_exec_index);

                    // CRITICAL: Log each ValidatorInfo exactly as received from Go
                    for (idx, validator) in validator_info_list.validators.iter().enumerate() {
                        let auth_key_preview = if validator.authority_key.len() > 50 {
                            format!("{}...", &validator.authority_key[..50])
                        } else {
                            validator.authority_key.clone()
                        };
                        info!("📥 [RUST←GO] ValidatorInfo[{}]: address={}, stake={}, name={}, authority_key={}, protocol_key={}, network_key={}",
                            idx, validator.address, validator.stake, validator.name,
                            auth_key_preview, validator.protocol_key, validator.network_key);
                    }

                    return Ok((
                        validator_info_list.validators,
                        validator_info_list.epoch_timestamp_ms,
                        validator_info_list.last_global_exec_index,
                    ));
                }
                Some(proto::response::Payload::Error(error_msg)) => {
                    return Err(anyhow::anyhow!("Go returned error: {}", error_msg));
                }
                Some(proto::response::Payload::ValidatorList(_)) => {
                    return Err(anyhow::anyhow!(
                        "Unexpected ValidatorList response (expected ValidatorInfoList)"
                    ));
                }
                Some(proto::response::Payload::ServerStatus(_)) => {
                    return Err(anyhow::anyhow!(
                        "Unexpected ServerStatus response (expected ValidatorInfoList)"
                    ));
                }
                Some(proto::response::Payload::LastBlockNumberResponse(_)) => {
                    return Err(anyhow::anyhow!(
                        "Unexpected LastBlockNumberResponse response (expected ValidatorInfoList)"
                    ));
                }
                Some(proto::response::Payload::GetCurrentEpochResponse(_)) => {
                    return Err(anyhow::anyhow!(
                        "Unexpected GetCurrentEpochResponse response (expected ValidatorInfoList)"
                    ));
                }
                Some(proto::response::Payload::GetEpochStartTimestampResponse(_)) => {
                    return Err(anyhow::anyhow!("Unexpected GetEpochStartTimestampResponse response (expected ValidatorInfoList)"));
                }
                Some(proto::response::Payload::AdvanceEpochResponse(_)) => {
                    return Err(anyhow::anyhow!(
                        "Unexpected AdvanceEpochResponse response (expected ValidatorInfoList)"
                    ));
                }
                Some(proto::response::Payload::EpochBoundaryData(_)) => {
                    return Err(anyhow::anyhow!(
                        "Unexpected EpochBoundaryData response (expected ValidatorInfoList)"
                    ));
                }
                Some(proto::response::Payload::SetConsensusStartBlockResponse(_))
                | Some(proto::response::Payload::SetSyncStartBlockResponse(_))
                | Some(proto::response::Payload::WaitForSyncToBlockResponse(_)) => {
                    return Err(anyhow::anyhow!(
                        "Unexpected Transition Handoff response (expected ValidatorInfoList)"
                    ));
                }
                Some(proto::response::Payload::GetBlocksRangeResponse(_))
                | Some(proto::response::Payload::SyncBlocksResponse(_)) => {
                    return Err(anyhow::anyhow!(
                        "Unexpected Block Sync response (expected ValidatorInfoList)"
                    ));
                }
                None => {
                    return Err(anyhow::anyhow!("Unexpected response type from Go. Response payload: None. Response bytes (hex): {}", hex::encode(&response_buf)));
                }
            }
        } else {
            return Err(anyhow::anyhow!("Request connection is not available"));
        }
    }

    /// Get last block number AND last global exec index from Go Master
    /// Returns (last_block_number, last_global_exec_index)
    /// CRITICAL: last_block_number counts only non-empty commits (actual blocks)
    ///           last_global_exec_index counts ALL commits (including empty ones)
    ///           Use last_global_exec_index for epoch transition SYNC WAIT comparison
    pub async fn get_last_block_number(&self) -> Result<u64> {
        if !self.is_enabled() {
            return Err(anyhow::anyhow!("Executor client is not enabled"));
        }

        // Circuit breaker check
        if let Err(reason) = self.rpc_circuit_breaker.check("get_last_block_number") {
            return Err(anyhow::anyhow!("Circuit breaker: {}", reason));
        }

        // Connect to Go request socket if needed
        if let Err(e) = self.connect_request().await {
            return Err(anyhow::anyhow!(
                "Failed to connect to Go request socket: {}",
                e
            ));
        }

        // Create GetLastBlockNumberRequest
        let request = Request {
            payload: Some(proto::request::Payload::GetLastBlockNumberRequest(
                proto::GetLastBlockNumberRequest {},
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
                "📤 [EXECUTOR-REQ] Sent GetLastBlockNumberRequest to Go (size: {} bytes)",
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

            // HEX DUMP: Log raw proto bytes to diagnose gei=0 decode bug
            let hex_preview: String = response_buf.iter().take(64).map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join(" ");
            info!(
                "📥 [EXECUTOR-REQ] Received {} bytes from Go, hex={}, decoding...",
                response_buf.len(), hex_preview
            );

            // Decode response
            let response = Response::decode(&response_buf[..])
                .map_err(|e| anyhow::anyhow!("Failed to decode response from Go: {}", e))?;

            match response.payload {
                Some(proto::response::Payload::NotifyEpochChangeResponse(_)) => {
                    return Err(anyhow::anyhow!("Unexpected NotifyEpochChangeResponse"));
                }
                Some(proto::response::Payload::LastBlockNumberResponse(res)) => {
                    let last_block_number = res.last_block_number;
                    let last_gei = res.last_global_exec_index;
                    info!(
                        "✅ [EXECUTOR-REQ] Received LastBlockNumberResponse: block={}, gei={}",
                        last_block_number, last_gei
                    );

                    // Persist for crash recovery
                    if let Some(ref storage_path) = self.storage_path {
                        if let Err(e) =
                            persist_last_block_number(storage_path, last_block_number).await
                        {
                            warn!(
                                "⚠️ [PERSIST] Failed to persist last block number {}: {}",
                                last_block_number, e
                            );
                        }
                    }

                    return Ok(last_block_number);
                }
                Some(proto::response::Payload::Error(error_msg)) => {
                    return Err(anyhow::anyhow!("Go returned error: {}", error_msg));
                }
                _ => {
                    return Err(anyhow::anyhow!(
                        "Unexpected response type from Go (expected LastBlockNumberResponse)"
                    ));
                }
            }
        } else {
            return Err(anyhow::anyhow!("Request connection is not available"));
        }
    }

    /// Get last global exec index from Go Master
    /// CRITICAL: This returns the global_exec_index (tracks ALL commits including empty)
    /// Use this for epoch transition SYNC WAIT — NOT get_last_block_number!
    pub async fn get_last_global_exec_index(&self) -> Result<u64> {
        if !self.is_enabled() {
            return Err(anyhow::anyhow!("Executor client is not enabled"));
        }

        // Circuit breaker check
        if let Err(reason) = self.rpc_circuit_breaker.check("get_last_global_exec_index") {
            return Err(anyhow::anyhow!("Circuit breaker: {}", reason));
        }

        // Connect to Go request socket if needed
        if let Err(e) = self.connect_request().await {
            return Err(anyhow::anyhow!(
                "Failed to connect to Go request socket: {}",
                e
            ));
        }

        // Reuse GetLastBlockNumberRequest — Go response now includes last_global_exec_index
        let request = Request {
            payload: Some(proto::request::Payload::GetLastBlockNumberRequest(
                proto::GetLastBlockNumberRequest {},
            )),
        };

        let mut request_buf = Vec::new();
        request.encode(&mut request_buf)?;

        let mut conn_guard = self.request_connection.lock().await;
        if let Some(ref mut stream) = *conn_guard {
            let len = request_buf.len() as u32;
            let len_bytes = len.to_be_bytes();
            stream.write_all(&len_bytes).await?;
            stream.write_all(&request_buf).await?;
            stream.flush().await?;

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

            // HEX DUMP: Log raw proto bytes to diagnose gei=0 decode bug
            let hex_preview: String = response_buf.iter().take(64).map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join(" ");
            info!(
                "📥 [EXECUTOR-REQ-GEI] Received {} bytes from Go, hex={}",
                response_buf.len(), hex_preview
            );

            let response = Response::decode(&response_buf[..])
                .map_err(|e| anyhow::anyhow!("Failed to decode response from Go: {}", e))?;

            match response.payload {
                Some(proto::response::Payload::LastBlockNumberResponse(res)) => {
                    let last_gei = res.last_global_exec_index;
                    info!(
                        "✅ [EXECUTOR-REQ] Go last_global_exec_index={} (block={})",
                        last_gei, res.last_block_number
                    );
                    return Ok(last_gei);
                }
                Some(proto::response::Payload::Error(error_msg)) => {
                    return Err(anyhow::anyhow!("Go returned error: {}", error_msg));
                }
                _ => {
                    return Err(anyhow::anyhow!(
                        "Unexpected response type from Go (expected LastBlockNumberResponse)"
                    ));
                }
            }
        } else {
            return Err(anyhow::anyhow!("Request connection is not available"));
        }
    }
}
