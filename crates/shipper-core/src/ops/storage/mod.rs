//! Storage backend trait and filesystem-backed implementation.
//!
//! **Layer:** ops (internal)
//!
//! This module was the runtime portion of the standalone `shipper-storage`
//! crate. It is now crate-private inside `shipper` because only filesystem
//! storage is fully implemented — cloud backends (S3/GCS/Azure) currently
//! bail with "not yet implemented". Promising a public `StorageBackend`
//! trait via crates.io would freeze a half-finished design.
//!
//! The configuration data types (`CloudStorageConfig`, `StorageType`) live
//! in the stable `shipper_types::storage` contract crate — embedders can
//! declare their storage choice without depending on this internal trait.

use std::env;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub(crate) use shipper_types::storage::{CloudStorageConfig, StorageType};

/// Common trait for all storage backends.
///
/// Provides a unified interface for storage operations across different
/// providers. Today only the filesystem implementation is real; S3/GCS/Azure
/// adapters are stubbed in [`build_storage_backend`] pending implementation.
pub(crate) trait StorageBackend: Send + Sync {
    /// Read data from storage at the given path
    fn read(&self, path: &str) -> Result<Vec<u8>>;

    /// Write data to storage at the given path
    fn write(&self, path: &str, data: &[u8]) -> Result<()>;

    /// Delete data from storage at the given path
    fn delete(&self, path: &str) -> Result<()>;

    /// Check if data exists at the given path
    fn exists(&self, path: &str) -> Result<bool>;

    /// List all paths matching a prefix
    fn list(&self, prefix: &str) -> Result<Vec<String>>;

    /// Get the storage type
    fn storage_type(&self) -> StorageType;

    /// Get the bucket/container name
    fn bucket(&self) -> &str;

    /// Get the base path within the storage
    fn base_path(&self) -> &str;

    /// Copy data from one path to another within the same storage
    fn copy(&self, from: &str, to: &str) -> Result<()> {
        let data = self.read(from)?;
        self.write(to, &data)
    }

    /// Move data from one path to another within the same storage
    fn mv(&self, from: &str, to: &str) -> Result<()> {
        self.copy(from, to)?;
        self.delete(from)
    }
}

/// Filesystem-based storage backend. Writes atomically via temp file + rename.
#[derive(Debug, Clone)]
pub(crate) struct FileStorage {
    base_path: PathBuf,
}

impl FileStorage {
    /// Create a new FileStorage with the specified base path
    pub(crate) fn new(base_path: PathBuf) -> Self {
        Self { base_path }
    }

    /// Get the base path as a `Path`.
    pub(crate) fn path(&self) -> &Path {
        &self.base_path
    }

    /// Get the base path as `&PathBuf` (kept for historical API compatibility
    /// within the crate).
    pub(crate) fn base_path_buf(&self) -> &PathBuf {
        &self.base_path
    }

    /// Get the full path for a relative path
    pub(crate) fn full_path(&self, relative_path: &str) -> PathBuf {
        self.base_path.join(relative_path)
    }

    /// Ensure the base directory exists
    pub(crate) fn ensure_base_dir(&self) -> Result<()> {
        if !self.base_path.exists() {
            std::fs::create_dir_all(&self.base_path).with_context(|| {
                format!("failed to create directory: {}", self.base_path.display())
            })?;
        }
        Ok(())
    }
}

impl StorageBackend for FileStorage {
    fn read(&self, path: &str) -> Result<Vec<u8>> {
        let full_path = self.base_path.join(path);
        std::fs::read(&full_path)
            .with_context(|| format!("failed to read file: {}", full_path.display()))
    }

    fn write(&self, path: &str, data: &[u8]) -> Result<()> {
        let full_path = self.base_path.join(path);

        // Create parent directories if they don't exist
        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create directory: {}", parent.display()))?;
        }

        // Write to a unique temp file first, then rename for atomicity.
        // The temp filename must be unique per-call so concurrent writes to
        // the same destination do not race: with a shared temp name, one
        // thread's rename can move the file away before another thread's
        // rename runs, causing spurious ENOENT.
        let tid = std::thread::current().id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let pid = std::process::id();
        let tmp_name = format!(
            "{}.{pid}.{tid:?}.{nanos}.tmp",
            full_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("shipper-storage")
        );
        let tmp_path = full_path.with_file_name(tmp_name);
        std::fs::write(&tmp_path, data)
            .with_context(|| format!("failed to write file: {}", tmp_path.display()))?;

        std::fs::rename(&tmp_path, &full_path)
            .with_context(|| format!("failed to rename file to: {}", full_path.display()))?;

        Ok(())
    }

    fn delete(&self, path: &str) -> Result<()> {
        let full_path = self.base_path.join(path);
        if full_path.exists() {
            std::fs::remove_file(&full_path)
                .with_context(|| format!("failed to delete file: {}", full_path.display()))?;
        }
        Ok(())
    }

    fn exists(&self, path: &str) -> Result<bool> {
        let full_path = self.base_path.join(path);
        Ok(full_path.exists())
    }

    fn list(&self, prefix: &str) -> Result<Vec<String>> {
        let base = self.base_path.join(prefix);
        let mut results = Vec::new();

        if !base.exists() {
            return Ok(results);
        }

        fn collect_files(dir: &PathBuf, base: &PathBuf, results: &mut Vec<String>) -> Result<()> {
            for entry in std::fs::read_dir(dir)? {
                let entry = entry?;
                let path = entry.path();

                if path.is_dir() {
                    collect_files(&path, base, results)?;
                } else if let Ok(relative) = path.strip_prefix(base)
                    && let Some(s) = relative.to_str()
                {
                    results.push(s.replace('\\', "/"));
                }
            }
            Ok(())
        }

        collect_files(&base, &self.base_path, &mut results)?;
        Ok(results)
    }

    fn storage_type(&self) -> StorageType {
        StorageType::File
    }

    fn bucket(&self) -> &str {
        "local"
    }

    fn base_path(&self) -> &str {
        self.base_path.to_str().unwrap_or("")
    }
}

/// Build a storage backend from configuration.
///
/// Currently only filesystem storage is fully implemented. S3/GCS/Azure
/// return an error — the trait exists so future cloud backends can plug in
/// without breaking embedders that already depend on the stable config
/// types in `shipper_types::storage`.
pub(crate) fn build_storage_backend(
    config: &CloudStorageConfig,
) -> Result<Box<dyn StorageBackend>> {
    config
        .validate()
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;

    match config.storage_type {
        StorageType::File => Ok(Box::new(FileStorage::new(PathBuf::from(&config.base_path)))),
        StorageType::S3 => {
            anyhow::bail!("S3 storage is not yet implemented. Use file storage for now.")
        }
        StorageType::Gcs => {
            anyhow::bail!("GCS storage is not yet implemented. Use file storage for now.")
        }
        StorageType::Azure => {
            anyhow::bail!("Azure storage is not yet implemented. Use file storage for now.")
        }
    }
}

/// Read a [`CloudStorageConfig`] from environment variables.
///
/// This is an opt-in form: returns `None` if `SHIPPER_STORAGE_TYPE` is unset
/// or unrecognized (the previous monolithic shipper API shape).
///
/// Environment variables:
/// - `SHIPPER_STORAGE_TYPE`: file, s3, gcs, or azure
/// - `SHIPPER_STORAGE_BUCKET`: bucket/container name (required when type is set)
/// - `SHIPPER_STORAGE_REGION`: region (for S3) or project ID (for GCS)
/// - `SHIPPER_STORAGE_BASE_PATH`: base path within bucket or local path
/// - `SHIPPER_STORAGE_ENDPOINT`: custom endpoint (for S3-compatible services)
/// - `SHIPPER_STORAGE_ACCESS_KEY_ID`: access key ID
/// - `SHIPPER_STORAGE_SECRET_ACCESS_KEY`: secret access key
/// - `SHIPPER_STORAGE_SESSION_TOKEN`: session token (optional)
pub(crate) fn config_from_env() -> Option<CloudStorageConfig> {
    let storage_type_str = env::var("SHIPPER_STORAGE_TYPE").ok()?;
    let storage_type = match storage_type_str.as_str() {
        "file" | "local" => StorageType::File,
        "s3" => StorageType::S3,
        "gcs" | "gs" => StorageType::Gcs,
        "azure" | "blob" => StorageType::Azure,
        _ => return None,
    };

    let bucket = env::var("SHIPPER_STORAGE_BUCKET").ok()?;
    let mut config = CloudStorageConfig::new(storage_type, bucket);

    if let Ok(region) = env::var("SHIPPER_STORAGE_REGION") {
        config.region = Some(region);
    }
    if let Ok(base_path) = env::var("SHIPPER_STORAGE_BASE_PATH") {
        config.base_path = base_path;
    }
    if let Ok(endpoint) = env::var("SHIPPER_STORAGE_ENDPOINT") {
        config.endpoint = Some(endpoint);
    }
    if let Ok(access_key_id) = env::var("SHIPPER_STORAGE_ACCESS_KEY_ID") {
        config.access_key_id = Some(access_key_id);
    }
    if let Ok(secret_access_key) = env::var("SHIPPER_STORAGE_SECRET_ACCESS_KEY") {
        config.secret_access_key = Some(secret_access_key);
    }
    if let Ok(session_token) = env::var("SHIPPER_STORAGE_SESSION_TOKEN") {
        config.session_token = Some(session_token);
    }

    Some(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn file_storage_new_and_paths() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());
        assert_eq!(storage.path(), td.path());
        assert_eq!(storage.base_path_buf(), &td.path().to_path_buf());
    }

    #[test]
    fn file_storage_write_and_read() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());

        storage.write("test.txt", b"hello world").expect("write");

        let data = storage.read("test.txt").expect("read");
        assert_eq!(data, b"hello world");
    }

    #[test]
    fn file_storage_write_creates_dirs() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());

        storage
            .write("nested/deep/path/test.txt", b"data")
            .expect("write");

        let data = storage.read("nested/deep/path/test.txt").expect("read");
        assert_eq!(data, b"data");
    }

    #[test]
    fn file_storage_exists() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());

        storage.write("test.txt", b"data").expect("write");

        assert!(storage.exists("test.txt").expect("exists"));
        assert!(!storage.exists("missing.txt").expect("exists"));
    }

    #[test]
    fn file_storage_delete() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());

        storage.write("test.txt", b"data").expect("write");
        assert!(storage.exists("test.txt").expect("exists"));

        storage.delete("test.txt").expect("delete");
        assert!(!storage.exists("test.txt").expect("exists"));
    }

    #[test]
    fn file_storage_delete_missing_ok() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());

        storage.delete("missing.txt").expect("delete");
    }

    #[test]
    fn file_storage_list() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());

        storage.write("a.txt", b"a").expect("write");
        storage.write("b.txt", b"b").expect("write");
        storage.write("sub/c.txt", b"c").expect("write");

        let files = storage.list("").expect("list");
        assert_eq!(files.len(), 3);
        assert!(files.contains(&"a.txt".to_string()));
        assert!(files.contains(&"b.txt".to_string()));
        assert!(files.contains(&"sub/c.txt".to_string()));
    }

    #[test]
    fn file_storage_list_with_prefix() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());

        storage.write("state/a.json", b"a").expect("write");
        storage.write("state/b.json", b"b").expect("write");
        storage.write("other/c.json", b"c").expect("write");

        let files = storage.list("state").expect("list");
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn file_storage_copy() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());

        storage.write("original.txt", b"data").expect("write");
        storage.copy("original.txt", "copy.txt").expect("copy");

        assert!(storage.exists("original.txt").expect("exists"));
        assert!(storage.exists("copy.txt").expect("exists"));
        assert_eq!(storage.read("copy.txt").expect("read"), b"data");
    }

    #[test]
    fn file_storage_mv() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());

        storage.write("original.txt", b"data").expect("write");
        storage.mv("original.txt", "moved.txt").expect("mv");

        assert!(!storage.exists("original.txt").expect("exists"));
        assert!(storage.exists("moved.txt").expect("exists"));
        assert_eq!(storage.read("moved.txt").expect("read"), b"data");
    }

    #[test]
    fn file_storage_storage_type() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());
        assert_eq!(storage.storage_type(), StorageType::File);
        assert_eq!(storage.bucket(), "local");
    }

    #[test]
    fn build_storage_backend_file() {
        let config = CloudStorageConfig::file("/tmp/test");
        let backend = build_storage_backend(&config).expect("build");
        assert_eq!(backend.storage_type(), StorageType::File);
    }

    #[test]
    fn build_storage_backend_s3_not_implemented() {
        let config = CloudStorageConfig::s3("bucket");
        assert!(build_storage_backend(&config).is_err());
    }

    #[test]
    fn build_storage_backend_gcs_not_implemented() {
        let config = CloudStorageConfig::gcs("bucket");
        assert!(build_storage_backend(&config).is_err());
    }

    #[test]
    fn build_storage_backend_azure_not_implemented() {
        let config = CloudStorageConfig::azure("container");
        assert!(build_storage_backend(&config).is_err());
    }

    #[test]
    fn empty_file_content_write_and_read() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());

        storage.write("empty.txt", b"").expect("write empty");
        let data = storage.read("empty.txt").expect("read empty");
        assert!(data.is_empty());
        assert!(storage.exists("empty.txt").expect("exists"));
    }

    #[test]
    fn large_file_content_over_1mb() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());

        let size = 1_500_000;
        let data: Vec<u8> = (0..size).map(|i| (i % 256) as u8).collect();

        storage.write("large.bin", &data).expect("write large");
        let read_back = storage.read("large.bin").expect("read large");
        assert_eq!(read_back.len(), size);
        assert_eq!(read_back, data);
    }

    #[test]
    fn atomic_write_no_tmp_file_remains() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());

        storage.write("atomic.txt", b"content").expect("write");

        let tmp_path = td.path().join("atomic.tmp");
        assert!(
            !tmp_path.exists(),
            ".tmp file should not remain after successful write"
        );
        assert!(td.path().join("atomic.txt").exists());
    }

    #[test]
    fn atomic_write_simulated_interrupt_stale_tmp() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());

        let tmp_path = td.path().join("interrupted.tmp");
        std::fs::write(&tmp_path, b"stale temp").expect("create stale tmp");

        storage
            .write("interrupted.txt", b"completed")
            .expect("write");
        let data = storage.read("interrupted.txt").expect("read");
        assert_eq!(data, b"completed");
    }

    #[test]
    fn read_nonexistent_file_returns_error() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());

        let result = storage.read("does_not_exist.txt");
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("failed to read file"));
    }

    #[test]
    fn write_to_path_blocked_by_existing_file() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());

        storage
            .write("blocker", b"I am a file")
            .expect("write blocker");

        let result = storage.write("blocker/sub/file.txt", b"should fail");
        assert!(result.is_err());
    }

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
        storage.ensure_base_dir().unwrap();
    }

    #[test]
    fn full_path_joins_correctly() {
        let storage = FileStorage::new(PathBuf::from("/base/dir"));
        assert_eq!(
            storage.full_path("state.json"),
            PathBuf::from("/base/dir/state.json")
        );
    }

    #[test]
    fn list_uses_forward_slashes_on_all_platforms() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());

        storage.write("a/b/c.txt", b"x").unwrap();
        let files = storage.list("").unwrap();
        for f in &files {
            assert!(!f.contains('\\'), "path should use / not \\: {f}");
        }
    }

    #[test]
    fn unknown_storage_type_from_env_returns_none() {
        temp_env::with_vars(
            [
                ("SHIPPER_STORAGE_TYPE", Some("bogus")),
                ("SHIPPER_STORAGE_BUCKET", Some("bucket")),
            ],
            || {
                assert!(config_from_env().is_none());
            },
        );
    }

    #[test]
    fn config_from_env_populates_all_fields() {
        temp_env::with_vars(
            [
                ("SHIPPER_STORAGE_TYPE", Some("s3")),
                ("SHIPPER_STORAGE_BUCKET", Some("my-bucket")),
                ("SHIPPER_STORAGE_REGION", Some("us-west-2")),
                ("SHIPPER_STORAGE_BASE_PATH", Some("state")),
                ("SHIPPER_STORAGE_ENDPOINT", None::<&str>),
                ("SHIPPER_STORAGE_ACCESS_KEY_ID", Some("AKIA123")),
                ("SHIPPER_STORAGE_SECRET_ACCESS_KEY", Some("secret")),
                ("SHIPPER_STORAGE_SESSION_TOKEN", None::<&str>),
            ],
            || {
                let config = config_from_env().expect("config");
                assert_eq!(config.storage_type, StorageType::S3);
                assert_eq!(config.bucket, "my-bucket");
                assert_eq!(config.region, Some("us-west-2".to_string()));
                assert_eq!(config.base_path, "state");
                assert_eq!(config.access_key_id, Some("AKIA123".to_string()));
            },
        );
    }

    #[test]
    fn config_from_env_returns_none_without_type() {
        temp_env::with_vars(
            [
                ("SHIPPER_STORAGE_TYPE", None::<&str>),
                ("SHIPPER_STORAGE_BUCKET", None::<&str>),
            ],
            || {
                assert!(config_from_env().is_none());
            },
        );
    }
}
