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

    // ── Core encrypt/decrypt roundtrip ──────────────────────────────────

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

    // ── Empty input ─────────────────────────────────────────────────────

    #[test]
    fn encrypt_decrypt_empty_input() {
        let plaintext = b"";
        let passphrase = "test-passphrase";

        let encrypted = encrypt(plaintext, passphrase).expect("encryption should succeed");
        let encrypted_str = String::from_utf8(encrypted).expect("valid UTF-8");
        let decrypted = decrypt(&encrypted_str, passphrase).expect("decryption should succeed");

        assert_eq!(plaintext.to_vec(), decrypted);
    }

    #[test]
    fn encrypt_empty_with_empty_passphrase() {
        let plaintext = b"";
        let passphrase = "";

        let encrypted = encrypt(plaintext, passphrase).expect("encryption should succeed");
        let encrypted_str = String::from_utf8(encrypted).expect("valid UTF-8");
        let decrypted = decrypt(&encrypted_str, passphrase).expect("decryption should succeed");

        assert_eq!(plaintext.to_vec(), decrypted);
    }

    // ── Large input ─────────────────────────────────────────────────────

    #[test]
    fn encrypt_decrypt_large_input() {
        // 1 MiB of data
        let plaintext: Vec<u8> = (0..1_048_576).map(|i| (i % 256) as u8).collect();
        let passphrase = "large-data-passphrase";

        let encrypted = encrypt(&plaintext, passphrase).expect("encryption should succeed");
        let encrypted_str = String::from_utf8(encrypted).expect("valid UTF-8");
        let decrypted = decrypt(&encrypted_str, passphrase).expect("decryption should succeed");

        assert_eq!(plaintext, decrypted);
    }

    #[test]
    fn encrypt_decrypt_single_byte() {
        let plaintext = b"\x42";
        let passphrase = "single-byte";

        let encrypted = encrypt(plaintext, passphrase).expect("encryption should succeed");
        let encrypted_str = String::from_utf8(encrypted).expect("valid UTF-8");
        let decrypted = decrypt(&encrypted_str, passphrase).expect("decryption should succeed");

        assert_eq!(plaintext.to_vec(), decrypted);
    }

    // ── Decrypt error cases ─────────────────────────────────────────────

    #[test]
    fn decrypt_invalid_base64_fails() {
        let result = decrypt("not-valid-base64!!!", "passphrase");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("base64"),
            "error should mention base64, got: {err}"
        );
    }

    #[test]
    fn decrypt_too_short_data_fails() {
        // Encode data that is too short (less than salt + nonce + 16-byte tag)
        let short_data = vec![0u8; SALT_SIZE + NONCE_SIZE + 15];
        let encoded = BASE64.encode(&short_data);

        let result = decrypt(&encoded, "passphrase");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("too short"),
            "error should mention 'too short', got: {err}"
        );
    }

    #[test]
    fn decrypt_corrupted_ciphertext_fails() {
        let plaintext = b"Some data to encrypt";
        let passphrase = "test-pass";

        let encrypted = encrypt(plaintext, passphrase).expect("encryption should succeed");
        let encrypted_str = String::from_utf8(encrypted).expect("valid UTF-8");

        // Decode, corrupt a byte in the ciphertext region, re-encode
        let mut raw = BASE64.decode(&encrypted_str).expect("valid base64");
        let idx = SALT_SIZE + NONCE_SIZE + 1;
        raw[idx] ^= 0xFF;
        let corrupted = BASE64.encode(&raw);

        let result = decrypt(&corrupted, passphrase);
        assert!(result.is_err());
    }

    #[test]
    fn decrypt_corrupted_salt_fails() {
        let plaintext = b"Some data";
        let passphrase = "test-pass";

        let encrypted = encrypt(plaintext, passphrase).expect("encryption should succeed");
        let encrypted_str = String::from_utf8(encrypted).expect("valid UTF-8");

        // Flip a bit in the salt region
        let mut raw = BASE64.decode(&encrypted_str).expect("valid base64");
        raw[0] ^= 0xFF;
        let corrupted = BASE64.encode(&raw);

        let result = decrypt(&corrupted, passphrase);
        assert!(result.is_err());
    }

    #[test]
    fn decrypt_corrupted_nonce_fails() {
        let plaintext = b"Some data";
        let passphrase = "test-pass";

        let encrypted = encrypt(plaintext, passphrase).expect("encryption should succeed");
        let encrypted_str = String::from_utf8(encrypted).expect("valid UTF-8");

        // Flip a bit in the nonce region
        let mut raw = BASE64.decode(&encrypted_str).expect("valid base64");
        raw[SALT_SIZE] ^= 0xFF;
        let corrupted = BASE64.encode(&raw);

        let result = decrypt(&corrupted, passphrase);
        assert!(result.is_err());
    }

    #[test]
    fn decrypt_empty_string_fails() {
        let result = decrypt("", "passphrase");
        assert!(result.is_err());
    }

    #[test]
    fn decrypt_exactly_minimum_length_minus_one_fails() {
        // Exactly salt + nonce + 15 bytes (one less than the 16-byte auth tag)
        let data = vec![0u8; SALT_SIZE + NONCE_SIZE + 15];
        let encoded = BASE64.encode(&data);
        assert!(decrypt(&encoded, "pass").is_err());
    }

    #[test]
    fn decrypt_exactly_minimum_length_fails_with_wrong_key() {
        // salt + nonce + 16 bytes of garbage "ciphertext"
        let data = vec![0u8; SALT_SIZE + NONCE_SIZE + 16];
        let encoded = BASE64.encode(&data);
        // Passes the length check but fails decryption
        assert!(decrypt(&encoded, "pass").is_err());
    }

    // ── is_encrypted heuristic ──────────────────────────────────────────

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
    fn is_encrypted_rejects_empty_string() {
        assert!(!is_encrypted(""));
    }

    #[test]
    fn is_encrypted_rejects_short_base64() {
        // Valid base64 but too short to be encrypted data
        let short = BASE64.encode(vec![0u8; SALT_SIZE + NONCE_SIZE + 10]);
        assert!(!is_encrypted(&short));
    }

    #[test]
    fn is_encrypted_rejects_non_base64() {
        assert!(!is_encrypted("definitely not base64 $$$ !!!"));
    }

    // ── Passphrase edge cases ───────────────────────────────────────────

    #[test]
    fn roundtrip_with_unicode_passphrase() {
        let plaintext = b"Unicode passphrase test";
        let passphrase = "pässwörd-密码-🔑";

        let encrypted = encrypt(plaintext, passphrase).expect("encryption should succeed");
        let encrypted_str = String::from_utf8(encrypted).expect("valid UTF-8");
        let decrypted = decrypt(&encrypted_str, passphrase).expect("decryption should succeed");

        assert_eq!(plaintext.to_vec(), decrypted);
    }

    #[test]
    fn roundtrip_with_very_long_passphrase() {
        let plaintext = b"Long passphrase test";
        let passphrase: String = "a".repeat(10_000);

        let encrypted = encrypt(plaintext, &passphrase).expect("encryption should succeed");
        let encrypted_str = String::from_utf8(encrypted).expect("valid UTF-8");
        let decrypted = decrypt(&encrypted_str, &passphrase).expect("decryption should succeed");

        assert_eq!(plaintext.to_vec(), decrypted);
    }

    #[test]
    fn different_passphrases_produce_different_ciphertexts_when_decoded() {
        let plaintext = b"Same plaintext";
        let pass1 = "passphrase-one";
        let pass2 = "passphrase-two";

        let enc1 = encrypt(plaintext, pass1).expect("encrypt");
        let enc2 = encrypt(plaintext, pass2).expect("encrypt");

        // Different passphrases → different raw bytes (even ignoring salt/nonce randomness)
        let raw1 = BASE64
            .decode(String::from_utf8(enc1).expect("utf8"))
            .expect("base64");
        let raw2 = BASE64
            .decode(String::from_utf8(enc2).expect("utf8"))
            .expect("base64");

        // Ciphertext portions must differ
        let ct1 = &raw1[SALT_SIZE + NONCE_SIZE..];
        let ct2 = &raw2[SALT_SIZE + NONCE_SIZE..];
        assert_ne!(ct1, ct2);
    }

    // ── Binary / non-UTF8 plaintext ─────────────────────────────────────

    #[test]
    fn roundtrip_binary_data() {
        let plaintext: Vec<u8> = (0..=255).collect();
        let passphrase = "binary-data-test";

        let encrypted = encrypt(&plaintext, passphrase).expect("encrypt");
        let encrypted_str = String::from_utf8(encrypted).expect("valid UTF-8");
        let decrypted = decrypt(&encrypted_str, passphrase).expect("decrypt");

        assert_eq!(plaintext, decrypted);
    }

    #[test]
    fn roundtrip_all_zero_bytes() {
        let plaintext = vec![0u8; 1024];
        let passphrase = "zeroes";

        let encrypted = encrypt(&plaintext, passphrase).expect("encrypt");
        let encrypted_str = String::from_utf8(encrypted).expect("valid UTF-8");
        let decrypted = decrypt(&encrypted_str, passphrase).expect("decrypt");

        assert_eq!(plaintext, decrypted);
    }

    // ── derive_key ──────────────────────────────────────────────────────

    #[test]
    fn derive_key_produces_consistent_output() {
        let passphrase = "test-passphrase";
        let salt = [0u8; SALT_SIZE];

        let key1 = derive_key(passphrase, &salt);
        let key2 = derive_key(passphrase, &salt);

        assert_eq!(key1, key2);
    }

    #[test]
    fn derive_key_different_salts_produce_different_keys() {
        let passphrase = "test-passphrase";
        let salt1 = [0u8; SALT_SIZE];
        let mut salt2 = [0u8; SALT_SIZE];
        salt2[0] = 1;

        let key1 = derive_key(passphrase, &salt1);
        let key2 = derive_key(passphrase, &salt2);

        assert_ne!(key1, key2);
    }

    #[test]
    fn derive_key_different_passphrases_produce_different_keys() {
        let salt = [42u8; SALT_SIZE];

        let key1 = derive_key("passphrase-a", &salt);
        let key2 = derive_key("passphrase-b", &salt);

        assert_ne!(key1, key2);
    }

    #[test]
    fn derive_key_empty_passphrase() {
        let salt = [0u8; SALT_SIZE];
        // Should not panic – just produces a deterministic key
        let key1 = derive_key("", &salt);
        let key2 = derive_key("", &salt);
        assert_eq!(key1, key2);
    }

    // ── EncryptionConfig ────────────────────────────────────────────────

    #[test]
    fn encryption_config_default_is_disabled() {
        let cfg = EncryptionConfig::default();
        assert!(!cfg.enabled);
        assert!(cfg.passphrase.is_none());
        assert!(cfg.env_var.is_none());
    }

    #[test]
    fn encryption_config_new_is_enabled() {
        let cfg = EncryptionConfig::new("secret".to_string());
        assert!(cfg.enabled);
        assert_eq!(cfg.passphrase.as_deref(), Some("secret"));
        assert!(cfg.env_var.is_none());
    }

    #[test]
    fn encryption_config_from_env_is_enabled() {
        let cfg = EncryptionConfig::from_env("MY_VAR".to_string());
        assert!(cfg.enabled);
        assert!(cfg.passphrase.is_none());
        assert_eq!(cfg.env_var.as_deref(), Some("MY_VAR"));
    }

    #[test]
    fn encryption_config_get_passphrase_direct() {
        let cfg = EncryptionConfig::new("hello".to_string());
        assert_eq!(cfg.get_passphrase().unwrap(), Some("hello".to_string()));
    }

    #[test]
    fn encryption_config_get_passphrase_none_when_disabled() {
        let cfg = EncryptionConfig::default();
        assert_eq!(cfg.get_passphrase().unwrap(), None);
    }

    #[test]
    fn encryption_config_serde_roundtrip() {
        let cfg = EncryptionConfig::new("test".to_string());
        let json = serde_json::to_string(&cfg).expect("serialize");
        let deserialized: EncryptionConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deserialized.enabled, cfg.enabled);
        assert_eq!(deserialized.passphrase, cfg.passphrase);
    }

    #[test]
    fn encryption_config_serde_skips_none_fields() {
        let cfg = EncryptionConfig::default();
        let json = serde_json::to_string(&cfg).expect("serialize");
        assert!(!json.contains("passphrase"));
        assert!(!json.contains("env_var"));
    }

    // ── StateEncryption ─────────────────────────────────────────────────

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
        let encrypted_str = String::from_utf8(encrypted).expect("valid UTF-8");
        let decrypted =
            decrypt(&encrypted_str, "my-secret-passphrase").expect("decryption should succeed");

        assert_eq!(data.to_vec(), decrypted);
    }

    #[test]
    fn state_encryption_decrypt_roundtrip() {
        let config = EncryptionConfig::new("my-pass".to_string());
        let encryption = StateEncryption::new(config).expect("should create");

        let data = b"state data to encrypt";
        let encrypted = encryption.encrypt(data).expect("encrypt");
        let decrypted = encryption.decrypt(&encrypted).expect("decrypt");

        assert_eq!(data.to_vec(), decrypted);
    }

    #[test]
    fn state_encryption_disabled_passthrough() {
        let config = EncryptionConfig::default();
        let encryption = StateEncryption::new(config).expect("should create");

        let data = b"Plain text data";

        let result = encryption.decrypt(data).expect("should succeed");
        assert_eq!(data.to_vec(), result);
    }

    #[test]
    fn state_encryption_disabled_encrypt_passthrough_on_decrypt() {
        // When disabled, decrypt returns data as-is even if it looks like garbage
        let config = EncryptionConfig::default();
        let encryption = StateEncryption::new(config).expect("should create");

        let garbage = b"\x00\x01\x02\x03";
        let result = encryption.decrypt(garbage).expect("should succeed");
        assert_eq!(garbage.to_vec(), result);
    }

    // ── encrypt output format ───────────────────────────────────────────

    #[test]
    fn encrypt_produces_valid_base64() {
        let plaintext = b"Test data";
        let passphrase = "test";

        let encrypted = encrypt(plaintext, passphrase).expect("should encrypt");
        let encrypted_str = String::from_utf8(encrypted.clone()).expect("valid UTF-8");

        let decoded = BASE64.decode(&encrypted_str).expect("valid base64");
        assert!(decoded.len() > plaintext.len());
    }

    #[test]
    fn encrypted_output_has_expected_structure() {
        let plaintext = b"Hello";
        let passphrase = "test";

        let encrypted = encrypt(plaintext, passphrase).expect("encrypt");
        let encrypted_str = String::from_utf8(encrypted).expect("valid UTF-8");
        let raw = BASE64.decode(&encrypted_str).expect("base64");

        // raw = salt(16) + nonce(12) + ciphertext(len(plaintext) + 16 for GCM tag)
        let expected_len = SALT_SIZE + NONCE_SIZE + plaintext.len() + 16;
        assert_eq!(raw.len(), expected_len);
    }

    // ── File I/O ────────────────────────────────────────────────────────

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
    fn read_decrypted_wrong_passphrase_fails() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("test.enc");

        write_encrypted(&path, b"data", "correct").expect("write");
        let result = read_decrypted(&path, "wrong");
        assert!(result.is_err());
    }

    #[test]
    fn read_decrypted_nonexistent_file_fails() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("does-not-exist.enc");

        let result = read_decrypted(&path, "pass");
        assert!(result.is_err());
    }

    #[test]
    fn write_encrypted_file_is_base64_on_disk() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("test.enc");

        write_encrypted(&path, b"data", "pass").expect("write");
        let on_disk = std::fs::read_to_string(&path).expect("read");

        // Should be valid base64
        assert!(BASE64.decode(&on_disk).is_ok());
        // Should NOT be the plaintext
        assert_ne!(on_disk, "data");
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

    #[test]
    fn state_encryption_disabled_writes_plaintext() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("plain.json");

        let config = EncryptionConfig::default();
        let encryption = StateEncryption::new(config).expect("create");

        let data = b"plain text content";
        encryption.write_file(&path, data).expect("write");

        let on_disk = std::fs::read(&path).expect("read");
        assert_eq!(data.to_vec(), on_disk);
    }

    #[test]
    fn state_encryption_disabled_reads_plaintext() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("plain.txt");
        std::fs::write(&path, "hello").expect("write");

        let config = EncryptionConfig::default();
        let encryption = StateEncryption::new(config).expect("create");

        let content = encryption.read_file(&path).expect("read");
        assert_eq!(content, "hello");
    }

    #[test]
    fn state_encryption_read_nonexistent_file_fails() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("nope.json");

        let config = EncryptionConfig::new("pass".to_string());
        let encryption = StateEncryption::new(config).expect("create");

        assert!(encryption.read_file(&path).is_err());
    }
}

// ── Property-based tests ────────────────────────────────────────────────

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn roundtrip_arbitrary_data(data in proptest::collection::vec(any::<u8>(), 0..4096)) {
            let passphrase = "prop-test-pass";
            let encrypted = encrypt(&data, passphrase).expect("encrypt");
            let encrypted_str = String::from_utf8(encrypted).expect("valid UTF-8");
            let decrypted = decrypt(&encrypted_str, passphrase).expect("decrypt");
            prop_assert_eq!(data, decrypted);
        }

        #[test]
        fn roundtrip_arbitrary_passphrase(passphrase in "\\PC{1,200}") {
            let plaintext = b"fixed plaintext for passphrase fuzz";
            let encrypted = encrypt(plaintext, &passphrase).expect("encrypt");
            let encrypted_str = String::from_utf8(encrypted).expect("valid UTF-8");
            let decrypted = decrypt(&encrypted_str, &passphrase).expect("decrypt");
            prop_assert_eq!(plaintext.to_vec(), decrypted);
        }

        #[test]
        fn roundtrip_arbitrary_data_and_passphrase(
            data in proptest::collection::vec(any::<u8>(), 0..1024),
            passphrase in "\\PC{1,100}",
        ) {
            let encrypted = encrypt(&data, &passphrase).expect("encrypt");
            let encrypted_str = String::from_utf8(encrypted).expect("valid UTF-8");
            let decrypted = decrypt(&encrypted_str, &passphrase).expect("decrypt");
            prop_assert_eq!(data, decrypted);
        }

        #[test]
        fn encrypted_output_is_valid_base64(data in proptest::collection::vec(any::<u8>(), 0..512)) {
            let encrypted = encrypt(&data, "test").expect("encrypt");
            let encrypted_str = String::from_utf8(encrypted).expect("valid UTF-8");
            prop_assert!(BASE64.decode(&encrypted_str).is_ok());
        }

        #[test]
        fn wrong_passphrase_always_fails(
            data in proptest::collection::vec(any::<u8>(), 1..512),
            correct in "[a-z]{8,16}",
            wrong in "[A-Z]{8,16}",
        ) {
            // Ensure passphrases actually differ
            prop_assume!(correct != wrong);
            let encrypted = encrypt(&data, &correct).expect("encrypt");
            let encrypted_str = String::from_utf8(encrypted).expect("valid UTF-8");
            prop_assert!(decrypt(&encrypted_str, &wrong).is_err());
        }

        #[test]
        fn encrypted_size_is_deterministic(data in proptest::collection::vec(any::<u8>(), 0..2048)) {
            let encrypted = encrypt(&data, "pass").expect("encrypt");
            let encrypted_str = String::from_utf8(encrypted).expect("valid UTF-8");
            let raw = BASE64.decode(&encrypted_str).expect("base64");
            // salt(16) + nonce(12) + plaintext_len + gcm_tag(16)
            let expected = SALT_SIZE + NONCE_SIZE + data.len() + 16;
            prop_assert_eq!(raw.len(), expected);
        }
    }
}
