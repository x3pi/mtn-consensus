// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

//! Simple file-based storage for ExecutableBlock protobuf bytes.
//!
//! After sending ExecutableBlocks to Go Master, we persist them here so that
//! peer sync nodes can fetch them directly from Rust — no Go PebbleDB needed.
//!
//! Storage layout:
//!   {storage_path}/executable_blocks/{global_exec_index}.bin
//!
//! Each file contains the raw protobuf-encoded ExecutableBlock bytes,
//! exactly as sent to Go Master via dataChan.

use anyhow::Result;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

/// Directory name under storage_path for executable blocks
const BLOCKS_DIR: &str = "executable_blocks";

/// Persist a single ExecutableBlock's protobuf bytes to disk.
/// Called after successful send to Go Master.
pub async fn store_executable_block(
    storage_path: &Path,
    global_exec_index: u64,
    data: &[u8],
) -> Result<()> {
    if data.is_empty() {
        return Ok(()); // Skip empty blocks (no-op commits)
    }

    let dir = storage_path.join(BLOCKS_DIR);
    tokio::fs::create_dir_all(&dir).await?;

    let file_path = dir.join(format!("{}.bin", global_exec_index));
    tokio::fs::write(&file_path, data).await?;

    debug!(
        "💾 [BLOCK STORE] Stored executable block GEI={} ({} bytes)",
        global_exec_index,
        data.len()
    );
    Ok(())
}

/// Persist a batch of ExecutableBlocks to disk.
/// More efficient than storing one at a time.
pub async fn store_executable_blocks_batch(
    storage_path: &Path,
    blocks: &[(u64, &[u8])], // (global_exec_index, data)
) -> Result<()> {
    if blocks.is_empty() {
        return Ok(());
    }

    let dir = storage_path.join(BLOCKS_DIR);
    tokio::fs::create_dir_all(&dir).await?;

    let mut stored = 0u64;
    for (gei, data) in blocks {
        if data.is_empty() {
            continue;
        }
        let file_path = dir.join(format!("{}.bin", gei));
        tokio::fs::write(&file_path, data).await?;
        stored += 1;
    }

    if stored > 0 {
        let first_gei = blocks.first().map(|(g, _)| *g).unwrap_or(0);
        let last_gei = blocks.last().map(|(g, _)| *g).unwrap_or(0);
        info!(
            "💾 [BLOCK STORE] Stored {} executable blocks (GEI {}→{})",
            stored, first_gei, last_gei
        );
    }
    Ok(())
}

/// Load a single ExecutableBlock's protobuf bytes from disk.
pub async fn load_executable_block(
    storage_path: &Path,
    global_exec_index: u64,
) -> Result<Vec<u8>> {
    let file_path = storage_path
        .join(BLOCKS_DIR)
        .join(format!("{}.bin", global_exec_index));
    let data = tokio::fs::read(&file_path).await?;
    Ok(data)
}

/// Load a range of ExecutableBlocks from disk.
/// Returns Vec of (global_exec_index, data) for blocks that exist.
/// Missing blocks in the range are skipped (logged as warnings).
pub async fn load_executable_blocks_range(
    storage_path: &Path,
    from_gei: u64,
    to_gei: u64,
) -> Result<Vec<(u64, Vec<u8>)>> {
    let dir = storage_path.join(BLOCKS_DIR);
    let mut blocks = Vec::new();

    for gei in from_gei..=to_gei {
        let file_path = dir.join(format!("{}.bin", gei));
        match tokio::fs::read(&file_path).await {
            Ok(data) => {
                blocks.push((gei, data));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                warn!(
                    "⚠️ [BLOCK STORE] Block GEI={} not found in store (skipping)",
                    gei
                );
            }
            Err(e) => {
                warn!(
                    "⚠️ [BLOCK STORE] Failed to read block GEI={}: {}",
                    gei, e
                );
            }
        }
    }

    info!(
        "📦 [BLOCK STORE] Loaded {}/{} blocks (GEI {}→{})",
        blocks.len(),
        to_gei - from_gei + 1,
        from_gei,
        to_gei
    );
    Ok(blocks)
}

/// Get the path to the executable blocks directory
pub fn blocks_dir(storage_path: &Path) -> PathBuf {
    storage_path.join(BLOCKS_DIR)
}

/// Get the highest GEI stored on disk (for sync peers to know what's available)
pub async fn get_max_stored_gei(storage_path: &Path) -> Result<Option<u64>> {
    let dir = storage_path.join(BLOCKS_DIR);
    if !dir.exists() {
        return Ok(None);
    }

    let mut max_gei: Option<u64> = None;
    let mut entries = tokio::fs::read_dir(&dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        if let Some(name) = entry.file_name().to_str() {
            if let Some(stem) = name.strip_suffix(".bin") {
                if let Ok(gei) = stem.parse::<u64>() {
                    max_gei = Some(max_gei.map_or(gei, |m: u64| m.max(gei)));
                }
            }
        }
    }

    Ok(max_gei)
}
