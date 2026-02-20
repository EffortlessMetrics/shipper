#![no_main]

use libfuzzer_sys::fuzz_target;
use shipper::encryption::{decrypt, encrypt};

fuzz_target!(|data: &[u8]| {
    // Test encryption/decryption roundtrip with random data
    let passphrase = "test-passphrase-fuzz";
    
    if let Ok(encrypted) = encrypt(data, passphrase) {
        if let Ok(encrypted_str) = std::str::from_utf8(&encrypted) {
            if let Ok(decrypted) = decrypt(encrypted_str, passphrase) {
                // Roundtrip should preserve data
                assert_eq!(data.to_vec(), decrypted);
            }
        }
    }
});