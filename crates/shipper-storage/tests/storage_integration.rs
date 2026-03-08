//! Integration tests for `shipper-storage` FileStorage backend.

use shipper_storage::{
    CloudStorageConfig, FileStorage, StorageBackend, StorageType, build_storage_backend,
};
use std::path::PathBuf;
use tempfile::tempdir;

// ---------------------------------------------------------------------------
// Directory creation
// ---------------------------------------------------------------------------

#[test]
fn ensure_base_dir_creates_nested_directories() {
    let td = tempdir().unwrap();
    let nested = td.path().join("a").join("b").join("c");
    let storage = FileStorage::new(nested.clone());

    assert!(!nested.exists());
    storage.ensure_base_dir().unwrap();
    assert!(nested.exists());
}

#[test]
fn ensure_base_dir_is_idempotent() {
    let td = tempdir().unwrap();
    let storage = FileStorage::new(td.path().to_path_buf());

    storage.ensure_base_dir().unwrap();
    storage.ensure_base_dir().unwrap(); // second call must not fail
}

#[test]
fn write_creates_intermediate_directories() {
    let td = tempdir().unwrap();
    let storage = FileStorage::new(td.path().to_path_buf());

    storage
        .write("deeply/nested/dir/file.txt", b"content")
        .unwrap();

    assert!(td.path().join("deeply/nested/dir/file.txt").exists());
    assert_eq!(
        storage.read("deeply/nested/dir/file.txt").unwrap(),
        b"content"
    );
}

// ---------------------------------------------------------------------------
// File read / write operations
// ---------------------------------------------------------------------------

#[test]
fn round_trip_text_data() {
    let td = tempdir().unwrap();
    let storage = FileStorage::new(td.path().to_path_buf());

    storage.write("hello.txt", b"hello world").unwrap();
    assert_eq!(storage.read("hello.txt").unwrap(), b"hello world");
}

#[test]
fn round_trip_binary_data() {
    let td = tempdir().unwrap();
    let storage = FileStorage::new(td.path().to_path_buf());

    let binary: Vec<u8> = (0..=255).collect();
    storage.write("binary.bin", &binary).unwrap();
    assert_eq!(storage.read("binary.bin").unwrap(), binary);
}

#[test]
fn write_empty_file() {
    let td = tempdir().unwrap();
    let storage = FileStorage::new(td.path().to_path_buf());

    storage.write("empty.txt", b"").unwrap();
    assert_eq!(storage.read("empty.txt").unwrap(), b"");
}

#[test]
fn overwrite_existing_file() {
    let td = tempdir().unwrap();
    let storage = FileStorage::new(td.path().to_path_buf());

    storage.write("f.txt", b"first").unwrap();
    storage.write("f.txt", b"second").unwrap();
    assert_eq!(storage.read("f.txt").unwrap(), b"second");
}

#[test]
fn write_large_payload() {
    let td = tempdir().unwrap();
    let storage = FileStorage::new(td.path().to_path_buf());

    let large = vec![0xABu8; 1024 * 1024]; // 1 MiB
    storage.write("large.bin", &large).unwrap();
    assert_eq!(storage.read("large.bin").unwrap(), large);
}

// ---------------------------------------------------------------------------
// Atomic file operations (copy / move)
// ---------------------------------------------------------------------------

#[test]
fn write_is_atomic_no_leftover_tmp() {
    let td = tempdir().unwrap();
    let storage = FileStorage::new(td.path().to_path_buf());

    storage.write("atomic.txt", b"data").unwrap();

    // The .tmp file used during atomic write should not remain
    assert!(!td.path().join("atomic.tmp").exists());
    assert!(td.path().join("atomic.txt").exists());
}

#[test]
fn copy_preserves_source() {
    let td = tempdir().unwrap();
    let storage = FileStorage::new(td.path().to_path_buf());

    storage.write("src.txt", b"payload").unwrap();
    storage.copy("src.txt", "dst.txt").unwrap();

    assert!(storage.exists("src.txt").unwrap());
    assert_eq!(storage.read("dst.txt").unwrap(), b"payload");
}

#[test]
fn copy_into_nested_destination() {
    let td = tempdir().unwrap();
    let storage = FileStorage::new(td.path().to_path_buf());

    storage.write("root.txt", b"data").unwrap();
    storage.copy("root.txt", "sub/dir/root_copy.txt").unwrap();

    assert_eq!(storage.read("sub/dir/root_copy.txt").unwrap(), b"data");
}

#[test]
fn mv_removes_source() {
    let td = tempdir().unwrap();
    let storage = FileStorage::new(td.path().to_path_buf());

    storage.write("before.txt", b"content").unwrap();
    storage.mv("before.txt", "after.txt").unwrap();

    assert!(!storage.exists("before.txt").unwrap());
    assert_eq!(storage.read("after.txt").unwrap(), b"content");
}

// ---------------------------------------------------------------------------
// Cleanup / delete operations
// ---------------------------------------------------------------------------

#[test]
fn delete_existing_file() {
    let td = tempdir().unwrap();
    let storage = FileStorage::new(td.path().to_path_buf());

    storage.write("remove_me.txt", b"x").unwrap();
    assert!(storage.exists("remove_me.txt").unwrap());

    storage.delete("remove_me.txt").unwrap();
    assert!(!storage.exists("remove_me.txt").unwrap());
}

#[test]
fn delete_nonexistent_file_is_ok() {
    let td = tempdir().unwrap();
    let storage = FileStorage::new(td.path().to_path_buf());

    storage.delete("does_not_exist.txt").unwrap();
}

#[test]
fn exists_reports_correctly() {
    let td = tempdir().unwrap();
    let storage = FileStorage::new(td.path().to_path_buf());

    assert!(!storage.exists("missing").unwrap());

    storage.write("present.txt", b"y").unwrap();
    assert!(storage.exists("present.txt").unwrap());
}

// ---------------------------------------------------------------------------
// List operations
// ---------------------------------------------------------------------------

#[test]
fn list_returns_all_files_recursively() {
    let td = tempdir().unwrap();
    let storage = FileStorage::new(td.path().to_path_buf());

    storage.write("a.json", b"{}").unwrap();
    storage.write("sub/b.json", b"{}").unwrap();
    storage.write("sub/deep/c.json", b"{}").unwrap();

    let mut files = storage.list("").unwrap();
    files.sort();

    assert_eq!(files.len(), 3);
    assert!(files.contains(&"a.json".to_string()));
    assert!(files.contains(&"sub/b.json".to_string()));
    assert!(files.contains(&"sub/deep/c.json".to_string()));
}

#[test]
fn list_with_prefix_scopes_results() {
    let td = tempdir().unwrap();
    let storage = FileStorage::new(td.path().to_path_buf());

    storage.write("state/run1.json", b"1").unwrap();
    storage.write("state/run2.json", b"2").unwrap();
    storage.write("receipts/r1.json", b"r").unwrap();

    let state_files = storage.list("state").unwrap();
    assert_eq!(state_files.len(), 2);

    let receipt_files = storage.list("receipts").unwrap();
    assert_eq!(receipt_files.len(), 1);
}

#[test]
fn list_missing_prefix_returns_empty() {
    let td = tempdir().unwrap();
    let storage = FileStorage::new(td.path().to_path_buf());

    let files = storage.list("nonexistent").unwrap();
    assert!(files.is_empty());
}

// ---------------------------------------------------------------------------
// Error handling
// ---------------------------------------------------------------------------

#[test]
fn read_missing_file_returns_error() {
    let td = tempdir().unwrap();
    let storage = FileStorage::new(td.path().to_path_buf());

    assert!(storage.read("no_such_file.txt").is_err());
}

#[test]
fn read_from_nonexistent_base_returns_error() {
    let storage = FileStorage::new(PathBuf::from("totally_nonexistent_dir_12345/sub"));

    assert!(storage.read("anything.txt").is_err());
}

#[test]
fn copy_missing_source_returns_error() {
    let td = tempdir().unwrap();
    let storage = FileStorage::new(td.path().to_path_buf());

    assert!(storage.copy("missing.txt", "dest.txt").is_err());
}

#[test]
fn mv_missing_source_returns_error() {
    let td = tempdir().unwrap();
    let storage = FileStorage::new(td.path().to_path_buf());

    assert!(storage.mv("missing.txt", "dest.txt").is_err());
}

// ---------------------------------------------------------------------------
// Path resolution
// ---------------------------------------------------------------------------

#[test]
fn full_path_joins_correctly() {
    let storage = FileStorage::new(PathBuf::from("/base/dir"));
    assert_eq!(
        storage.full_path("state.json"),
        PathBuf::from("/base/dir/state.json")
    );
}

#[test]
fn base_path_accessor() {
    let td = tempdir().unwrap();
    let storage = FileStorage::new(td.path().to_path_buf());

    assert_eq!(storage.base_path(), td.path().to_str().unwrap());
}

#[test]
fn path_accessor_returns_pathbuf() {
    let td = tempdir().unwrap();
    let storage = FileStorage::new(td.path().to_path_buf());
    assert_eq!(storage.path(), td.path());
}

// ---------------------------------------------------------------------------
// build_storage_backend
// ---------------------------------------------------------------------------

#[test]
fn build_file_backend_via_config() {
    let td = tempdir().unwrap();
    let config = CloudStorageConfig::file(td.path().to_str().unwrap());

    let backend = build_storage_backend(&config).unwrap();
    assert_eq!(backend.storage_type(), StorageType::File);
    assert_eq!(backend.bucket(), "local");
}

#[test]
fn build_cloud_backends_are_not_implemented() {
    let s3 = CloudStorageConfig::s3("bucket");
    assert!(build_storage_backend(&s3).is_err());

    let gcs = CloudStorageConfig::gcs("bucket");
    assert!(build_storage_backend(&gcs).is_err());

    let azure = CloudStorageConfig::azure("container");
    assert!(build_storage_backend(&azure).is_err());
}

// ---------------------------------------------------------------------------
// CloudStorageConfig validation
// ---------------------------------------------------------------------------

#[test]
fn validate_rejects_empty_bucket_for_cloud() {
    let config = CloudStorageConfig::new(StorageType::S3, "");
    assert!(config.validate().is_err());
}

#[test]
fn validate_accepts_file_type_without_bucket() {
    let config = CloudStorageConfig::file("/any/path");
    assert!(config.validate().is_ok());
}

#[test]
fn cloud_config_full_path_with_trailing_slash() {
    let config = CloudStorageConfig::s3("b").with_base_path("prefix/");
    assert_eq!(config.full_path("key.json"), "prefix/key.json");
}

#[test]
fn cloud_config_full_path_empty_base() {
    let config = CloudStorageConfig::s3("b");
    assert_eq!(config.full_path("key.json"), "key.json");
}

// ---------------------------------------------------------------------------
// StorageType round-trip
// ---------------------------------------------------------------------------

#[test]
fn storage_type_parse_round_trip() {
    for (input, expected) in [
        ("file", StorageType::File),
        ("local", StorageType::File),
        ("s3", StorageType::S3),
        ("gcs", StorageType::Gcs),
        ("gs", StorageType::Gcs),
        ("azure", StorageType::Azure),
        ("blob", StorageType::Azure),
    ] {
        let parsed: StorageType = input.parse().unwrap();
        assert_eq!(parsed, expected);
    }
}

#[test]
fn storage_type_unknown_input_fails() {
    let result: Result<StorageType, _> = "ftp".parse();
    assert!(result.is_err());
}
