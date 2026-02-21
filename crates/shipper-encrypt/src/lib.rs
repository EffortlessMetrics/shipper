//! State file encryption using AES-256-GCM with PBKDF2 key derivation.
//!
//! This crate provides transparent encryption and decryption of sensitive data using
//! AES-256-GCM with PBKDF2 key derivation from user passphrases.
//!
//! ## Usage
//!
//! ```
//! use shipper_encrypt::{encrypt, decrypt};
//!
//! let plaintext = b"Secret data";
//! let passphrase = "my-secret-passphrase";
//!
//! let encrypted = encrypt(plaintext, passphrase).expect("encryption failed");
//! let encrypted_str = String::from_utf8(encrypted).expect("valid UTF-8");
//! let decrypted = decrypt(&encrypted_str, passphrase).expect("decryption failed");
//!
//! assert_eq!(plaintext.to_vec(), decrypted);
//! ```
//!
//! ## Security
//!
//! - Uses AES-256-GCM for authenticated encryption
//! - PBKDF2 with 100,000 iterations for key derivation
//! - Random salt and nonce for each encryption operation
//! - Encrypted data format: base64(salt || nonce || ciphertext || auth_tag)

use std::path::Path;

use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit, OsRng, rand_core::RngCore},
};
use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use pbkdf2::pbkdf2_hmac_array;
use serde::{Deserialize, Serialize};
use sha2::Sha256;

/// Size of the salt for key derivation (16 bytes)
const SALT_SIZE: usize = 16;
/// Size of the nonce for AES-GCM (12 bytes)
const NONCE_SIZE: usize = 12;
/// Number of PBKDF2 iterations
const PBKDF2_ITERATIONS: u32 = 100_000;
/// Size of the derived key (256 bits for AES-256)
const KEY_SIZE: usize = 32;

/// Encryption configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EncryptionConfig {
    /// Whether encryption is enabled
    #[serde(default)]
    pub enabled: bool,
    /// Passphrase for encryption/decryption (if enabled)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub passphrase: Option<String>,
    /// Environment variable name to read passphrase from
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env_var: Option<String>,
}

impl EncryptionConfig {
    /// Create a new encryption config with the given passphrase
    pub fn new(passphrase: String) -> Self {
        Self {
            enabled: true,
            passphrase: Some(passphrase),
            env_var: None,
        }
    }

    /// Create a new encryption config that reads passphrase from environment variable
    pub fn from_env(env_var: String) -> Self {
        Self {
            enabled: true,
            passphrase: None,
            env_var: Some(env_var),
        }
    }

    /// Get the passphrase, either directly or from environment
    pub fn get_passphrase(&self) -> Result<Option<String>> {
        if let Some(passphrase) = &self.passphrase {
            return Ok(Some(passphrase.clone()));
        }

        if let Some(ref env_var) = self.env_var {
            return Ok(std::env::var(env_var).ok());
        }

        Ok(None)
    }
}

/// Encrypt data using AES-256-GCM with PBKDF2 key derivation
///
/// # Arguments
/// * `data` - The plaintext data to encrypt
/// * `passphrase` - The passphrase to derive the encryption key from
///
/// # Returns
/// Base64-encoded encrypted data with format: salt || nonce || ciphertext
///
/// # Example
///
/// ```
/// use shipper_encrypt::encrypt;
///
/// let data = b"Secret message";
/// let passphrase = "my-passphrase";
///
/// let encrypted = encrypt(data, passphrase).expect("encryption failed");
/// // encrypted is base64-encoded and can be safely stored as text
/// ```
pub fn encrypt(data: &[u8], passphrase: &str) -> Result<Vec<u8>> {
    // Generate random salt and nonce
    let mut salt = [0u8; SALT_SIZE];
    let mut nonce_bytes = [0u8; NONCE_SIZE];
    OsRng.fill_bytes(&mut salt);
    OsRng.fill_bytes(&mut nonce_bytes);

    // Derive key from passphrase using PBKDF2
    let key = derive_key(passphrase, &salt);

    // Create cipher and encrypt
    let cipher = Aes256Gcm::new_from_slice(&key).context("failed to create AES-256-GCM cipher")?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, data)
        .map_err(|e| anyhow::anyhow!("encryption failed: {:?}", e))?;

    // Format: salt || nonce || ciphertext
    let mut result = Vec::with_capacity(SALT_SIZE + NONCE_SIZE + ciphertext.len());
    result.extend_from_slice(&salt);
    result.extend_from_slice(&nonce_bytes);
    result.extend_from_slice(&ciphertext);

    // Return base64-encoded result
    Ok(BASE64.encode(&result).into_bytes())
}

/// Decrypt data using AES-256-GCM with PBKDF2 key derivation
///
/// # Arguments
/// * `encrypted_data` - Base64-encoded encrypted data (as string or bytes)
/// * `passphrase` - The passphrase to derive the decryption key from
///
/// # Returns
/// The decrypted plaintext data
///
/// # Example
///
/// ```
/// use shipper_encrypt::{encrypt, decrypt};
///
/// let data = b"Secret message";
/// let passphrase = "my-passphrase";
///
/// let encrypted = encrypt(data, passphrase).expect("encryption failed");
/// let encrypted_str = String::from_utf8(encrypted).expect("valid UTF-8");
/// let decrypted = decrypt(&encrypted_str, passphrase).expect("decryption failed");
///
/// assert_eq!(data.to_vec(), decrypted);
/// ```
pub fn decrypt(encrypted_data: impl AsRef<str>, passphrase: &str) -> Result<Vec<u8>> {
    let encrypted_str = encrypted_data.as_ref();
    // Decode base64
    let data = BASE64
        .decode(encrypted_str)
        .context("invalid base64 encoding")?;

    // Check minimum length
    if data.len() < SALT_SIZE + NONCE_SIZE + 16 {
        bail!("encrypted data too short");
    }

    // Extract salt, nonce, and ciphertext
    let salt = &data[..SALT_SIZE];
    let nonce_bytes = &data[SALT_SIZE..SALT_SIZE + NONCE_SIZE];
    let ciphertext = &data[SALT_SIZE + NONCE_SIZE..];

    // Derive key from passphrase using PBKDF2
    let key = derive_key(passphrase, salt);

    // Create cipher and decrypt
    let cipher = Aes256Gcm::new_from_slice(&key).context("failed to create AES-256-GCM cipher")?;
    let nonce = Nonce::from_slice(nonce_bytes);
    let plaintext = cipher.decrypt(nonce, ciphertext).map_err(|e| {
        anyhow::anyhow!(
            "decryption failed - wrong passphrase or corrupted data: {:?}",
            e
        )
    })?;

    Ok(plaintext)
}

/// Derive a 256-bit key from passphrase using PBKDF2-SHA256
fn derive_key(passphrase: &str, salt: &[u8]) -> [u8; KEY_SIZE] {
    pbkdf2_hmac_array::<Sha256, KEY_SIZE>(passphrase.as_bytes(), salt, PBKDF2_ITERATIONS)
}

/// Check if data appears to be encrypted (starts with base64-encoded salt)
/// This is a heuristic check - it may give false negatives for very short
/// or specially crafted plaintexts, but should work for normal JSON state files.
pub fn is_encrypted(content: &str) -> bool {
    // Try to decode as base64
    let Ok(data) = BASE64.decode(content) else {
        return false;
    };

    // Check minimum length for encrypted data
    if data.len() < SALT_SIZE + NONCE_SIZE + 16 {
        return false;
    }

    // Additional heuristic: encrypted data should have high entropy
    // and not be valid UTF-8 JSON (encrypted data is not valid JSON)
    // This is a simple check - we rely on the decryption to confirm

    true
}

/// Read and decrypt a file
///
/// # Arguments
/// * `path` - Path to the encrypted file
/// * `passphrase` - The passphrase to decrypt with
///
/// # Returns
/// The decrypted file contents as a string
pub fn read_decrypted(path: &Path, passphrase: &str) -> Result<String> {
    let encrypted = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read encrypted file: {}", path.display()))?;

    // Try to decrypt
    let decrypted = decrypt(&encrypted, passphrase)?;
    String::from_utf8(decrypted).context("decrypted data is not valid UTF-8")
}

/// Write and encrypt data to a file
///
/// # Arguments
/// * `path` - Path to the file
/// * `data` - The plaintext data to encrypt and write
/// * `passphrase` - The passphrase to encrypt with
pub fn write_encrypted(path: &Path, data: &[u8], passphrase: &str) -> Result<()> {
    let encrypted = encrypt(data, passphrase)?;

    // Write as base64 string
    let encrypted_str =
        String::from_utf8(encrypted).context("encrypted data is not valid UTF-8")?;

    std::fs::write(path, encrypted_str)
        .with_context(|| format!("failed to write encrypted file: {}", path.display()))?;

    Ok(())
}

/// Transparent encryption wrapper for file operations.
///
/// This provides a simple interface for encrypting/decrypting files
/// transparently without changing the rest of the codebase.
pub struct StateEncryption {
    config: EncryptionConfig,
}

impl StateEncryption {
    /// Create a new state encryption handler
    pub fn new(config: EncryptionConfig) -> Result<Self> {
        Ok(Self { config })
    }

    /// Get the passphrase, trying environment variable first if configured
    fn get_passphrase(&self) -> Result<Option<String>> {
        if !self.config.enabled {
            return Ok(None);
        }

        // Try env var first if configured
        if let Some(ref env_var) = self.config.env_var
            && let Ok(passphrase) = std::env::var(env_var)
        {
            return Ok(Some(passphrase));
        }

        // Fall back to direct passphrase
        self.config.get_passphrase()
    }

    /// Check if encryption is enabled and we have a passphrase
    pub fn is_enabled(&self) -> bool {
        self.config.enabled && self.get_passphrase().ok().flatten().is_some()
    }

    /// Encrypt data if encryption is enabled
    pub fn encrypt(&self, data: &[u8]) -> Result<Vec<u8>> {
        let passphrase = self.get_passphrase()?.context(
            "encryption is enabled but no passphrase available. Set SHIPPER_ENCRYPT_KEY environment variable or provide passphrase in config.",
        )?;

        encrypt(data, &passphrase)
    }

    /// Decrypt data if encryption is enabled
    pub fn decrypt(&self, data: &[u8]) -> Result<Vec<u8>> {
        // First, try to decrypt assuming it's encrypted
        if let Some(passphrase) = self.get_passphrase()? {
            // Try decryption first
            if let Ok(decrypted) = decrypt(String::from_utf8_lossy(data), &passphrase) {
                return Ok(decrypted);
            }
        }

        // If decryption didn't work or encryption not enabled, return original data
        // This allows for transparent fallback to unencrypted data
        Ok(data.to_vec())
    }

    /// Read and decrypt a file if encrypted
    pub fn read_file(&self, path: &Path) -> Result<String> {
        if !self.is_enabled() {
            // Read as plain text
            return std::fs::read_to_string(path)
                .with_context(|| format!("failed to read file: {}", path.display()));
        }

        let passphrase = self
            .get_passphrase()?
            .context("encryption is enabled but no passphrase available")?;

        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read file: {}", path.display()))?;

        // Try to decrypt - if it fails, assume it's not encrypted
        match decrypt(&content, &passphrase) {
            Ok(decrypted) => {
                String::from_utf8(decrypted).context("decrypted data is not valid UTF-8")
            }
            Err(_) => {
                // File might not be encrypted yet - try reading as plain
                Ok(content)
            }
        }
    }

    /// Write and encrypt a file if encryption is enabled
    pub fn write_file(&self, path: &Path, data: &[u8]) -> Result<()> {
        if !self.is_enabled() {
            // Write as plain text
            return std::fs::write(path, data)
                .with_context(|| format!("failed to write file: {}", path.display()));
        }

        let passphrase = self
            .get_passphrase()?
            .context("encryption is enabled but no passphrase available")?;

        let encrypted = encrypt(data, &passphrase)?;
        let encrypted_str =
            String::from_utf8(encrypted).context("encrypted data is not valid UTF-8")?;

        std::fs::write(path, encrypted_str)
            .with_context(|| format!("failed to write encrypted file: {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let plaintext = b"Hello, World! This is a test message.";
        let passphrase = "test-passphrase-123";

        let encrypted = encrypt(plaintext, passphrase).expect("encryption should succeed");
        let encrypted_str = String::from_utf8(encrypted).expect("valid UTF-8");
        let decrypted = decrypt(&encrypted_str, passphrase).expect("decryption should succeed");

        assert_eq!(plaintext.to_vec(), decrypted);
    }

    #[test]
    fn encrypt_produces_different_output_for_same_plaintext() {
        let plaintext = b"Hello, World!";
        let passphrase = "test-passphrase";

        let encrypted1 = encrypt(plaintext, passphrase).expect("encryption should succeed");
        let encrypted2 = encrypt(plaintext, passphrase).expect("encryption should succeed");

        // Should be different due to random salt/nonce
        assert_ne!(encrypted1, encrypted2);

        // But both should decrypt to the same plaintext
        let decrypted1 = decrypt(
            String::from_utf8(encrypted1).expect("valid UTF-8"),
            passphrase,
        )
        .expect("decryption should succeed");
        let decrypted2 = decrypt(
            String::from_utf8(encrypted2).expect("valid UTF-8"),
            passphrase,
        )
        .expect("decryption should succeed");

        assert_eq!(decrypted1, decrypted2);
    }

    #[test]
    fn decrypt_wrong_passphrase_fails() {
        let plaintext = b"Secret data";
        let passphrase = "correct-passphrase";
        let wrong_passphrase = "wrong-passphrase";

        let encrypted = encrypt(plaintext, passphrase).expect("encryption should succeed");
        let encrypted_str = String::from_utf8(encrypted).expect("valid UTF-8");

        let result = decrypt(&encrypted_str, wrong_passphrase);
        assert!(result.is_err());
    }

    #[test]
    fn is_encrypted_detects_encrypted_data() {
        let plaintext = b"Hello, World!";
        let passphrase = "test-passphrase";

        let encrypted = encrypt(plaintext, passphrase).expect("encryption should succeed");
        let encrypted_str = String::from_utf8(encrypted).expect("valid UTF-8");

        assert!(is_encrypted(&encrypted_str));
    }

    #[test]
    fn is_encrypted_rejects_plaintext() {
        let plaintext = r#"{"key": "value"}"#;
        assert!(!is_encrypted(plaintext));
    }

    #[test]
    fn state_encryption_enabled_disabled() {
        let config = EncryptionConfig::default();
        let encryption = StateEncryption::new(config.clone()).expect("should create");
        assert!(!encryption.is_enabled());

        let config = EncryptionConfig::new("test-passphrase".to_string());
        let encryption = StateEncryption::new(config).expect("should create");
        assert!(encryption.is_enabled());
    }

    #[test]
    fn state_encryption_roundtrip() {
        let config = EncryptionConfig::new("my-secret-passphrase".to_string());
        let encryption = StateEncryption::new(config).expect("should create");

        let data = b"Test state data";

        let encrypted = encryption.encrypt(data).expect("encryption should succeed");
        // Convert to string for decrypt
        let encrypted_str = String::from_utf8(encrypted).expect("valid UTF-8");
        let decrypted =
            decrypt(&encrypted_str, "my-secret-passphrase").expect("decryption should succeed");

        assert_eq!(data.to_vec(), decrypted);
    }

    #[test]
    fn state_encryption_disabled_passthrough() {
        let config = EncryptionConfig::default();
        let encryption = StateEncryption::new(config).expect("should create");

        let data = b"Plain text data";

        // Without encryption, should return data as-is
        let result = encryption.decrypt(data).expect("should succeed");
        assert_eq!(data.to_vec(), result);
    }

    #[test]
    fn encrypt_produces_valid_base64() {
        let plaintext = b"Test data";
        let passphrase = "test";

        let encrypted = encrypt(plaintext, passphrase).expect("should encrypt");
        let encrypted_str = String::from_utf8(encrypted.clone()).expect("valid UTF-8");

        // Should be valid base64 - decode and check it has expected length
        let decoded = BASE64.decode(&encrypted_str).expect("valid base64");
        // The decoded length should be greater than the plaintext (salt + nonce + ciphertext)
        assert!(decoded.len() > plaintext.len());
    }

    #[test]
    fn derive_key_produces_consistent_output() {
        let passphrase = "test-passphrase";
        let salt = [0u8; SALT_SIZE];

        let key1 = derive_key(passphrase, &salt);
        let key2 = derive_key(passphrase, &salt);

        // Same passphrase and salt should produce same key
        assert_eq!(key1, key2);
    }

    #[test]
    fn derive_key_different_salts_produce_different_keys() {
        let passphrase = "test-passphrase";
        let salt1 = [0u8; SALT_SIZE];
        let mut salt2 = [0u8; SALT_SIZE];
        salt2[0] = 1; // Different salt

        let key1 = derive_key(passphrase, &salt1);
        let key2 = derive_key(passphrase, &salt2);

        // Different salts should produce different keys
        assert_ne!(key1, key2);
    }

    #[test]
    fn read_write_encrypted_file() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("test.enc");

        let plaintext = b"Secret file content";
        let passphrase = "file-passphrase";

        write_encrypted(&path, plaintext, passphrase).expect("write encrypted");
        let decrypted = read_decrypted(&path, passphrase).expect("read decrypted");

        assert_eq!(plaintext.to_vec(), decrypted.into_bytes());
    }

    #[test]
    fn state_encryption_file_roundtrip() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("state.json");

        let config = EncryptionConfig::new("test-pass".to_string());
        let encryption = StateEncryption::new(config).expect("should create");

        let data = br#"{"key": "value"}"#;

        encryption.write_file(&path, data).expect("write file");
        let content = encryption.read_file(&path).expect("read file");

        assert_eq!(String::from_utf8_lossy(data), content);
    }

    #[test]
    fn state_encryption_unencrypted_fallback() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("plain.json");

        let config = EncryptionConfig::new("test-pass".to_string());
        let encryption = StateEncryption::new(config).expect("should create");

        // Write unencrypted file directly
        let data = r#"{"plain": "data"}"#;
        std::fs::write(&path, data).expect("write plain");

        // Should be able to read it back
        let content = encryption.read_file(&path).expect("read file");
        assert_eq!(data, content);
    }
}