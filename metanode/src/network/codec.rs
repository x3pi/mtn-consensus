// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

use anyhow::Result;
use tokio::io::{AsyncRead, AsyncReadExt};

/// Maximum transaction size allowed (100MB to support up to 50k transactions in a single batch)
const MAX_FRAME_SIZE: usize = 100 * 1024 * 1024;

/// Read a length-prefixed frame from an async reader.
/// Format: [4 bytes length (big-endian)][length bytes of data]
///
/// Returns the data bytes if successful, or an error if:
/// - Length prefix cannot be read
/// - Length is invalid (0 or > MAX_FRAME_SIZE)
/// - Data cannot be read
pub async fn read_length_prefixed_frame<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Vec<u8>> {
    // Read length prefix (4 bytes, big-endian)
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).await?;
    let data_len = u32::from_be_bytes(len_buf) as usize;

    // Validate length
    if data_len == 0 {
        return Err(anyhow::anyhow!("Empty frame (length = 0)"));
    }
    if data_len > MAX_FRAME_SIZE {
        return Err(anyhow::anyhow!(
            "Frame too large: {} bytes (max: {})",
            data_len,
            MAX_FRAME_SIZE
        ));
    }

    // Read data
    let mut data = vec![0u8; data_len];
    reader.read_exact(&mut data).await?;
    Ok(data)
}

/// Read a length-prefixed frame with timeout.
/// This is a convenience function that wraps read_length_prefixed_frame with a timeout.
#[allow(dead_code)]
pub async fn read_length_prefixed_frame_with_timeout<R: AsyncRead + Unpin>(
    reader: &mut R,
    timeout_duration: std::time::Duration,
) -> Result<Vec<u8>> {
    tokio::time::timeout(timeout_duration, read_length_prefixed_frame(reader))
        .await
        .map_err(|_| anyhow::anyhow!("Timeout reading frame after {:?}", timeout_duration))?
}
