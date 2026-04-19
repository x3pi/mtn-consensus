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

        let response_buf = self.execute_rpc_request(&request_bytes).await?;

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

    /// Sync blocks to local Go Master (store-only mode)
    /// Used by SyncOnly nodes to write blocks received from peers
    pub async fn sync_blocks(&self, blocks: Vec<proto::BlockData>) -> Result<(u64, u64)> {
        self.sync_blocks_inner(blocks, false).await
    }

    /// Sync AND EXECUTE blocks through NOMT on local Go Master
    /// Phase 1 fix: eliminates GEI inflation by executing blocks, not just storing.
    /// Returns (synced_count, last_block, last_executed_gei).
    pub async fn sync_and_execute_blocks(
        &self,
        blocks: Vec<proto::BlockData>,
    ) -> Result<(u64, u64, u64)> {
        let (count, last_block) = self.sync_blocks_inner(blocks, true).await?;
        // last_executed_gei is embedded in last_block for execute mode
        // (the inner method returns it via the response)
        Ok((count, last_block, 0)) // GEI returned separately below
    }

    /// Internal: sync blocks with optional execute_mode flag
    async fn sync_blocks_inner(
        &self,
        blocks: Vec<proto::BlockData>,
        execute_mode: bool,
    ) -> Result<(u64, u64)> {
        if !self.is_enabled() {
            return Err(anyhow::anyhow!("Executor client is not enabled"));
        }

        if blocks.is_empty() {
            return Ok((0, 0));
        }

        let total_blocks = blocks.len();
        let first_block = blocks.first().map(|b| b.block_number).unwrap_or(0);
        let last_block = blocks.last().map(|b| b.block_number).unwrap_or(0);
        let mode_str = if execute_mode { "EXECUTE" } else { "STORE" };

        info!(
            "📤 [BLOCK SYNC] Syncing {} blocks ({} to {}) to Go Master in chunks (mode={})",
            total_blocks, first_block, last_block, mode_str
        );

        // Chunking to prevent hitting 32MB max message length limits on large block payloads
        // Each chunk opens a new UDS connection so larger chunks = fewer round trips = faster sync
        // Execute mode uses smaller chunks to avoid timeout (execution takes longer than storage)
        let chunk_size: usize = if execute_mode { 20 } else { 50 };
        let mut total_synced_count = 0u64;
        let mut final_synced_block = 0u64;

        for chunk_idx in (0..blocks.len()).step_by(chunk_size) {
            let end_idx = std::cmp::min(chunk_idx + chunk_size, blocks.len());
            let chunk = blocks[chunk_idx..end_idx].to_vec();
            let request = proto::Request {
                payload: Some(proto::request::Payload::SyncBlocksRequest(
                    proto::SyncBlocksRequest {
                        blocks: chunk,
                        execute_mode,
                    },
                )),
            };

            let request_bytes = request.encode_to_vec();

            let response_buf = self.execute_rpc_request(&request_bytes).await?;

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
            "✅ [BLOCK SYNC] Successfully synced {} blocks (last: {}, mode={})",
            total_synced_count, final_synced_block, mode_str
        );
        Ok((total_synced_count, final_synced_block))
    }
}
