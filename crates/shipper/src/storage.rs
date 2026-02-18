//! Storage backends for state persistence.
//!
//! This module provides the [`StorageBackend`] trait and a filesystem-backed
//! implementation ([`FileStorage`]). Cloud backends (S3, GCS, Azure Blob)
//! are planned for a future release.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Represents the type of storage backend
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
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

    /// Get the storage type
    fn storage_type(&self) -> StorageType;

    /// Get the bucket/container name
    fn bucket(&self) -> &str;

    /// Get the base path within the storage
    fn base_path(&self) -> &str;
}

/// Configuration for cloud storage backends
#[derive(Debug, Clone)]
pub struct CloudStorageConfig {
    /// Storage type (s3, gcs, azure)
    pub storage_type: StorageType,
    /// Bucket/container name
    pub bucket: String,
    /// Region for S3, project ID for GCS
    pub region: Option<String>,
    /// Base path within the bucket
    pub base_path: String,
    /// Custom endpoint (for S3-compatible services like MinIO, DigitalOcean Spaces)
    pub endpoint: Option<String>,
    /// Access key ID
    pub access_key_id: Option<String>,
    /// Secret access key
    pub secret_access_key: Option<String>,
    /// Session token (for temporary credentials)
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
}

/// Filesystem-based storage backend.
///
/// This is the default implementation that stores data in a local directory.
pub struct FileStorage {
    base_path: PathBuf,
}

impl FileStorage {
    /// Create a new FileStorage with the specified base path
    pub fn new(base_path: PathBuf) -> Self {
        Self { base_path }
    }

    /// Get the base path
    pub fn base_path(&self) -> &PathBuf {
        &self.base_path
    }
}

impl StorageBackend for FileStorage {
    fn read(&self, path: &str) -> Result<Vec<u8>> {
        let full_path = self.base_path.join(path);
        std::fs::read(&full_path)
            .with_context(|| format!("failed to read file {}", full_path.display()))
    }

    fn write(&self, path: &str, data: &[u8]) -> Result<()> {
        let full_path = self.base_path.join(path);
        // Create parent directories if they don't exist
        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create directory {}", parent.display()))?;
        }
        std::fs::write(&full_path, data)
            .with_context(|| format!("failed to write file {}", full_path.display()))
    }

    fn delete(&self, path: &str) -> Result<()> {
        let full_path = self.base_path.join(path);
        if full_path.exists() {
            std::fs::remove_file(&full_path)
                .with_context(|| format!("failed to delete file {}", full_path.display()))?;
        }
        Ok(())
    }

    fn exists(&self, path: &str) -> Result<bool> {
        let full_path = self.base_path.join(path);
        Ok(full_path.exists())
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
/// Currently only filesystem storage is supported. Cloud backends (S3, GCS, Azure)
/// are planned for a future release.
pub fn build_storage_backend(config: &CloudStorageConfig) -> Result<Box<dyn StorageBackend>> {
    match config.storage_type {
        StorageType::File => Ok(Box::new(FileStorage::new(PathBuf::from(
            config.base_path.clone(),
        )))),
        StorageType::S3 => {
            anyhow::bail!("S3 storage is not yet implemented")
        }
        StorageType::Gcs => {
            anyhow::bail!("GCS storage is not yet implemented")
        }
        StorageType::Azure => {
            anyhow::bail!("Azure storage is not yet implemented")
        }
    }
}

/// Get a storage backend from environment variables.
///
/// This is a convenience function that reads configuration from environment variables:
/// - SHIPPER_STORAGE_TYPE: file, s3, gcs, or azure
/// - SHIPPER_STORAGE_BUCKET: bucket/container name
/// - SHIPPER_STORAGE_REGION: region (for S3) or project ID (for GCS)
/// - SHIPPER_STORAGE_BASE_PATH: base path within bucket
/// - SHIPPER_STORAGE_ENDPOINT: custom endpoint (for S3-compatible services)
/// - SHIPPER_STORAGE_ACCESS_KEY_ID: access key ID
/// - SHIPPER_STORAGE_SECRET_ACCESS_KEY: secret access key
/// - SHIPPER_STORAGE_SESSION_TOKEN: session token (optional)
///
/// # Returns
/// * `Option<CloudStorageConfig>` - Configuration if env vars are set, None otherwise
pub fn config_from_env() -> Option<CloudStorageConfig> {
    use std::env;

    let storage_type = match env::var("SHIPPER_STORAGE_TYPE").ok()?.as_str() {
        "file" => StorageType::File,
        "s3" => StorageType::S3,
        "gcs" => StorageType::Gcs,
        "azure" => StorageType::Azure,
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
    fn test_file_storage_read_write() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());

        // Write some data
        storage
            .write("test.txt", b"hello world")
            .expect("write should succeed");

        // Read it back
        let data = storage.read("test.txt").expect("read should succeed");
        assert_eq!(data, b"hello world");

        // Check exists
        assert!(storage.exists("test.txt").expect("exists should succeed"));
        assert!(
            !storage
                .exists("missing.txt")
                .expect("exists should succeed")
        );
    }

    #[test]
    fn test_file_storage_delete() {
        let td = tempdir().expect("tempdir");
        let storage = FileStorage::new(td.path().to_path_buf());

        // Write and then delete
        storage
            .write("test.txt", b"hello")
            .expect("write should succeed");
        assert!(storage.exists("test.txt").expect("exists should succeed"));

        storage.delete("test.txt").expect("delete should succeed");
        assert!(!storage.exists("test.txt").expect("exists should succeed"));
    }

    #[test]
    fn test_cloud_storage_config() {
        let config = CloudStorageConfig::new(StorageType::S3, "my-bucket")
            .with_region("us-west-2")
            .with_base_path("shipper/state")
            .with_credentials("access-key", "secret-key");

        assert_eq!(config.storage_type, StorageType::S3);
        assert_eq!(config.bucket, "my-bucket");
        assert_eq!(config.region, Some("us-west-2".to_string()));
        assert_eq!(config.base_path, "shipper/state");
        assert_eq!(config.access_key_id, Some("access-key".to_string()));
        assert_eq!(config.secret_access_key, Some("secret-key".to_string()));
    }

    #[test]
    fn test_cloud_storage_config_full_path() {
        let config =
            CloudStorageConfig::new(StorageType::S3, "my-bucket").with_base_path("shipper");

        assert_eq!(config.full_path("state.json"), "shipper/state.json");
        assert_eq!(config.full_path("receipt.json"), "shipper/receipt.json");
    }

    #[test]
    fn test_cloud_storage_config_full_path_no_base() {
        let config = CloudStorageConfig::new(StorageType::S3, "my-bucket");

        assert_eq!(config.full_path("state.json"), "state.json");
    }

    #[test]
    fn test_storage_type_display() {
        assert_eq!(StorageType::File.to_string(), "file");
        assert_eq!(StorageType::S3.to_string(), "s3");
        assert_eq!(StorageType::Gcs.to_string(), "gcs");
        assert_eq!(StorageType::Azure.to_string(), "azure");
    }
}
