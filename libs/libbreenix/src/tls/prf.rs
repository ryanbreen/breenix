//! TLS 1.2 PRF (Pseudorandom Function) and key derivation (RFC 5246 section 5)
//!
//! Implements the TLS 1.2 PRF based on HMAC-SHA256, along with the key
//! derivation functions needed for the handshake:
//!
//! - Master secret derivation (RFC 5246 section 8.1)
//! - Key material expansion (RFC 5246 section 6.3)
//! - Finished message verify_data computation (RFC 5246 section 7.4.9)

use crate::crypto::hmac::hmac_sha256;

/// P_hash function using HMAC-SHA256 (RFC 5246 section 5).
///
/// Generates arbitrary-length output from a secret and seed by iteratively
/// applying HMAC-SHA256:
///
/// ```text
/// A(0) = seed
/// A(i) = HMAC_hash(secret, A(i-1))
/// P_hash(secret, seed) = HMAC_hash(secret, A(1) + seed) +
///                         HMAC_hash(secret, A(2) + seed) +
///                         HMAC_hash(secret, A(3) + seed) + ...
/// ```
///
/// Output is truncated to exactly `output.len()` bytes.
fn p_sha256(secret: &[u8], seed: &[u8], output: &mut [u8]) {
    let mut remaining = output.len();
    let mut offset = 0;

    // A(1) = HMAC(secret, seed)
    let mut a = hmac_sha256(secret, seed);

    while remaining > 0 {
        // HMAC(secret, A(i) + seed)
        let mut a_and_seed = Vec::with_capacity(a.len() + seed.len());
        a_and_seed.extend_from_slice(&a);
        a_and_seed.extend_from_slice(seed);
        let p = hmac_sha256(secret, &a_and_seed);

        let to_copy = remaining.min(32);
        output[offset..offset + to_copy].copy_from_slice(&p[..to_copy]);
        offset += to_copy;
        remaining -= to_copy;

        // A(i+1) = HMAC(secret, A(i))
        a = hmac_sha256(secret, &a);
    }
}

/// TLS 1.2 PRF using HMAC-SHA256 (RFC 5246 section 5).
///
/// Computes `PRF(secret, label, seed) = P_SHA256(secret, label + seed)`.
///
/// The label and seed are concatenated to form the combined seed passed to
/// [`p_sha256`]. The output buffer is filled with exactly `output.len()` bytes
/// of pseudorandom data.
pub fn prf(secret: &[u8], label: &[u8], seed: &[u8], output: &mut [u8]) {
    let mut combined_seed = Vec::with_capacity(label.len() + seed.len());
    combined_seed.extend_from_slice(label);
    combined_seed.extend_from_slice(seed);

    p_sha256(secret, &combined_seed, output);
}

/// Derive the 48-byte master secret from pre-master secret and randoms
/// (RFC 5246 section 8.1).
///
/// ```text
/// master_secret = PRF(pre_master_secret, "master secret",
///                     ClientHello.random + ServerHello.random)[0..47]
/// ```
///
/// The client and server randoms are concatenated (client first) to form the
/// seed. The result is always exactly 48 bytes.
pub fn derive_master_secret(
    pre_master_secret: &[u8],
    client_random: &[u8; 32],
    server_random: &[u8; 32],
) -> [u8; 48] {
    let mut seed = [0u8; 64];
    seed[..32].copy_from_slice(client_random);
    seed[32..].copy_from_slice(server_random);

    let mut master_secret = [0u8; 48];
    prf(pre_master_secret, b"master secret", &seed, &mut master_secret);
    master_secret
}

/// Derived key material for AES-128-GCM (RFC 5246 section 6.3).
///
/// Contains the symmetric keys and implicit IVs needed for record-layer
/// encryption in both directions.
#[derive(Debug, Clone)]
pub struct KeyBlock {
    /// 16-byte AES-128 key for encrypting client-to-server traffic.
    pub client_write_key: [u8; 16],
    /// 16-byte AES-128 key for encrypting server-to-client traffic.
    pub server_write_key: [u8; 16],
    /// 4-byte implicit nonce prefix for client-to-server GCM.
    pub client_write_iv: [u8; 4],
    /// 4-byte implicit nonce prefix for server-to-client GCM.
    pub server_write_iv: [u8; 4],
}

/// Derive encryption keys from the master secret (RFC 5246 section 6.3).
///
/// ```text
/// key_block = PRF(master_secret, "key expansion",
///                 server_random + client_random)
/// ```
///
/// For AES-128-GCM the key block layout is:
/// - `client_write_key` (16 bytes)
/// - `server_write_key` (16 bytes)
/// - `client_write_iv`  (4 bytes)
/// - `server_write_iv`  (4 bytes)
///
/// Total: 40 bytes.
///
/// Note: key expansion uses server_random **first**, then client_random. This
/// is the opposite order from master secret derivation.
pub fn derive_key_material(
    master_secret: &[u8; 48],
    server_random: &[u8; 32],
    client_random: &[u8; 32],
) -> KeyBlock {
    // Seed is server_random + client_random (opposite order from master secret)
    let mut seed = [0u8; 64];
    seed[..32].copy_from_slice(server_random);
    seed[32..].copy_from_slice(client_random);

    let mut key_block_bytes = [0u8; 40];
    prf(master_secret, b"key expansion", &seed, &mut key_block_bytes);

    let mut client_write_key = [0u8; 16];
    let mut server_write_key = [0u8; 16];
    let mut client_write_iv = [0u8; 4];
    let mut server_write_iv = [0u8; 4];

    client_write_key.copy_from_slice(&key_block_bytes[0..16]);
    server_write_key.copy_from_slice(&key_block_bytes[16..32]);
    client_write_iv.copy_from_slice(&key_block_bytes[32..36]);
    server_write_iv.copy_from_slice(&key_block_bytes[36..40]);

    KeyBlock {
        client_write_key,
        server_write_key,
        client_write_iv,
        server_write_iv,
    }
}

/// Compute the `verify_data` for a TLS Finished message (RFC 5246 section 7.4.9).
///
/// ```text
/// verify_data = PRF(master_secret, finished_label,
///                   Hash(handshake_messages))[0..11]
/// ```
///
/// The `label` should be `b"client finished"` or `b"server finished"`.
/// The `handshake_hash` is the SHA-256 hash of all handshake messages
/// exchanged so far.
///
/// Returns a 12-byte verification tag.
pub fn compute_verify_data(
    master_secret: &[u8; 48],
    label: &[u8],
    handshake_hash: &[u8; 32],
) -> [u8; 12] {
    let mut verify_data = [0u8; 12];
    prf(master_secret, label, handshake_hash, &mut verify_data);
    verify_data
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prf_output_length() {
        let secret = b"test secret";
        let label = b"test label";
        let seed = b"test seed";

        // Verify PRF produces exactly the requested number of bytes for
        // various output sizes, including sizes that are not multiples of 32.
        for &len in &[0, 1, 12, 31, 32, 33, 48, 64, 100, 128] {
            let mut output = vec![0u8; len];
            prf(secret, label, seed, &mut output);
            assert_eq!(output.len(), len);

            // Non-zero lengths should produce non-zero output (with overwhelming
            // probability for a properly functioning PRF).
            if len > 0 {
                assert!(output.iter().any(|&b| b != 0), "PRF output was all zeros for len={}", len);
            }
        }
    }

    #[test]
    fn test_prf_deterministic() {
        let secret = b"deterministic secret";
        let label = b"deterministic label";
        let seed = b"deterministic seed";

        let mut output1 = [0u8; 64];
        let mut output2 = [0u8; 64];
        prf(secret, label, seed, &mut output1);
        prf(secret, label, seed, &mut output2);

        assert_eq!(output1, output2, "PRF must be deterministic");
    }

    #[test]
    fn test_prf_different_inputs_different_outputs() {
        let secret = b"secret";
        let label = b"label";

        let mut output_a = [0u8; 32];
        let mut output_b = [0u8; 32];
        prf(secret, label, b"seed A", &mut output_a);
        prf(secret, label, b"seed B", &mut output_b);

        assert_ne!(output_a, output_b, "Different seeds must produce different outputs");
    }

    #[test]
    fn test_derive_master_secret_length() {
        let pre_master_secret = [0x03u8; 48];
        let client_random = [0x01u8; 32];
        let server_random = [0x02u8; 32];

        let master_secret = derive_master_secret(&pre_master_secret, &client_random, &server_random);
        assert_eq!(master_secret.len(), 48);
        assert!(
            master_secret.iter().any(|&b| b != 0),
            "Master secret should not be all zeros"
        );
    }

    #[test]
    fn test_derive_master_secret_deterministic() {
        let pre_master_secret = [0xABu8; 48];
        let client_random = [0xCDu8; 32];
        let server_random = [0xEFu8; 32];

        let ms1 = derive_master_secret(&pre_master_secret, &client_random, &server_random);
        let ms2 = derive_master_secret(&pre_master_secret, &client_random, &server_random);

        assert_eq!(ms1, ms2, "Master secret derivation must be deterministic");
    }

    #[test]
    fn test_derive_key_material_structure() {
        let master_secret = [0x42u8; 48];
        let server_random = [0x01u8; 32];
        let client_random = [0x02u8; 32];

        let key_block = derive_key_material(&master_secret, &server_random, &client_random);

        // Verify field sizes
        assert_eq!(key_block.client_write_key.len(), 16);
        assert_eq!(key_block.server_write_key.len(), 16);
        assert_eq!(key_block.client_write_iv.len(), 4);
        assert_eq!(key_block.server_write_iv.len(), 4);

        // Keys should not be all zeros
        assert!(
            key_block.client_write_key.iter().any(|&b| b != 0),
            "client_write_key should not be all zeros"
        );
        assert!(
            key_block.server_write_key.iter().any(|&b| b != 0),
            "server_write_key should not be all zeros"
        );

        // Client and server keys must differ
        assert_ne!(
            key_block.client_write_key, key_block.server_write_key,
            "Client and server write keys must differ"
        );
    }

    #[test]
    fn test_derive_key_material_deterministic() {
        let master_secret = [0x42u8; 48];
        let server_random = [0x01u8; 32];
        let client_random = [0x02u8; 32];

        let kb1 = derive_key_material(&master_secret, &server_random, &client_random);
        let kb2 = derive_key_material(&master_secret, &server_random, &client_random);

        assert_eq!(kb1.client_write_key, kb2.client_write_key);
        assert_eq!(kb1.server_write_key, kb2.server_write_key);
        assert_eq!(kb1.client_write_iv, kb2.client_write_iv);
        assert_eq!(kb1.server_write_iv, kb2.server_write_iv);
    }

    #[test]
    fn test_derive_key_material_random_order_matters() {
        let master_secret = [0x42u8; 48];
        let random_a = [0x01u8; 32];
        let random_b = [0x02u8; 32];

        // key expansion uses (server_random, client_random) order
        let kb1 = derive_key_material(&master_secret, &random_a, &random_b);
        let kb2 = derive_key_material(&master_secret, &random_b, &random_a);

        // Swapping server/client randoms must produce different keys
        assert_ne!(
            kb1.client_write_key, kb2.client_write_key,
            "Swapping random order must change output"
        );
    }

    #[test]
    fn test_compute_verify_data_length() {
        let master_secret = [0x55u8; 48];
        let handshake_hash = [0xAAu8; 32];

        let client_vd = compute_verify_data(&master_secret, b"client finished", &handshake_hash);
        let server_vd = compute_verify_data(&master_secret, b"server finished", &handshake_hash);

        assert_eq!(client_vd.len(), 12);
        assert_eq!(server_vd.len(), 12);

        // Client and server labels must produce different verify_data
        assert_ne!(
            client_vd, server_vd,
            "Client and server verify_data must differ"
        );
    }

    #[test]
    fn test_compute_verify_data_deterministic() {
        let master_secret = [0x55u8; 48];
        let handshake_hash = [0xAAu8; 32];

        let vd1 = compute_verify_data(&master_secret, b"client finished", &handshake_hash);
        let vd2 = compute_verify_data(&master_secret, b"client finished", &handshake_hash);

        assert_eq!(vd1, vd2, "verify_data must be deterministic");
    }

    #[test]
    fn test_p_sha256_zero_length() {
        let secret = b"secret";
        let seed = b"seed";
        let mut output = [];

        // Should not panic on zero-length output
        p_sha256(secret, seed, &mut output);
    }

    #[test]
    fn test_prf_prefix_consistency() {
        // A longer PRF output should have the shorter output as its prefix.
        // This is an important property of the P_hash construction.
        let secret = b"prefix test secret";
        let label = b"prefix test label";
        let seed = b"prefix test seed";

        let mut short = [0u8; 32];
        let mut long = [0u8; 64];
        prf(secret, label, seed, &mut short);
        prf(secret, label, seed, &mut long);

        assert_eq!(
            &short[..],
            &long[..32],
            "Shorter PRF output must be a prefix of longer output"
        );
    }
}
