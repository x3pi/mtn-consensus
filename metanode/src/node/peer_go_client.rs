// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

//! PeerGoClient - Fetch blocks from peer's Go layer via TCP
//!
//! This enables Rust-centric sync where:
//! - Rust fetches blocks from any peer (Validator or SyncOnly)
//! - Rust controls epoch transitions
//! - Rust sends blocks to local Go for execution

use anyhow::Result;
use prost::Message;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tracing::{debug, info};

// Re-use protobuf definitions from executor_client
use super::executor_client::proto::{self, BlockData, GetBlocksRangeRequest, Request, Response};

/// Client to fetch blocks from a peer's Go layer via TCP
pub struct PeerGoClient {
    /// Peer's TCP address (host:port for Go's peer_rpc_port)
    peer_addr: SocketAddr,
    /// Connection timeout in seconds
    timeout_secs: u64,
}

impl PeerGoClient {
    /// Create a new PeerGoClient
    pub fn new(peer_addr: SocketAddr) -> Self {
        Self {
            peer_addr,
            timeout_secs: 10,
        }
    }

    /// Create from string address (e.g., "192.168.1.100:9001")
    pub fn from_str(addr: &str) -> Result<Self> {
        let peer_addr: SocketAddr = addr
            .parse()
            .map_err(|e| anyhow::anyhow!("Invalid peer address '{}': {}", addr, e))?;
        Ok(Self::new(peer_addr))
    }

    /// Set connection timeout
    #[allow(dead_code)]
    pub fn with_timeout(mut self, timeout_secs: u64) -> Self {
        self.timeout_secs = timeout_secs;
        self
    }

    /// Fetch blocks from peer's Go layer
    /// Returns blocks in the range [from_block, to_block] (inclusive)
    pub async fn get_blocks_range(&self, from_block: u64, to_block: u64) -> Result<Vec<BlockData>> {
        debug!(
            "📤 [PEER-GO] Requesting blocks {} to {} from peer {}",
            from_block, to_block, self.peer_addr
        );

        let mut all_blocks = Vec::new();
        let chunk_size = 5; // Chunk size matches the UDS limit to respect 32MB P2P constraints

        for current_from in (from_block..=to_block).step_by(chunk_size as usize) {
            let current_to = std::cmp::min(current_from + chunk_size - 1, to_block);

            // Connect to peer (new connection per chunk)
            let connect_timeout = Duration::from_secs(self.timeout_secs);
            let mut stream = timeout(connect_timeout, TcpStream::connect(self.peer_addr))
                .await
                .map_err(|_| anyhow::anyhow!("Connection timeout to peer {}", self.peer_addr))?
                .map_err(|e| {
                    anyhow::anyhow!("Failed to connect to peer {}: {}", self.peer_addr, e)
                })?;

            // Build request
            let request = Request {
                payload: Some(proto::request::Payload::GetBlocksRangeRequest(
                    GetBlocksRangeRequest {
                        from_block: current_from,
                        to_block: current_to,
                    },
                )),
            };

            // Encode and send
            let request_bytes = request.encode_to_vec();
            let len_bytes = (request_bytes.len() as u32).to_be_bytes();

            stream.write_all(&len_bytes).await?;
            stream.write_all(&request_bytes).await?;
            stream.flush().await?;

            // Read response length
            let read_timeout = Duration::from_secs(self.timeout_secs);
            let mut len_buf = [0u8; 4];
            timeout(read_timeout, stream.read_exact(&mut len_buf))
                .await
                .map_err(|_| anyhow::anyhow!("Timeout reading response length from peer"))??;

            let response_len = u32::from_be_bytes(len_buf) as usize;
            if response_len == 0 || response_len > 100_000_000 {
                return Err(anyhow::anyhow!("Invalid response length: {}", response_len));
            }

            // Read response data
            let mut response_buf = vec![0u8; response_len];
            timeout(read_timeout, stream.read_exact(&mut response_buf))
                .await
                .map_err(|_| anyhow::anyhow!("Timeout reading response data from peer"))??;

            // Decode response
            let response = Response::decode(&response_buf[..])
                .map_err(|e| anyhow::anyhow!("Failed to decode response: {}", e))?;

            match response.payload {
                Some(proto::response::Payload::GetBlocksRangeResponse(resp)) => {
                    if !resp.error.is_empty() {
                        return Err(anyhow::anyhow!("Peer returned error: {}", resp.error));
                    }
                    all_blocks.extend(resp.blocks);
                }
                Some(proto::response::Payload::Error(e)) => {
                    return Err(anyhow::anyhow!("Peer error: {}", e))
                }
                _ => return Err(anyhow::anyhow!("Unexpected response type from peer")),
            }
        }

        info!(
            "✅ [PEER-GO] Received {} total blocks across chunks from peer {}",
            all_blocks.len(),
            self.peer_addr
        );
        Ok(all_blocks)
    }

    /// Get peer's current epoch
    #[allow(dead_code)]
    pub async fn get_current_epoch(&self) -> Result<u64> {
        let connect_timeout = Duration::from_secs(self.timeout_secs);
        let mut stream = timeout(connect_timeout, TcpStream::connect(self.peer_addr))
            .await
            .map_err(|_| anyhow::anyhow!("Connection timeout"))??;

        let request = Request {
            payload: Some(proto::request::Payload::GetCurrentEpochRequest(
                proto::GetCurrentEpochRequest {},
            )),
        };

        let request_bytes = request.encode_to_vec();
        let len_bytes = (request_bytes.len() as u32).to_be_bytes();
        stream.write_all(&len_bytes).await?;
        stream.write_all(&request_bytes).await?;
        stream.flush().await?;

        let read_timeout = Duration::from_secs(self.timeout_secs);
        let mut len_buf = [0u8; 4];
        timeout(read_timeout, stream.read_exact(&mut len_buf)).await??;
        let response_len = u32::from_be_bytes(len_buf) as usize;

        let mut response_buf = vec![0u8; response_len];
        timeout(read_timeout, stream.read_exact(&mut response_buf)).await??;

        let response = Response::decode(&response_buf[..])?;
        match response.payload {
            Some(proto::response::Payload::GetCurrentEpochResponse(resp)) => Ok(resp.epoch),
            Some(proto::response::Payload::Error(e)) => Err(anyhow::anyhow!("Peer error: {}", e)),
            _ => Err(anyhow::anyhow!("Unexpected response type")),
        }
    }

    /// Get peer's last block number
    #[allow(dead_code)]
    pub async fn get_last_block_number(&self) -> Result<u64> {
        let connect_timeout = Duration::from_secs(self.timeout_secs);
        let mut stream = timeout(connect_timeout, TcpStream::connect(self.peer_addr))
            .await
            .map_err(|_| anyhow::anyhow!("Connection timeout"))??;

        let request = Request {
            payload: Some(proto::request::Payload::GetLastBlockNumberRequest(
                proto::GetLastBlockNumberRequest {},
            )),
        };

        let request_bytes = request.encode_to_vec();
        let len_bytes = (request_bytes.len() as u32).to_be_bytes();
        stream.write_all(&len_bytes).await?;
        stream.write_all(&request_bytes).await?;
        stream.flush().await?;

        let read_timeout = Duration::from_secs(self.timeout_secs);
        let mut len_buf = [0u8; 4];
        timeout(read_timeout, stream.read_exact(&mut len_buf)).await??;
        let response_len = u32::from_be_bytes(len_buf) as usize;

        let mut response_buf = vec![0u8; response_len];
        timeout(read_timeout, stream.read_exact(&mut response_buf)).await??;

        let response = Response::decode(&response_buf[..])?;
        match response.payload {
            Some(proto::response::Payload::LastBlockNumberResponse(resp)) => {
                Ok(resp.last_block_number)
            }
            Some(proto::response::Payload::Error(e)) => Err(anyhow::anyhow!("Peer error: {}", e)),
            _ => Err(anyhow::anyhow!("Unexpected response type")),
        }
    }

    /// Get epoch boundary data from peer
    #[allow(dead_code)]
    pub async fn get_epoch_boundary_data(
        &self,
        epoch: u64,
    ) -> Result<(u64, u64, u64, Vec<proto::ValidatorInfo>, u64)> {
        let connect_timeout = Duration::from_secs(self.timeout_secs);
        let mut stream = timeout(connect_timeout, TcpStream::connect(self.peer_addr))
            .await
            .map_err(|_| anyhow::anyhow!("Connection timeout"))??;

        let request = Request {
            payload: Some(proto::request::Payload::GetEpochBoundaryDataRequest(
                proto::GetEpochBoundaryDataRequest { epoch },
            )),
        };

        let request_bytes = request.encode_to_vec();
        let len_bytes = (request_bytes.len() as u32).to_be_bytes();
        stream.write_all(&len_bytes).await?;
        stream.write_all(&request_bytes).await?;
        stream.flush().await?;

        let read_timeout = Duration::from_secs(self.timeout_secs);
        let mut len_buf = [0u8; 4];
        timeout(read_timeout, stream.read_exact(&mut len_buf)).await??;
        let response_len = u32::from_be_bytes(len_buf) as usize;

        let mut response_buf = vec![0u8; response_len];
        timeout(read_timeout, stream.read_exact(&mut response_buf)).await??;

        let response = Response::decode(&response_buf[..])?;
        match response.payload {
            Some(proto::response::Payload::EpochBoundaryData(data)) => {
                let epoch_duration = if data.epoch_duration_seconds > 0 { data.epoch_duration_seconds } else { 900 };
                Ok((
                    data.epoch,
                    data.epoch_start_timestamp_ms,
                    data.boundary_block,
                    data.validators,
                    epoch_duration,
                ))
            },
            Some(proto::response::Payload::Error(e)) => Err(anyhow::anyhow!("Peer error: {}", e)),
            _ => Err(anyhow::anyhow!("Unexpected response type")),
        }
    }
}

/// Helper to pick best peer from a list
#[allow(dead_code)]
pub async fn pick_best_peer(peer_addrs: &[SocketAddr]) -> Option<(SocketAddr, u64)> {
    let mut best_peer = None;
    let mut best_block = 0u64;

    for addr in peer_addrs {
        let client = PeerGoClient::new(*addr);
        if let Ok(block_num) = client.get_last_block_number().await {
            if block_num > best_block {
                best_block = block_num;
                best_peer = Some(*addr);
            }
        }
    }

    best_peer.map(|addr| (addr, best_block))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_peer_client_from_str() {
        let client = PeerGoClient::from_str("127.0.0.1:9001").unwrap();
        assert_eq!(client.peer_addr.port(), 9001);
    }
}
