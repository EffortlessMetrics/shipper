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

use std::fmt;
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

impl fmt::Display for EncryptionConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.enabled {
            return write!(f, "encryption: disabled");
        }
        match (&self.passphrase, &self.env_var) {
            (Some(p), _) => write!(
                f,
                "encryption: enabled (passphrase: {})",
                mask_passphrase(p)
            ),
            (None, Some(var)) => write!(f, "encryption: enabled (env: {var})"),
            (None, None) => write!(f, "encryption: enabled (no passphrase configured)"),
        }
    }
}

/// Mask a passphrase for safe display, showing only the first and last
/// characters with asterisks in between. Passphrases with fewer than 3
/// characters are fully masked.
pub fn mask_passphrase(passphrase: &str) -> String {
    let chars: Vec<char> = passphrase.chars().collect();
    if chars.len() < 3 {
        return "*".repeat(chars.len().max(1));
    }
    let first = chars[0];
    let last = chars[chars.len() - 1];
    format!("{first}{}{last}", "*".repeat(chars.len() - 2))
}

impl fmt::Display for StateEncryption {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.config)
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

    // ── Edge-case: large data >1 MB ─────────────────────────────────────

    #[test]
    fn encrypt_decrypt_data_over_1mb() {
        // 2 MiB of pseudo-random data
        let plaintext: Vec<u8> = (0u64..2_097_152)
            .map(|i| (i.wrapping_mul(7) % 256) as u8)
            .collect();
        let passphrase = "large-2mb-passphrase";

        let encrypted = encrypt(&plaintext, passphrase).expect("encryption should succeed");
        let encrypted_str = String::from_utf8(encrypted).expect("valid UTF-8");
        let decrypted = decrypt(&encrypted_str, passphrase).expect("decryption should succeed");

        assert_eq!(plaintext, decrypted);
    }

    // ── Edge-case: key boundary values ──────────────────────────────────

    #[test]
    fn roundtrip_single_char_passphrase() {
        let plaintext = b"single char key";
        let passphrase = "x";

        let encrypted = encrypt(plaintext, passphrase).expect("encrypt");
        let encrypted_str = String::from_utf8(encrypted).expect("valid UTF-8");
        let decrypted = decrypt(&encrypted_str, passphrase).expect("decrypt");

        assert_eq!(plaintext.to_vec(), decrypted);
    }

    #[test]
    fn roundtrip_whitespace_only_passphrase() {
        let plaintext = b"whitespace key test";
        let passphrase = "   \t\n  ";

        let encrypted = encrypt(plaintext, passphrase).expect("encrypt");
        let encrypted_str = String::from_utf8(encrypted).expect("valid UTF-8");
        let decrypted = decrypt(&encrypted_str, passphrase).expect("decrypt");

        assert_eq!(plaintext.to_vec(), decrypted);
    }

    #[test]
    fn roundtrip_max_reasonable_passphrase() {
        let plaintext = b"max key test";
        // 100 KB passphrase
        let passphrase: String = "Z".repeat(100_000);

        let encrypted = encrypt(plaintext, &passphrase).expect("encrypt");
        let encrypted_str = String::from_utf8(encrypted).expect("valid UTF-8");
        let decrypted = decrypt(&encrypted_str, &passphrase).expect("decrypt");

        assert_eq!(plaintext.to_vec(), decrypted);
    }

    // ── Edge-case: nonce uniqueness at raw byte level ───────────────────

    #[test]
    fn nonce_uniqueness_raw_salt_and_nonce_differ() {
        let plaintext = b"nonce uniqueness check";
        let passphrase = "same-passphrase";

        let enc1 = encrypt(plaintext, passphrase).expect("encrypt");
        let enc2 = encrypt(plaintext, passphrase).expect("encrypt");

        let raw1 = BASE64
            .decode(String::from_utf8(enc1).expect("utf8"))
            .expect("base64");
        let raw2 = BASE64
            .decode(String::from_utf8(enc2).expect("utf8"))
            .expect("base64");

        let salt_nonce_1 = &raw1[..SALT_SIZE + NONCE_SIZE];
        let salt_nonce_2 = &raw2[..SALT_SIZE + NONCE_SIZE];

        // Random salt+nonce must differ between encryptions
        assert_ne!(salt_nonce_1, salt_nonce_2);
    }

    // ── Edge-case: tampered auth tag detection ──────────────────────────

    #[test]
    fn tampered_auth_tag_detected() {
        let plaintext = b"auth tag tamper test";
        let passphrase = "test-pass";

        let encrypted = encrypt(plaintext, passphrase).expect("encrypt");
        let encrypted_str = String::from_utf8(encrypted).expect("valid UTF-8");

        let mut raw = BASE64.decode(&encrypted_str).expect("base64");
        // Flip the last byte (inside the GCM auth tag)
        let last = raw.len() - 1;
        raw[last] ^= 0xFF;
        let corrupted = BASE64.encode(&raw);

        assert!(decrypt(&corrupted, passphrase).is_err());
    }

    #[test]
    fn tampered_single_bit_flip_detected() {
        let plaintext = b"bit flip detection";
        let passphrase = "bit-flip-pass";

        let encrypted = encrypt(plaintext, passphrase).expect("encrypt");
        let encrypted_str = String::from_utf8(encrypted).expect("valid UTF-8");

        let mut raw = BASE64.decode(&encrypted_str).expect("base64");
        // Flip a single bit in the middle of the ciphertext
        let mid = raw.len() / 2;
        raw[mid] ^= 0x01;
        let corrupted = BASE64.encode(&raw);

        assert!(decrypt(&corrupted, passphrase).is_err());
    }

    // ── Edge-case: wrong key returns error, not garbage ─────────────────

    #[test]
    fn wrong_key_returns_error_not_garbage_data() {
        let plaintext = b"This must not leak through wrong key";
        let correct = "correct-key";
        let wrong = "wrong-key";

        let encrypted = encrypt(plaintext, correct).expect("encrypt");
        let encrypted_str = String::from_utf8(encrypted).expect("valid UTF-8");

        let result = decrypt(&encrypted_str, wrong);
        // Must be Err — AES-GCM authenticated encryption must reject wrong keys
        assert!(
            result.is_err(),
            "wrong key must return Err, not Ok with garbage"
        );

        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("wrong passphrase or corrupted data"),
            "error message should indicate wrong passphrase, got: {err_msg}"
        );
    }

    #[test]
    fn wrong_key_similar_passphrase_returns_error() {
        let plaintext = b"subtle key difference";
        let correct = "my-passphrase-abc";
        let wrong = "my-passphrase-abd"; // off by one char

        let encrypted = encrypt(plaintext, correct).expect("encrypt");
        let encrypted_str = String::from_utf8(encrypted).expect("valid UTF-8");

        assert!(
            decrypt(&encrypted_str, wrong).is_err(),
            "even a single-char difference must cause decryption failure"
        );
    }

    // ── Realistic data roundtrips ───────────────────────────────────────

    #[test]
    fn roundtrip_realistic_json_state() {
        let state_json = br#"{
            "plan_id": "abc123",
            "workspace": "/home/user/project",
            "crates": [
                {"name": "core", "version": "0.1.0", "status": "published"},
                {"name": "cli", "version": "0.2.0", "status": "pending"}
            ],
            "started_at": "2024-01-15T10:30:00Z",
            "token": "cio_supersecrettoken1234567890"
        }"#;
        let passphrase = "ci-pipeline-key-2024";

        let encrypted = encrypt(state_json, passphrase).expect("encrypt");
        let encrypted_str = String::from_utf8(encrypted).expect("valid UTF-8");
        let decrypted = decrypt(&encrypted_str, passphrase).expect("decrypt");

        assert_eq!(state_json.to_vec(), decrypted);
    }

    #[test]
    fn roundtrip_event_log_jsonl() {
        let events = b"{\"event\":\"publish_start\",\"crate\":\"core\",\"ts\":1700000000}\n\
                       {\"event\":\"publish_ok\",\"crate\":\"core\",\"ts\":1700000005}\n\
                       {\"event\":\"publish_start\",\"crate\":\"cli\",\"ts\":1700000010}\n";
        let passphrase = "event-log-key";

        let encrypted = encrypt(events, passphrase).expect("encrypt");
        let encrypted_str = String::from_utf8(encrypted).expect("valid UTF-8");
        let decrypted = decrypt(&encrypted_str, passphrase).expect("decrypt");

        assert_eq!(events.to_vec(), decrypted);
    }

    // ── Key derivation edge cases ───────────────────────────────────────

    #[test]
    fn derive_key_always_produces_32_bytes() {
        for passphrase in ["", "a", "short", &"x".repeat(10_000)] {
            for salt in [&[0u8; 0][..], &[0u8; 1], &[0u8; SALT_SIZE], &[0xFF; 64]] {
                let key = derive_key(passphrase, salt);
                assert_eq!(
                    key.len(),
                    KEY_SIZE,
                    "key must be {KEY_SIZE} bytes for passphrase len={}, salt len={}",
                    passphrase.len(),
                    salt.len()
                );
            }
        }
    }

    #[test]
    fn derive_key_with_empty_salt() {
        let key1 = derive_key("passphrase", &[]);
        let key2 = derive_key("passphrase", &[]);
        assert_eq!(key1, key2, "empty salt should still be deterministic");
        assert_eq!(key1.len(), KEY_SIZE);
    }

    // ── AES block boundary tests ────────────────────────────────────────

    #[test]
    fn encrypt_decrypt_exactly_aes_block_size() {
        // AES block size is 16 bytes; GCM is a stream mode, but block boundary is interesting
        let plaintext = [0xABu8; 16];
        let passphrase = "block-boundary";

        let encrypted = encrypt(&plaintext, passphrase).expect("encrypt");
        let encrypted_str = String::from_utf8(encrypted).expect("valid UTF-8");
        let decrypted = decrypt(&encrypted_str, passphrase).expect("decrypt");

        assert_eq!(plaintext.to_vec(), decrypted);
    }

    #[test]
    fn encrypt_decrypt_multi_block_boundaries() {
        let passphrase = "multi-block";
        for size in [15, 16, 17, 31, 32, 33, 48, 64, 128, 255, 256, 257] {
            let plaintext: Vec<u8> = (0..size).map(|i| (i % 256) as u8).collect();
            let encrypted = encrypt(&plaintext, passphrase).expect("encrypt");
            let encrypted_str = String::from_utf8(encrypted).expect("valid UTF-8");
            let decrypted = decrypt(&encrypted_str, passphrase).expect("decrypt");
            assert_eq!(plaintext, decrypted, "roundtrip failed for size {size}");
        }
    }

    // ── StateEncryption error paths ─────────────────────────────────────

    #[test]
    fn state_encryption_encrypt_enabled_no_passphrase_errors() {
        let config = EncryptionConfig {
            enabled: true,
            passphrase: None,
            env_var: None,
        };
        let encryption = StateEncryption::new(config).expect("create");
        assert!(!encryption.is_enabled());

        let result = encryption.encrypt(b"data");
        assert!(result.is_err(), "encrypt with no passphrase should fail");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("no passphrase"),
            "error should mention missing passphrase, got: {err}"
        );
    }

    #[test]
    fn state_encryption_cross_config_decrypt_fails() {
        let config_a = EncryptionConfig::new("key-alpha".to_string());
        let config_b = EncryptionConfig::new("key-beta".to_string());
        let enc_a = StateEncryption::new(config_a).expect("create");
        let enc_b = StateEncryption::new(config_b).expect("create");

        let data = b"cross-config secret";
        let encrypted = enc_a.encrypt(data).expect("encrypt with A");

        // B's decrypt should fall back to returning raw data (not the plaintext)
        let result = enc_b.decrypt(&encrypted).expect("decrypt returns fallback");
        assert_ne!(
            result,
            data.to_vec(),
            "wrong config must not produce original plaintext"
        );
    }

    // ── Truncation / malformed ciphertext ───────────────────────────────

    #[test]
    fn decrypt_truncated_after_header_fails() {
        let plaintext = b"data to truncate";
        let passphrase = "trunc-pass";

        let encrypted = encrypt(plaintext, passphrase).expect("encrypt");
        let encrypted_str = String::from_utf8(encrypted).expect("valid UTF-8");
        let raw = BASE64.decode(&encrypted_str).expect("base64");

        // Truncate to just salt + nonce + 16 bytes (minimum that passes length check)
        let truncated = &raw[..SALT_SIZE + NONCE_SIZE + 16];
        let encoded = BASE64.encode(truncated);

        assert!(
            decrypt(&encoded, passphrase).is_err(),
            "truncated ciphertext must fail decryption"
        );
    }

    // ── is_encrypted edge cases ─────────────────────────────────────────

    #[test]
    fn is_encrypted_accepts_exact_minimum_length() {
        // Exactly salt(16) + nonce(12) + 16 bytes = 44 bytes of raw data
        let data = vec![0u8; SALT_SIZE + NONCE_SIZE + 16];
        let encoded = BASE64.encode(&data);
        assert!(
            is_encrypted(&encoded),
            "minimum-length valid base64 should pass heuristic"
        );
    }

    // ── Repeated operations ─────────────────────────────────────────────

    #[test]
    fn multiple_sequential_encrypt_decrypt_cycles() {
        let passphrase = "cycle-test";
        let mut data = b"initial plaintext".to_vec();

        for i in 0..50 {
            let encrypted = encrypt(&data, passphrase)
                .unwrap_or_else(|e| panic!("encrypt failed on cycle {i}: {e}"));
            let encrypted_str = String::from_utf8(encrypted)
                .unwrap_or_else(|e| panic!("utf8 failed on cycle {i}: {e}"));
            let decrypted = decrypt(&encrypted_str, passphrase)
                .unwrap_or_else(|e| panic!("decrypt failed on cycle {i}: {e}"));
            assert_eq!(data, decrypted, "mismatch on cycle {i}");
            // Mutate data slightly for next cycle
            data.push((i % 256) as u8);
        }
    }

    // ── Null bytes and high-entropy data ────────────────────────────────

    #[test]
    fn roundtrip_null_bytes_in_plaintext() {
        let plaintext = b"before\x00middle\x00\x00after\x00";
        let passphrase = "null-byte-pass";

        let encrypted = encrypt(plaintext, passphrase).expect("encrypt");
        let encrypted_str = String::from_utf8(encrypted).expect("valid UTF-8");
        let decrypted = decrypt(&encrypted_str, passphrase).expect("decrypt");

        assert_eq!(plaintext.to_vec(), decrypted);
    }

    #[test]
    fn roundtrip_all_0xff_bytes() {
        let plaintext = vec![0xFFu8; 512];
        let passphrase = "high-entropy";

        let encrypted = encrypt(&plaintext, passphrase).expect("encrypt");
        let encrypted_str = String::from_utf8(encrypted).expect("valid UTF-8");
        let decrypted = decrypt(&encrypted_str, passphrase).expect("decrypt");

        assert_eq!(plaintext, decrypted);
    }

    // ── Env-var passphrase resolution (temp_env) ────────────────────────

    #[test]
    fn env_var_passphrase_resolution() {
        let cfg = EncryptionConfig::from_env("SHIPPER_TEST_PASS_1".to_string());
        temp_env::with_var("SHIPPER_TEST_PASS_1", Some("env-secret"), || {
            let passphrase = cfg.get_passphrase().unwrap();
            assert_eq!(passphrase, Some("env-secret".to_string()));
        });
    }

    #[test]
    fn env_var_passphrase_missing_returns_none() {
        let cfg = EncryptionConfig::from_env("SHIPPER_TEST_MISSING_VAR".to_string());
        temp_env::with_var("SHIPPER_TEST_MISSING_VAR", None::<&str>, || {
            let passphrase = cfg.get_passphrase().unwrap();
            assert_eq!(passphrase, None);
        });
    }

    #[test]
    fn state_encryption_from_env_var_roundtrip() {
        let config = EncryptionConfig::from_env("SHIPPER_TEST_ENC_PASS".to_string());
        let encryption = StateEncryption::new(config).expect("create");

        temp_env::with_var("SHIPPER_TEST_ENC_PASS", Some("my-env-key"), || {
            assert!(encryption.is_enabled());

            let data = b"env-var encrypted data";
            let encrypted = encryption.encrypt(data).expect("encrypt");
            let decrypted = encryption.decrypt(&encrypted).expect("decrypt");
            assert_eq!(data.to_vec(), decrypted);
        });
    }

    #[test]
    fn state_encryption_env_var_takes_precedence() {
        let config = EncryptionConfig {
            enabled: true,
            passphrase: Some("inline-pass".to_string()),
            env_var: Some("SHIPPER_TEST_PRIO_PASS".to_string()),
        };
        let encryption = StateEncryption::new(config).expect("create");

        temp_env::with_var("SHIPPER_TEST_PRIO_PASS", Some("env-pass"), || {
            // StateEncryption.get_passphrase tries env var first
            let data = b"priority test";
            let encrypted = encryption.encrypt(data).expect("encrypt");

            // Must decrypt with "env-pass" (env takes priority in StateEncryption)
            let encrypted_str = String::from_utf8(encrypted).expect("utf8");
            assert!(
                decrypt(&encrypted_str, "env-pass").is_ok(),
                "env var passphrase should take priority"
            );
        });
    }

    #[test]
    fn state_encryption_file_roundtrip_with_env_var() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("env_enc.json");

        let config = EncryptionConfig::from_env("SHIPPER_TEST_FILE_PASS".to_string());
        let encryption = StateEncryption::new(config).expect("create");

        temp_env::with_var("SHIPPER_TEST_FILE_PASS", Some("file-env-key"), || {
            let data = br#"{"encrypted_via": "env_var"}"#;
            encryption.write_file(&path, data).expect("write");
            let content = encryption.read_file(&path).expect("read");
            assert_eq!(String::from_utf8_lossy(data), content);
        });
    }

    // ── Salt uniqueness across many encryptions ─────────────────────────

    #[test]
    fn salt_uniqueness_across_10_encryptions() {
        let plaintext = b"salt uniqueness test";
        let passphrase = "salt-test";

        let mut salts = Vec::new();
        for _ in 0..10 {
            let encrypted = encrypt(plaintext, passphrase).expect("encrypt");
            let encrypted_str = String::from_utf8(encrypted).expect("utf8");
            let raw = BASE64.decode(&encrypted_str).expect("base64");
            let salt = raw[..SALT_SIZE].to_vec();
            salts.push(salt);
        }

        // All salts must be unique
        for i in 0..salts.len() {
            for j in (i + 1)..salts.len() {
                assert_ne!(salts[i], salts[j], "salt collision at indices {i} and {j}");
            }
        }
    }

    // ── Key derivation with special passphrases ─────────────────────────

    #[test]
    fn derive_key_unicode_passphrase_is_deterministic() {
        let passphrase = "пароль-密码-🔑";
        let salt = [0x42u8; SALT_SIZE];
        let key1 = derive_key(passphrase, &salt);
        let key2 = derive_key(passphrase, &salt);
        assert_eq!(key1, key2);
    }

    #[test]
    fn derive_key_newline_passphrase_differs_from_stripped() {
        let salt = [0u8; SALT_SIZE];
        let key_with_newlines = derive_key("pass\nphrase\n", &salt);
        let key_stripped = derive_key("passphrase", &salt);
        assert_ne!(key_with_newlines, key_stripped);
    }

    // ── Double encryption ───────────────────────────────────────────────

    #[test]
    fn double_encrypt_roundtrip() {
        let plaintext = b"double layer secret";
        let pass1 = "outer-key";
        let pass2 = "inner-key";

        let inner = encrypt(plaintext, pass1).expect("encrypt inner");
        let outer = encrypt(&inner, pass2).expect("encrypt outer");

        let outer_str = String::from_utf8(outer).expect("utf8");
        let decrypted_outer = decrypt(&outer_str, pass2).expect("decrypt outer");
        let inner_str = String::from_utf8(decrypted_outer).expect("utf8");
        let decrypted_inner = decrypt(&inner_str, pass1).expect("decrypt inner");

        assert_eq!(plaintext.to_vec(), decrypted_inner);
    }

    // ── is_encrypted edge cases ─────────────────────────────────────────

    #[test]
    fn is_encrypted_rejects_whitespace_around_base64() {
        let data = vec![0u8; SALT_SIZE + NONCE_SIZE + 16];
        let encoded = format!("  {}  ", BASE64.encode(&data));
        // Leading/trailing whitespace makes it invalid base64
        assert!(!is_encrypted(&encoded));
    }

    #[test]
    fn is_encrypted_rejects_json_object() {
        assert!(!is_encrypted(r#"{"plan_id":"abc","crates":[]}"#));
    }

    // ── StateEncryption fallback on malformed data ──────────────────────

    #[test]
    fn state_encryption_decrypt_returns_original_on_bad_encrypted_data() {
        let config = EncryptionConfig::new("test-pass".to_string());
        let encryption = StateEncryption::new(config).expect("create");

        // Data that isn't valid encrypted content
        let raw_json = b"plain JSON content";
        let result = encryption.decrypt(raw_json).expect("should fall back");
        assert_eq!(raw_json.to_vec(), result);
    }

    // ── File I/O with unicode content ───────────────────────────────────

    #[test]
    fn file_roundtrip_unicode_content() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("unicode.enc");

        let plaintext = "Ünïcödé cöntënt: 日本語テスト 🎉";
        write_encrypted(&path, plaintext.as_bytes(), "unicode-pass").expect("write");
        let decrypted = read_decrypted(&path, "unicode-pass").expect("read");
        assert_eq!(plaintext, decrypted);
    }

    // ── Encrypt/decrypt with GCM tag-sized plaintext ────────────────────

    #[test]
    fn encrypt_decrypt_exactly_gcm_tag_size() {
        // 16 bytes, same as the GCM authentication tag size
        let plaintext = [0xCD; 16];
        let passphrase = "tag-size-test";

        let encrypted = encrypt(&plaintext, passphrase).expect("encrypt");
        let encrypted_str = String::from_utf8(encrypted).expect("utf8");
        let decrypted = decrypt(&encrypted_str, passphrase).expect("decrypt");
        assert_eq!(plaintext.to_vec(), decrypted);
    }

    // ── EncryptionConfig Display with both sources ──────────────────────

    #[test]
    fn display_config_passphrase_takes_precedence_in_display() {
        let cfg = EncryptionConfig {
            enabled: true,
            passphrase: Some("my-pass".to_string()),
            env_var: Some("MY_ENV".to_string()),
        };
        let display = cfg.to_string();
        // Display shows passphrase arm (first match) when both are present
        assert!(
            display.contains("passphrase:"),
            "should show passphrase branch, got: {display}"
        );
    }

    // ── mask_passphrase additional cases ─────────────────────────────────

    #[test]
    fn mask_passphrase_four_chars() {
        let masked = mask_passphrase("abcd");
        assert_eq!(masked, "a**d");
    }

    #[test]
    fn mask_passphrase_five_chars() {
        let masked = mask_passphrase("hello");
        assert_eq!(masked, "h***o");
    }

    #[test]
    fn mask_passphrase_with_spaces() {
        let masked = mask_passphrase("a b c");
        assert_eq!(masked, "a***c");
    }

    // ── StateEncryption disabled ignores env var ────────────────────────

    #[test]
    fn state_encryption_disabled_ignores_env_var() {
        let config = EncryptionConfig {
            enabled: false,
            passphrase: None,
            env_var: Some("SHIPPER_TEST_IGNORED_VAR".to_string()),
        };
        let encryption = StateEncryption::new(config).expect("create");

        temp_env::with_var("SHIPPER_TEST_IGNORED_VAR", Some("secret"), || {
            assert!(!encryption.is_enabled());
            // decrypt should pass through raw data
            let data = b"not encrypted";
            let result = encryption.decrypt(data).expect("passthrough");
            assert_eq!(data.to_vec(), result);
        });
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

        #[test]
        fn is_encrypted_true_for_encrypt_output(data in proptest::collection::vec(any::<u8>(), 0..512)) {
            let encrypted = encrypt(&data, "test-pass").expect("encrypt");
            let encrypted_str = String::from_utf8(encrypted).expect("valid UTF-8");
            prop_assert!(is_encrypted(&encrypted_str));
        }

        #[test]
        fn is_encrypted_never_panics(s in "\\PC{0,500}") {
            let _ = is_encrypted(&s);
        }

        #[test]
        fn decrypt_arbitrary_string_never_panics(s in "\\PC{0,500}") {
            let _ = decrypt(&s, "passphrase");
        }

        #[test]
        fn encrypt_output_is_always_utf8(
            data in proptest::collection::vec(any::<u8>(), 0..1024),
            passphrase in "\\PC{1,50}",
        ) {
            let encrypted = encrypt(&data, &passphrase).expect("encrypt");
            prop_assert!(String::from_utf8(encrypted).is_ok());
        }

        #[test]
        fn each_encrypt_produces_unique_ciphertext(data in proptest::collection::vec(any::<u8>(), 0..256)) {
            let a = encrypt(&data, "same-pass").expect("encrypt");
            let b = encrypt(&data, "same-pass").expect("encrypt");
            prop_assert_ne!(a, b);
        }

        #[test]
        fn encryption_config_serde_roundtrip_arbitrary(passphrase in "\\PC{1,100}") {
            let cfg = EncryptionConfig::new(passphrase.clone());
            let json = serde_json::to_string(&cfg).expect("serialize");
            let de: EncryptionConfig = serde_json::from_str(&json).expect("deserialize");
            prop_assert_eq!(de.enabled, true);
            prop_assert_eq!(de.passphrase.as_deref(), Some(passphrase.as_str()));
        }

        #[test]
        fn state_encryption_roundtrip_arbitrary(data in proptest::collection::vec(any::<u8>(), 0..1024)) {
            let config = EncryptionConfig::new("state-prop-pass".to_string());
            let se = StateEncryption::new(config).expect("create");
            let encrypted = se.encrypt(&data).expect("encrypt");
            let decrypted = se.decrypt(&encrypted).expect("decrypt");
            prop_assert_eq!(data, decrypted);
        }

        #[test]
        fn encryption_output_always_longer_than_input(data in proptest::collection::vec(any::<u8>(), 0..2048)) {
            let encrypted = encrypt(&data, "length-test").expect("encrypt");
            // Encrypted output (base64 of salt+nonce+ciphertext+tag) is always longer than plaintext
            prop_assert!(encrypted.len() > data.len());
        }

        #[test]
        fn tampered_ciphertext_always_fails(data in proptest::collection::vec(any::<u8>(), 1..512)) {
            let passphrase = "tamper-prop-test";
            let encrypted = encrypt(&data, passphrase).expect("encrypt");
            let encrypted_str = String::from_utf8(encrypted).expect("utf8");

            let mut raw = BASE64.decode(&encrypted_str).expect("base64");
            // Flip a byte in the ciphertext region (after salt+nonce)
            let idx = SALT_SIZE + NONCE_SIZE + (raw.len() - SALT_SIZE - NONCE_SIZE) / 2;
            raw[idx] ^= 0xFF;
            let corrupted = BASE64.encode(&raw);

            prop_assert!(decrypt(&corrupted, passphrase).is_err());
        }

        #[test]
        fn derive_key_always_produces_32_bytes_prop(
            passphrase in "\\PC{0,200}",
            salt in proptest::collection::vec(any::<u8>(), 0..64),
        ) {
            let key = derive_key(&passphrase, &salt);
            prop_assert_eq!(key.len(), KEY_SIZE);
        }

        #[test]
        fn decrypt_truncated_ciphertext_always_fails_prop(
            data in proptest::collection::vec(any::<u8>(), 1..512),
            trim in 1usize..17,
        ) {
            let passphrase = "truncation-prop";
            let encrypted = encrypt(&data, passphrase).expect("encrypt");
            let encrypted_str = String::from_utf8(encrypted).expect("utf8");
            let raw = BASE64.decode(&encrypted_str).expect("base64");

            // Trim `trim` bytes off the end (corrupts auth tag or ciphertext)
            if raw.len() > SALT_SIZE + NONCE_SIZE + 16 {
                let truncated = &raw[..raw.len() - trim];
                if truncated.len() >= SALT_SIZE + NONCE_SIZE + 16 {
                    let encoded = BASE64.encode(truncated);
                    prop_assert!(decrypt(&encoded, passphrase).is_err());
                }
            }
        }

        #[test]
        fn derive_key_deterministic_prop(
            passphrase in "\\PC{0,100}",
            salt in proptest::collection::vec(any::<u8>(), 0..32),
        ) {
            let key1 = derive_key(&passphrase, &salt);
            let key2 = derive_key(&passphrase, &salt);
            prop_assert_eq!(key1, key2, "derive_key must be deterministic");
        }

        #[test]
        fn salt_differs_across_encryptions_prop(data in proptest::collection::vec(any::<u8>(), 0..256)) {
            let a = encrypt(&data, "same-pass").expect("encrypt");
            let b = encrypt(&data, "same-pass").expect("encrypt");
            let raw_a = BASE64.decode(String::from_utf8(a).expect("utf8")).expect("base64");
            let raw_b = BASE64.decode(String::from_utf8(b).expect("utf8")).expect("base64");
            let salt_a = &raw_a[..SALT_SIZE];
            let salt_b = &raw_b[..SALT_SIZE];
            prop_assert_ne!(salt_a.to_vec(), salt_b.to_vec(), "salts must differ");
        }

        #[test]
        fn double_encrypt_roundtrip_prop(data in proptest::collection::vec(any::<u8>(), 0..256)) {
            let pass1 = "layer-one";
            let pass2 = "layer-two";
            let enc1 = encrypt(&data, pass1).expect("encrypt 1");
            let enc2 = encrypt(&enc1, pass2).expect("encrypt 2");
            let enc2_str = String::from_utf8(enc2).expect("utf8");
            let dec2 = decrypt(&enc2_str, pass2).expect("decrypt 2");
            let dec2_str = String::from_utf8(dec2).expect("utf8");
            let dec1 = decrypt(&dec2_str, pass1).expect("decrypt 1");
            prop_assert_eq!(data, dec1);
        }
    }
}

// ── Snapshot tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use insta::{assert_debug_snapshot, assert_snapshot};

    // ── EncryptionConfig serialization ──────────────────────────────────

    #[test]
    fn config_default_json() {
        let cfg = EncryptionConfig::default();
        let json = serde_json::to_string_pretty(&cfg).expect("serialize");
        assert_snapshot!(json);
    }

    #[test]
    fn config_with_passphrase_json() {
        let cfg = EncryptionConfig::new("my-secret".to_string());
        let json = serde_json::to_string_pretty(&cfg).expect("serialize");
        assert_snapshot!(json);
    }

    #[test]
    fn config_with_env_var_json() {
        let cfg = EncryptionConfig::from_env("SHIPPER_ENCRYPT_KEY".to_string());
        let json = serde_json::to_string_pretty(&cfg).expect("serialize");
        assert_snapshot!(json);
    }

    #[test]
    fn config_enabled_no_passphrase_json() {
        let cfg = EncryptionConfig {
            enabled: true,
            passphrase: None,
            env_var: None,
        };
        let json = serde_json::to_string_pretty(&cfg).expect("serialize");
        assert_snapshot!(json);
    }

    #[test]
    fn config_with_both_passphrase_and_env_json() {
        let cfg = EncryptionConfig {
            enabled: true,
            passphrase: Some("inline-pass".to_string()),
            env_var: Some("SHIPPER_ENCRYPT_KEY".to_string()),
        };
        let json = serde_json::to_string_pretty(&cfg).expect("serialize");
        assert_snapshot!(json);
    }

    // ── Masked token format ─────────────────────────────────────────────

    #[test]
    fn mask_passphrase_normal() {
        assert_snapshot!(mask_passphrase("my-secret-passphrase"));
    }

    #[test]
    fn mask_passphrase_short_three_chars() {
        assert_snapshot!(mask_passphrase("abc"));
    }

    #[test]
    fn mask_passphrase_two_chars() {
        assert_snapshot!(mask_passphrase("ab"));
    }

    #[test]
    fn mask_passphrase_single_char() {
        assert_snapshot!(mask_passphrase("x"));
    }

    #[test]
    fn mask_passphrase_empty() {
        assert_snapshot!(mask_passphrase(""));
    }

    #[test]
    fn mask_passphrase_unicode() {
        assert_snapshot!(mask_passphrase("🔑secret🔒"));
    }

    // ── StateEncryption config display ──────────────────────────────────

    #[test]
    fn display_config_disabled() {
        let cfg = EncryptionConfig::default();
        assert_snapshot!(cfg.to_string());
    }

    #[test]
    fn display_config_with_passphrase() {
        let cfg = EncryptionConfig::new("super-secret-key".to_string());
        assert_snapshot!(cfg.to_string());
    }

    #[test]
    fn display_config_with_env_var() {
        let cfg = EncryptionConfig::from_env("SHIPPER_ENCRYPT_KEY".to_string());
        assert_snapshot!(cfg.to_string());
    }

    #[test]
    fn display_config_enabled_no_source() {
        let cfg = EncryptionConfig {
            enabled: true,
            passphrase: None,
            env_var: None,
        };
        assert_snapshot!(cfg.to_string());
    }

    #[test]
    fn display_state_encryption_wrapper() {
        let cfg = EncryptionConfig::new("my-passphrase".to_string());
        let se = StateEncryption::new(cfg).expect("create");
        assert_snapshot!(se.to_string());
    }

    // ── Decryption failure error messages ───────────────────────────────

    #[test]
    fn error_invalid_base64() {
        let err = decrypt("not-valid-base64!!!", "pass").unwrap_err();
        assert_snapshot!(err.to_string());
    }

    #[test]
    fn error_data_too_short() {
        let short = BASE64.encode(vec![0u8; SALT_SIZE + NONCE_SIZE + 15]);
        let err = decrypt(&short, "pass").unwrap_err();
        assert_snapshot!(err.to_string());
    }

    #[test]
    fn error_wrong_passphrase() {
        let encrypted = encrypt(b"secret data", "correct-pass").expect("encrypt");
        let encrypted_str = String::from_utf8(encrypted).expect("utf8");
        let err = decrypt(&encrypted_str, "wrong-pass").unwrap_err();
        assert_snapshot!(err.to_string());
    }

    #[test]
    fn error_corrupted_ciphertext() {
        let encrypted = encrypt(b"data", "pass").expect("encrypt");
        let encrypted_str = String::from_utf8(encrypted).expect("utf8");
        let mut raw = BASE64.decode(&encrypted_str).expect("base64");
        raw[SALT_SIZE + NONCE_SIZE + 1] ^= 0xFF;
        let corrupted = BASE64.encode(&raw);
        let err = decrypt(&corrupted, "pass").unwrap_err();
        assert_snapshot!(err.to_string());
    }

    #[test]
    fn error_empty_input() {
        let err = decrypt("", "pass").unwrap_err();
        assert_snapshot!(err.to_string());
    }

    // ── Snapshot: error types for tampered regions ──────────────────────

    #[test]
    fn error_corrupted_salt_message() {
        let encrypted = encrypt(b"snapshot salt", "pass").expect("encrypt");
        let encrypted_str = String::from_utf8(encrypted).expect("utf8");
        let mut raw = BASE64.decode(&encrypted_str).expect("base64");
        raw[0] ^= 0xFF;
        let corrupted = BASE64.encode(&raw);
        let err = decrypt(&corrupted, "pass").unwrap_err();
        assert_snapshot!(err.to_string());
    }

    #[test]
    fn error_corrupted_nonce_message() {
        let encrypted = encrypt(b"snapshot nonce", "pass").expect("encrypt");
        let encrypted_str = String::from_utf8(encrypted).expect("utf8");
        let mut raw = BASE64.decode(&encrypted_str).expect("base64");
        raw[SALT_SIZE] ^= 0xFF;
        let corrupted = BASE64.encode(&raw);
        let err = decrypt(&corrupted, "pass").unwrap_err();
        assert_snapshot!(err.to_string());
    }

    #[test]
    fn error_corrupted_auth_tag_message() {
        let encrypted = encrypt(b"snapshot tag", "pass").expect("encrypt");
        let encrypted_str = String::from_utf8(encrypted).expect("utf8");
        let mut raw = BASE64.decode(&encrypted_str).expect("base64");
        let last = raw.len() - 1;
        raw[last] ^= 0xFF;
        let corrupted = BASE64.encode(&raw);
        let err = decrypt(&corrupted, "pass").unwrap_err();
        assert_snapshot!(err.to_string());
    }

    // ── Snapshot: key generation output format ──────────────────────────

    #[test]
    fn snapshot_derive_key_output_format() {
        let key = derive_key("test-passphrase", &[0u8; SALT_SIZE]);
        // Snapshot the hex-encoded key to verify deterministic output format
        let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
        assert_snapshot!(hex);
    }

    #[test]
    fn snapshot_derive_key_length() {
        let key = derive_key("any-passphrase", &[42u8; SALT_SIZE]);
        assert_debug_snapshot!(key.len());
    }

    // ── Snapshot: EncryptionConfig Debug output ─────────────────────────

    #[test]
    fn snapshot_encryption_config_debug_default() {
        let cfg = EncryptionConfig::default();
        assert_debug_snapshot!(cfg);
    }

    #[test]
    fn snapshot_encryption_config_debug_with_passphrase() {
        let cfg = EncryptionConfig::new("debug-pass".to_string());
        assert_debug_snapshot!(cfg);
    }

    #[test]
    fn snapshot_encryption_config_debug_from_env() {
        let cfg = EncryptionConfig::from_env("MY_SECRET_VAR".to_string());
        assert_debug_snapshot!(cfg);
    }

    // ── Snapshot: encrypted output structure ─────────────────────────────

    #[test]
    fn snapshot_encrypted_data_component_sizes() {
        let plaintext = b"snapshot-structure-test";
        let encrypted = encrypt(plaintext, "snap-pass").expect("encrypt");
        let encrypted_str = String::from_utf8(encrypted).expect("utf8");
        let raw = BASE64.decode(&encrypted_str).expect("base64");

        let info = format!(
            "salt_bytes={}, nonce_bytes={}, ciphertext_plus_tag_bytes={}, plaintext_len={}, overhead={}",
            SALT_SIZE,
            NONCE_SIZE,
            raw.len() - SALT_SIZE - NONCE_SIZE,
            plaintext.len(),
            raw.len() - plaintext.len(),
        );
        assert_snapshot!(info);
    }

    #[test]
    fn snapshot_derive_key_alternate_passphrase() {
        let key = derive_key("alternate-passphrase-for-snapshot", &[0xAB; SALT_SIZE]);
        let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
        assert_snapshot!(hex);
    }

    #[test]
    fn snapshot_is_encrypted_results() {
        let results = format!(
            "empty={}, json={}, short_b64={}, garbage={}",
            is_encrypted(""),
            is_encrypted(r#"{"key":"value"}"#),
            is_encrypted(&BASE64.encode(vec![0u8; 10])),
            is_encrypted("!!!not-base64!!!"),
        );
        assert_snapshot!(results);
    }

    // ── Snapshot: StateEncryption no-passphrase error ────────────────────

    #[test]
    fn snapshot_state_encryption_no_passphrase_error() {
        let config = EncryptionConfig {
            enabled: true,
            passphrase: None,
            env_var: None,
        };
        let encryption = StateEncryption::new(config).expect("create");
        let err = encryption.encrypt(b"data").unwrap_err();
        assert_snapshot!(err.to_string());
    }

    // ── Snapshot: Display with both passphrase and env_var ───────────────

    #[test]
    fn snapshot_display_config_with_both_sources() {
        let cfg = EncryptionConfig {
            enabled: true,
            passphrase: Some("inline-secret".to_string()),
            env_var: Some("SHIPPER_KEY".to_string()),
        };
        assert_snapshot!(cfg.to_string());
    }

    // ── Snapshot: mask_passphrase additional lengths ─────────────────────

    #[test]
    fn snapshot_mask_passphrase_four_chars() {
        assert_snapshot!(mask_passphrase("abcd"));
    }

    #[test]
    fn snapshot_mask_passphrase_with_spaces() {
        assert_snapshot!(mask_passphrase("a b c d"));
    }

    #[test]
    fn snapshot_mask_passphrase_with_newline() {
        assert_snapshot!(mask_passphrase("pass\nword"));
    }

    // ── Snapshot: encrypted output overhead for empty plaintext ──────────

    #[test]
    fn snapshot_encrypted_empty_plaintext_structure() {
        let encrypted = encrypt(b"", "snap-pass").expect("encrypt");
        let encrypted_str = String::from_utf8(encrypted).expect("utf8");
        let raw = BASE64.decode(&encrypted_str).expect("base64");

        let info = format!(
            "raw_len={}, salt={}, nonce={}, ciphertext_plus_tag={}, plaintext_len=0",
            raw.len(),
            SALT_SIZE,
            NONCE_SIZE,
            raw.len() - SALT_SIZE - NONCE_SIZE,
        );
        assert_snapshot!(info);
    }
}
