//! AES-GCM Authenticated Encryption with Associated Data (NIST SP 800-38D)
//!
//! Provides AES-128-GCM encryption and decryption using the GHASH universal
//! hash function over GF(2^128) and AES in CTR mode.

use super::aes::Aes128;

/// Reduction polynomial for GF(2^128): x^128 + x^7 + x^2 + x + 1.
/// Represented as the high byte 0xe1 in big-endian (MSB-first) format.
const R: [u8; 16] = [0xe1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

/// Multiply two elements in GF(2^128) using bit-by-bit schoolbook multiplication.
///
/// The field uses the reduction polynomial x^128 + x^7 + x^2 + x + 1.
/// Both inputs and the output are 128-bit values in big-endian (MSB-first) byte order.
pub fn ghash_multiply(x: &[u8; 16], h: &[u8; 16]) -> [u8; 16] {
    let mut z = [0u8; 16]; // accumulator (result)
    let mut v = *h; // running value of H shifted

    for i in 0..128 {
        let byte_idx = i / 8;
        let bit_idx = 7 - (i % 8);
        let xi = (x[byte_idx] >> bit_idx) & 1;

        // If the current bit of x is set, XOR v into the accumulator
        if xi == 1 {
            for j in 0..16 {
                z[j] ^= v[j];
            }
        }

        // Multiply v by x in GF(2^128): shift right by 1, conditionally XOR with R
        let lsb = v[15] & 1;
        // Shift v right by 1 bit
        for j in (1..16).rev() {
            v[j] = (v[j] >> 1) | (v[j - 1] << 7);
        }
        v[0] >>= 1;

        // If the LSB was set before the shift, reduce by XORing with R
        if lsb == 1 {
            for j in 0..16 {
                v[j] ^= R[j];
            }
        }
    }

    z
}

/// XOR a 16-byte block into an accumulator in place.
fn xor_block(acc: &mut [u8; 16], block: &[u8; 16]) {
    for i in 0..16 {
        acc[i] ^= block[i];
    }
}

/// Compute GHASH over associated data and ciphertext.
///
/// GHASH is defined as:
///   1. Process AAD in 16-byte blocks (zero-padded to block boundary)
///   2. Process ciphertext in 16-byte blocks (zero-padded to block boundary)
///   3. Process a final block containing the bit lengths of AAD and ciphertext
///      as 64-bit big-endian integers.
///
/// Each step: XOR the block into the accumulator, then multiply by H.
pub fn ghash(h: &[u8; 16], aad: &[u8], ciphertext: &[u8]) -> [u8; 16] {
    let mut acc = [0u8; 16];

    // Process AAD blocks
    let mut offset = 0;
    while offset < aad.len() {
        let mut block = [0u8; 16];
        let remaining = aad.len() - offset;
        let copy_len = if remaining >= 16 { 16 } else { remaining };
        block[..copy_len].copy_from_slice(&aad[offset..offset + copy_len]);
        // Remaining bytes stay zero (padding)

        xor_block(&mut acc, &block);
        acc = ghash_multiply(&acc, h);
        offset += 16;
    }

    // Process ciphertext blocks
    offset = 0;
    while offset < ciphertext.len() {
        let mut block = [0u8; 16];
        let remaining = ciphertext.len() - offset;
        let copy_len = if remaining >= 16 { 16 } else { remaining };
        block[..copy_len].copy_from_slice(&ciphertext[offset..offset + copy_len]);

        xor_block(&mut acc, &block);
        acc = ghash_multiply(&acc, h);
        offset += 16;
    }

    // Final block: 64-bit big-endian bit lengths
    let aad_bits = (aad.len() as u64) * 8;
    let ct_bits = (ciphertext.len() as u64) * 8;
    let mut len_block = [0u8; 16];
    len_block[..8].copy_from_slice(&aad_bits.to_be_bytes());
    len_block[8..16].copy_from_slice(&ct_bits.to_be_bytes());

    xor_block(&mut acc, &len_block);
    acc = ghash_multiply(&acc, h);

    acc
}

/// Increment the rightmost 32 bits of a 16-byte counter block (big-endian).
fn increment_counter(counter: &mut [u8; 16]) {
    let ctr = u32::from_be_bytes([counter[12], counter[13], counter[14], counter[15]]);
    let incremented = ctr.wrapping_add(1);
    let bytes = incremented.to_be_bytes();
    counter[12] = bytes[0];
    counter[13] = bytes[1];
    counter[14] = bytes[2];
    counter[15] = bytes[3];
}

/// AES-128-GCM authenticated encryption and decryption.
pub struct AesGcm {
    cipher: Aes128,
}

impl AesGcm {
    /// Create a new AES-GCM instance with the given 128-bit key.
    pub fn new(key: &[u8; 16]) -> Self {
        AesGcm {
            cipher: Aes128::new(key),
        }
    }

    /// Encrypt plaintext with associated data, producing ciphertext and an authentication tag.
    ///
    /// - `iv`: 12-byte initialization vector (nonce). Must be unique per encryption.
    /// - `aad`: additional authenticated data (authenticated but not encrypted).
    /// - `plaintext`: data to encrypt.
    /// - `ciphertext`: output buffer, must be at least `plaintext.len()` bytes.
    /// - `tag`: output 16-byte authentication tag.
    pub fn encrypt(
        &self,
        iv: &[u8; 12],
        aad: &[u8],
        plaintext: &[u8],
        ciphertext: &mut [u8],
        tag: &mut [u8; 16],
    ) {
        assert!(
            ciphertext.len() >= plaintext.len(),
            "ciphertext buffer too small"
        );

        // Compute H = AES_K(0^128)
        let mut h = [0u8; 16];
        self.cipher.encrypt_block(&mut h);

        // Construct J0 = IV || 0x00000001
        let mut j0 = [0u8; 16];
        j0[..12].copy_from_slice(iv);
        j0[12] = 0x00;
        j0[13] = 0x00;
        j0[14] = 0x00;
        j0[15] = 0x01;

        // CTR encryption: counter starts at J0 + 1
        let mut counter = j0;
        let mut offset = 0;
        while offset < plaintext.len() {
            increment_counter(&mut counter);
            let mut keystream = counter;
            self.cipher.encrypt_block(&mut keystream);

            let remaining = plaintext.len() - offset;
            let block_len = if remaining >= 16 { 16 } else { remaining };
            for i in 0..block_len {
                ciphertext[offset + i] = plaintext[offset + i] ^ keystream[i];
            }
            offset += block_len;
        }

        // Compute GHASH over AAD and ciphertext
        let ghash_val = ghash(&h, aad, &ciphertext[..plaintext.len()]);

        // Tag = AES_K(J0) XOR GHASH
        let mut encrypted_j0 = j0;
        self.cipher.encrypt_block(&mut encrypted_j0);
        for i in 0..16 {
            tag[i] = encrypted_j0[i] ^ ghash_val[i];
        }
    }

    /// Decrypt ciphertext with associated data, verifying the authentication tag.
    ///
    /// Returns `true` if the tag is valid and decryption succeeded, `false` otherwise.
    /// If the tag is invalid, the plaintext buffer contents are zeroed.
    ///
    /// - `iv`: 12-byte initialization vector (nonce).
    /// - `aad`: additional authenticated data.
    /// - `ciphertext`: data to decrypt.
    /// - `tag`: 16-byte authentication tag to verify.
    /// - `plaintext`: output buffer, must be at least `ciphertext.len()` bytes.
    pub fn decrypt(
        &self,
        iv: &[u8; 12],
        aad: &[u8],
        ciphertext: &[u8],
        tag: &[u8; 16],
        plaintext: &mut [u8],
    ) -> bool {
        assert!(
            plaintext.len() >= ciphertext.len(),
            "plaintext buffer too small"
        );

        // Compute H = AES_K(0^128)
        let mut h = [0u8; 16];
        self.cipher.encrypt_block(&mut h);

        // Construct J0 = IV || 0x00000001
        let mut j0 = [0u8; 16];
        j0[..12].copy_from_slice(iv);
        j0[12] = 0x00;
        j0[13] = 0x00;
        j0[14] = 0x00;
        j0[15] = 0x01;

        // Compute GHASH over AAD and ciphertext
        let ghash_val = ghash(&h, aad, ciphertext);

        // Compute expected tag = AES_K(J0) XOR GHASH
        let mut encrypted_j0 = j0;
        self.cipher.encrypt_block(&mut encrypted_j0);
        let mut expected_tag = [0u8; 16];
        for i in 0..16 {
            expected_tag[i] = encrypted_j0[i] ^ ghash_val[i];
        }

        // Constant-time tag comparison to prevent timing attacks
        let mut diff: u8 = 0;
        for i in 0..16 {
            diff |= expected_tag[i] ^ tag[i];
        }

        if diff != 0 {
            // Tag mismatch: zero the plaintext buffer and return false
            for byte in plaintext[..ciphertext.len()].iter_mut() {
                *byte = 0;
            }
            return false;
        }

        // CTR decryption (identical to encryption)
        let mut counter = j0;
        let mut offset = 0;
        while offset < ciphertext.len() {
            increment_counter(&mut counter);
            let mut keystream = counter;
            self.cipher.encrypt_block(&mut keystream);

            let remaining = ciphertext.len() - offset;
            let block_len = if remaining >= 16 { 16 } else { remaining };
            for i in 0..block_len {
                plaintext[offset + i] = ciphertext[offset + i] ^ keystream[i];
            }
            offset += block_len;
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: parse a hex string into a Vec<u8>.
    fn hex_to_bytes(hex: &str) -> Vec<u8> {
        let hex = hex.replace(' ', "");
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect()
    }

    /// NIST SP 800-38D Test Case 1: AES-128-GCM with empty plaintext and empty AAD.
    #[test]
    fn test_nist_aes128_gcm_empty() {
        let key = [0u8; 16];
        let iv = [0u8; 12];
        let aad: &[u8] = &[];
        let plaintext: &[u8] = &[];

        let expected_tag = hex_to_bytes("58e2fccefa7e3061367f1d57a4e7455a");

        let gcm = AesGcm::new(&key);
        let mut ciphertext = vec![0u8; plaintext.len()];
        let mut tag = [0u8; 16];
        gcm.encrypt(&iv, aad, plaintext, &mut ciphertext, &mut tag);

        assert_eq!(ciphertext.len(), 0);
        assert_eq!(&tag[..], &expected_tag[..], "Tag mismatch for NIST test case 1");

        // Verify decryption
        let mut decrypted = vec![0u8; ciphertext.len()];
        let valid = gcm.decrypt(&iv, aad, &ciphertext, &tag, &mut decrypted);
        assert!(valid, "Decryption should succeed with correct tag");
    }

    /// NIST SP 800-38D Test Case 3: AES-128-GCM with data, no AAD.
    #[test]
    fn test_nist_aes128_gcm_with_data() {
        let key = hex_to_bytes("feffe9928665731c6d6a8f9467308308");
        let iv = hex_to_bytes("cafebabefacedbaddecaf888");
        let plaintext = hex_to_bytes(
            "d9313225f88406e5a55909c5aff5269a\
             86a7a9531534f7da2e4c303d8a318a72\
             1c3c0c95956809532fcf0e2449a6b525\
             b16aedf5aa0de657ba637b391aafd255",
        );
        let aad: &[u8] = &[];
        let expected_ciphertext = hex_to_bytes(
            "42831ec2217774244b7221b784d0d49c\
             e3aa212f2c02a4e035c17e2329aca12e\
             21d514b25466931c7d8f6a5aac84aa05\
             1ba30b396a0aac973d58e091473f5985",
        );
        let expected_tag = hex_to_bytes("4d5c2af327cd64a62cf35abd2ba6fab4");

        let key_arr: [u8; 16] = key.try_into().unwrap();
        let iv_arr: [u8; 12] = iv.try_into().unwrap();

        let gcm = AesGcm::new(&key_arr);
        let mut ciphertext = vec![0u8; plaintext.len()];
        let mut tag = [0u8; 16];
        gcm.encrypt(&iv_arr, aad, &plaintext, &mut ciphertext, &mut tag);

        assert_eq!(
            &ciphertext[..],
            &expected_ciphertext[..],
            "Ciphertext mismatch for NIST test case 3"
        );
        assert_eq!(
            &tag[..],
            &expected_tag[..],
            "Tag mismatch for NIST test case 3"
        );

        // Verify decryption
        let mut decrypted = vec![0u8; ciphertext.len()];
        let valid = gcm.decrypt(&iv_arr, aad, &ciphertext, &tag, &mut decrypted);
        assert!(valid, "Decryption should succeed with correct tag");
        assert_eq!(
            &decrypted[..],
            &plaintext[..],
            "Decrypted plaintext mismatch"
        );
    }

    /// Verify that decryption fails with a tampered tag.
    #[test]
    fn test_decrypt_rejects_bad_tag() {
        let key = [0u8; 16];
        let iv = [0u8; 12];
        let aad: &[u8] = &[];
        let plaintext = b"hello world!!!!!"; // 16 bytes

        let gcm = AesGcm::new(&key);
        let mut ciphertext = vec![0u8; plaintext.len()];
        let mut tag = [0u8; 16];
        gcm.encrypt(&iv, aad, plaintext, &mut ciphertext, &mut tag);

        // Tamper with the tag
        tag[0] ^= 0xff;

        let mut decrypted = vec![0u8; ciphertext.len()];
        let valid = gcm.decrypt(&iv, aad, &ciphertext, &tag, &mut decrypted);
        assert!(!valid, "Decryption should fail with tampered tag");
        // Plaintext buffer should be zeroed on failure
        assert_eq!(&decrypted[..], &[0u8; 16][..], "Plaintext should be zeroed on auth failure");
    }

    /// Verify that decryption fails with tampered ciphertext.
    #[test]
    fn test_decrypt_rejects_tampered_ciphertext() {
        let key = hex_to_bytes("feffe9928665731c6d6a8f9467308308");
        let iv = hex_to_bytes("cafebabefacedbaddecaf888");
        let plaintext = hex_to_bytes(
            "d9313225f88406e5a55909c5aff5269a\
             86a7a9531534f7da2e4c303d8a318a72\
             1c3c0c95956809532fcf0e2449a6b525\
             b16aedf5aa0de657ba637b391aafd255",
        );

        let key_arr: [u8; 16] = key.try_into().unwrap();
        let iv_arr: [u8; 12] = iv.try_into().unwrap();

        let gcm = AesGcm::new(&key_arr);
        let mut ciphertext = vec![0u8; plaintext.len()];
        let mut tag = [0u8; 16];
        gcm.encrypt(&iv_arr, &[], &plaintext, &mut ciphertext, &mut tag);

        // Tamper with the ciphertext
        ciphertext[0] ^= 0x01;

        let mut decrypted = vec![0u8; ciphertext.len()];
        let valid = gcm.decrypt(&iv_arr, &[], &ciphertext, &tag, &mut decrypted);
        assert!(!valid, "Decryption should fail with tampered ciphertext");
    }
}
