//! Cloud storage backends for state persistence.
//!
//! This module provides storage backends for cloud storage services (S3, GCS, Azure Blob)
//! to enable distributed/team workflows. The backends implement the [`StorageBackend`] trait
//! which provides a common interface for storage operations.
//!
//! ## Feature Flags
//!
//! - `s3`: Enable AWS S3 storage backend
//! - `gcs`: Enable Google Cloud Storage backend
//! - `azure`: Enable Azure Blob Storage backend
//!
//! ## Usage
//!
//! ```ignore
//! use shipper::storage::{StorageBackend, S3Storage};
//!
//! let s3 = S3Storage::new("my-bucket", "us-east-1", None)?;
//! let data = s3.read("path/to/file.json")?;
//! ```

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

#[cfg(feature = "s3")]
pub mod s3 {
    //! AWS S3 storage backend.
    //!
    //! This module provides an S3-based storage backend using the AWS SDK.
    //! Enable the `s3` feature flag to use this backend.

    use super::{CloudStorageConfig, Result, StorageBackend, StorageType};
    use aws_config::defaults::{self, BehaviorVersion};
    use aws_credential_types::provider::ProvideCredentials;
    use aws_sdk_s3::{Client, config::AsyncClient};
    use std::sync::Arc;

    /// S3 storage backend implementation.
    pub struct S3Storage {
        client: Client,
        bucket: String,
        base_path: String,
        region: String,
    }

    impl S3Storage {
        /// Create a new S3Storage from configuration.
        ///
        /// # Arguments
        /// * `config` - Cloud storage configuration
        ///
        /// # Returns
        /// * `Result<Self>` - New S3Storage instance or error
        pub fn new(config: &CloudStorageConfig) -> Result<Self> {
            let region = config
                .region
                .clone()
                .unwrap_or_else(|| "us-east-1".to_string());

            // Build SDK config
            let mut builder = defaults(BehaviorVersion::latest())
                .region(aws_sdk_s3::config::Region::new(region.clone()));

            // Add credentials if provided
            if let (Some(access_key), Some(secret_key)) = (
                config.access_key_id.as_ref(),
                config.secret_access_key.as_ref(),
            ) {
                let credentials =
                    aws_credential_types::provider::static_provider::StaticCredentials::new(
                        aws_sdk_s3::config::Credentials::new(
                            access_key.clone(),
                            secret_key.clone(),
                            config.session_token.clone(),
                            None,
                            "shipper",
                        ),
                    );
                builder = builder.credentials_provider(credentials);
            }

            // Add custom endpoint if provided (for S3-compatible services)
            if let Some(endpoint) = &config.endpoint {
                builder = builder.endpoint_url(endpoint.clone());
            }

            let client = Client::from_conf(builder.load().await);

            Ok(Self {
                client,
                bucket: config.bucket.clone(),
                base_path: config.base_path.clone(),
                region,
            })
        }

        /// Create a new S3Storage with simple credentials.
        ///
        /// # Arguments
        /// * `bucket` - S3 bucket name
        /// * `region` - AWS region
        /// * `credentials` - Optional tuple of (access_key_id, secret_access_key)
        ///
        /// # Returns
        /// * `Result<Self>` - New S3Storage instance or error
        pub async fn with_credentials(
            bucket: impl Into<String>,
            region: impl Into<String>,
            credentials: Option<(impl Into<String>, impl Into<String>)>,
        ) -> Result<Self> {
            let config = CloudStorageConfig::new(StorageType::S3, bucket).with_region(region);

            if let Some((access_key, secret_key)) = credentials {
                S3Storage::new(&config.with_credentials(access_key, secret_key))
            } else {
                S3Storage::new(&config)
            }
        }

        /// Get the full S3 key for a relative path
        fn full_key(&self, path: &str) -> String {
            if self.base_path.is_empty() {
                path.to_string()
            } else {
                format!("{}/{}", self.base_path.trim_end_matches('/'), path)
            }
        }
    }

    impl StorageBackend for S3Storage {
        fn read(&self, path: &str) -> Result<Vec<u8>> {
            let key = self.full_key(path);
            let output = self
                .client
                .get_object()
                .bucket(&self.bucket)
                .key(&key)
                .send()
                .blocking()
                .with_context(|| {
                    format!("failed to read S3 object s3://{}/{}", self.bucket, key)
                })?;

            let bytes = output
                .body
                .collect()
                .blocking()
                .with_context(|| format!("failed to read S3 object body"))?
                .to_vec();

            Ok(bytes)
        }

        fn write(&self, path: &str, data: &[u8]) -> Result<()> {
            let key = self.full_key(path);
            self.client
                .put_object()
                .bucket(&self.bucket)
                .key(&key)
                .body(aws_sdk_s3::primitives::ByteStream::from(data.to_vec()))
                .send()
                .blocking()
                .with_context(|| {
                    format!("failed to write S3 object s3://{}/{}", self.bucket, key)
                })?;

            Ok(())
        }

        fn delete(&self, path: &str) -> Result<()> {
            let key = self.full_key(path);
            self.client
                .delete_object()
                .bucket(&self.bucket)
                .key(&key)
                .send()
                .blocking()
                .with_context(|| {
                    format!("failed to delete S3 object s3://{}/{}", self.bucket, key)
                })?;

            Ok(())
        }

        fn exists(&self, path: &str) -> Result<bool> {
            let key = self.full_key(path);
            let result = self
                .client
                .head_object()
                .bucket(&self.bucket)
                .key(&key)
                .send()
                .blocking();

            match result {
                Ok(_) => Ok(true),
                Err(e) => {
                    if e.as_service_error()
                        .map(|s| s.is_not_found())
                        .unwrap_or(false)
                    {
                        Ok(false)
                    } else {
                        Err(anyhow::anyhow!(
                            "failed to check S3 object existence: {}",
                            e
                        ))
                    }
                }
            }
        }

        fn storage_type(&self) -> StorageType {
            StorageType::S3
        }

        fn bucket(&self) -> &str {
            &self.bucket
        }

        fn base_path(&self) -> &str {
            &self.base_path
        }
    }
}

#[cfg(feature = "gcs")]
pub mod gcs {
    //! Google Cloud Storage backend.
    //!
    //! This module provides a GCS-based storage backend using the Google Cloud SDK.
    //! Enable the `gcs` feature flag to use this backend.

    use super::{CloudStorageConfig, Result, StorageBackend, StorageType};
    use std::sync::Arc;

    /// GCS storage backend implementation.
    pub struct GcsStorage {
        client: google_cloud_storage::client::Client,
        bucket: String,
        base_path: String,
        project_id: Option<String>,
    }

    impl GcsStorage {
        /// Create a new GcsStorage from configuration.
        ///
        /// # Arguments
        /// * `config` - Cloud storage configuration
        ///
        /// # Returns
        /// * `Result<Self>` - New GcsStorage instance or error
        pub fn new(config: &CloudStorageConfig) -> Result<Self> {
            let client = google_cloud_storage::client::Client::default();

            Ok(Self {
                client,
                bucket: config.bucket.clone(),
                base_path: config.base_path.clone(),
                project_id: config.region.clone(), // Use region as project ID
            })
        }

        /// Get the full GCS object name for a relative path
        fn full_object_name(&self, path: &str) -> String {
            if self.base_path.is_empty() {
                path.to_string()
            } else {
                format!("{}/{}", self.base_path.trim_end_matches('/'), path)
            }
        }
    }

    impl StorageBackend for GcsStorage {
        fn read(&self, path: &str) -> Result<Vec<u8>> {
            let object_name = self.full_object_name(path);
            let data = self
                .client
                .download_object(&self.bucket, &object_name, None)
                .with_context(|| {
                    format!(
                        "failed to read GCS object gs://{}/{}",
                        self.bucket, object_name
                    )
                })?;

            Ok(data)
        }

        fn write(&self, path: &str, data: &[u8]) -> Result<()> {
            let object_name = self.full_object_name(path);
            self.client
                .upload_object(
                    &self.bucket,
                    &object_name,
                    data,
                    &google_cloud_storage::http::objects::upload::UploadType::SimpleUpload(
                        google_cloud_storage::http::objects::upload::SimpleUploadRequest {
                            name: object_name,
                            ..Default::default()
                        },
                    ),
                )
                .with_context(|| {
                    format!(
                        "failed to write GCS object gs://{}/{}",
                        self.bucket, object_name
                    )
                })?;

            Ok(())
        }

        fn delete(&self, path: &str) -> Result<()> {
            let object_name = self.full_object_name(path);
            self.client
                .delete_object(&self.bucket, &object_name, None)
                .with_context(|| {
                    format!(
                        "failed to delete GCS object gs://{}/{}",
                        self.bucket, object_name
                    )
                })?;

            Ok(())
        }

        fn exists(&self, path: &str) -> Result<bool> {
            let object_name = self.full_object_name(path);
            match self.client.get_object(&self.bucket, &object_name, None) {
                Ok(_) => Ok(true),
                Err(e) => {
                    if e.to_string().contains("NotFound") || e.to_string().contains("404") {
                        Ok(false)
                    } else {
                        Err(e)
                    }
                }
            }
        }

        fn storage_type(&self) -> StorageType {
            StorageType::Gcs
        }

        fn bucket(&self) -> &str {
            &self.bucket
        }

        fn base_path(&self) -> &str {
            &self.base_path
        }
    }
}

#[cfg(feature = "azure")]
pub mod azure {
    //! Azure Blob Storage backend.
    //!
    //! This module provides an Azure Blob-based storage backend using the Azure SDK.
    //! Enable the `azure` feature flag to use this backend.

    use super::{CloudStorageConfig, Result, StorageBackend, StorageType};
    use azure_storage::StorageCredentials;
    use azure_storage_blobs::container::operations::PutBlockBlobResponse;
    use azure_storage_blobs::prelude::BlobClient;
    use std::sync::Arc;

    /// Azure Blob Storage backend implementation.
    pub struct AzureStorage {
        client: azure_storage_blobs::prelude::ContainerClient,
        container: String,
        base_path: String,
    }

    impl AzureStorage {
        /// Create a new AzureStorage from configuration.
        ///
        /// # Arguments
        /// * `config` - Cloud storage configuration
        ///
        /// # Returns
        /// * `Result<Self>` - New AzureStorage instance or error
        pub fn new(config: &CloudStorageConfig) -> Result<Self> {
            let account_name = config
                .access_key_id
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Azure storage requires account_name"))?
                .clone();
            let account_key = config
                .secret_access_key
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Azure storage requires account_key"))?
                .clone();

            let credentials = StorageCredentials::account_key(account_name.clone(), account_key);
            let storage_account = azure_storage::StorageAccount::new(
                format!("https://{}.blob.core.windows.net", account_name),
                credentials,
            );
            let client = storage_account.container_client(config.bucket.clone());

            Ok(Self {
                client,
                container: config.bucket.clone(),
                base_path: config.base_path.clone(),
            })
        }

        /// Create a new AzureStorage with connection string.
        ///
        /// # Arguments
        /// * `connection_string` - Azure storage connection string
        /// * `container` - Container name
        /// * `base_path` - Base path within container
        ///
        /// # Returns
        /// * `Result<Self>` - New AzureStorage instance or error
        pub fn from_connection_string(
            connection_string: impl Into<String>,
            container: impl Into<String>,
            base_path: impl Into<String>,
        ) -> Result<Self> {
            let (account, key) = azure_storage::connection_string_parser::parse_connection_string(
                &connection_string.into(),
            )
            .map_err(|e| anyhow::anyhow!("Failed to parse connection string: {}", e))?;

            let credentials = StorageCredentials::account_key(account, key);
            let storage_account = azure_storage::StorageAccount::new(
                format!("https://{}.blob.core.windows.net", account),
                credentials,
            );
            let client = storage_account.container_client(container.into());

            Ok(Self {
                client,
                container: container.into(),
                base_path: base_path.into(),
            })
        }

        /// Get the full blob name for a relative path
        fn full_blob_name(&self, path: &str) -> String {
            if self.base_path.is_empty() {
                path.to_string()
            } else {
                format!("{}/{}", self.base_path.trim_end_matches('/'), path)
            }
        }

        /// Get a blob client for the given path
        fn blob_client(&self, path: &str) -> BlobClient {
            let blob_name = self.full_blob_name(path);
            self.client.blob_client(blob_name)
        }
    }

    impl StorageBackend for AzureStorage {
        fn read(&self, path: &str) -> Result<Vec<u8>> {
            let blob_client = self.blob_client(path);
            let data = blob_client
                .get_content()
                .execute()
                .with_context(|| {
                    format!(
                        "failed to read Azure blob {} in container {}",
                        path, self.container
                    )
                })?
                .data
                .to_vec();

            Ok(data)
        }

        fn write(&self, path: &str, data: &[u8]) -> Result<()> {
            let blob_client = self.blob_client(path);
            blob_client
                .put_block_blob(data)
                .content_type("application/octet-stream")
                .execute()
                .with_context(|| {
                    format!(
                        "failed to write Azure blob {} in container {}",
                        path, self.container
                    )
                })?;

            Ok(())
        }

        fn delete(&self, path: &str) -> Result<()> {
            let blob_client = self.blob_client(path);
            blob_client.delete().execute().with_context(|| {
                format!(
                    "failed to delete Azure blob {} in container {}",
                    path, self.container
                )
            })?;

            Ok(())
        }

        fn exists(&self, path: &str) -> Result<bool> {
            let blob_client = self.blob_client(path);
            match blob_client.get().execute() {
                Ok(_) => Ok(true),
                Err(e) => {
                    if e.to_string().contains("BlobNotFound") || e.to_string().contains("404") {
                        Ok(false)
                    } else {
                        Err(e)
                    }
                }
            }
        }

        fn storage_type(&self) -> StorageType {
            StorageType::Azure
        }

        fn bucket(&self) -> &str {
            &self.container
        }

        fn base_path(&self) -> &str {
            &self.base_path
        }
    }
}

/// Build a storage backend from configuration.
///
/// This function creates the appropriate storage backend based on the
/// configuration type. Returns an error if the required feature flag
/// is not enabled for the requested storage type.
pub fn build_storage_backend(config: &CloudStorageConfig) -> Result<Box<dyn StorageBackend>> {
    match config.storage_type {
        StorageType::File => {
            // File storage doesn't need any feature flags
            Ok(Box::new(FileStorage::new(PathBuf::from(
                config.base_path.clone(),
            ))))
        }
        #[cfg(feature = "s3")]
        StorageType::S3 => {
            use crate::storage::s3::S3Storage;
            Ok(Box::new(S3Storage::new(config)?))
        }
        #[cfg(not(feature = "s3"))]
        StorageType::S3 => {
            anyhow::bail!("S3 storage requires the 's3' feature flag")
        }
        #[cfg(feature = "gcs")]
        StorageType::Gcs => {
            use crate::storage::gcs::GcsStorage;
            Ok(Box::new(GcsStorage::new(config)?))
        }
        #[cfg(not(feature = "gcs"))]
        StorageType::Gcs => {
            anyhow::bail!("GCS storage requires the 'gcs' feature flag")
        }
        #[cfg(feature = "azure")]
        StorageType::Azure => {
            use crate::storage::azure::AzureStorage;
            Ok(Box::new(AzureStorage::new(config)?))
        }
        #[cfg(not(feature = "azure"))]
        StorageType::Azure => {
            anyhow::bail!("Azure storage requires the 'azure' feature flag")
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
    use std::env;
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
