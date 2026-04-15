//! Storage backends for shipper.
//!
//! This crate provides the [`StorageBackend`] trait and implementations
//! for different storage providers (filesystem, S3, GCS, Azure Blob).
//!
//! # Example
//!
//! ```
//! use shipper_storage::{StorageBackend, FileStorage, StorageType};
//! use std::path::PathBuf;
//!
//! let storage = FileStorage::new(PathBuf::from(".shipper"));
//!
//! // Write data
//! storage.write("test.txt", b"hello world").expect("write");
//!
//! // Read data back
//! let data = storage.read("test.txt").expect("read");
//! assert_eq!(data, b"hello world");
//!
//! // Check storage type
//! assert_eq!(storage.storage_type(), StorageType::File);
//! ```

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Represents the type of storage backend
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum StorageType {
    /// Local filesystem storage
    #[default]
    File,
    /// Amazon S3 storage
    S3,
    /// Google Cloud Storage
    Gcs,
    /// Azure Blob Storage
    Azure,
}

impl std::fmt::Display for StorageType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StorageType::File => write!(f, "file"),
            StorageType::S3 => write!(f, "s3"),
            StorageType::Gcs => write!(f, "gcs"),
            StorageType::Azure => write!(f, "azure"),
        }
    }
}

impl std::str::FromStr for StorageType {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "file" | "local" => Ok(StorageType::File),
            "s3" => Ok(StorageType::S3),
            "gcs" | "gs" => Ok(StorageType::Gcs),
            "azure" | "blob" => Ok(StorageType::Azure),
            _ => anyhow::bail!("unknown storage type: {}", s),
        }
    }
}

/// Common trait for all storage backends.
///
/// This trait provides a unified interface for storage operations across
/// different storage providers (filesystem, S3, GCS, Azure Blob).
pub trait StorageBackend: Send + Sync {
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

/// Configuration for cloud storage backends
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudStorageConfig {
    /// Storage type (s3, gcs, azure)
    pub storage_type: StorageType,
    /// Bucket/container name
    pub bucket: String,
    /// Region for S3, project ID for GCS
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// Base path within the bucket
    #[serde(default)]
    pub base_path: String,
    /// Custom endpoint (for S3-compatible services like MinIO, DigitalOcean Spaces)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// Access key ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_key_id: Option<String>,
    /// Secret access key
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_access_key: Option<String>,
    /// Session token (for temporary credentials)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_token: Option<String>,
}

impl Default for CloudStorageConfig {
    fn default() -> Self {
        Self {
            storage_type: StorageType::File,
            bucket: String::new(),
            region: None,
            base_path: String::new(),
            endpoint: None,
            access_key_id: None,
            secret_access_key: None,
            session_token: None,
        }
    }
}

impl CloudStorageConfig {
    /// Create a new CloudStorageConfig with the given bucket
    pub fn new(storage_type: StorageType, bucket: impl Into<String>) -> Self {
        Self {
            storage_type,
            bucket: bucket.into(),
            ..Default::default()
        }
    }

    /// Create a file storage config
    pub fn file(base_path: impl Into<String>) -> Self {
        Self {
            storage_type: StorageType::File,
            base_path: base_path.into(),
            ..Default::default()
        }
    }

    /// Create an S3 storage config
    pub fn s3(bucket: impl Into<String>) -> Self {
        Self::new(StorageType::S3, bucket)
    }

    /// Create a GCS storage config
    pub fn gcs(bucket: impl Into<String>) -> Self {
        Self::new(StorageType::Gcs, bucket)
    }

    /// Create an Azure storage config
    pub fn azure(container: impl Into<String>) -> Self {
        Self::new(StorageType::Azure, container)
    }

    /// Set the region
    pub fn with_region(mut self, region: impl Into<String>) -> Self {
        self.region = Some(region.into());
        self
    }

    /// Set the base path
    pub fn with_base_path(mut self, path: impl Into<String>) -> Self {
        self.base_path = path.into();
        self
    }

    /// Set custom endpoint (for S3-compatible services)
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = Some(endpoint.into());
        self
    }

    /// Set credentials
    pub fn with_credentials(
        mut self,
        access_key_id: impl Into<String>,
        secret_access_key: impl Into<String>,
    ) -> Self {
        self.access_key_id = Some(access_key_id.into());
        self.secret_access_key = Some(secret_access_key.into());
        self
    }

    /// Set session token
    pub fn with_session_token(mut self, token: impl Into<String>) -> Self {
        self.session_token = Some(token.into());
        self
    }

    /// Build full path from relative path
    pub fn full_path(&self, relative_path: &str) -> String {
        if self.base_path.is_empty() {
            relative_path.to_string()
        } else {
            format!("{}/{}", self.base_path.trim_end_matches('/'), relative_path)
        }
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<()> {
        match self.storage_type {
            StorageType::File => {
                // File storage is always valid
            }
            StorageType::S3 | StorageType::Gcs | StorageType::Azure => {
                if self.bucket.is_empty() {
                    anyhow::bail!("bucket/container name is required for cloud storage");
                }
            }
        }
        Ok(())
    }
}

/// Filesystem-based storage backend.
#[derive(Debug, Clone)]
pub struct FileStorage {
    base_path: PathBuf,
}

impl FileStorage {
    /// Create a new FileStorage with the specified base path
    pub fn new(base_path: PathBuf) -> Self {
        Self { base_path }
    }

    /// Get the base path
    pub fn path(&self) -> &PathBuf {
        &self.base_path
    }

    /// Get the full path for a relative path
    pub fn full_path(&self, relative_path: &str) -> PathBuf {
        self.base_path.join(relative_path)
    }

    /// Ensure the base directory exists
    pub fn ensure_base_dir(&self) -> Result<()> {
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
/// Currently only filesystem storage is fully implemented.
pub fn build_storage_backend(config: &CloudStorageConfig) -> Result<Box<dyn StorageBackend>> {
    config.validate()?;

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

/// Get a storage backend from environment variables.
///
/// Environment variables:
/// - `SHIPPER_STORAGE_TYPE`: file, s3, gcs, or azure (default: file)
/// - `SHIPPER_STORAGE_BUCKET`: bucket/container name
/// - `SHIPPER_STORAGE_REGION`: region (for S3) or project ID (for GCS)
/// - `SHIPPER_STORAGE_BASE_PATH`: base path within bucket or local path
/// - `SHIPPER_STORAGE_ENDPOINT`: custom endpoint (for S3-compatible services)
/// - `SHIPPER_STORAGE_ACCESS_KEY_ID`: access key ID
/// - `SHIPPER_STORAGE_SECRET_ACCESS_KEY`: secret access key
/// - `SHIPPER_STORAGE_SESSION_TOKEN`: session token (optional)
pub fn config_from_env() -> CloudStorageConfig {
    use std::env;

    let storage_type = env::var("SHIPPER_STORAGE_TYPE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(StorageType::File);

    let mut config = CloudStorageConfig {
        storage_type,
        bucket: env::var("SHIPPER_STORAGE_BUCKET").unwrap_or_default(),
        region: env::var("SHIPPER_STORAGE_REGION").ok(),
        base_path: env::var("SHIPPER_STORAGE_BASE_PATH").unwrap_or_default(),
        endpoint: env::var("SHIPPER_STORAGE_ENDPOINT").ok(),
        access_key_id: env::var("SHIPPER_STORAGE_ACCESS_KEY_ID").ok(),
        secret_access_key: env::var("SHIPPER_STORAGE_SECRET_ACCESS_KEY").ok(),
        session_token: env::var("SHIPPER_STORAGE_SESSION_TOKEN").ok(),
    };

    // For file storage, use base_path or default
    if config.storage_type == StorageType::File && config.base_path.is_empty() {
        config.base_path = ".shipper".to_string();
    }

    config
}

/// Create a default file storage backend
pub fn default_storage() -> FileStorage {
    FileStorage::new(PathBuf::from(".shipper"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;
    use tempfile::tempdir;

    #[test]
    fn storage_type_from_str() {
        assert_eq!(StorageType::from_str("file").unwrap(), StorageType::File);
        assert_eq!(StorageType::from_str("local").unwrap(), StorageType::File);
        assert_eq!(StorageType::from_str("s3").unwrap(), StorageType::S3);
        assert_eq!(StorageType::from_str("gcs").unwrap(), StorageType::Gcs);
        assert_eq!(StorageType::from_str("gs").unwrap(), StorageType::Gcs);
        assert_eq!(StorageType::from_str("azure").unwrap(), StorageType::Azure);
        assert!(StorageType::from_str("unknown").is_err());
    }

    #[test]
    fn storage_type_display() {
        assert_eq!(StorageType::File.to_string(), "file");
        assert_eq!(StorageType::S3.to_string(), "s3");
        assert_eq!(StorageType::Gcs.to_string(), "gcs");
        assert_eq!(StorageType::Azure.to_string(), "azure");
    }

    #[test]
    fn storage_type_default() {
        assert_eq!(StorageType::default(), StorageType::File);
    }

    #[test]
    fn cloud_storage_config_new() {
        let config = CloudStorageConfig::new(StorageType::S3, "my-bucket");
        assert_eq!(config.storage_type, StorageType::S3);
        assert_eq!(config.bucket, "my-bucket");
        assert!(config.region.is_none());
    }

    #[test]
    fn cloud_storage_config_file() {
        let config = CloudStorageConfig::file("/path/to/state");
        assert_eq!(config.storage_type, StorageType::File);
        assert_eq!(config.base_path, "/path/to/state");
    }

    #[test]
    fn cloud_storage_config_s3() {
        let config = CloudStorageConfig::s3("my-bucket")
            .with_region("us-west-2")
            .with_credentials("key", "secret");

        assert_eq!(config.storage_type, StorageType::S3);
        assert_eq!(config.bucket, "my-bucket");
        assert_eq!(config.region, Some("us-west-2".to_string()));
    }

    #[test]
    fn cloud_storage_config_full_path() {
        let config = CloudStorageConfig::s3("bucket").with_base_path("prefix");
        assert_eq!(config.full_path("state.json"), "prefix/state.json");

        let config2 = CloudStorageConfig::s3("bucket");
        assert_eq!(config2.full_path("state.json"), "state.json");
    }

    #[test]
    fn cloud_storage_config_validate() {
        let config = CloudStorageConfig::file("/path");
        assert!(config.validate().is_ok());

        let config2 = CloudStorageConfig::s3(""); // Empty bucket
        assert!(config2.validate().is_err());
    }

    #[test]
    fn file_storage_new() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());
        assert_eq!(storage.path(), td.path());
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

        // Deleting a non-existent file should succeed
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
    fn default_storage_works() {
        let storage = default_storage();
        assert_eq!(storage.path(), &PathBuf::from(".shipper"));
    }

    #[test]
    fn cloud_storage_config_serialization() {
        let config = CloudStorageConfig::s3("bucket")
            .with_region("us-east-1")
            .with_base_path("prefix");

        let json = serde_json::to_string(&config).expect("serialize");
        assert!(json.contains("\"storage_type\":\"S3\""));
        assert!(json.contains("\"bucket\":\"bucket\""));
        assert!(json.contains("\"region\":\"us-east-1\""));
    }

    // --- Edge-case tests ---

    #[test]
    #[cfg_attr(
        target_os = "windows",
        ignore = "atomic rename races with concurrent readers on Windows"
    )]
    fn concurrent_reads_and_writes_same_file() {
        use std::sync::Arc;
        use std::thread;

        let td = tempdir().expect("tempdir");
        let storage = Arc::new(FileStorage::new(td.path().to_path_buf()));

        storage
            .write("shared.txt", b"initial")
            .expect("initial write");

        let mut handles = vec![];

        for i in 0..5 {
            let s = Arc::clone(&storage);
            handles.push(thread::spawn(move || {
                let data = format!("writer-{i}");
                s.write("shared.txt", data.as_bytes()).expect("write");
            }));
        }

        for _ in 0..5 {
            let s = Arc::clone(&storage);
            handles.push(thread::spawn(move || {
                let _ = s.read("shared.txt");
            }));
        }

        for h in handles {
            h.join().expect("thread join");
        }

        let data = storage.read("shared.txt").expect("final read");
        assert!(!data.is_empty());
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
    fn empty_file_content_write_and_read() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());

        storage.write("empty.txt", b"").expect("write empty");
        let data = storage.read("empty.txt").expect("read empty");
        assert!(data.is_empty());
        assert!(storage.exists("empty.txt").expect("exists"));
    }

    #[test]
    fn unicode_in_content() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());

        let content = "こんにちは世界 🌍 Ñoño café résumé 中文 Ελληνικά";
        storage
            .write("unicode.txt", content.as_bytes())
            .expect("write unicode");
        let data = storage.read("unicode.txt").expect("read unicode");
        assert_eq!(String::from_utf8(data).unwrap(), content);
    }

    #[test]
    fn unicode_in_file_paths() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());

        storage
            .write("données/café.txt", b"data")
            .expect("write unicode path");
        let data = storage.read("données/café.txt").expect("read unicode path");
        assert_eq!(data, b"data");
        assert!(storage.exists("données/café.txt").expect("exists"));
    }

    #[test]
    fn deeply_nested_directory_creation_20_levels() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());

        let segments: Vec<String> = (0..20).map(|i| format!("level_{i}")).collect();
        let deep_path = format!("{}/file.txt", segments.join("/"));

        storage.write(&deep_path, b"deep").expect("write deep");
        let data = storage.read(&deep_path).expect("read deep");
        assert_eq!(data, b"deep");

        let mut dir = td.path().to_path_buf();
        for seg in &segments {
            dir = dir.join(seg);
            assert!(dir.exists(), "directory {} should exist", dir.display());
        }
    }

    #[test]
    fn atomic_write_no_tmp_file_remains() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());

        storage.write("atomic.txt", b"content").expect("write");

        // No files with a `.tmp` extension should remain after successful write.
        let leftover: Vec<_> = std::fs::read_dir(td.path())
            .expect("read_dir")
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().map(|x| x == "tmp").unwrap_or(false))
            .collect();
        assert!(
            leftover.is_empty(),
            ".tmp file should not remain after successful write: {leftover:?}"
        );
        assert!(td.path().join("atomic.txt").exists());
    }

    #[test]
    fn atomic_write_simulated_interrupt_stale_tmp() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());

        // Simulate a prior interrupted write by leaving a stale .tmp file
        let tmp_path = td.path().join("interrupted.tmp");
        std::fs::write(&tmp_path, b"stale temp").expect("create stale tmp");

        // A new write to the same logical name should succeed
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

        // Create a regular file where a directory would need to be
        storage
            .write("blocker", b"I am a file")
            .expect("write blocker");

        // Try to write below it — should fail because "blocker" is not a directory
        let result = storage.write("blocker/sub/file.txt", b"should fail");
        assert!(result.is_err());
    }

    #[test]
    fn config_from_env_defaults_with_temp_env() {
        temp_env::with_vars(
            [
                ("SHIPPER_STORAGE_TYPE", None::<&str>),
                ("SHIPPER_STORAGE_BUCKET", None::<&str>),
                ("SHIPPER_STORAGE_REGION", None::<&str>),
                ("SHIPPER_STORAGE_BASE_PATH", None::<&str>),
                ("SHIPPER_STORAGE_ENDPOINT", None::<&str>),
                ("SHIPPER_STORAGE_ACCESS_KEY_ID", None::<&str>),
                ("SHIPPER_STORAGE_SECRET_ACCESS_KEY", None::<&str>),
                ("SHIPPER_STORAGE_SESSION_TOKEN", None::<&str>),
            ],
            || {
                let config = config_from_env();
                assert_eq!(config.storage_type, StorageType::File);
                assert_eq!(config.base_path, ".shipper");
                assert!(config.region.is_none());
                assert!(config.endpoint.is_none());
            },
        );
    }

    #[test]
    fn config_from_env_s3_with_temp_env() {
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
                let config = config_from_env();
                assert_eq!(config.storage_type, StorageType::S3);
                assert_eq!(config.bucket, "my-bucket");
                assert_eq!(config.region, Some("us-west-2".to_string()));
                assert_eq!(config.base_path, "state");
                assert_eq!(config.access_key_id, Some("AKIA123".to_string()));
            },
        );
    }

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        /// Strategy for generating safe directory/file name segments.
        fn safe_name_strategy() -> impl Strategy<Value = String> {
            "[a-zA-Z0-9][a-zA-Z0-9_]{0,19}".prop_filter("non-empty", |s| !s.is_empty())
        }

        proptest! {
            #[test]
            fn storage_path_construction(
                base in safe_name_strategy(),
                relative in safe_name_strategy(),
            ) {
                let storage = FileStorage::new(PathBuf::from(&base));
                let full = storage.full_path(&relative);
                // The full path must start with the base and end with the relative component
                prop_assert!(full.starts_with(&base));
                prop_assert!(full.ends_with(&relative));
            }

            #[test]
            fn cloud_config_full_path_with_base(
                base in safe_name_strategy(),
                relative in safe_name_strategy(),
            ) {
                let config = CloudStorageConfig::s3("bucket").with_base_path(&base);
                let full = config.full_path(&relative);
                prop_assert!(full.starts_with(&base));
                prop_assert!(full.ends_with(&relative));
                prop_assert!(full.contains('/'));
            }

            #[test]
            fn cloud_config_full_path_no_base(relative in safe_name_strategy()) {
                let config = CloudStorageConfig::s3("bucket");
                let full = config.full_path(&relative);
                prop_assert_eq!(full, relative);
            }

            #[test]
            fn atomic_write_read_roundtrip(content in proptest::collection::vec(any::<u8>(), 0..4096)) {
                let td = tempdir().expect("tempdir");
                let storage = FileStorage::new(td.path().to_path_buf());

                storage.write("roundtrip.bin", &content).expect("write");
                let read_back = storage.read("roundtrip.bin").expect("read");
                prop_assert_eq!(read_back, content);
            }

            #[test]
            fn write_read_arbitrary_filename(
                name in safe_name_strategy(),
                content in proptest::collection::vec(any::<u8>(), 0..512),
            ) {
                let td = tempdir().expect("tempdir");
                let storage = FileStorage::new(td.path().to_path_buf());

                storage.write(&name, &content).expect("write");
                prop_assert!(storage.exists(&name).expect("exists"));
                let read_back = storage.read(&name).expect("read");
                prop_assert_eq!(read_back, content);
            }

            #[test]
            fn nested_directory_creation(
                segments in proptest::collection::vec(safe_name_strategy(), 1..5),
                content in proptest::collection::vec(any::<u8>(), 0..256),
            ) {
                let td = tempdir().expect("tempdir");
                let storage = FileStorage::new(td.path().to_path_buf());

                let nested_path = format!("{}/file.bin", segments.join("/"));
                storage.write(&nested_path, &content).expect("write");

                prop_assert!(storage.exists(&nested_path).expect("exists"));
                let read_back = storage.read(&nested_path).expect("read");
                prop_assert_eq!(read_back, content);

                // Verify intermediate directories were created
                let mut dir = td.path().to_path_buf();
                for seg in &segments {
                    dir = dir.join(seg);
                    prop_assert!(dir.exists(), "directory {} should exist", dir.display());
                }
            }

            #[test]
            fn delete_after_write(
                name in safe_name_strategy(),
                content in proptest::collection::vec(any::<u8>(), 1..256),
            ) {
                let td = tempdir().expect("tempdir");
                let storage = FileStorage::new(td.path().to_path_buf());

                storage.write(&name, &content).expect("write");
                prop_assert!(storage.exists(&name).expect("exists"));

                storage.delete(&name).expect("delete");
                prop_assert!(!storage.exists(&name).expect("exists after delete"));
            }

            #[test]
            fn write_then_read_roundtrip_large_preserves_content(
                content in proptest::collection::vec(any::<u8>(), 0..131072),
            ) {
                let td = tempdir().expect("tempdir");
                let storage = FileStorage::new(td.path().to_path_buf());

                storage.write("roundtrip_large.bin", &content).expect("write");
                let read_back = storage.read("roundtrip_large.bin").expect("read");
                prop_assert_eq!(read_back, content);
            }

            #[test]
            fn write_then_read_roundtrip_unicode_content(
                content in "[a-zA-Z0-9 \\n\\t]{0,500}",
            ) {
                let td = tempdir().expect("tempdir");
                let storage = FileStorage::new(td.path().to_path_buf());

                storage.write("unicode_rt.txt", content.as_bytes()).expect("write");
                let read_back = storage.read("unicode_rt.txt").expect("read");
                prop_assert_eq!(String::from_utf8(read_back).unwrap(), content);
            }

            /// Overwrite never corrupts: write A, overwrite with B, read gives B.
            #[test]
            fn overwrite_preserves_latest_content(
                first in proptest::collection::vec(any::<u8>(), 0..2048),
                second in proptest::collection::vec(any::<u8>(), 0..2048),
            ) {
                let td = tempdir().expect("tempdir");
                let storage = FileStorage::new(td.path().to_path_buf());

                storage.write("overwrite.bin", &first).expect("write first");
                storage.write("overwrite.bin", &second).expect("write second");
                let read_back = storage.read("overwrite.bin").expect("read");
                prop_assert_eq!(read_back, second);
            }

            /// Copy produces an independent identical copy.
            #[test]
            fn copy_roundtrip_preserves_content(
                content in proptest::collection::vec(any::<u8>(), 0..2048),
            ) {
                let td = tempdir().expect("tempdir");
                let storage = FileStorage::new(td.path().to_path_buf());

                storage.write("orig.bin", &content).expect("write");
                storage.copy("orig.bin", "dup.bin").expect("copy");
                let orig = storage.read("orig.bin").expect("read orig");
                let dup = storage.read("dup.bin").expect("read dup");
                prop_assert_eq!(&orig, &content);
                prop_assert_eq!(&dup, &content);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Atomic writes – temp-file + rename pattern
    // -----------------------------------------------------------------------

    #[test]
    fn atomic_write_multiple_files_no_leftover_tmp() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());

        for i in 0..10 {
            let name = format!("file_{i}.txt");
            storage
                .write(&name, format!("content-{i}").as_bytes())
                .expect("write");
        }

        // No .tmp files should remain
        let entries: Vec<_> = std::fs::read_dir(td.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "tmp"))
            .collect();
        assert!(entries.is_empty(), "leftover .tmp files: {entries:?}");
    }

    #[test]
    fn atomic_write_overwrites_stale_tmp_from_prior_crash() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());

        // Simulate a stale .tmp from a prior crash. Writes now use a unique
        // temp name per-call, so they do not collide with (and do not rely
        // on cleaning up) any pre-existing stale temp file from a prior run.
        std::fs::write(td.path().join("state.tmp"), b"stale").unwrap();

        storage.write("state.json", b"fresh").expect("write");
        assert_eq!(storage.read("state.json").unwrap(), b"fresh");
    }

    // -----------------------------------------------------------------------
    // Directory creation – nested & idempotent
    // -----------------------------------------------------------------------

    #[test]
    fn ensure_base_dir_creates_deeply_nested_path() {
        let td = tempdir().expect("tempdir");
        let deep = td.path().join("a").join("b").join("c").join("d");
        let storage = FileStorage::new(deep.clone());

        storage.ensure_base_dir().unwrap();
        assert!(deep.exists());
        // Idempotent: second call is fine
        storage.ensure_base_dir().unwrap();
    }

    #[test]
    fn write_creates_parent_dirs_on_demand() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());

        storage.write("x/y/z/data.bin", b"\x00\x01").unwrap();
        assert!(td.path().join("x").join("y").join("z").is_dir());
        assert_eq!(storage.read("x/y/z/data.bin").unwrap(), b"\x00\x01");
    }

    // -----------------------------------------------------------------------
    // Read/write roundtrips
    // -----------------------------------------------------------------------

    #[test]
    fn roundtrip_json_content() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());

        let json = br#"{"plan_id":"abc-123","crates":["foo","bar"]}"#;
        storage.write("state.json", json).unwrap();
        assert_eq!(storage.read("state.json").unwrap(), json);
    }

    #[test]
    fn roundtrip_binary_all_byte_values() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());

        let all_bytes: Vec<u8> = (0..=255).collect();
        storage.write("all_bytes.bin", &all_bytes).unwrap();
        assert_eq!(storage.read("all_bytes.bin").unwrap(), all_bytes);
    }

    #[test]
    fn overwrite_reduces_file_size() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());

        storage.write("shrink.txt", &vec![0u8; 10_000]).unwrap();
        storage.write("shrink.txt", b"tiny").unwrap();
        assert_eq!(storage.read("shrink.txt").unwrap(), b"tiny");
    }

    // -----------------------------------------------------------------------
    // Error handling
    // -----------------------------------------------------------------------

    #[test]
    fn read_from_completely_nonexistent_base_dir() {
        let storage = FileStorage::new(PathBuf::from("nonexistent_9f8a7b6c/deeper/still"));
        assert!(storage.read("file.txt").is_err());
    }

    #[test]
    fn copy_nonexistent_source_returns_error() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());
        let err = storage.copy("ghost.txt", "dest.txt");
        assert!(err.is_err());
    }

    #[test]
    fn mv_nonexistent_source_returns_error() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());
        let err = storage.mv("ghost.txt", "dest.txt");
        assert!(err.is_err());
    }

    #[test]
    fn write_where_parent_is_a_file_returns_error() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());

        storage.write("conflict", b"file").unwrap();
        let result = storage.write("conflict/sub.txt", b"oops");
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // Path handling – relative, absolute, special chars
    // -----------------------------------------------------------------------

    #[test]
    fn full_path_with_absolute_base() {
        let storage = FileStorage::new(PathBuf::from("/absolute/base"));
        assert_eq!(
            storage.full_path("sub/file.txt"),
            PathBuf::from("/absolute/base/sub/file.txt"),
        );
    }

    #[test]
    fn full_path_with_relative_base() {
        let storage = FileStorage::new(PathBuf::from("relative/base"));
        assert_eq!(
            storage.full_path("file.txt"),
            PathBuf::from("relative/base/file.txt"),
        );
    }

    #[test]
    fn write_read_file_with_spaces_in_name() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());

        storage
            .write("dir with spaces/file name.txt", b"spaced")
            .unwrap();
        assert_eq!(
            storage.read("dir with spaces/file name.txt").unwrap(),
            b"spaced",
        );
    }

    #[test]
    fn write_read_file_with_dots_and_dashes() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());

        storage
            .write("my-project/v0.3.0-rc.1/state.json", b"{}")
            .unwrap();
        assert_eq!(
            storage.read("my-project/v0.3.0-rc.1/state.json").unwrap(),
            b"{}",
        );
    }

    // -----------------------------------------------------------------------
    // Concurrent access – multiple independent writers
    // -----------------------------------------------------------------------

    #[test]
    fn concurrent_writes_to_different_files() {
        use std::sync::Arc;
        use std::thread;

        let td = tempdir().expect("tempdir");
        let storage = Arc::new(FileStorage::new(td.path().to_path_buf()));

        let handles: Vec<_> = (0..10)
            .map(|i| {
                let s = Arc::clone(&storage);
                thread::spawn(move || {
                    let name = format!("file_{i}.txt");
                    let data = format!("data-{i}");
                    s.write(&name, data.as_bytes()).expect("write");
                })
            })
            .collect();

        for h in handles {
            h.join().expect("join");
        }

        for i in 0..10 {
            let name = format!("file_{i}.txt");
            let expected = format!("data-{i}");
            assert_eq!(
                storage.read(&name).unwrap(),
                expected.as_bytes(),
                "mismatch for {name}",
            );
        }
    }

    // -----------------------------------------------------------------------
    // List edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn list_empty_base_dir_returns_empty() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());
        assert!(storage.list("").unwrap().is_empty());
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
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use insta::assert_yaml_snapshot;
    use std::str::FromStr;
    use tempfile::tempdir;

    // --- StorageType display & serialization ---

    #[test]
    fn storage_type_display_all() {
        let displays: Vec<String> = [
            StorageType::File,
            StorageType::S3,
            StorageType::Gcs,
            StorageType::Azure,
        ]
        .iter()
        .map(|t| t.to_string())
        .collect();
        assert_yaml_snapshot!(displays);
    }

    #[test]
    fn storage_type_serde_roundtrip() {
        let types = vec![
            StorageType::File,
            StorageType::S3,
            StorageType::Gcs,
            StorageType::Azure,
        ];
        assert_yaml_snapshot!(types);
    }

    #[test]
    fn storage_type_default() {
        assert_yaml_snapshot!(StorageType::default());
    }

    #[test]
    fn storage_type_from_str_aliases() {
        let cases: Vec<(&str, String)> = vec!["file", "local", "s3", "gcs", "gs", "azure", "blob"]
            .into_iter()
            .map(|s| (s, StorageType::from_str(s).unwrap().to_string()))
            .collect();
        assert_yaml_snapshot!(cases);
    }

    #[test]
    fn storage_type_from_str_error() {
        let err = StorageType::from_str("ftp").unwrap_err();
        assert_yaml_snapshot!(err.to_string());
    }

    // --- CloudStorageConfig serialization ---

    #[test]
    fn cloud_config_s3_full() {
        let config = CloudStorageConfig::s3("my-releases")
            .with_region("eu-west-1")
            .with_base_path("shipper/state")
            .with_endpoint("https://s3.custom.example.com")
            .with_credentials("AKIAEXAMPLE", "secret-key")
            .with_session_token("session-tok");
        assert_yaml_snapshot!(config);
    }

    #[test]
    fn cloud_config_minimal_file() {
        let config = CloudStorageConfig::file(".shipper");
        assert_yaml_snapshot!(config);
    }

    #[test]
    fn cloud_config_gcs() {
        let config = CloudStorageConfig::gcs("gcs-bucket").with_region("us-central1");
        assert_yaml_snapshot!(config);
    }

    #[test]
    fn cloud_config_azure() {
        let config = CloudStorageConfig::azure("my-container").with_base_path("releases/v1");
        assert_yaml_snapshot!(config);
    }

    #[test]
    fn cloud_config_default() {
        assert_yaml_snapshot!(CloudStorageConfig::default());
    }

    // --- CloudStorageConfig full_path ---

    #[test]
    fn cloud_config_full_path_variants() {
        let results: Vec<(&str, &str, String)> = vec![
            (
                "prefix",
                "state.json",
                CloudStorageConfig::s3("b")
                    .with_base_path("prefix")
                    .full_path("state.json"),
            ),
            (
                "prefix/",
                "state.json",
                CloudStorageConfig::s3("b")
                    .with_base_path("prefix/")
                    .full_path("state.json"),
            ),
            (
                "",
                "state.json",
                CloudStorageConfig::s3("b").full_path("state.json"),
            ),
            (
                "a/b/c",
                "d.json",
                CloudStorageConfig::s3("b")
                    .with_base_path("a/b/c")
                    .full_path("d.json"),
            ),
        ];
        assert_yaml_snapshot!(results);
    }

    // --- CloudStorageConfig validation ---

    #[test]
    fn cloud_config_validate_errors() {
        let cases: Vec<(&str, String)> = vec![
            (
                "s3_empty_bucket",
                CloudStorageConfig::s3("")
                    .validate()
                    .unwrap_err()
                    .to_string(),
            ),
            (
                "gcs_empty_bucket",
                CloudStorageConfig::gcs("")
                    .validate()
                    .unwrap_err()
                    .to_string(),
            ),
            (
                "azure_empty_bucket",
                CloudStorageConfig::azure("")
                    .validate()
                    .unwrap_err()
                    .to_string(),
            ),
        ];
        assert_yaml_snapshot!(cases);
    }

    #[test]
    fn cloud_config_validate_file_always_ok() {
        let result = CloudStorageConfig::file("").validate().is_ok();
        assert_yaml_snapshot!(result);
    }

    // --- build_storage_backend errors ---

    #[test]
    fn build_backend_unimplemented_errors() {
        let s3_err = build_storage_backend(&CloudStorageConfig::s3("bucket"))
            .err()
            .expect("expected error")
            .to_string();
        let gcs_err = build_storage_backend(&CloudStorageConfig::gcs("bucket"))
            .err()
            .expect("expected error")
            .to_string();
        let azure_err = build_storage_backend(&CloudStorageConfig::azure("container"))
            .err()
            .expect("expected error")
            .to_string();
        let errors: Vec<(&str, String)> =
            vec![("s3", s3_err), ("gcs", gcs_err), ("azure", azure_err)];
        assert_yaml_snapshot!(errors);
    }

    #[test]
    fn build_backend_file_type() {
        let backend = build_storage_backend(&CloudStorageConfig::file("/tmp/test")).unwrap();
        assert_yaml_snapshot!(backend.storage_type());
    }

    // --- FileStorage operations ---

    #[test]
    fn file_storage_read_missing_error() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());
        let err = storage.read("nonexistent.txt").unwrap_err();
        // Snapshot only the prefix to avoid platform-specific path details
        let msg = err.to_string();
        let stable = msg.split(':').next().unwrap_or(&msg).to_string();
        assert_yaml_snapshot!(stable);
    }

    #[test]
    fn file_storage_write_read_snapshot() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());

        storage.write("data.json", br#"{"key":"value"}"#).unwrap();
        let content = String::from_utf8(storage.read("data.json").unwrap()).unwrap();
        assert_yaml_snapshot!(content);
    }

    #[test]
    fn file_storage_list_sorted() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());

        storage.write("c.txt", b"c").unwrap();
        storage.write("a.txt", b"a").unwrap();
        storage.write("b.txt", b"b").unwrap();
        storage.write("sub/d.txt", b"d").unwrap();

        let mut files = storage.list("").unwrap();
        files.sort();
        assert_yaml_snapshot!(files);
    }

    #[test]
    fn file_storage_list_empty_dir() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());

        let files = storage.list("nonexistent").unwrap();
        assert_yaml_snapshot!(files);
    }

    #[test]
    fn file_storage_exists_snapshot() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());
        storage.write("present.txt", b"yes").unwrap();

        let results: Vec<(&str, bool)> = vec![
            ("present.txt", storage.exists("present.txt").unwrap()),
            ("absent.txt", storage.exists("absent.txt").unwrap()),
        ];
        assert_yaml_snapshot!(results);
    }

    #[test]
    fn file_storage_copy_and_mv() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());
        storage.write("src.txt", b"payload").unwrap();

        storage.copy("src.txt", "copied.txt").unwrap();
        storage.mv("src.txt", "moved.txt").unwrap();

        let state: Vec<(&str, bool)> = vec![
            ("src.txt", storage.exists("src.txt").unwrap()),
            ("copied.txt", storage.exists("copied.txt").unwrap()),
            ("moved.txt", storage.exists("moved.txt").unwrap()),
        ];
        let copied_content = String::from_utf8(storage.read("copied.txt").unwrap()).unwrap();
        let moved_content = String::from_utf8(storage.read("moved.txt").unwrap()).unwrap();

        assert_yaml_snapshot!("file_state", state);
        assert_yaml_snapshot!("copied_content", copied_content);
        assert_yaml_snapshot!("moved_content", moved_content);
    }

    #[test]
    fn file_storage_metadata() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());
        let meta = (
            storage.storage_type().to_string(),
            storage.bucket().to_string(),
        );
        assert_yaml_snapshot!(meta);
    }

    // --- JSON roundtrip for CloudStorageConfig ---

    #[test]
    fn cloud_config_json_roundtrip() {
        let config = CloudStorageConfig::s3("my-bucket")
            .with_region("ap-southeast-1")
            .with_base_path("releases");

        let json = serde_json::to_string_pretty(&config).unwrap();
        let parsed: CloudStorageConfig = serde_json::from_str(&json).unwrap();
        assert_yaml_snapshot!("json_output", json);
        assert_yaml_snapshot!("parsed_back", parsed);
    }

    // --- Debug snapshot tests for storage config/options ---

    #[test]
    fn snapshot_debug_storage_type_all() {
        let types = vec![
            StorageType::File,
            StorageType::S3,
            StorageType::Gcs,
            StorageType::Azure,
        ];
        insta::assert_debug_snapshot!(types);
    }

    #[test]
    fn snapshot_debug_cloud_config_all_options() {
        let config = CloudStorageConfig::s3("release-artifacts")
            .with_region("eu-central-1")
            .with_base_path("shipper/state")
            .with_endpoint("https://minio.internal:9000")
            .with_credentials("ACCESS_KEY", "SECRET_KEY")
            .with_session_token("session-token-xyz");
        insta::assert_debug_snapshot!(config);
    }

    #[test]
    fn snapshot_debug_cloud_config_defaults() {
        insta::assert_debug_snapshot!(CloudStorageConfig::default());
    }

    #[test]
    fn snapshot_debug_file_storage() {
        let storage = FileStorage::new(PathBuf::from("/mock/path"));
        insta::assert_debug_snapshot!(storage);
    }

    // --- New hardened snapshot tests ---

    #[test]
    fn snapshot_atomic_write_roundtrip_state() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());

        storage.write("state.json", b"{}").unwrap();
        storage.write("receipt.json", b"[]").unwrap();
        storage.write("events.jsonl", b"").unwrap();

        let mut files = storage.list("").unwrap();
        files.sort();

        let state: Vec<(&str, bool)> = vec![
            ("state.json exists", storage.exists("state.json").unwrap()),
            (
                "receipt.json exists",
                storage.exists("receipt.json").unwrap(),
            ),
            (
                "events.jsonl exists",
                storage.exists("events.jsonl").unwrap(),
            ),
            ("state.tmp absent", !td.path().join("state.tmp").exists()),
            (
                "receipt.tmp absent",
                !td.path().join("receipt.tmp").exists(),
            ),
        ];
        assert_yaml_snapshot!("atomic_write_roundtrip_files", files);
        assert_yaml_snapshot!("atomic_write_roundtrip_state", state);
    }

    #[test]
    fn snapshot_delete_lifecycle() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());

        storage.write("ephemeral.txt", b"temp data").unwrap();

        let before = storage.exists("ephemeral.txt").unwrap();
        let content = String::from_utf8(storage.read("ephemeral.txt").unwrap()).unwrap();
        storage.delete("ephemeral.txt").unwrap();
        let after = storage.exists("ephemeral.txt").unwrap();

        let lifecycle: Vec<(&str, String)> = vec![
            ("exists_before_delete", before.to_string()),
            ("content", content),
            ("exists_after_delete", after.to_string()),
        ];
        assert_yaml_snapshot!(lifecycle);
    }
}
