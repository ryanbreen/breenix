//! RSA PKCS#1 v1.5 signature verification (verification only, no signing).
//!
//! Implements RSA signature verification with SHA-256 for TLS certificate
//! chain validation. This module does not support encryption/decryption or
//! signature generation -- only the verification path needed by a TLS client.
//!
//! The implementation follows PKCS#1 v1.5 (RFC 8017, Section 8.2.2).

use super::bignum::BigNum;
use super::sha256::sha256;

/// ASN.1 DER-encoded DigestInfo prefix for SHA-256.
///
/// This is the fixed prefix that appears before the hash value in a PKCS#1 v1.5
/// signature block:
///
/// ```text
/// SEQUENCE {
///   SEQUENCE {
///     OID 2.16.840.1.101.3.4.2.1 (sha-256)
///     NULL
///   }
///   OCTET STRING (32 bytes follow)
/// }
/// ```
const SHA256_DIGEST_INFO_PREFIX: [u8; 19] = [
    0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01,
    0x05, 0x00, 0x04, 0x20,
];

/// An RSA public key consisting of modulus and public exponent.
pub struct RsaPublicKey {
    /// Modulus n
    pub n: BigNum,
    /// Public exponent e (typically 65537)
    pub e: BigNum,
}

/// Constant-time byte comparison to prevent timing side-channel attacks.
///
/// Returns `true` if and only if `a` and `b` have the same length and
/// identical contents. The comparison always examines every byte regardless
/// of where the first difference occurs.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

/// Verify an RSA PKCS#1 v1.5 signature with SHA-256.
///
/// This performs the verification side of RSASSA-PKCS1-v1_5 (RFC 8017 Section 8.2.2):
///
/// 1. Compute `m = signature^e mod n` (the "signature representative")
/// 2. Convert `m` to a byte string the same length as the modulus
/// 3. Verify PKCS#1 v1.5 padding structure:
///    `0x00 || 0x01 || [>= 8 bytes of 0xFF] || 0x00 || DigestInfo || hash`
/// 4. Verify the DigestInfo OID matches SHA-256
/// 5. Verify the embedded hash matches `message_hash` using constant-time comparison
///
/// # Arguments
///
/// * `key` - The RSA public key (modulus and exponent)
/// * `signature` - The raw signature bytes (must be the same length as the modulus)
/// * `message_hash` - The expected SHA-256 hash (32 bytes)
///
/// # Returns
///
/// `true` if the signature is valid, `false` otherwise.
pub fn rsa_verify_pkcs1_sha256(
    key: &RsaPublicKey,
    signature: &[u8],
    message_hash: &[u8; 32],
) -> bool {
    // Step 1: Convert signature bytes to BigNum and compute m = sig^e mod n
    let sig = BigNum::from_be_bytes(signature);
    let m = sig.mod_exp(&key.e, &key.n);

    // Step 2: Convert m to bytes and left-pad with zeros to match modulus byte length
    let mod_len = (key.n.bit_len() + 7) / 8;
    let m_bytes = m.to_be_bytes();

    // The encoded message must be exactly mod_len bytes. Left-pad if needed.
    let mut em = vec![0u8; mod_len];
    if m_bytes.len() > mod_len {
        // Signature representative is larger than the modulus -- invalid
        return false;
    }
    let pad_offset = mod_len - m_bytes.len();
    em[pad_offset..].copy_from_slice(&m_bytes);

    // Step 3: Verify PKCS#1 v1.5 padding structure
    //
    // Expected format:
    //   em[0]     = 0x00
    //   em[1]     = 0x01
    //   em[2..t]  = 0xFF  (at least 8 bytes of padding)
    //   em[t]     = 0x00  (separator)
    //   em[t+1..] = DigestInfo prefix || hash
    //
    // The suffix (DigestInfo + hash) has a fixed length:
    //   19 bytes DigestInfo prefix + 32 bytes hash = 51 bytes
    let suffix_len = SHA256_DIGEST_INFO_PREFIX.len() + 32; // 19 + 32 = 51

    // Minimum message length: 0x00 + 0x01 + 8*0xFF + 0x00 + suffix = 2 + 8 + 1 + 51 = 62
    if mod_len < 2 + 8 + 1 + suffix_len {
        return false;
    }

    // Check leading bytes
    if em[0] != 0x00 || em[1] != 0x01 {
        return false;
    }

    // The separator 0x00 must appear after at least 8 bytes of 0xFF padding.
    // The suffix starts at position (mod_len - suffix_len), so the separator
    // is at (mod_len - suffix_len - 1).
    let separator_pos = mod_len - suffix_len - 1;

    // There must be at least 8 bytes of 0xFF between em[2] and the separator.
    // The padding region is em[2..separator_pos].
    let padding_len = separator_pos - 2;
    if padding_len < 8 {
        return false;
    }

    // Verify all padding bytes are 0xFF
    for i in 2..separator_pos {
        if em[i] != 0xFF {
            return false;
        }
    }

    // Verify separator byte
    if em[separator_pos] != 0x00 {
        return false;
    }

    // Step 4: Verify DigestInfo prefix (SHA-256 OID)
    let digest_info_start = separator_pos + 1;
    let digest_info_end = digest_info_start + SHA256_DIGEST_INFO_PREFIX.len();
    if !ct_eq(
        &em[digest_info_start..digest_info_end],
        &SHA256_DIGEST_INFO_PREFIX,
    ) {
        return false;
    }

    // Step 5: Verify the hash using constant-time comparison
    let hash_start = digest_info_end;
    let hash_end = hash_start + 32;
    ct_eq(&em[hash_start..hash_end], message_hash)
}

/// Verify an RSA PKCS#1 v1.5 signature over a message.
///
/// Convenience wrapper that hashes the message with SHA-256 first, then
/// verifies the signature against the resulting digest.
///
/// # Arguments
///
/// * `key` - The RSA public key
/// * `signature` - The raw signature bytes
/// * `message` - The message whose signature is being verified
///
/// # Returns
///
/// `true` if the signature is valid, `false` otherwise.
pub fn rsa_verify_sha256(key: &RsaPublicKey, signature: &[u8], message: &[u8]) -> bool {
    let hash = sha256(message);
    rsa_verify_pkcs1_sha256(key, signature, &hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to convert a hex string to a Vec<u8>.
    fn hex_to_bytes(hex: &str) -> Vec<u8> {
        let hex = hex.replace(' ', "");
        assert!(hex.len() % 2 == 0, "hex string must have even length");
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn test_digest_info_prefix() {
        // The SHA-256 DigestInfo prefix is a well-known constant from PKCS#1.
        // Verify it matches the expected DER encoding.
        let expected = hex_to_bytes("30 31 30 0d 06 09 60 86 48 01 65 03 04 02 01 05 00 04 20");
        assert_eq!(SHA256_DIGEST_INFO_PREFIX.len(), 19);
        assert_eq!(&SHA256_DIGEST_INFO_PREFIX[..], &expected[..]);
    }

    #[test]
    fn test_ct_eq_equal() {
        let a = [0x01, 0x02, 0x03, 0x04];
        let b = [0x01, 0x02, 0x03, 0x04];
        assert!(ct_eq(&a, &b));
    }

    #[test]
    fn test_ct_eq_different() {
        let a = [0x01, 0x02, 0x03, 0x04];
        let b = [0x01, 0x02, 0x03, 0x05];
        assert!(!ct_eq(&a, &b));
    }

    #[test]
    fn test_ct_eq_different_lengths() {
        let a = [0x01, 0x02, 0x03];
        let b = [0x01, 0x02, 0x03, 0x04];
        assert!(!ct_eq(&a, &b));
    }

    #[test]
    fn test_ct_eq_empty() {
        let a: [u8; 0] = [];
        let b: [u8; 0] = [];
        assert!(ct_eq(&a, &b));
    }

    #[test]
    fn test_ct_eq_single_bit_difference() {
        let a = [0xFF; 32];
        let mut b = [0xFF; 32];
        // Flip a single bit in the last byte
        b[31] = 0xFE;
        assert!(!ct_eq(&a, &b));
    }

    /// Construct a valid PKCS#1 v1.5 padded message manually and verify the
    /// padding verification logic accepts it.
    ///
    /// This test exercises the padding validation without requiring a real RSA
    /// key by constructing the expected output of `sig^e mod n` directly and
    /// using an identity-like RSA operation (e=1, n larger than the message).
    #[test]
    fn test_pkcs1_padding_verification() {
        // We simulate a 256-byte (2048-bit) modulus.
        let mod_len: usize = 256;

        // The hash we expect to verify against
        let message_hash: [u8; 32] = sha256(b"test message");

        // Construct a valid PKCS#1 v1.5 encoded message:
        //   0x00 0x01 [0xFF padding] 0x00 [DigestInfo] [hash]
        let suffix_len = SHA256_DIGEST_INFO_PREFIX.len() + 32; // 51
        let padding_len = mod_len - 3 - suffix_len; // 256 - 3 - 51 = 202

        let mut em = vec![0u8; mod_len];
        em[0] = 0x00;
        em[1] = 0x01;
        for i in 2..2 + padding_len {
            em[i] = 0xFF;
        }
        em[2 + padding_len] = 0x00;
        em[2 + padding_len + 1..2 + padding_len + 1 + SHA256_DIGEST_INFO_PREFIX.len()]
            .copy_from_slice(&SHA256_DIGEST_INFO_PREFIX);
        em[2 + padding_len + 1 + SHA256_DIGEST_INFO_PREFIX.len()..].copy_from_slice(&message_hash);

        // Use e=1 so that sig^e mod n = sig (the identity operation).
        // Choose n to be larger than our encoded message so the modular
        // reduction is also identity.
        let mut n_bytes = vec![0xFF; mod_len];
        n_bytes[0] = 0xFF; // Ensure n > em (em starts with 0x00)

        let key = RsaPublicKey {
            n: BigNum::from_be_bytes(&n_bytes),
            e: BigNum::from_be_bytes(&[0x01]),
        };

        // The "signature" is exactly the encoded message (since e=1, sig^1 mod n = sig)
        let signature = em.clone();
        assert!(rsa_verify_pkcs1_sha256(&key, &signature, &message_hash));
    }

    /// Verify that a wrong hash is rejected.
    #[test]
    fn test_pkcs1_wrong_hash_rejected() {
        let mod_len: usize = 256;
        let message_hash: [u8; 32] = sha256(b"test message");
        let wrong_hash: [u8; 32] = sha256(b"wrong message");

        let suffix_len = SHA256_DIGEST_INFO_PREFIX.len() + 32;
        let padding_len = mod_len - 3 - suffix_len;

        let mut em = vec![0u8; mod_len];
        em[0] = 0x00;
        em[1] = 0x01;
        for i in 2..2 + padding_len {
            em[i] = 0xFF;
        }
        em[2 + padding_len] = 0x00;
        em[2 + padding_len + 1..2 + padding_len + 1 + SHA256_DIGEST_INFO_PREFIX.len()]
            .copy_from_slice(&SHA256_DIGEST_INFO_PREFIX);
        // Embed the CORRECT hash in the signature block
        em[2 + padding_len + 1 + SHA256_DIGEST_INFO_PREFIX.len()..].copy_from_slice(&message_hash);

        let mut n_bytes = vec![0xFF; mod_len];
        n_bytes[0] = 0xFF;

        let key = RsaPublicKey {
            n: BigNum::from_be_bytes(&n_bytes),
            e: BigNum::from_be_bytes(&[0x01]),
        };

        // Try to verify against the WRONG hash -- must fail
        assert!(!rsa_verify_pkcs1_sha256(&key, &em, &wrong_hash));
    }

    /// Verify that corrupted padding is rejected.
    #[test]
    fn test_pkcs1_bad_padding_rejected() {
        let mod_len: usize = 256;
        let message_hash: [u8; 32] = sha256(b"test message");

        let suffix_len = SHA256_DIGEST_INFO_PREFIX.len() + 32;
        let padding_len = mod_len - 3 - suffix_len;

        let mut em = vec![0u8; mod_len];
        em[0] = 0x00;
        em[1] = 0x01;
        for i in 2..2 + padding_len {
            em[i] = 0xFF;
        }
        em[2 + padding_len] = 0x00;
        em[2 + padding_len + 1..2 + padding_len + 1 + SHA256_DIGEST_INFO_PREFIX.len()]
            .copy_from_slice(&SHA256_DIGEST_INFO_PREFIX);
        em[2 + padding_len + 1 + SHA256_DIGEST_INFO_PREFIX.len()..].copy_from_slice(&message_hash);

        // Corrupt one padding byte (change 0xFF to 0xFE)
        em[10] = 0xFE;

        let mut n_bytes = vec![0xFF; mod_len];
        n_bytes[0] = 0xFF;

        let key = RsaPublicKey {
            n: BigNum::from_be_bytes(&n_bytes),
            e: BigNum::from_be_bytes(&[0x01]),
        };

        assert!(!rsa_verify_pkcs1_sha256(&key, &em, &message_hash));
    }

    /// Verify that a missing separator byte (0x00 replaced with 0xFF) is rejected.
    #[test]
    fn test_pkcs1_missing_separator_rejected() {
        let mod_len: usize = 256;
        let message_hash: [u8; 32] = sha256(b"test message");

        let suffix_len = SHA256_DIGEST_INFO_PREFIX.len() + 32;
        let padding_len = mod_len - 3 - suffix_len;

        let mut em = vec![0u8; mod_len];
        em[0] = 0x00;
        em[1] = 0x01;
        for i in 2..2 + padding_len {
            em[i] = 0xFF;
        }
        // Set separator to 0xFF instead of 0x00
        em[2 + padding_len] = 0xFF;
        em[2 + padding_len + 1..2 + padding_len + 1 + SHA256_DIGEST_INFO_PREFIX.len()]
            .copy_from_slice(&SHA256_DIGEST_INFO_PREFIX);
        em[2 + padding_len + 1 + SHA256_DIGEST_INFO_PREFIX.len()..].copy_from_slice(&message_hash);

        let mut n_bytes = vec![0xFF; mod_len];
        n_bytes[0] = 0xFF;

        let key = RsaPublicKey {
            n: BigNum::from_be_bytes(&n_bytes),
            e: BigNum::from_be_bytes(&[0x01]),
        };

        assert!(!rsa_verify_pkcs1_sha256(&key, &em, &message_hash));
    }

    /// Verify that a wrong block type (0x02 instead of 0x01) is rejected.
    #[test]
    fn test_pkcs1_wrong_block_type_rejected() {
        let mod_len: usize = 256;
        let message_hash: [u8; 32] = sha256(b"test message");

        let suffix_len = SHA256_DIGEST_INFO_PREFIX.len() + 32;
        let padding_len = mod_len - 3 - suffix_len;

        let mut em = vec![0u8; mod_len];
        em[0] = 0x00;
        em[1] = 0x02; // Wrong! Should be 0x01 for signature verification
        for i in 2..2 + padding_len {
            em[i] = 0xFF;
        }
        em[2 + padding_len] = 0x00;
        em[2 + padding_len + 1..2 + padding_len + 1 + SHA256_DIGEST_INFO_PREFIX.len()]
            .copy_from_slice(&SHA256_DIGEST_INFO_PREFIX);
        em[2 + padding_len + 1 + SHA256_DIGEST_INFO_PREFIX.len()..].copy_from_slice(&message_hash);

        let mut n_bytes = vec![0xFF; mod_len];
        n_bytes[0] = 0xFF;

        let key = RsaPublicKey {
            n: BigNum::from_be_bytes(&n_bytes),
            e: BigNum::from_be_bytes(&[0x01]),
        };

        assert!(!rsa_verify_pkcs1_sha256(&key, &em, &message_hash));
    }

    /// Verify that the convenience function rsa_verify_sha256 hashes the message
    /// and verifies correctly.
    #[test]
    fn test_rsa_verify_sha256_convenience() {
        let mod_len: usize = 256;
        let message = b"hello world";
        let message_hash: [u8; 32] = sha256(message);

        let suffix_len = SHA256_DIGEST_INFO_PREFIX.len() + 32;
        let padding_len = mod_len - 3 - suffix_len;

        let mut em = vec![0u8; mod_len];
        em[0] = 0x00;
        em[1] = 0x01;
        for i in 2..2 + padding_len {
            em[i] = 0xFF;
        }
        em[2 + padding_len] = 0x00;
        em[2 + padding_len + 1..2 + padding_len + 1 + SHA256_DIGEST_INFO_PREFIX.len()]
            .copy_from_slice(&SHA256_DIGEST_INFO_PREFIX);
        em[2 + padding_len + 1 + SHA256_DIGEST_INFO_PREFIX.len()..].copy_from_slice(&message_hash);

        let mut n_bytes = vec![0xFF; mod_len];
        n_bytes[0] = 0xFF;

        let key = RsaPublicKey {
            n: BigNum::from_be_bytes(&n_bytes),
            e: BigNum::from_be_bytes(&[0x01]),
        };

        assert!(rsa_verify_sha256(&key, &em, message));
        assert!(!rsa_verify_sha256(&key, &em, b"different message"));
    }

    /// Test with a 128-byte (1024-bit) modulus to verify the padding length
    /// calculation works with different key sizes.
    #[test]
    fn test_pkcs1_1024bit_key() {
        let mod_len: usize = 128;
        let message_hash: [u8; 32] = sha256(b"1024-bit test");

        let suffix_len = SHA256_DIGEST_INFO_PREFIX.len() + 32; // 51
        let padding_len = mod_len - 3 - suffix_len; // 128 - 3 - 51 = 74

        // 74 >= 8, so this key size is large enough
        assert!(padding_len >= 8);

        let mut em = vec![0u8; mod_len];
        em[0] = 0x00;
        em[1] = 0x01;
        for i in 2..2 + padding_len {
            em[i] = 0xFF;
        }
        em[2 + padding_len] = 0x00;
        em[2 + padding_len + 1..2 + padding_len + 1 + SHA256_DIGEST_INFO_PREFIX.len()]
            .copy_from_slice(&SHA256_DIGEST_INFO_PREFIX);
        em[2 + padding_len + 1 + SHA256_DIGEST_INFO_PREFIX.len()..].copy_from_slice(&message_hash);

        let mut n_bytes = vec![0xFF; mod_len];
        n_bytes[0] = 0xFF;

        let key = RsaPublicKey {
            n: BigNum::from_be_bytes(&n_bytes),
            e: BigNum::from_be_bytes(&[0x01]),
        };

        assert!(rsa_verify_pkcs1_sha256(&key, &em, &message_hash));
    }
}
