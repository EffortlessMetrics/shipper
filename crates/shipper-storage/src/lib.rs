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
            std::fs::create_dir_all(&self.base_path)
                .with_context(|| format!("failed to create directory: {}", self.base_path.display()))?;
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
        
        // Write to a temp file first, then rename for atomicity
        let tmp_path = full_path.with_extension("tmp");
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

        storage.write("nested/deep/path/test.txt", b"data").expect("write");

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
}