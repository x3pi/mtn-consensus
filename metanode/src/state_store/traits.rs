use anyhow::Result;

/// StateStore defines the key-value storage interface for the state layer.
/// Implementations: MdbxStore (MDBX), and future backends.
pub trait StateStore: Send + Sync {
    /// Get the value for a key. Returns None if key not found.
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>>;

    /// Put a single key-value pair.
    fn put(&self, key: &[u8], value: &[u8]) -> Result<()>;

    /// Delete a key.
    fn delete(&self, key: &[u8]) -> Result<()>;

    /// Put multiple key-value pairs atomically in a single transaction.
    fn batch_put(&self, pairs: &[(&[u8], &[u8])]) -> Result<()>;

    /// Scan all key-value pairs with the given prefix.
    /// Returns pairs with the prefix stripped from keys.
    fn prefix_scan(&self, prefix: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>>;

    /// Flush/sync to disk (for backends that buffer writes).
    fn flush(&self) -> Result<()>;
}
