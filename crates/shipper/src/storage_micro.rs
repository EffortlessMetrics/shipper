//! Storage backends for shipper, backed by `shipper-storage`.
//!
//! This shim keeps the in-crate storage module API stable while reusing the
//! shared microcrate implementation.

use std::env;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub use shipper_storage::{CloudStorageConfig, StorageBackend, StorageType};

/// Compatibility re-export for the concrete storage backend type.
///
/// The standalone `shipper-storage` crate uses `path()` as the accessor,
/// while the monolithic `shipper` module historically exposed `base_path()`.
/// This wrapper keeps the old API stable while preserving a clean boundary.
#[derive(Debug, Clone)]
pub struct FileStorage {
    inner: shipper_storage::FileStorage,
}

impl FileStorage {
    pub fn new(base_path: PathBuf) -> Self {
        Self {
            inner: shipper_storage::FileStorage::new(base_path),
        }
    }

    /// Get the base path for this storage backend.
    pub fn base_path(&self) -> &PathBuf {
        self.inner.path()
    }

    /// Path to the storage root.
    pub fn path(&self) -> &Path {
        self.inner.path()
    }
}

impl StorageBackend for FileStorage {
    fn read(&self, path: &str) -> Result<Vec<u8>> {
        self.inner.read(path).context("failed to read from storage")
    }

    fn write(&self, path: &str, data: &[u8]) -> Result<()> {
        self.inner.write(path, data).context("failed to write to storage")
    }

    fn delete(&self, path: &str) -> Result<()> {
        self.inner.delete(path).context("failed to delete from storage")
    }

    fn exists(&self, path: &str) -> Result<bool> {
        self.inner.exists(path).context("failed to check storage path")
    }

    fn list(&self, prefix: &str) -> Result<Vec<String>> {
        self.inner
            .list(prefix)
            .context("failed to list storage path")
    }

    fn storage_type(&self) -> StorageType {
        self.inner.storage_type()
    }

    fn bucket(&self) -> &str {
        self.inner.bucket()
    }

    fn base_path(&self) -> &str {
        self.inner.base_path()
    }
}

pub fn build_storage_backend(config: &CloudStorageConfig) -> Result<Box<dyn StorageBackend>> {
    if config.storage_type == StorageType::File {
        return Ok(Box::new(FileStorage::new(PathBuf::from(&config.base_path))));
    }

    shipper_storage::build_storage_backend(config).context("failed to build storage backend")
}

/// Keep the previous monolithic API shape where storage env support is opt-in.
pub fn config_from_env() -> Option<CloudStorageConfig> {
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

    #[test]
    fn unknown_storage_type_from_env_returns_none() {
        temp_env::with_vars([("SHIPPER_STORAGE_TYPE", Some("bogus")), ("SHIPPER_STORAGE_BUCKET", Some("bucket"))], || {
            assert!(config_from_env().is_none());
        });
    }

    #[test]
    fn file_storage_exposes_base_path_compatibly() {
        let file_storage = FileStorage::new(PathBuf::from("/tmp/shipper-storage"));
        assert_eq!(file_storage.base_path(), &PathBuf::from("/tmp/shipper-storage"));
        assert_eq!(file_storage.path(), Path::new("/tmp/shipper-storage"));
    }
}
