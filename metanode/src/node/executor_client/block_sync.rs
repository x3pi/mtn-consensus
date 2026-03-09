use anyhow::Result;
use prost::Message;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{info, warn};

use super::proto;
use super::socket_stream::SocketStream;
use super::ExecutorClient;

impl ExecutorClient {
    /// Get a range of blocks from Go Master
    /// Used by validators to serve blocks to SyncOnly nodes
    pub async fn get_blocks_range(
        &self,
        from_block: u64,
        to_block: u64,
    ) -> Result<Vec<proto::BlockData>> {
        if !self.is_enabled() {
            return Err(anyhow::anyhow!("Executor client is not enabled"));
        }

        info!(
            "📤 [BLOCK SYNC] Requesting blocks {} to {} from Go Master",
            from_block, to_block
        );

        let request = proto::Request {
            payload: Some(proto::request::Payload::GetBlocksRangeRequest(
                proto::GetBlocksRangeRequest {
                    from_block,
                    to_block,
                },
            )),
        };

        let request_bytes = request.encode_to_vec();

        let mut stream = SocketStream::connect(&self.request_socket_address, 5).await?;

        let len_bytes = (request_bytes.len() as u32).to_be_bytes();
        stream.write_all(&len_bytes).await?;
        stream.write_all(&request_bytes).await?;
        stream.flush().await?;

        // Read response with 60s timeout (Go may take a while to read+marshal historical blocks)
        let response_result = tokio::time::timeout(std::time::Duration::from_secs(60), async {
            let mut len_buf = [0u8; 4];
            stream.read_exact(&mut len_buf).await?;
            let response_len = u32::from_be_bytes(len_buf) as usize;

            let mut response_buf = vec![0u8; response_len];
            stream.read_exact(&mut response_buf).await?;

            Ok::<Vec<u8>, std::io::Error>(response_buf)
        })
        .await;

        let response_buf = match response_result {
            Ok(Ok(buf)) => buf,
            Ok(Err(e)) => return Err(anyhow::anyhow!("UDS read error: {}", e)),
            Err(_) => {
                warn!(
                    "⏱️ [BLOCK SYNC] Timeout (60s) reading blocks {}-{} from Go Master",
                    from_block, to_block
                );
                return Err(anyhow::anyhow!("Timeout reading blocks from Go Master"));
            }
        };

        let response: proto::Response = proto::Response::decode(&*response_buf)?;

        match response.payload {
            Some(proto::response::Payload::GetBlocksRangeResponse(resp)) => {
                if !resp.error.is_empty() {
                    return Err(anyhow::anyhow!("Go returned error: {}", resp.error));
                }
                info!(
                    "✅ [BLOCK SYNC] Received {} blocks from Go Master",
                    resp.count
                );
                Ok(resp.blocks)
            }
            Some(proto::response::Payload::Error(e)) => {
                Err(anyhow::anyhow!("Go Master error: {}", e))
            }
            _ => Err(anyhow::anyhow!("Unexpected response type from Go Master")),
        }
    }

    /// Sync blocks to local Go Master
    /// Used by SyncOnly nodes to write blocks received from peers
    pub async fn sync_blocks(&self, blocks: Vec<proto::BlockData>) -> Result<(u64, u64)> {
        if !self.is_enabled() {
            return Err(anyhow::anyhow!("Executor client is not enabled"));
        }

        if blocks.is_empty() {
            return Ok((0, 0));
        }

        let total_blocks = blocks.len();
        let first_block = blocks.first().map(|b| b.block_number).unwrap_or(0);
        let last_block = blocks.last().map(|b| b.block_number).unwrap_or(0);

        info!(
            "📤 [BLOCK SYNC] Syncing {} blocks ({} to {}) to Go Master in chunks",
            total_blocks, first_block, last_block
        );

        // Chunking to prevent hitting 32MB max message length limits on large block payloads
        const CHUNK_SIZE: usize = 5;
        let mut total_synced_count = 0u64;
        let mut final_synced_block = 0u64;

        for chunk_idx in (0..blocks.len()).step_by(CHUNK_SIZE) {
            let end_idx = std::cmp::min(chunk_idx + CHUNK_SIZE, blocks.len());
            let chunk = blocks[chunk_idx..end_idx].to_vec();
            let request = proto::Request {
                payload: Some(proto::request::Payload::SyncBlocksRequest(
                    proto::SyncBlocksRequest { blocks: chunk },
                )),
            };

            let request_bytes = request.encode_to_vec();

            let mut stream = SocketStream::connect(&self.request_socket_address, 5).await?;

            let len_bytes = (request_bytes.len() as u32).to_be_bytes();
            stream.write_all(&len_bytes).await?;
            stream.write_all(&request_bytes).await?;
            stream.flush().await?;

            let mut len_buf = [0u8; 4];
            stream.read_exact(&mut len_buf).await?;
            let response_len = u32::from_be_bytes(len_buf) as usize;

            let mut response_buf = vec![0u8; response_len];
            stream.read_exact(&mut response_buf).await?;

            let response: proto::Response = proto::Response::decode(&*response_buf)?;

            match response.payload {
                Some(proto::response::Payload::SyncBlocksResponse(resp)) => {
                    if !resp.error.is_empty() {
                        return Err(anyhow::anyhow!("Go returned error: {}", resp.error));
                    }
                    total_synced_count += resp.synced_count;
                    if resp.last_synced_block > final_synced_block {
                        final_synced_block = resp.last_synced_block;
                    }
                }
                Some(proto::response::Payload::Error(e)) => {
                    return Err(anyhow::anyhow!("Go Master error: {}", e));
                }
                _ => return Err(anyhow::anyhow!("Unexpected response type from Go Master")),
            }
        }

        info!(
            "✅ [BLOCK SYNC] Successfully synced {} blocks (last: {})",
            total_synced_count, final_synced_block
        );
        Ok((total_synced_count, final_synced_block))
    }
}
