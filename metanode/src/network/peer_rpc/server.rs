// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

//! Peer RPC Server — HTTP endpoints for peer queries.
//!
//! ## Endpoints
//!
//! - `GET /peer_info` - Returns current node's epoch and block info
//! - `GET /health` - Health check endpoint
//! - `GET /get_epoch_boundary_data?epoch=X` - Epoch boundary data
//! - `GET /get_blocks?from=X&to=Y` - Block data range
//! - `POST /submit_transaction` - Forward transactions to consensus

use anyhow::Result;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tracing::{error, info, warn};

use crate::node::executor_client::ExecutorClient;

use super::types::*;

/// Peer RPC Server for exposing node info over HTTP
pub struct PeerRpcServer {
    /// Node ID
    node_id: usize,
    /// Port to listen on
    port: u16,
    /// Network address for this node
    network_address: String,
    /// Executor client for querying Go Master
    executor_client: Arc<ExecutorClient>,
    /// Optional dynamic reference to the ConsensusNode for fetching the transaction_submitter
    node: Option<Arc<tokio::sync::Mutex<crate::node::ConsensusNode>>>,
    /// Shared index to get the last global execution index
    shared_last_global_exec_index: Arc<tokio::sync::Mutex<u64>>,
}

impl PeerRpcServer {
    /// Create new Peer RPC Server
    pub fn new(
        node_id: usize,
        port: u16,
        network_address: String,
        executor_client: Arc<ExecutorClient>,
        shared_last_global_exec_index: Arc<tokio::sync::Mutex<u64>>,
    ) -> Self {
        Self {
            node_id,
            port,
            network_address,
            executor_client,
            node: None,
            shared_last_global_exec_index,
        }
    }

    /// Set node reference for dynamic transaction submitter fetching
    pub fn with_node(mut self, node: Arc<tokio::sync::Mutex<crate::node::ConsensusNode>>) -> Self {
        self.node = Some(node);
        self
    }

    /// Start the Peer RPC Server
    pub async fn start(self) -> Result<()> {
        // Listen on all interfaces for WAN access
        let addr = format!("0.0.0.0:{}", self.port);
        let listener = TcpListener::bind(&addr).await?;
        info!(
            "🌐 [PEER RPC] Started on {} (node_id={}, network_address={})",
            addr, self.node_id, self.network_address
        );

        let executor_client = Arc::clone(&self.executor_client);
        let node_opt = self.node.clone();
        let node_id = self.node_id;
        let network_address = self.network_address.clone();
        let shared_index_arc = self.shared_last_global_exec_index.clone();

        loop {
            let (mut stream, peer_addr) = match listener.accept().await {
                Ok((s, addr)) => (s, addr),
                Err(e) => {
                    error!("🌐 [PEER RPC] Failed to accept connection: {}", e);
                    continue;
                }
            };

            let executor = Arc::clone(&executor_client);
            let node = node_opt.clone();
            let net_addr = network_address.clone();
            let shared_exec_index = shared_index_arc.clone();

            tokio::spawn(async move {
                // Read HTTP request (handle full POST bodies)
                let mut request_bytes = Vec::new();
                let mut buffer = [0u8; 8192];
                let mut expected_len = None;

                loop {
                    let read_result = tokio::time::timeout(
                        std::time::Duration::from_secs(5),
                        stream.read(&mut buffer),
                    )
                    .await;

                    match read_result {
                        Ok(Ok(0)) => break, // EOF
                        Ok(Ok(n)) => {
                            request_bytes.extend_from_slice(&buffer[..n]);

                            if expected_len.is_none() {
                                if let Some(headers_end) = request_bytes
                                    .windows(4)
                                    .position(|window| window == b"\r\n\r\n")
                                {
                                    let headers_str =
                                        String::from_utf8_lossy(&request_bytes[..headers_end]);
                                    let mut cl_len = 0;
                                    for line in headers_str.lines() {
                                        if line.to_lowercase().starts_with("content-length:") {
                                            if let Some(val) = line.split(':').nth(1) {
                                                if let Ok(len) = val.trim().parse::<usize>() {
                                                    cl_len = len;
                                                    break;
                                                }
                                            }
                                        }
                                    }
                                    expected_len = Some(headers_end + 4 + cl_len);
                                }
                            }

                            if let Some(expected) = expected_len {
                                if request_bytes.len() >= expected {
                                    break;
                                }
                            } else if request_bytes.len() > 4 && !request_bytes.starts_with(b"POST")
                            {
                                if request_bytes.windows(4).any(|w| w == b"\r\n\r\n") {
                                    break;
                                }
                            }
                        }
                        Ok(Err(e)) => {
                            warn!("🌐 [PEER RPC] Failed to read from {}: {}", peer_addr, e);
                            return;
                        }
                        Err(_) => {
                            warn!("🌐 [PEER RPC] Timeout reading from {}", peer_addr);
                            return;
                        }
                    }
                }

                if request_bytes.is_empty() {
                    return;
                }
                let request = String::from_utf8_lossy(&request_bytes);

                // Route request
                if request.starts_with("GET /peer_info") {
                    Self::handle_peer_info(
                        &mut stream,
                        &executor,
                        node_id,
                        &net_addr,
                        &shared_exec_index,
                    )
                    .await;
                } else if request.starts_with("GET /get_epoch_boundary_data") {
                    Self::handle_get_epoch_boundary_data(&mut stream, &executor, &request).await;
                } else if request.starts_with("GET /get_executable_blocks") {
                    Self::handle_get_executable_blocks(&mut stream, &executor, node_id, &request).await;
                } else if request.starts_with("GET /get_blocks") {
                    Self::handle_get_blocks(&mut stream, &executor, node_id, &request).await;
                } else if request.starts_with("GET /health") {
                    Self::handle_health(&mut stream).await;
                } else if request.starts_with("POST /submit_transaction") {
                    Self::handle_submit_transaction(&mut stream, node.as_ref(), &request).await;
                } else {
                    // Return 404 for unknown routes
                    let response = "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\n\r\n{\"error\":\"Not Found\"}";
                    let _ = stream.write_all(response.as_bytes()).await;
                }
            });
        }
    }

    /// Handle /peer_info request
    async fn handle_peer_info(
        stream: &mut tokio::net::TcpStream,
        executor: &Arc<ExecutorClient>,
        node_id: usize,
        network_address: &str,
        shared_exec_index: &Arc<tokio::sync::Mutex<u64>>,
    ) {
        // Query epoch and block from Go Master
        let epoch = match executor.get_current_epoch().await {
            Ok(e) => e,
            Err(e) => {
                error!("🌐 [PEER RPC] Failed to get epoch: {}", e);
                let response = format!(
                    "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\n\r\n{{\"error\":\"Failed to get epoch: {}\"}}",
                    e.to_string().replace('"', "\\\"")
                );
                let _ = stream.write_all(response.as_bytes()).await;
                return;
            }
        };

        let last_block = match executor.get_last_block_number().await {
            Ok((b, _)) => b,
            Err(e) => {
                error!("🌐 [PEER RPC] Failed to get last block: {}", e);
                let response = format!(
                    "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\n\r\n{{\"error\":\"Failed to get last block: {}\"}}",
                    e.to_string().replace('"', "\\\"")
                );
                let _ = stream.write_all(response.as_bytes()).await;
                return;
            }
        };

        let timestamp_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let last_global_exec_index = {
            let guard = shared_exec_index.lock().await;
            *guard
        };

        let info = PeerInfoResponse {
            node_id,
            epoch,
            last_block,
            last_global_exec_index,
            network_address: network_address.to_string(),
            timestamp_ms,
        };

        let json = serde_json::to_string(&info).unwrap_or_else(|_| "{}".to_string());
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\n\r\n{}",
            json
        );

        if let Err(e) = stream.write_all(response.as_bytes()).await {
            error!("🌐 [PEER RPC] Failed to write response: {}", e);
        }

        info!(
            "🌐 [PEER RPC] Served /peer_info: epoch={}, last_block={}, global_exec_index={}",
            epoch, last_block, last_global_exec_index
        );
    }

    /// Handle /health request
    async fn handle_health(stream: &mut tokio::net::TcpStream) {
        let response =
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\"status\":\"ok\"}";
        let _ = stream.write_all(response.as_bytes()).await;
    }

    /// Handle /get_epoch_boundary_data request
    /// URL format: GET /get_epoch_boundary_data?epoch=X
    /// Returns epoch boundary data for the specified epoch (validators, timestamp, boundary block)
    async fn handle_get_epoch_boundary_data(
        stream: &mut tokio::net::TcpStream,
        executor: &Arc<ExecutorClient>,
        request: &str,
    ) {
        // Parse epoch from query parameter
        let epoch = Self::parse_epoch_param(request);

        let Some(target_epoch) = epoch else {
            let response = EpochBoundaryDataResponse {
                epoch: 0,
                timestamp_ms: 0,
                boundary_block: 0,
                validators: vec![],
                boundary_gei: 0,
                error: Some("Missing or invalid 'epoch' parameter".to_string()),
            };
            let json = serde_json::to_string(&response).unwrap_or_else(|_| "{}".to_string());
            let http_response = format!(
                "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\n\r\n{}",
                json
            );
            let _ = stream.write_all(http_response.as_bytes()).await;
            return;
        };

        info!(
            "🌐 [PEER RPC] /get_epoch_boundary_data request: epoch={}",
            target_epoch
        );

        // Fetch from local Go Master
        match executor.get_epoch_boundary_data(target_epoch).await {
            Ok((epoch, timestamp_ms, boundary_block, validators, _, boundary_gei)) => {
                // Convert ValidatorInfo to ValidatorInfoSimple for JSON transport
                let validators_simple: Vec<ValidatorInfoSimple> = validators
                    .iter()
                    .map(|v| ValidatorInfoSimple {
                        name: v.name.clone(),
                        address: v.address.clone(),
                        stake: v.stake.parse::<u64>().unwrap_or(0),
                        protocol_key: v.protocol_key.clone(),
                        network_key: v.network_key.clone(),
                        authority_key: v.authority_key.clone(),
                    })
                    .collect();

                let response = EpochBoundaryDataResponse {
                    epoch,
                    timestamp_ms,
                    boundary_block,
                    validators: validators_simple,
                    boundary_gei,
                    error: None,
                };

                let json = serde_json::to_string(&response).unwrap_or_else(|_| "{}".to_string());
                let http_response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\n\r\n{}",
                    json
                );

                if let Err(e) = stream.write_all(http_response.as_bytes()).await {
                    error!(
                        "🌐 [PEER RPC] Failed to write /get_epoch_boundary_data response: {}",
                        e
                    );
                }

                info!(
                    "🌐 [PEER RPC] Served /get_epoch_boundary_data: epoch={}, timestamp={}, boundary_block={}, validators={}",
                    epoch, timestamp_ms, boundary_block, validators.len()
                );
            }
            Err(e) => {
                warn!("🌐 [PEER RPC] Failed to get epoch boundary data: {}", e);
                let response = EpochBoundaryDataResponse {
                    epoch: target_epoch,
                    timestamp_ms: 0,
                    boundary_block: 0,
                    validators: vec![],
                    boundary_gei: 0,
                    error: Some(format!("{}", e)),
                };

                let json = serde_json::to_string(&response).unwrap_or_else(|_| "{}".to_string());
                let http_response = format!(
                    "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\n\r\n{}",
                    json
                );

                if let Err(e) = stream.write_all(http_response.as_bytes()).await {
                    error!("🌐 [PEER RPC] Failed to write error response: {}", e);
                }
            }
        }
    }

    /// Parse epoch parameter from request query string
    fn parse_epoch_param(request: &str) -> Option<u64> {
        if let Some(query_start) = request.find('?') {
            let query_end = request[query_start..]
                .find(' ')
                .unwrap_or(request.len() - query_start);
            let query = &request[query_start + 1..query_start + query_end];

            for param in query.split('&') {
                let parts: Vec<&str> = param.split('=').collect();
                if parts.len() == 2 && parts[0] == "epoch" {
                    return parts[1].parse().ok();
                }
            }
        }
        None
    }

    /// Handle /get_executable_blocks request
    /// Reads ExecutableBlock protobuf bytes directly from Rust file store.
    /// No Go PebbleDB involved — pure Rust-to-Rust sync.
    /// URL format: GET /get_executable_blocks?from=X&to=Y
    async fn handle_get_executable_blocks(
        stream: &mut tokio::net::TcpStream,
        executor: &Arc<ExecutorClient>,
        node_id: usize,
        request: &str,
    ) {
        let (from_block, to_block) = Self::parse_block_range(request);

        let (Some(from), Some(to)) = (from_block, to_block) else {
            let response = GetBlocksResponse {
                node_id,
                blocks: std::collections::HashMap::new(),
                count: 0,
                error: Some("Missing or invalid from/to parameters".to_string()),
            };
            let json = serde_json::to_string(&response).unwrap_or_else(|_| "{}".to_string());
            let http_response = format!(
                "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\n\r\n{}",
                json
            );
            let _ = stream.write_all(http_response.as_bytes()).await;
            return;
        };

        let max_batch = 500u64;
        let actual_to = std::cmp::min(to, from + max_batch - 1);

        info!(
            "🌐 [PEER RPC] /get_executable_blocks request: from={}, to={} (actual_to={})",
            from, to, actual_to
        );

        // Read from Rust file store — no Go PebbleDB
        let storage_path = match executor.storage_path() {
            Some(p) => p,
            None => {
                let response = GetBlocksResponse {
                    node_id,
                    blocks: std::collections::HashMap::new(),
                    count: 0,
                    error: Some("No storage path configured".to_string()),
                };
                let json = serde_json::to_string(&response).unwrap_or_else(|_| "{}".to_string());
                let http_response = format!(
                    "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\n\r\n{}",
                    json
                );
                let _ = stream.write_all(http_response.as_bytes()).await;
                return;
            }
        };

        match crate::node::executor_client::block_store::load_executable_blocks_range(
            storage_path,
            from,
            actual_to,
        )
        .await
        {
            Ok(block_list) => {
                let mut blocks = std::collections::HashMap::new();
                for (gei, data) in &block_list {
                    blocks.insert(*gei, hex::encode(data));
                }

                let count = blocks.len();
                info!("🌐 [PEER RPC] Returning {} executable blocks from Rust store", count);

                let response = GetBlocksResponse {
                    node_id,
                    blocks,
                    count,
                    error: None,
                };

                let json = serde_json::to_string(&response).unwrap_or_else(|_| "{}".to_string());
                let http_response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\n\r\n{}",
                    json
                );

                if let Err(e) = stream.write_all(http_response.as_bytes()).await {
                    error!("🌐 [PEER RPC] Failed to write /get_executable_blocks response: {}", e);
                }
            }
            Err(e) => {
                warn!("🌐 [PEER RPC] Failed to load executable blocks from store: {}", e);
                let response = GetBlocksResponse {
                    node_id,
                    blocks: std::collections::HashMap::new(),
                    count: 0,
                    error: Some(format!("Failed to load blocks: {}", e)),
                };

                let json = serde_json::to_string(&response).unwrap_or_else(|_| "{}".to_string());
                let http_response = format!(
                    "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\n\r\n{}",
                    json
                );
                let _ = stream.write_all(http_response.as_bytes()).await;
            }
        }
    }

    /// Handle /get_blocks request
    /// URL format: GET /get_blocks?from=X&to=Y
    async fn handle_get_blocks(
        stream: &mut tokio::net::TcpStream,
        executor: &Arc<ExecutorClient>,
        node_id: usize,
        request: &str,
    ) {
        // Parse query parameters from request line
        let (from_block, to_block) = Self::parse_block_range(request);

        let (Some(from), Some(to)) = (from_block, to_block) else {
            let response = GetBlocksResponse {
                node_id,
                blocks: std::collections::HashMap::new(),
                count: 0,
                error: Some("Missing or invalid from/to parameters".to_string()),
            };
            let json = serde_json::to_string(&response).unwrap_or_else(|_| "{}".to_string());
            let http_response = format!(
                "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\n\r\n{}",
                json
            );
            let _ = stream.write_all(http_response.as_bytes()).await;
            return;
        };

        // Limit batch size to prevent DoS or timeouts on huge blocks (200MB+)
        let max_batch = 500u64;
        let actual_to = std::cmp::min(to, from + max_batch - 1);

        info!(
            "🌐 [PEER RPC] /get_blocks request: from={}, to={} (actual_to={})",
            from, to, actual_to
        );

        // Fetch blocks from Go Master via executor_client
        match executor.get_blocks_range(from, actual_to).await {
            Ok(block_data_list) => {
                // Convert proto::BlockData to HashMap<u64, String> for response
                // Encode FULL protobuf BlockData (not just extra_data) so receiver
                // can reconstruct complete BlockData objects for sync_blocks()
                use prost::Message;
                let mut blocks = std::collections::HashMap::new();
                for block in &block_data_list {
                    // Encode full BlockData as protobuf bytes, then hex for JSON transport
                    let proto_bytes = block.encode_to_vec();
                    blocks.insert(block.block_number, hex::encode(&proto_bytes));
                }

                let count = blocks.len();
                info!("🌐 [PEER RPC] Returning {} blocks from Go Master", count);

                let response = GetBlocksResponse {
                    node_id,
                    blocks,
                    count,
                    error: None,
                };

                let json = serde_json::to_string(&response).unwrap_or_else(|_| "{}".to_string());
                let http_response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\n\r\n{}",
                    json
                );

                if let Err(e) = stream.write_all(http_response.as_bytes()).await {
                    error!("🌐 [PEER RPC] Failed to write /get_blocks response: {}", e);
                }
            }
            Err(e) => {
                warn!("🌐 [PEER RPC] Failed to fetch blocks from Go Master: {}", e);
                let response = GetBlocksResponse {
                    node_id,
                    blocks: std::collections::HashMap::new(),
                    count: 0,
                    error: Some(format!("Failed to fetch blocks: {}", e)),
                };

                let json = serde_json::to_string(&response).unwrap_or_else(|_| "{}".to_string());
                let http_response = format!(
                    "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\n\r\n{}",
                    json
                );

                if let Err(e) = stream.write_all(http_response.as_bytes()).await {
                    error!("🌐 [PEER RPC] Failed to write error response: {}", e);
                }
            }
        }
    }

    /// Parse from and to block numbers from request query string
    fn parse_block_range(request: &str) -> (Option<u64>, Option<u64>) {
        let mut from_block = None;
        let mut to_block = None;

        // Find query string in request line
        if let Some(query_start) = request.find('?') {
            let query_end = request[query_start..]
                .find(' ')
                .unwrap_or(request.len() - query_start);
            let query = &request[query_start + 1..query_start + query_end];

            for param in query.split('&') {
                let parts: Vec<&str> = param.split('=').collect();
                if parts.len() == 2 {
                    match parts[0] {
                        "from" => from_block = parts[1].parse().ok(),
                        "to" => to_block = parts[1].parse().ok(),
                        _ => {}
                    }
                }
            }
        }

        (from_block, to_block)
    }

    /// Handle /submit_transaction POST request
    /// Receives transactions forwarded from SyncOnly nodes and submits them to local consensus.
    /// SyncOnly nodes use forward_transaction_to_validators() which sends TXs to this endpoint
    /// on validator nodes. The validator must submit these TXs to its local consensus (DAG).
    async fn handle_submit_transaction(
        stream: &mut tokio::net::TcpStream,
        node: Option<&Arc<tokio::sync::Mutex<crate::node::ConsensusNode>>>,
        request: &str,
    ) {
        // Parse POST body - find content after double newline
        let body_start = request
            .find("\r\n\r\n")
            .map(|i| i + 4)
            .or_else(|| request.find("\n\n").map(|i| i + 2))
            .unwrap_or(0);

        let body = &request[body_start..];

        // Parse JSON request (validate format)
        let submit_req: SubmitTransactionRequest = match serde_json::from_str(body.trim()) {
            Ok(req) => req,
            Err(e) => {
                let response = SubmitTransactionResponse {
                    success: false,
                    count: 0,
                    error: Some(format!("Invalid JSON: {}", e)),
                };
                let json = serde_json::to_string(&response).unwrap_or_else(|_| "{}".to_string());
                let http_response = format!(
                    "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\n\r\n{}",
                    json
                );
                let _ = stream.write_all(http_response.as_bytes()).await;
                return;
            }
        };

        let tx_count = submit_req.transactions_hex.len();

        let mut submitter = None;
        if let Some(ref wrapped_node) = node {
            submitter = wrapped_node.lock().await.transaction_submitter();
        }

        // Submit TXs to local consensus if submitter is available (validator nodes)
        if let Some(submitter) = submitter {
            // Decode all TX hex values into byte vectors
            let mut all_tx_bytes: Vec<Vec<u8>> = Vec::with_capacity(tx_count);
            let mut decode_errors = Vec::new();

            for tx_hex in &submit_req.transactions_hex {
                match hex::decode(tx_hex) {
                    Ok(tx_bytes) => {
                        all_tx_bytes.push(tx_bytes);
                    }
                    Err(e) => {
                        decode_errors.push(format!("Hex decode error: {}", e));
                    }
                }
            }

            if !all_tx_bytes.is_empty() {
                match submitter.submit(all_tx_bytes.clone()).await {
                    Ok((_block_ref, _indices, _status_rx)) => {
                        info!(
                            "📡 [TX SUBMIT] Received {} TXs from peer, submitted {} to local consensus",
                            tx_count, all_tx_bytes.len()
                        );
                    }
                    Err(e) => {
                        warn!(
                            "📡 [TX SUBMIT] Failed to submit {} TXs to consensus: {}",
                            all_tx_bytes.len(), e
                        );
                        decode_errors.push(format!("Consensus submit error: {}", e));
                    }
                }
            }

            let submitted = if decode_errors.iter().any(|e| e.contains("Consensus submit")) {
                0
            } else {
                all_tx_bytes.len()
            };

            let response = SubmitTransactionResponse {
                success: decode_errors.is_empty(),
                count: submitted,
                error: if decode_errors.is_empty() {
                    None
                } else {
                    Some(decode_errors.join("; "))
                },
            };
            let json = serde_json::to_string(&response).unwrap_or_else(|_| "{}".to_string());
            let http_response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\n\r\n{}",
                json
            );
            let _ = stream.write_all(http_response.as_bytes()).await;
        } else {
            // No submitter available (SyncOnly node) — just acknowledge
            info!(
                "📡 [TX BROADCAST-RECV] Acknowledged {} TXs (no submitter — SyncOnly node)",
                tx_count
            );

            let response = SubmitTransactionResponse {
                success: true,
                count: tx_count,
                error: None,
            };
            let json = serde_json::to_string(&response).unwrap_or_else(|_| "{}".to_string());
            let http_response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\n\r\n{}",
                json
            );
            let _ = stream.write_all(http_response.as_bytes()).await;
        }
    }
}
