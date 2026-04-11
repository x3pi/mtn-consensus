// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

//! Persistence helpers for crash recovery of executor state.

use anyhow::Result;
use std::path::Path;
use tracing::{info, trace, warn};

/// Write uvarint to buffer (Go's binary.ReadUvarint format)
pub fn write_uvarint(buf: &mut Vec<u8>, mut value: u64) -> Result<()> {
    use std::io::Write;
    loop {
        let mut b = (value & 0x7F) as u8;
        value >>= 7;
        if value != 0 {
            b |= 0x80;
        }
        Write::write_all(buf, &[b])?;
        if value == 0 {
            break;
        }
    }
    Ok(())
}

// Persist last successfully sent index AND commit_index to file for crash recovery
// Uses atomic write (temp file + rename) to prevent corruption
pub async fn persist_last_sent_index(
    storage_path: &Path,
    index: u64,
    commit_index: u32,
) -> Result<()> {
    use tokio::io::AsyncWriteExt;

    let persist_dir = storage_path.join("executor_state");
    std::fs::create_dir_all(&persist_dir)?;

    let temp_path = persist_dir.join("last_sent_index.tmp");
    let final_path = persist_dir.join("last_sent_index.bin");

    // Write to temp file
    let mut file = tokio::fs::File::create(&temp_path).await?;
    // Format: [global_exec_index: u64][commit_index: u32]
    file.write_all(&index.to_le_bytes()).await?;
    file.write_all(&commit_index.to_le_bytes()).await?;
    file.flush().await?;
    file.sync_all().await?;
    drop(file);

    // Atomic rename
    std::fs::rename(&temp_path, &final_path)?;

    trace!(
        "💾 [PERSIST] Saved last_sent_index={}, commit_index={} to {:?}",
        index,
        commit_index,
        final_path
    );
    Ok(())
}

// Persist the last block number retrieved from Go for crash recovery
pub async fn persist_last_block_number(storage_path: &Path, block_number: u64) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    let persist_dir = storage_path.join("executor_state");
    std::fs::create_dir_all(&persist_dir)?;
    let temp_path = persist_dir.join("last_block_number.tmp");
    let final_path = persist_dir.join("last_block_number.bin");
    let mut file = tokio::fs::File::create(&temp_path).await?;
    file.write_all(&block_number.to_le_bytes()).await?;
    file.flush().await?;
    file.sync_all().await?;
    drop(file);
    std::fs::rename(&temp_path, &final_path)?;
    trace!(
        "💾 [PERSIST] Saved last_block_number={} to {:?}",
        block_number,
        final_path
    );
    Ok(())
}

// Read persisted last block number, if any
pub async fn read_last_block_number(storage_path: &Path) -> Result<u64> {
    use tokio::io::AsyncReadExt;
    let persist_dir = storage_path.join("executor_state");
    let final_path = persist_dir.join("last_block_number.bin");
    let mut file = tokio::fs::File::open(&final_path).await?;
    let mut buf = [0u8; 8];
    file.read_exact(&mut buf).await?;
    Ok(u64::from_le_bytes(buf))
}

/// RS-2: Persist cumulative_fragment_offset to disk for crash recovery.
/// During block fragmentation (commits with >MAX_TXS_PER_GO_BLOCK TXs), the offset
/// accumulates extra GEIs. If a node crashes mid-epoch, this offset MUST be recovered
/// to compute correct GEIs — otherwise the node diverges (fork).
/// Uses atomic write (temp + rename) for corruption safety.
pub async fn persist_fragment_offset(storage_path: &Path, offset: u64) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    let persist_dir = storage_path.join("executor_state");
    std::fs::create_dir_all(&persist_dir)?;
    let temp_path = persist_dir.join("fragment_offset.tmp");
    let final_path = persist_dir.join("fragment_offset.bin");
    let mut file = tokio::fs::File::create(&temp_path).await?;
    file.write_all(&offset.to_le_bytes()).await?;
    file.flush().await?;
    file.sync_all().await?;
    drop(file);
    std::fs::rename(&temp_path, &final_path)?;
    trace!(
        "💾 [PERSIST] Saved fragment_offset={} to {:?}",
        offset,
        final_path
    );
    Ok(())
}

/// Reset/Delete persisted fragment offset from disk.
/// Call this during epoch transitions to prevent carrying over the offset into the new epoch.
pub async fn reset_fragment_offset(storage_path: &Path) -> Result<()> {
    let persist_path = storage_path
        .join("executor_state")
        .join("fragment_offset.bin");

    if persist_path.exists() {
        tokio::fs::remove_file(&persist_path).await?;
        info!("📂 [PERSIST] Reset/Removed fragment_offset file for new epoch.");
    }
    Ok(())
}

/// RS-2: Load persisted fragment offset from disk.
/// Returns 0 if file doesn't exist (first start or clean epoch).
pub fn load_fragment_offset(storage_path: &Path) -> u64 {
    let persist_path = storage_path
        .join("executor_state")
        .join("fragment_offset.bin");

    if !persist_path.exists() {
        return 0;
    }

    match std::fs::read(&persist_path) {
        Ok(bytes) if bytes.len() == 8 => {
            let offset = u64::from_le_bytes([
                bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            ]);
            info!(
                "📂 [PERSIST] Loaded persisted fragment_offset={} from {:?}",
                offset, persist_path
            );
            offset
        }
        Ok(bytes) => {
            warn!(
                "⚠️ [PERSIST] Corrupted fragment_offset file: {} bytes (expected 8)",
                bytes.len()
            );
            0
        }
        Err(e) => {
            warn!("⚠️ [PERSIST] Failed to read fragment_offset: {}", e);
            0
        }
    }
}

/// Load persisted last sent index from file
/// Returns None if file doesn't exist or is corrupted
/// Returns (global_exec_index, commit_index)
pub fn load_persisted_last_index(storage_path: &Path) -> Option<(u64, u32)> {
    let persist_path = storage_path
        .join("executor_state")
        .join("last_sent_index.bin");

    if !persist_path.exists() {
        return None;
    }

    match std::fs::read(&persist_path) {
        Ok(bytes) => {
            if bytes.len() == 12 {
                // New format: u64 + u32
                let index = u64::from_le_bytes([
                    bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
                ]);
                let commit = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
                info!(
                    "📂 [PERSIST] Loaded persisted last_sent_index={}, commit_index={} from {:?}",
                    index, commit, persist_path
                );
                Some((index, commit))
            } else if bytes.len() == 8 {
                // Legacy format: u64 only (commit_index assumed 0 or unknown)
                let index = u64::from_le_bytes([
                    bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
                ]);
                warn!(
                    "⚠️ [PERSIST] Legacy format detected (only u64). Defaulting commit_index to 0."
                );
                Some((index, 0))
            } else {
                warn!(
                    "⚠️ [PERSIST] Corrupted last_sent_index file: {} bytes (expected 8 or 12)",
                    bytes.len()
                );
                None
            }
        }
        Err(e) => {
            warn!("⚠️ [PERSIST] Failed to read last_sent_index: {}", e);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_write_uvarint_small_value() {
        let mut buf = Vec::new();
        write_uvarint(&mut buf, 42).unwrap();
        assert_eq!(buf, vec![42u8]); // Values < 128 fit in one byte
    }

    #[test]
    fn test_write_uvarint_large_value() {
        let mut buf = Vec::new();
        write_uvarint(&mut buf, 300).unwrap();
        // 300 = 0b100101100
        // First byte: 0b00101100 | 0x80 = 0xAC
        // Second byte: 0b00000010 = 0x02
        assert_eq!(buf, vec![0xAC, 0x02]);
    }

    #[tokio::test]
    async fn test_persist_and_load_last_index() {
        let dir = tempdir().unwrap();
        let path = dir.path();

        persist_last_sent_index(path, 12345, 67).await.unwrap();

        let result = load_persisted_last_index(path);
        assert_eq!(result, Some((12345, 67)));
    }

    #[test]
    fn test_load_corrupted_file() {
        let dir = tempdir().unwrap();
        let persist_dir = dir.path().join("executor_state");
        std::fs::create_dir_all(&persist_dir).unwrap();
        let file_path = persist_dir.join("last_sent_index.bin");
        std::fs::write(&file_path, [1, 2, 3]).unwrap(); // 3 bytes = corrupted

        let result = load_persisted_last_index(dir.path());
        assert_eq!(result, None);
    }

    #[test]
    fn test_load_legacy_format() {
        let dir = tempdir().unwrap();
        let persist_dir = dir.path().join("executor_state");
        std::fs::create_dir_all(&persist_dir).unwrap();
        let file_path = persist_dir.join("last_sent_index.bin");
        std::fs::write(&file_path, 999u64.to_le_bytes()).unwrap(); // 8 bytes = legacy

        let result = load_persisted_last_index(dir.path());
        assert_eq!(result, Some((999, 0))); // commit_index defaults to 0
    }

    #[tokio::test]
    async fn test_persist_and_read_block_number() {
        let dir = tempdir().unwrap();
        let path = dir.path();

        persist_last_block_number(path, 42069).await.unwrap();

        let result = read_last_block_number(path).await.unwrap();
        assert_eq!(result, 42069);
    }
}
