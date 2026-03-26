use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use libmdbx::{
    Database, DatabaseOptions, Mode, NoWriteMap, ReadWriteOptions, SyncMode, TableFlags,
    WriteFlags,
};
use tracing::{info, warn};

use super::traits::StateStore;

/// MdbxStore is a high-performance key-value store backed by MDBX.
///
/// MDBX uses memory-mapped I/O for zero-copy reads and B+tree structure
/// for O(log N) operations. Used by Erigon, Reth, and other major
/// blockchain clients.
///
/// Key features:
/// - Zero-copy reads via mmap (10-50x faster than LSM-tree DBs)
/// - No WAL → no maintenance, no crash recovery needed
/// - Wait-free parallel reads
/// - Copy-on-write for atomic writes
/// MdbxEnv manages the single MDBX environment for the process.
/// It allows creating multiple isolated MdbxStore tables.
pub struct MdbxEnv {
    db: Arc<Database<NoWriteMap>>,
}

pub struct MdbxStore {
    pub(crate) db: Arc<Database<NoWriteMap>>,
    pub(crate) table_name: String,
}

impl MdbxEnv {
    /// Open the MDBX environment at the given path.
    pub fn open(path: &Path, max_size_gb: usize) -> Result<Self> {
        std::fs::create_dir_all(path)
            .with_context(|| format!("Failed to create MDBX directory: {}", path.display()))?;

        let max_size_bytes = (max_size_gb * 1024 * 1024 * 1024) as isize;

        let db = Database::open_with_options(
            path,
            DatabaseOptions {
                max_tables: Some(16), // Enough for state + future tables
                mode: Mode::ReadWrite(ReadWriteOptions {
                    sync_mode: SyncMode::Durable,
                    min_size: None,
                    max_size: Some(max_size_bytes),
                    growth_step: Some(256 * 1024 * 1024), // 256MB growth steps
                    shrink_threshold: None,
                }),
                ..Default::default()
            },
        )
        .with_context(|| format!("Failed to open MDBX at {}", path.display()))?;

        info!(
            "📦 [MDBX] Opened environment at {} (max_size={}GB)",
            path.display(),
            max_size_gb
        );

        Ok(Self { db: Arc::new(db) })
    }

    /// Open or create a specific table store within the environment.
    pub fn open_store(&self, table_name: &str) -> Result<MdbxStore> {
        // Ensure the table exists
        {
            let tx = self.db.begin_rw_txn()?;
            tx.create_table(Some(table_name), TableFlags::default())?;
            tx.commit()?;
        }

        info!("📦 [MDBX] Opened table store: {}", table_name);

        Ok(MdbxStore {
            db: self.db.clone(),
            table_name: table_name.to_string(),
        })
    }
}

impl MdbxStore {
    /// Get database statistics (page size, depth, entries, etc.)
    pub fn stats(&self) -> Result<String> {
        let tx = self.db.begin_ro_txn()?;
        let table = tx.open_table(Some(self.table_name.as_str()))?;
        let stat = tx.table_stat(&table)?;
        Ok(format!(
            "MDBX stats ({}): entries={}, depth={}, page_size={}, branch_pages={}, leaf_pages={}, overflow_pages={}",
            self.table_name,
            stat.entries(),
            stat.depth(),
            stat.page_size(),
            stat.branch_pages(),
            stat.leaf_pages(),
            stat.overflow_pages(),
        ))
    }
}

impl StateStore for MdbxStore {
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>> {
        let tx = self.db.begin_ro_txn()?;
        let table = tx.open_table(Some(self.table_name.as_str()))?;

        match tx.get::<Vec<u8>>(&table, key) {
            Ok(Some(value)) => Ok(Some(value)),
            Ok(None) => Ok(None),
            Err(libmdbx::Error::NotFound) => Ok(None),
            Err(e) => Err(anyhow::anyhow!("MDBX get error: {}", e)),
        }
    }

    fn put(&self, key: &[u8], value: &[u8]) -> Result<()> {
        let tx = self.db.begin_rw_txn()?;
        let table = tx.open_table(Some(self.table_name.as_str()))?;
        tx.put(&table, key, value, WriteFlags::default())?;
        tx.commit()?;
        Ok(())
    }

    fn delete(&self, key: &[u8]) -> Result<()> {
        let tx = self.db.begin_rw_txn()?;
        let table = tx.open_table(Some(self.table_name.as_str()))?;
        match tx.del(&table, key, None) {
            Ok(_) => {}
            Err(libmdbx::Error::NotFound) => {} // Key doesn't exist, that's fine
            Err(e) => return Err(anyhow::anyhow!("MDBX delete error: {}", e)),
        }
        tx.commit()?;
        Ok(())
    }

    fn batch_put(&self, pairs: &[(&[u8], &[u8])]) -> Result<()> {
        if pairs.is_empty() {
            return Ok(());
        }

        let tx = self.db.begin_rw_txn()?;
        let table = tx.open_table(Some(self.table_name.as_str()))?;

        for (key, value) in pairs {
            tx.put(&table, *key, *value, WriteFlags::default())?;
        }

        tx.commit()?;
        Ok(())
    }

    fn prefix_scan(&self, prefix: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        let tx = self.db.begin_ro_txn()?;
        let table = tx.open_table(Some(self.table_name.as_str()))?;
        let mut cursor = tx.cursor(&table)?;
        let mut results = Vec::new();

        // Seek to the first key >= prefix
        let iter = cursor.iter_from::<Vec<u8>, Vec<u8>>(prefix);

        for item in iter {
            match item {
                Ok((key, value)) => {
                    if key.starts_with(prefix) {
                        // Strip prefix from key
                        let stripped_key = key[prefix.len()..].to_vec();
                        results.push((stripped_key, value));
                    } else {
                        break; // Past the prefix range
                    }
                }
                Err(e) => {
                    warn!("MDBX prefix_scan cursor error: {}", e);
                    break;
                }
            }
        }

        Ok(results)
    }

    fn flush(&self) -> Result<()> {
        // MDBX with Durable sync mode already flushes on commit.
        // This is a no-op for explicit flush requests.
        Ok(())
    }
}

impl Clone for MdbxStore {
    fn clone(&self) -> Self {
        Self {
            db: self.db.clone(),
            table_name: self.table_name.clone(),
        }
    }
}
