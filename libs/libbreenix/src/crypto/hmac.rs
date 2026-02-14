//! HMAC-SHA256 implementation (RFC 2104)
//!
//! Keyed-Hash Message Authentication Code using SHA-256 as the underlying
//! hash function. Supports both incremental and one-shot computation.

use super::sha256::Sha256;

const BLOCK_SIZE: usize = 64;
const IPAD: u8 = 0x36;
const OPAD: u8 = 0x5c;

/// HMAC-SHA256 incremental computation.
///
/// Implements the HMAC construction from RFC 2104 using SHA-256.
///
/// # Example
///
/// ```
/// use libbreenix::crypto::hmac::HmacSha256;
///
/// let mut hmac = HmacSha256::new(b"secret key");
/// hmac.update(b"message part 1");
/// hmac.update(b"message part 2");
/// let mac = hmac.finalize();
/// ```
pub struct HmacSha256 {
    inner: Sha256,
    outer_key_pad: [u8; BLOCK_SIZE],
}

impl HmacSha256 {
    /// Create a new HMAC-SHA256 instance with the given key.
    ///
    /// If the key is longer than 64 bytes (the SHA-256 block size), it is
    /// first hashed with SHA-256 to produce a 32-byte key. Keys shorter
    /// than 64 bytes are zero-padded on the right.
    pub fn new(key: &[u8]) -> Self {
        // Step 1: If key is longer than block size, hash it.
        let mut key_block = [0u8; BLOCK_SIZE];
        if key.len() > BLOCK_SIZE {
            let hashed_key = {
                let mut hasher = Sha256::new();
                hasher.update(key);
                hasher.finalize()
            };
            key_block[..32].copy_from_slice(&hashed_key);
        } else {
            key_block[..key.len()].copy_from_slice(key);
        }

        // Step 2: Create inner key pad (key XOR ipad) and outer key pad (key XOR opad).
        let mut inner_key_pad = [0u8; BLOCK_SIZE];
        let mut outer_key_pad = [0u8; BLOCK_SIZE];
        for i in 0..BLOCK_SIZE {
            inner_key_pad[i] = key_block[i] ^ IPAD;
            outer_key_pad[i] = key_block[i] ^ OPAD;
        }

        // Step 3: Initialize inner hash with inner key pad.
        let mut inner = Sha256::new();
        inner.update(&inner_key_pad);

        Self {
            inner,
            outer_key_pad,
        }
    }

    /// Feed data into the HMAC computation.
    ///
    /// Can be called multiple times to process data incrementally.
    pub fn update(&mut self, data: &[u8]) {
        self.inner.update(data);
    }

    /// Finalize the HMAC computation and return the 32-byte MAC.
    ///
    /// Completes the inner hash, then computes the outer hash:
    /// `SHA256(outer_key_pad || SHA256(inner_key_pad || message))`
    ///
    /// Consumes the `HmacSha256` instance.
    pub fn finalize(self) -> [u8; 32] {
        // Finalize the inner hash: H(inner_key_pad || message)
        let inner_hash = self.inner.finalize();

        // Compute outer hash: H(outer_key_pad || inner_hash)
        let mut outer = Sha256::new();
        outer.update(&self.outer_key_pad);
        outer.update(&inner_hash);
        outer.finalize()
    }
}

/// Compute HMAC-SHA256 in a single call.
///
/// Convenience function for computing the MAC over a complete message.
///
/// # Arguments
///
/// * `key` - The secret key (any length; keys > 64 bytes are hashed first)
/// * `message` - The message to authenticate
///
/// # Returns
///
/// A 32-byte HMAC-SHA256 tag.
pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut hmac = HmacSha256::new(key);
    hmac.update(message);
    hmac.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to convert a hex string to a byte array.
    fn hex_to_bytes(hex: &str) -> [u8; 32] {
        let mut result = [0u8; 32];
        for i in 0..32 {
            result[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap();
        }
        result
    }

    /// RFC 4231 Test Case 1
    ///
    /// Key:  20 bytes of 0x0b
    /// Data: "Hi There"
    #[test]
    fn test_rfc4231_case1() {
        let key = [0x0bu8; 20];
        let data = b"Hi There";
        let expected =
            hex_to_bytes("b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7");

        let result = hmac_sha256(&key, data);
        assert_eq!(result, expected);
    }

    /// RFC 4231 Test Case 2
    ///
    /// Key:  "Jefe"
    /// Data: "what do ya want for nothing?"
    #[test]
    fn test_rfc4231_case2() {
        let key = b"Jefe";
        let data = b"what do ya want for nothing?";
        let expected =
            hex_to_bytes("5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");

        let result = hmac_sha256(key, data);
        assert_eq!(result, expected);
    }

    /// Verify incremental update produces the same result as one-shot.
    #[test]
    fn test_incremental_matches_oneshot() {
        let key = b"test key";
        let message = b"hello world";

        let oneshot = hmac_sha256(key, message);

        let mut hmac = HmacSha256::new(key);
        hmac.update(b"hello ");
        hmac.update(b"world");
        let incremental = hmac.finalize();

        assert_eq!(oneshot, incremental);
    }
}
