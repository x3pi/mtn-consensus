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

        // Create GetCurrentEpochRequest
        let request = Request {
            payload: Some(proto::request::Payload::GetCurrentEpochRequest(
                GetCurrentEpochRequest {},
            )),
        };

        // Encode request to protobuf bytes
        let mut request_buf = Vec::new();
        request.encode(&mut request_buf)?;

        // FFI INTEGRATION: Send request directly via CGo callback
        let response_buf = self.execute_rpc_request(&request_buf).await?;

        info!(
            "📥 [EXECUTOR-REQ] Received {} bytes from Go FFI, decoding...",
            response_buf.len()
        );

        // Decode response
        let response = Response::decode(&response_buf[..]).map_err(|e| {
            anyhow::anyhow!(
                "Failed to decode response from Go: {}. Response length: {} bytes.",
                e,
                response_buf.len()
            )
        })?;

        info!("🔍 [EXECUTOR-REQ] Decoded response successfully");
        info!(
            "🔍 [EXECUTOR-REQ] Response payload type: {:?}",
            response.payload
        );

        match response.payload {
            Some(proto::response::Payload::NotifyEpochChangeResponse(_)) => {
                Err(anyhow::anyhow!("Unexpected NotifyEpochChangeResponse"))
            }
            Some(proto::response::Payload::GetCurrentEpochResponse(get_current_epoch_response)) => {
                let current_epoch = get_current_epoch_response.epoch;
                info!(
                    "✅ [EXECUTOR-REQ] Received current epoch from Go FFI: {}",
                    current_epoch
                );
                Ok(current_epoch)
            }
            Some(proto::response::Payload::Error(error_msg)) => {
                Err(anyhow::anyhow!("Go returned error: {}", error_msg))
            }
            _ => Err(anyhow::anyhow!("Unexpected response payload type")),
        }
    }

    /// Get epoch start timestamp from Go state
    /// Used to sync timestamp after epoch transitions
    /// NOTE: This endpoint may not be implemented in Go yet - returns error in that case
    pub async fn get_epoch_start_timestamp(&self, epoch: u64) -> Result<u64> {
        if !self.is_enabled() {
            return Err(anyhow::anyhow!("Executor client is not enabled"));
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

        // FFI INTEGRATION: Send request directly via CGo callback
        let response_buf = self.execute_rpc_request(&request_buf).await?;

        info!(
            "📥 [EXECUTOR-REQ] Received {} bytes from Go FFI, decoding...",
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
                    let epoch_start_timestamp_ms = get_epoch_start_timestamp_response.timestamp_ms;
                    info!(
                        "✅ [EXECUTOR-REQ] Received epoch start timestamp from Go FFI: {}ms",
                        epoch_start_timestamp_ms
                    );
                    Ok(epoch_start_timestamp_ms)
                }
                proto::response::Payload::Error(error_msg) => {
                    Err(anyhow::anyhow!("Go returned error: {}", error_msg))
                }
                _ => Err(anyhow::anyhow!("Unexpected response payload type")),
            }
        } else {
            Err(anyhow::anyhow!("Response payload is missing"))
        }
    }

    /// Get unified epoch boundary data from Go Master (NEW: single authoritative source for epoch transitions)
    /// Returns: epoch, epoch_start_timestamp_ms, boundary_block, validators snapshot, epoch_duration_seconds, and boundary_gei
    /// This ensures consistency by getting all epoch transition data in a single atomic request
    ///
    /// CRITICAL FIX (2026-04-15): Added force_peer_check parameter. When true, always query peers
    /// for the correct boundary_gei regardless of Go's value. This is needed for cold-start
    /// after snapshot restore when Go may have stale non-zero boundary_gei.
    pub async fn get_safe_epoch_boundary_data(
        &self,
        epoch: u64,
        peer_rpc_addresses: &[String],
    ) -> Result<(u64, u64, u64, Vec<ValidatorInfo>, u64, u64)> {
        self.get_safe_epoch_boundary_data_with_force(epoch, peer_rpc_addresses, false)
            .await
    }

    /// Internal implementation with force_peer_check flag.
    /// When force_peer_check=true, always validates boundary_gei from peers (for cold-start).
    pub async fn get_safe_epoch_boundary_data_with_force(
        &self,
        epoch: u64,
        peer_rpc_addresses: &[String],
        force_peer_check: bool,
    ) -> Result<(u64, u64, u64, Vec<ValidatorInfo>, u64, u64)> {
        let result = self.get_epoch_boundary_data(epoch).await;
        match result {
            Ok((ret_epoch, timestamp, boundary_block, validators, arg5, boundary_gei)) => {
                // CRITICAL FIX (2026-04-15): Also query peers if force_peer_check is true (cold-start).
                // After snapshot restore, Go may have non-zero but stale boundary_gei.
                if (boundary_gei == 0 && epoch > 0) || (force_peer_check && epoch > 0) {
                    let reason = if boundary_gei == 0 {
                        "boundary_gei=0"
                    } else {
                        "cold-start validation (force_peer_check)"
                    };
                    tracing::warn!(
                        "⚠️ [FORK-SAFETY] {} for epoch {} from Go — querying peers...",
                        reason,
                        epoch
                    );

                    let mut peer_gei: Option<u64> = None;
                    for peer_addr in peer_rpc_addresses {
                        match crate::network::peer_rpc::query_peer_epoch_boundary_data(
                            peer_addr, epoch,
                        )
                        .await
                        {
                            Ok(pb) if pb.boundary_gei > 0 => {
                                tracing::info!(
                                    "✅ [FORK-SAFETY] Got safe boundary_gei={} from peer {} for epoch {}",
                                    pb.boundary_gei, peer_addr, epoch
                                );
                                // Update Go so subsequent queries return the right value
                                if let Err(e) = self
                                    .advance_epoch(
                                        epoch,
                                        timestamp,
                                        boundary_block,
                                        pb.boundary_gei,
                                    )
                                    .await
                                {
                                    tracing::warn!(
                                        "⚠️ [FORK-SAFETY] Failed to update Go's boundary_gei: {}",
                                        e
                                    );
                                }
                                peer_gei = Some(pb.boundary_gei);
                                break;
                            }
                            Ok(_) => {
                                tracing::warn!("⚠️ Peer {} returned boundary_gei=0", peer_addr)
                            }
                            Err(e) => {
                                tracing::warn!("⚠️ Peer {} query failed: {}", peer_addr, e)
                            }
                        }
                    }

                    let safe_boundary_gei = match peer_gei {
                        Some(gei) => gei,
                        None => {
                            tracing::error!(
                                "🚨 [FORK-SAFETY] No peer has boundary_gei for epoch {}! Using 0 — WILL LIKELY CAUSE FORK!",
                                epoch
                            );
                            0
                        }
                    };

                    Ok((
                        ret_epoch,
                        timestamp,
                        boundary_block,
                        validators,
                        arg5,
                        safe_boundary_gei,
                    ))
                } else {
                    Ok((
                        ret_epoch,
                        timestamp,
                        boundary_block,
                        validators,
                        arg5,
                        boundary_gei,
                    ))
                }
            }
            Err(e) => {
                tracing::warn!(
                    "⚠️ [FORK-SAFETY] executor_client.get_epoch_boundary_data failed: {}",
                    e
                );
                Err(e)
            }
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

        // Create GetEpochBoundaryDataRequest
        let request = Request {
            payload: Some(proto::request::Payload::GetEpochBoundaryDataRequest(
                proto::GetEpochBoundaryDataRequest { epoch },
            )),
        };

        // Encode request to protobuf bytes
        let mut request_buf = Vec::new();
        request.encode(&mut request_buf)?;

        // FFI INTEGRATION: Send request directly via CGo callback
        let response_buf = self.execute_rpc_request(&request_buf).await?;

        info!(
            "📥 [EXECUTOR-REQ] Received {} bytes from Go FFI (GetEpochBoundaryData), decoding...",
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
                Err(anyhow::anyhow!("Unexpected NotifyEpochChangeResponse"))
            }
            Some(proto::response::Payload::EpochBoundaryData(data)) => {
                info!("✅ [EPOCH BOUNDARY] Received unified epoch boundary data from Go FFI: epoch={}, timestamp_ms={}, boundary_block={}, validator_count={}",
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
                Ok((
                    data.epoch,
                    data.epoch_start_timestamp_ms,
                    data.boundary_block,
                    data.validators,
                    epoch_duration,
                    data.boundary_gei,
                ))
            }
            Some(proto::response::Payload::Error(error_msg)) => {
                Err(anyhow::anyhow!("Go returned error: {}", error_msg))
            }
            _ => Err(anyhow::anyhow!(
                "Unexpected response payload type for GetEpochBoundaryData"
            )),
        }
    }
}
