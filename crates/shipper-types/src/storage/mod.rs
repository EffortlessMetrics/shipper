//! Storage backend configuration types.
//!
//! These are pure data types describing which storage backend to use. Embedders
//! use [`CloudStorageConfig`] and [`StorageType`] to declare their storage
//! choice through the stable `shipper-types` contract crate.
//!
//! The runtime trait (`StorageBackend`) and filesystem implementation live in
//! `shipper::ops::storage` as crate-private internals — only filesystem storage
//! is fully implemented today, so we do not promise a public `StorageBackend`
//! trait until cloud backends are real.

use serde::{Deserialize, Serialize};

/// Represents the type of storage backend.
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

/// Error returned when parsing an unknown storage type name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseStorageTypeError(pub String);

impl std::fmt::Display for ParseStorageTypeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unknown storage type: {}", self.0)
    }
}

impl std::error::Error for ParseStorageTypeError {}

impl std::str::FromStr for StorageType {
    type Err = ParseStorageTypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "file" | "local" => Ok(StorageType::File),
            "s3" => Ok(StorageType::S3),
            "gcs" | "gs" => Ok(StorageType::Gcs),
            "azure" | "blob" => Ok(StorageType::Azure),
            _ => Err(ParseStorageTypeError(s.to_string())),
        }
    }
}

/// Configuration for any storage backend.
///
/// Pure data: no I/O, no policy decisions. Embedders construct this to
/// describe "use this storage backend" via the stable contract surface.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudStorageConfig {
    /// Storage type (file, s3, gcs, azure)
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

/// Error returned when validating a [`CloudStorageConfig`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidateStorageConfigError(pub String);

impl std::fmt::Display for ValidateStorageConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ValidateStorageConfigError {}

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
    pub fn validate(&self) -> Result<(), ValidateStorageConfigError> {
        match self.storage_type {
            StorageType::File => {
                // File storage is always valid
                Ok(())
            }
            StorageType::S3 | StorageType::Gcs | StorageType::Azure => {
                if self.bucket.is_empty() {
                    Err(ValidateStorageConfigError(
                        "bucket/container name is required for cloud storage".to_string(),
                    ))
                } else {
                    Ok(())
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

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
    fn cloud_storage_config_full_path_trailing_slash() {
        let config = CloudStorageConfig::s3("b").with_base_path("prefix/");
        assert_eq!(config.full_path("key.json"), "prefix/key.json");
    }

    #[test]
    fn cloud_storage_config_validate() {
        let config = CloudStorageConfig::file("/path");
        assert!(config.validate().is_ok());

        let config2 = CloudStorageConfig::s3(""); // Empty bucket
        assert!(config2.validate().is_err());
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

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        fn safe_name_strategy() -> impl Strategy<Value = String> {
            "[a-zA-Z0-9][a-zA-Z0-9_]{0,19}".prop_filter("non-empty", |s| !s.is_empty())
        }

        proptest! {
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
        }
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use insta::assert_yaml_snapshot;
    use std::str::FromStr;

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
    fn storage_type_default_snap() {
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
}
