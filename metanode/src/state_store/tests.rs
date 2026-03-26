#[cfg(test)]
mod state_store_tests {
    use tempfile::TempDir;

    use crate::state_store::mdbx_store::MdbxStore;
    use crate::state_store::traits::StateStore;

    fn create_test_store() -> (MdbxStore, TempDir) {
        let dir = TempDir::new().expect("Failed to create temp dir");
        let store = MdbxStore::open(dir.path(), 1).expect("Failed to open MDBX");
        (store, dir)
    }

    #[test]
    fn test_put_get() {
        let (store, _dir) = create_test_store();

        store.put(b"key1", b"value1").unwrap();
        let result = store.get(b"key1").unwrap();
        assert_eq!(result, Some(b"value1".to_vec()));
    }

    #[test]
    fn test_get_nonexistent() {
        let (store, _dir) = create_test_store();

        let result = store.get(b"nonexistent").unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_put_overwrite() {
        let (store, _dir) = create_test_store();

        store.put(b"key1", b"value1").unwrap();
        store.put(b"key1", b"value2").unwrap();

        let result = store.get(b"key1").unwrap();
        assert_eq!(result, Some(b"value2".to_vec()));
    }

    #[test]
    fn test_delete() {
        let (store, _dir) = create_test_store();

        store.put(b"key1", b"value1").unwrap();
        store.delete(b"key1").unwrap();

        let result = store.get(b"key1").unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_delete_nonexistent() {
        let (store, _dir) = create_test_store();

        // Deleting a non-existent key should not error
        store.delete(b"nonexistent").unwrap();
    }

    #[test]
    fn test_batch_put() {
        let (store, _dir) = create_test_store();

        let pairs: Vec<(&[u8], &[u8])> = vec![
            (b"key1", b"val1"),
            (b"key2", b"val2"),
            (b"key3", b"val3"),
        ];
        store.batch_put(&pairs).unwrap();

        assert_eq!(store.get(b"key1").unwrap(), Some(b"val1".to_vec()));
        assert_eq!(store.get(b"key2").unwrap(), Some(b"val2".to_vec()));
        assert_eq!(store.get(b"key3").unwrap(), Some(b"val3".to_vec()));
    }

    #[test]
    fn test_batch_put_large() {
        let (store, _dir) = create_test_store();

        // 10K entries in a single batch
        let keys: Vec<Vec<u8>> = (0..10_000u32)
            .map(|i| format!("key_{:06}", i).into_bytes())
            .collect();
        let values: Vec<Vec<u8>> = (0..10_000u32)
            .map(|i| format!("value_{:06}", i).into_bytes())
            .collect();

        let pairs: Vec<(&[u8], &[u8])> = keys
            .iter()
            .zip(values.iter())
            .map(|(k, v)| (k.as_slice(), v.as_slice()))
            .collect();

        store.batch_put(&pairs).unwrap();

        // Verify first and last
        assert_eq!(
            store.get(b"key_000000").unwrap(),
            Some(b"value_000000".to_vec())
        );
        assert_eq!(
            store.get(b"key_009999").unwrap(),
            Some(b"value_009999".to_vec())
        );
    }

    #[test]
    fn test_prefix_scan() {
        let (store, _dir) = create_test_store();

        let pairs: Vec<(&[u8], &[u8])> = vec![
            (b"fs:account_1", b"data1"),
            (b"fs:account_2", b"data2"),
            (b"fs:account_3", b"data3"),
            (b"fb:bucket_0", b"hash0"),
            (b"other_key", b"other"),
        ];
        store.batch_put(&pairs).unwrap();

        let results = store.prefix_scan(b"fs:").unwrap();
        assert_eq!(results.len(), 3);

        // Keys should have prefix stripped
        assert_eq!(results[0].0, b"account_1".to_vec());
        assert_eq!(results[0].1, b"data1".to_vec());
        assert_eq!(results[1].0, b"account_2".to_vec());
        assert_eq!(results[2].0, b"account_3".to_vec());
    }

    #[test]
    fn test_prefix_scan_empty() {
        let (store, _dir) = create_test_store();

        let results = store.prefix_scan(b"nonexistent:").unwrap();
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn test_stats() {
        let (store, _dir) = create_test_store();

        store.put(b"key1", b"value1").unwrap();
        let stats = store.stats().unwrap();
        assert!(stats.contains("entries=1"));
    }

    #[test]
    fn test_clone_shares_db() {
        let (store, _dir) = create_test_store();

        store.put(b"key1", b"value1").unwrap();

        let store2 = store.clone();
        assert_eq!(store2.get(b"key1").unwrap(), Some(b"value1".to_vec()));

        // Write through clone is visible in original
        store2.put(b"key2", b"value2").unwrap();
        assert_eq!(store.get(b"key2").unwrap(), Some(b"value2".to_vec()));
    }

    #[test]
    fn test_batch_put_empty() {
        let (store, _dir) = create_test_store();
        // Empty batch should be a no-op
        store.batch_put(&[]).unwrap();
    }
}
