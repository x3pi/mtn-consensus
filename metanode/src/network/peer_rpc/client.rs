// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

//! Peer RPC client functions — query remote peers over TCP/HTTP.
//!
//! Used for epoch discovery, fetching boundary data from peers, and
//! forwarding transactions from SyncOnly nodes to validators.

use anyhow::Result;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{info, warn};

use super::types::*;

// Re-export executor proto for block data types
use crate::node::executor_client::proto::BlockData;

/// Query peer info from a remote node via HTTP
pub async fn query_peer_info(peer_address: &str) -> Result<PeerInfoResponse> {
    use tokio::net::TcpStream;

    // Connect with timeout
    let mut stream = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        TcpStream::connect(peer_address),
    )
    .await
    .map_err(|_| anyhow::anyhow!("Connection timeout to {}", peer_address))?
    .map_err(|e| anyhow::anyhow!("Failed to connect to {}: {}", peer_address, e))?;

    // Send HTTP GET request
    let request = format!(
        "GET /peer_info HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        peer_address
    );
    stream.write_all(request.as_bytes()).await?;

    // Read response with timeout
    let mut buffer = Vec::new();
    let mut temp = [0u8; 4096];
    let read_result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            match stream.read(&mut temp).await {
                Ok(0) => break,
                Ok(n) => buffer.extend_from_slice(&temp[..n]),
                Err(e) => return Err(e),
            }
        }
        Ok(())
    })
    .await;

    match read_result {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => return Err(anyhow::anyhow!("Failed to read response: {}", e)),
        Err(_) => {
            return Err(anyhow::anyhow!(
                "Timeout reading response from {}",
                peer_address
            ))
        }
    }

    // Parse HTTP response
    let response_str = String::from_utf8_lossy(&buffer);

    // Find JSON body (after empty line)
    let body_start = response_str
        .find("\r\n\r\n")
        .map(|i| i + 4)
        .or_else(|| response_str.find("\n\n").map(|i| i + 2))
        .unwrap_or(0);

    let body = &response_str[body_start..];

    // Parse JSON
    let info: PeerInfoResponse = serde_json::from_str(body.trim()).map_err(|e| {
        anyhow::anyhow!(
            "Failed to parse peer info JSON: {} (body: {})",
            e,
            body.trim()
        )
    })?;

    Ok(info)
}

/// Query multiple peers and return the best one (highest epoch/block/global_exec_index)
pub async fn query_peer_epochs_network(
    peer_addresses: &[String],
) -> Result<(u64, u64, String, u64)> {
    info!(
        "🌐 [PEER RPC] Querying {} peer(s) over network for epoch discovery...",
        peer_addresses.len()
    );

    let mut best_epoch = 0u64;
    let mut best_block = 0u64;
    let mut best_address = String::new();
    let mut best_global_exec_index = 0u64;

    for peer_addr in peer_addresses {
        match query_peer_info(peer_addr).await {
            Ok(info) => {
                info!(
                    "🌐 [PEER RPC] Peer ({}): epoch={}, block={}, global_exec_index={}",
                    peer_addr, info.epoch, info.last_block, info.last_global_exec_index
                );

                // Use this peer if it has higher epoch, or same epoch and higher global_exec_index
                if best_address.is_empty()
                    || info.epoch > best_epoch
                    || (info.epoch == best_epoch
                        && info.last_global_exec_index > best_global_exec_index)
                {
                    best_epoch = info.epoch;
                    best_block = info.last_block;
                    best_global_exec_index = info.last_global_exec_index;
                    best_address = peer_addr.clone();
                    info!(
                        "🌐 [PEER RPC] New best peer: epoch={} block={} global_exec_index={} from {}",
                        best_epoch, best_block, best_global_exec_index, peer_addr
                    );
                }
            }
            Err(e) => {
                warn!("🌐 [PEER RPC] Failed to query peer ({}): {}", peer_addr, e);
            }
        }
    }

    if best_address.is_empty() {
        return Err(anyhow::anyhow!("No reachable peers found"));
    }

    info!(
        "🌐 [PEER RPC] Best peer found: epoch={} block={} global_exec_index={} from {}",
        best_epoch, best_block, best_global_exec_index, best_address
    );

    Ok((best_epoch, best_block, best_address, best_global_exec_index))
}

/// Query epoch boundary data from a remote peer via HTTP
/// This is used by late-joining validators to get epoch boundary data from peers
/// who have already witnessed the epoch transition
pub async fn query_peer_epoch_boundary_data(
    peer_address: &str,
    epoch: u64,
) -> Result<EpochBoundaryDataResponse> {
    use tokio::net::TcpStream;

    info!(
        "🌐 [PEER RPC] Querying epoch boundary data for epoch {} from {}",
        epoch, peer_address
    );

    // Connect with timeout
    let mut stream = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        TcpStream::connect(peer_address),
    )
    .await
    .map_err(|_| anyhow::anyhow!("Connection timeout to {}", peer_address))?
    .map_err(|e| anyhow::anyhow!("Failed to connect to {}: {}", peer_address, e))?;

    // Send HTTP GET request
    let request = format!(
        "GET /get_epoch_boundary_data?epoch={} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        epoch, peer_address
    );
    stream.write_all(request.as_bytes()).await?;

    // Read response with timeout
    let mut buffer = Vec::new();
    let mut temp = [0u8; 16384]; // Larger buffer for validator data
    let read_result = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            match stream.read(&mut temp).await {
                Ok(0) => break,
                Ok(n) => buffer.extend_from_slice(&temp[..n]),
                Err(e) => return Err(e),
            }
        }
        Ok(())
    })
    .await;

    match read_result {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => return Err(anyhow::anyhow!("Failed to read response: {}", e)),
        Err(_) => {
            return Err(anyhow::anyhow!(
                "Timeout reading response from {}",
                peer_address
            ))
        }
    }

    // Parse HTTP response
    let response_str = String::from_utf8_lossy(&buffer);

    // Find JSON body (after empty line)
    let body_start = response_str
        .find("\r\n\r\n")
        .map(|i| i + 4)
        .or_else(|| response_str.find("\n\n").map(|i| i + 2))
        .unwrap_or(0);

    let body = &response_str[body_start..];

    // Parse JSON
    let response: EpochBoundaryDataResponse = serde_json::from_str(body.trim()).map_err(|e| {
        anyhow::anyhow!(
            "Failed to parse epoch boundary data JSON: {} (body: {})",
            e,
            body.trim()
        )
    })?;

    // Check for error in response
    if let Some(error) = &response.error {
        return Err(anyhow::anyhow!("Peer returned error: {}", error));
    }

    info!(
        "🌐 [PEER RPC] Received epoch boundary data from {}: epoch={}, timestamp={}, boundary_block={}, validators={}",
        peer_address, response.epoch, response.timestamp_ms, response.boundary_block, response.validators.len()
    );

    Ok(response)
}

/// Forward transaction to all validator peers. Returns number of successful peers.
pub async fn broadcast_transaction_to_validators(
    peer_addresses: &[String],
    tx_batch: &[Vec<u8>],
) -> Result<usize> {
    use futures::future::join_all;
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpStream;

    if peer_addresses.is_empty() {
        return Ok(0);
    }

    let tx_hexes: Vec<String> = tx_batch.iter().map(|t| hex::encode(t)).collect();
    let request_body = serde_json::to_string(&SubmitTransactionRequest {
        transactions_hex: tx_hexes,
    })?;

    let mut tasks = Vec::new();
    let body_len = request_body.len();

    for peer_addr in peer_addresses {
        let addr = peer_addr.clone();
        let body = request_body.clone();

        let task = tokio::spawn(async move {
            let stream_result =
                tokio::time::timeout(std::time::Duration::from_secs(3), TcpStream::connect(&addr))
                    .await;

            let mut stream = match stream_result {
                Ok(Ok(s)) => s,
                _ => return false,
            };

            let http_request = format!(
                "POST /submit_transaction HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                addr,
                body_len,
                body
            );

            if stream.write_all(http_request.as_bytes()).await.is_err() {
                return false;
            }

            let mut buffer = [0u8; 1024];
            let read_result =
                tokio::time::timeout(std::time::Duration::from_secs(3), stream.read(&mut buffer))
                    .await;

            if let Ok(Ok(n)) = read_result {
                let response_str = String::from_utf8_lossy(&buffer[..n]);
                // Simplified fast check: "200 OK"
                response_str.contains("200 OK")
            } else {
                false
            }
        });
        tasks.push(task);
    }

    let results = join_all(tasks).await;
    let success_count = results
        .into_iter()
        .filter(|r| matches!(r, Ok(true)))
        .count();
    Ok(success_count)
}

/// Forward transaction to validator nodes via HTTP POST
/// This is used by SyncOnly nodes to forward transactions to validators for consensus
/// Uses round-robin retry logic for fault tolerance
pub async fn forward_transaction_to_validators(
    peer_addresses: &[String],
    tx_batch: &[Vec<u8>],
) -> Result<SubmitTransactionResponse> {
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpStream;

    if peer_addresses.is_empty() {
        return Err(anyhow::anyhow!(
            "No peer addresses configured for forwarding"
        ));
    }

    let tx_hexes: Vec<String> = tx_batch.iter().map(|t| hex::encode(t)).collect();
    let request_body = serde_json::to_string(&SubmitTransactionRequest {
        transactions_hex: tx_hexes,
    })?;

    // Round-robin through peers until one succeeds
    for peer_addr in peer_addresses {
        info!(
            "🔄 [TX FORWARD] Attempting to forward transaction to validator: {}",
            peer_addr
        );

        // Connect with timeout
        let stream_result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            TcpStream::connect(peer_addr),
        )
        .await;

        let mut stream = match stream_result {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                warn!("🔄 [TX FORWARD] Failed to connect to {}: {}", peer_addr, e);
                continue;
            }
            Err(_) => {
                warn!("🔄 [TX FORWARD] Timeout connecting to {}", peer_addr);
                continue;
            }
        };

        // Build HTTP POST request
        let http_request = format!(
            "POST /submit_transaction HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            peer_addr,
            request_body.len(),
            request_body
        );

        // Send request
        if let Err(e) = stream.write_all(http_request.as_bytes()).await {
            warn!(
                "🔄 [TX FORWARD] Failed to send request to {}: {}",
                peer_addr, e
            );
            continue;
        }

        // Read response with timeout
        let mut buffer = [0u8; 4096];
        let read_result =
            tokio::time::timeout(std::time::Duration::from_secs(10), stream.read(&mut buffer))
                .await;

        let n = match read_result {
            Ok(Ok(n)) => n,
            Ok(Err(e)) => {
                warn!(
                    "🔄 [TX FORWARD] Failed to read response from {}: {}",
                    peer_addr, e
                );
                continue;
            }
            Err(_) => {
                warn!(
                    "🔄 [TX FORWARD] Timeout reading response from {}",
                    peer_addr
                );
                continue;
            }
        };

        let response_str = String::from_utf8_lossy(&buffer[..n]);

        // Parse JSON body from HTTP response
        let body_start = response_str
            .find("\r\n\r\n")
            .map(|i| i + 4)
            .or_else(|| response_str.find("\n\n").map(|i| i + 2))
            .unwrap_or(0);

        let body = &response_str[body_start..];

        match serde_json::from_str::<SubmitTransactionResponse>(body.trim()) {
            Ok(resp) => {
                if resp.success {
                    info!(
                        "✅ [TX FORWARD] Successfully forwarded transaction to {}",
                        peer_addr
                    );
                    return Ok(resp);
                } else {
                    warn!(
                        "🔄 [TX FORWARD] Validator {} rejected transaction: {:?}",
                        peer_addr, resp.error
                    );
                    continue;
                }
            }
            Err(e) => {
                warn!(
                    "🔄 [TX FORWARD] Failed to parse response from {}: {}",
                    peer_addr, e
                );
                continue;
            }
        }
    }

    Err(anyhow::anyhow!(
        "Failed to forward transaction to any validator"
    ))
}

/// Fetch blocks from a peer node via HTTP /get_blocks endpoint.
/// Batches requests by 100 blocks (server-side limit).
/// Returns Vec<BlockData> ready for sync_blocks().
///
/// Used by Rust to orchestrate block sync during cross-epoch catch-up:
/// peer Go master → peer Rust → this function → local sync_blocks() → local Go master
pub async fn fetch_blocks_from_peer(
    peer_addresses: &[String],
    from_block: u64,
    to_block: u64,
) -> Result<Vec<BlockData>> {
    if peer_addresses.is_empty() {
        return Err(anyhow::anyhow!("No peer addresses configured"));
    }

    let total_blocks = to_block.saturating_sub(from_block) + 1;
    info!(
        "🔄 [BLOCK-FETCH] Fetching {} blocks ({} to {}) from {} peer(s)",
        total_blocks,
        from_block,
        to_block,
        peer_addresses.len()
    );

    let mut all_blocks = Vec::new();
    let batch_size = 5u64; // Smaller batches to avoid 1GB+ JSON payloads
    let mut current_from = from_block;

    while current_from <= to_block {
        let current_to = std::cmp::min(current_from + batch_size - 1, to_block);
        let mut batch_fetched = false;

        // Try each peer until one succeeds for this batch
        for peer_addr in peer_addresses {
            match fetch_block_batch(peer_addr, current_from, current_to).await {
                Ok(blocks) => {
                    if blocks.is_empty() {
                        info!(
                            "✅ [BLOCK-FETCH] Got 0 blocks ({}-{}) from peer {}",
                            current_from, current_to, peer_addr
                        );
                        continue; // Try next peer
                    }
                    info!(
                        "✅ [BLOCK-FETCH] Got {} blocks ({}-{}) from peer {}",
                        blocks.len(),
                        current_from,
                        current_to,
                        peer_addr
                    );
                    all_blocks.extend(blocks);
                    batch_fetched = true;
                    break;
                }
                Err(e) => {
                    warn!(
                        "⚠️ [BLOCK-FETCH] Peer {} failed for blocks {}-{}: {}",
                        peer_addr, current_from, current_to, e
                    );
                    continue;
                }
            }
        }

        if !batch_fetched {
            warn!(
                "⚠️ [BLOCK-FETCH] All peers failed for batch {}-{}. Returning {} blocks fetched so far.",
                current_from, current_to, all_blocks.len()
            );
            break;
        }

        // Yield execution to allow other async tasks (like socket listeners) to run
        tokio::task::yield_now().await;
        
        current_from = current_to + 1;
    }

    info!(
        "📦 [BLOCK-FETCH] Total: {} blocks fetched ({} to {})",
        all_blocks.len(),
        from_block,
        from_block + all_blocks.len() as u64 - 1
    );

    Ok(all_blocks)
}

/// Fetch a single batch of blocks from one peer via HTTP
async fn fetch_block_batch(
    peer_addr: &str,
    from_block: u64,
    to_block: u64,
) -> Result<Vec<BlockData>> {
    use prost::Message;
    use tokio::net::TcpStream;

    // Connect with timeout
    let mut stream = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        TcpStream::connect(peer_addr),
    )
    .await
    .map_err(|_| anyhow::anyhow!("Connection timeout to {}", peer_addr))?
    .map_err(|e| anyhow::anyhow!("Failed to connect to {}: {}", peer_addr, e))?;

    // Send HTTP GET request
    let request = format!(
        "GET /get_blocks?from={}&to={} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        from_block, to_block, peer_addr
    );
    stream.write_all(request.as_bytes()).await?;

    // Read response with timeout (block data can be large)
    let mut buffer = Vec::new();
    let mut temp = [0u8; 65536]; // 64KB buffer for block data
    let read_result = tokio::time::timeout(std::time::Duration::from_secs(300), async {
        loop {
            match stream.read(&mut temp).await {
                Ok(0) => break,
                Ok(n) => buffer.extend_from_slice(&temp[..n]),
                Err(e) => return Err(e),
            }
        }
        Ok(())
    })
    .await;

    match read_result {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => return Err(anyhow::anyhow!("Failed to read response: {}", e)),
        Err(_) => {
            return Err(anyhow::anyhow!(
                "Timeout reading response from {}",
                peer_addr
            ))
        }
    }

    // Parse HTTP response
    let response_str = String::from_utf8_lossy(&buffer);

    // Find JSON body
    let body_start = response_str
        .find("\r\n\r\n")
        .map(|i| i + 4)
        .or_else(|| response_str.find("\n\n").map(|i| i + 2))
        .unwrap_or(0);

    let body = &response_str[body_start..];

    // Parse GetBlocksResponse JSON
    let response: GetBlocksResponse = serde_json::from_str(body.trim())
        .map_err(|e| anyhow::anyhow!("Failed to parse blocks response JSON: {}", e))?;

    if let Some(error) = &response.error {
        return Err(anyhow::anyhow!("Peer returned error: {}", error));
    }

    // Decode protobuf-encoded BlockData from hex strings
    let mut blocks = Vec::with_capacity(response.blocks.len());
    for (block_num, hex_data) in &response.blocks {
        let proto_bytes = hex::decode(hex_data)
            .map_err(|e| anyhow::anyhow!("Failed to decode hex for block {}: {}", block_num, e))?;
        let block_data = BlockData::decode(&proto_bytes[..]).map_err(|e| {
            anyhow::anyhow!("Failed to decode protobuf for block {}: {}", block_num, e)
        })?;
        blocks.push(block_data);
    }

    // Sort blocks by block_number for sequential processing
    blocks.sort_by_key(|b| b.block_number);

    Ok(blocks)
}
