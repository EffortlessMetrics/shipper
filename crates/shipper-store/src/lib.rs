//! State store abstraction for shipper.
//!
//! This crate provides a trait-based abstraction for state storage,
//! allowing for future implementations like S3, GCS, or Azure Blob Storage.
//!
//! # Example
//!
//! ```
//! use shipper_store::{StateStore, FileStore, SchemaVersion};
//! use std::path::PathBuf;
//!
//! let store = FileStore::new(PathBuf::from(".shipper"));
//!
//! // Validate schema version
//! let version = SchemaVersion::parse("shipper.receipt.v2").expect("parse");
//! assert!(version.is_supported(1));
//! ```

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Minimum supported schema version
pub const MINIMUM_SUPPORTED_VERSION: u32 = 1;

/// Current schema version
pub const CURRENT_VERSION: u32 = 2;

/// Schema version for state files
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaVersion {
    /// Version number
    pub version: u32,
}

impl SchemaVersion {
    /// Parse a schema version from a string like "shipper.receipt.v2"
    pub fn parse(s: &str) -> Result<Self> {
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() != 3 || !parts[0].starts_with("shipper") || !parts[2].starts_with('v') {
            return Err(anyhow::anyhow!("invalid schema version format: {}", s));
        }

        let version_part = &parts[2][1..]; // Skip 'v'
        let version = version_part
            .parse::<u32>()
            .with_context(|| format!("invalid version number: {}", s))?;

        Ok(Self { version })
    }

    /// Create a new schema version
    pub fn new(version: u32) -> Self {
        Self { version }
    }

    /// Get the version number
    pub fn version(&self) -> u32 {
        self.version
    }

    /// Check if this version is supported (>= minimum)
    pub fn is_supported(&self, minimum: u32) -> bool {
        self.version >= minimum
    }

    /// Format as a version string
    pub fn to_version_string(&self, prefix: &str) -> String {
        format!("shipper.{}.v{}", prefix, self.version)
    }
}

impl Default for SchemaVersion {
    fn default() -> Self {
        Self { version: CURRENT_VERSION }
    }
}

impl std::fmt::Display for SchemaVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "v{}", self.version)
    }
}

/// Validate a schema version string
pub fn validate_schema_version(version: &str) -> Result<()> {
    let parsed = SchemaVersion::parse(version)?;
    
    if !parsed.is_supported(MINIMUM_SUPPORTED_VERSION) {
        anyhow::bail!(
            "schema version {} is too old. Minimum supported version is v{}",
            version,
            MINIMUM_SUPPORTED_VERSION
        );
    }

    Ok(())
}

/// Metadata about stored state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateMetadata {
    /// Schema version
    pub schema_version: SchemaVersion,
    /// When the state was created
    pub created_at: DateTime<Utc>,
    /// When the state was last updated
    pub updated_at: DateTime<Utc>,
    /// Optional checksum for integrity verification
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checksum: Option<String>,
}

impl StateMetadata {
    /// Create new metadata with current timestamp
    pub fn new() -> Self {
        let now = Utc::now();
        Self {
            schema_version: SchemaVersion::default(),
            created_at: now,
            updated_at: now,
            checksum: None,
        }
    }

    /// Update the modified timestamp
    pub fn touch(&mut self) {
        self.updated_at = Utc::now();
    }
}

impl Default for StateMetadata {
    fn default() -> Self {
        Self::new()
    }
}

/// Trait for state storage backends.
///
/// This trait abstracts the storage of state and receipts,
/// allowing for different storage backends (filesystem, S3, GCS, etc.).
pub trait StateStore: Send + Sync {
    /// Check if state exists
    fn exists(&self) -> bool;

    /// Clear all state
    fn clear(&self) -> Result<()>;

    /// Get the storage location description
    fn location(&self) -> String;
}

/// Trait for state data storage
pub trait DataStore<T>: StateStore {
    /// Save data to storage
    fn save(&self, data: &T) -> Result<()>;

    /// Load data from storage
    fn load(&self) -> Result<Option<T>>;
}

/// Filesystem-based state store implementation.
#[derive(Debug, Clone)]
pub struct FileStore {
    state_dir: PathBuf,
}

impl FileStore {
    /// Create a new FileStore with the specified state directory
    pub fn new(state_dir: PathBuf) -> Self {
        Self { state_dir }
    }

    /// Get the state directory path
    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    /// Get path to a file in the state directory
    pub fn file_path(&self, name: &str) -> PathBuf {
        self.state_dir.join(name)
    }

    /// Ensure the state directory exists
    pub fn ensure_dir(&self) -> Result<()> {
        if !self.state_dir.exists() {
            std::fs::create_dir_all(&self.state_dir)
                .with_context(|| format!("failed to create state dir: {}", self.state_dir.display()))?;
        }
        Ok(())
    }

    /// Write data to a file atomically
    pub fn write_file(&self, name: &str, content: &[u8]) -> Result<()> {
        self.ensure_dir()?;

        let path = self.file_path(name);
        let tmp_path = path.with_extension("tmp");

        std::fs::write(&tmp_path, content)
            .with_context(|| format!("failed to write file: {}", tmp_path.display()))?;

        std::fs::rename(&tmp_path, &path)
            .with_context(|| format!("failed to rename file to: {}", path.display()))?;

        Ok(())
    }

    /// Read data from a file
    pub fn read_file(&self, name: &str) -> Result<Option<Vec<u8>>> {
        let path = self.file_path(name);
        if !path.exists() {
            return Ok(None);
        }

        let content = std::fs::read(&path)
            .with_context(|| format!("failed to read file: {}", path.display()))?;

        Ok(Some(content))
    }

    /// Delete a file
    pub fn delete_file(&self, name: &str) -> Result<()> {
        let path = self.file_path(name);
        if path.exists() {
            std::fs::remove_file(&path)
                .with_context(|| format!("failed to delete file: {}", path.display()))?;
        }
        Ok(())
    }

    /// Check if a file exists
    pub fn file_exists(&self, name: &str) -> bool {
        self.file_path(name).exists()
    }

    /// List all files in the state directory
    pub fn list_files(&self) -> Result<Vec<String>> {
        if !self.state_dir.exists() {
            return Ok(Vec::new());
        }

        let mut files = Vec::new();
        for entry in std::fs::read_dir(&self.state_dir)
            .with_context(|| format!("failed to read dir: {}", self.state_dir.display()))?
        {
            let entry = entry?;
            if entry.file_type()?.is_file()
                && let Some(name) = entry.file_name().to_str()
            {
                files.push(name.to_string());
            }
        }

        Ok(files)
    }

    /// Save JSON data to a file
    pub fn save_json<T: Serialize>(&self, name: &str, data: &T) -> Result<()> {
        let content = serde_json::to_string_pretty(data)
            .context("failed to serialize JSON")?;
        self.write_file(name, content.as_bytes())
    }

    /// Load JSON data from a file
    pub fn load_json<T: for<'de> Deserialize<'de>>(&self, name: &str) -> Result<Option<T>> {
        let content = self.read_file(name)?;
        match content {
            Some(data) => {
                let parsed: T = serde_json::from_slice(&data)
                    .with_context(|| format!("failed to parse JSON from: {}", name))?;
                Ok(Some(parsed))
            }
            None => Ok(None),
        }
    }
}

impl StateStore for FileStore {
    fn exists(&self) -> bool {
        self.state_dir.exists()
    }

    fn clear(&self) -> Result<()> {
        if self.state_dir.exists() {
            std::fs::remove_dir_all(&self.state_dir)
                .with_context(|| format!("failed to remove state dir: {}", self.state_dir.display()))?;
        }
        Ok(())
    }

    fn location(&self) -> String {
        self.state_dir.display().to_string()
    }
}

/// Store statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StoreStats {
    /// Number of files in the store
    pub file_count: usize,
    /// Total size in bytes
    pub total_size: u64,
    /// Oldest file timestamp
    pub oldest_file: Option<DateTime<Utc>>,
    /// Newest file timestamp
    pub newest_file: Option<DateTime<Utc>>,
}

impl FileStore {
    /// Get statistics about the store
    pub fn stats(&self) -> Result<StoreStats> {
        let files = self.list_files()?;
        let mut stats = StoreStats {
            file_count: files.len(),
            ..Default::default()
        };

        for name in &files {
            let path = self.file_path(name);
            if let Ok(metadata) = std::fs::metadata(&path) {
                stats.total_size += metadata.len();
                
                if let Ok(modified) = metadata.modified() {
                    let datetime: DateTime<Utc> = modified.into();
                    if stats.oldest_file.is_none() || datetime < stats.oldest_file.unwrap() {
                        stats.oldest_file = Some(datetime);
                    }
                    if stats.newest_file.is_none() || datetime > stats.newest_file.unwrap() {
                        stats.newest_file = Some(datetime);
                    }
                }
            }
        }

        Ok(stats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn schema_version_parse_v1() {
        let v = SchemaVersion::parse("shipper.receipt.v1").expect("parse");
        assert_eq!(v.version(), 1);
    }

    #[test]
    fn schema_version_parse_v2() {
        let v = SchemaVersion::parse("shipper.receipt.v2").expect("parse");
        assert_eq!(v.version(), 2);
    }

    #[test]
    fn schema_version_parse_invalid_format() {
        assert!(SchemaVersion::parse("invalid").is_err());
        assert!(SchemaVersion::parse("shipper.receipt").is_err());
        assert!(SchemaVersion::parse("shipper.receipt.2").is_err());
        assert!(SchemaVersion::parse("other.receipt.v2").is_err());
    }

    #[test]
    fn schema_version_is_supported() {
        let v = SchemaVersion::new(2);
        assert!(v.is_supported(1));
        assert!(v.is_supported(2));
        assert!(!v.is_supported(3));
    }

    #[test]
    fn schema_version_to_string() {
        let v = SchemaVersion::new(2);
        assert_eq!(v.to_version_string("receipt"), "shipper.receipt.v2");
        assert_eq!(v.to_string(), "v2");
    }

    #[test]
    fn schema_version_default() {
        let v = SchemaVersion::default();
        assert_eq!(v.version(), CURRENT_VERSION);
    }

    #[test]
    fn validate_schema_version_accepts_v1() {
        assert!(validate_schema_version("shipper.receipt.v1").is_ok());
    }

    #[test]
    fn validate_schema_version_accepts_v2() {
        assert!(validate_schema_version("shipper.receipt.v2").is_ok());
    }

    #[test]
    fn validate_schema_version_rejects_v0() {
        assert!(validate_schema_version("shipper.receipt.v0").is_err());
    }

    #[test]
    fn state_metadata_new() {
        let meta = StateMetadata::new();
        assert_eq!(meta.schema_version.version, CURRENT_VERSION);
        assert!(meta.checksum.is_none());
    }

    #[test]
    fn state_metadata_touch() {
        let mut meta = StateMetadata::new();
        let original = meta.updated_at;
        std::thread::sleep(std::time::Duration::from_millis(10));
        meta.touch();
        assert!(meta.updated_at > original);
    }

    #[test]
    fn file_store_new() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());
        assert_eq!(store.state_dir(), td.path());
    }

    #[test]
    fn file_store_exists() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());
        assert!(store.exists());

        let store2 = FileStore::new(td.path().join("nonexistent"));
        assert!(!store2.exists());
    }

    #[test]
    fn file_store_write_and_read_file() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        store.write_file("test.txt", b"hello world").expect("write");

        let content = store.read_file("test.txt").expect("read");
        assert!(content.is_some());
        assert_eq!(content.unwrap(), b"hello world");
    }

    #[test]
    fn file_store_read_missing_file() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        let content = store.read_file("missing.txt").expect("read");
        assert!(content.is_none());
    }

    #[test]
    fn file_store_delete_file() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        store.write_file("test.txt", b"data").expect("write");
        assert!(store.file_exists("test.txt"));

        store.delete_file("test.txt").expect("delete");
        assert!(!store.file_exists("test.txt"));
    }

    #[test]
    fn file_store_list_files() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        store.write_file("a.txt", b"a").expect("write");
        store.write_file("b.txt", b"b").expect("write");

        let files = store.list_files().expect("list");
        assert_eq!(files.len(), 2);
        assert!(files.contains(&"a.txt".to_string()));
        assert!(files.contains(&"b.txt".to_string()));
    }

    #[test]
    fn file_store_save_and_load_json() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        #[derive(Debug, Serialize, Deserialize, PartialEq)]
        struct TestData {
            name: String,
            value: i32,
        }

        let data = TestData {
            name: "test".to_string(),
            value: 42,
        };

        store.save_json("test.json", &data).expect("save");

        let loaded: TestData = store.load_json("test.json").expect("load").expect("some");
        assert_eq!(loaded, data);
    }

    #[test]
    fn file_store_load_missing_json() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        #[derive(Serialize, Deserialize)]
        struct TestData {
            name: String,
        }

        let loaded: Option<TestData> = store.load_json("missing.json").expect("load");
        assert!(loaded.is_none());
    }

    #[test]
    fn file_store_clear() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        store.write_file("test.txt", b"data").expect("write");
        assert!(store.exists());
        assert!(store.file_exists("test.txt"));

        store.clear().expect("clear");
        assert!(!store.exists());
    }

    #[test]
    fn file_store_location() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());
        assert!(store.location().contains(td.path().to_str().unwrap()));
    }

    #[test]
    fn file_store_stats() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        store.write_file("a.txt", b"hello").expect("write");
        store.write_file("b.txt", b"world").expect("write");

        let stats = store.stats().expect("stats");
        assert_eq!(stats.file_count, 2);
        assert_eq!(stats.total_size, 10); // 5 + 5 bytes
        assert!(stats.newest_file.is_some());
        assert!(stats.oldest_file.is_some());
    }

    #[test]
    fn file_store_stats_empty() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        let stats = store.stats().expect("stats");
        assert_eq!(stats.file_count, 0);
        assert_eq!(stats.total_size, 0);
        assert!(stats.newest_file.is_none());
        assert!(stats.oldest_file.is_none());
    }
}