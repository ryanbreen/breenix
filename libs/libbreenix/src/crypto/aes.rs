//! AES-128/256 block cipher (FIPS 197)
//!
//! Pure Rust implementation with no external dependencies.
//! Provides AES-128 and AES-256 encryption in ECB mode (single block).
//!
//! # Column-Major State Layout
//!
//! AES operates on a 4x4 byte matrix in column-major order:
//! ```text
//! state[0]  state[4]  state[8]  state[12]
//! state[1]  state[5]  state[9]  state[13]
//! state[2]  state[6]  state[10] state[14]
//! state[3]  state[7]  state[11] state[15]
//! ```
//!
//! # Examples
//!
//! ```
//! use libbreenix::crypto::aes::Aes128;
//!
//! let key = [0u8; 16];
//! let cipher = Aes128::new(&key);
//! let mut block = [0u8; 16];
//! cipher.encrypt_block(&mut block);
//! ```

/// AES S-Box lookup table (FIPS 197, Section 5.1.1).
///
/// Maps each byte value to its substitution value using the multiplicative
/// inverse in GF(2^8) followed by an affine transformation.
#[rustfmt::skip]
pub const SBOX: [u8; 256] = [
    0x63, 0x7c, 0x77, 0x7b, 0xf2, 0x6b, 0x6f, 0xc5, 0x30, 0x01, 0x67, 0x2b, 0xfe, 0xd7, 0xab, 0x76,
    0xca, 0x82, 0xc9, 0x7d, 0xfa, 0x59, 0x47, 0xf0, 0xad, 0xd4, 0xa2, 0xaf, 0x9c, 0xa4, 0x72, 0xc0,
    0xb7, 0xfd, 0x93, 0x26, 0x36, 0x3f, 0xf7, 0xcc, 0x34, 0xa5, 0xe5, 0xf1, 0x71, 0xd8, 0x31, 0x15,
    0x04, 0xc7, 0x23, 0xc3, 0x18, 0x96, 0x05, 0x9a, 0x07, 0x12, 0x80, 0xe2, 0xeb, 0x27, 0xb2, 0x75,
    0x09, 0x83, 0x2c, 0x1a, 0x1b, 0x6e, 0x5a, 0xa0, 0x52, 0x3b, 0xd6, 0xb3, 0x29, 0xe3, 0x2f, 0x84,
    0x53, 0xd1, 0x00, 0xed, 0x20, 0xfc, 0xb1, 0x5b, 0x6a, 0xcb, 0xbe, 0x39, 0x4a, 0x4c, 0x58, 0xcf,
    0xd0, 0xef, 0xaa, 0xfb, 0x43, 0x4d, 0x33, 0x85, 0x45, 0xf9, 0x02, 0x7f, 0x50, 0x3c, 0x9f, 0xa8,
    0x51, 0xa3, 0x40, 0x8f, 0x92, 0x9d, 0x38, 0xf5, 0xbc, 0xb6, 0xda, 0x21, 0x10, 0xff, 0xf3, 0xd2,
    0xcd, 0x0c, 0x13, 0xec, 0x5f, 0x97, 0x44, 0x17, 0xc4, 0xa7, 0x7e, 0x3d, 0x64, 0x5d, 0x19, 0x73,
    0x60, 0x81, 0x4f, 0xdc, 0x22, 0x2a, 0x90, 0x88, 0x46, 0xee, 0xb8, 0x14, 0xde, 0x5e, 0x0b, 0xdb,
    0xe0, 0x32, 0x3a, 0x0a, 0x49, 0x06, 0x24, 0x5c, 0xc2, 0xd3, 0xac, 0x62, 0x91, 0x95, 0xe4, 0x79,
    0xe7, 0xc8, 0x37, 0x6d, 0x8d, 0xd5, 0x4e, 0xa9, 0x6c, 0x56, 0xf4, 0xea, 0x65, 0x7a, 0xae, 0x08,
    0xba, 0x78, 0x25, 0x2e, 0x1c, 0xa6, 0xb4, 0xc6, 0xe8, 0xdd, 0x74, 0x1f, 0x4b, 0xbd, 0x8b, 0x8a,
    0x70, 0x3e, 0xb5, 0x66, 0x48, 0x03, 0xf6, 0x0e, 0x61, 0x35, 0x57, 0xb9, 0x86, 0xc1, 0x1d, 0x9e,
    0xe1, 0xf8, 0x98, 0x11, 0x69, 0xd9, 0x8e, 0x94, 0x9b, 0x1e, 0x87, 0xe9, 0xce, 0x55, 0x28, 0xdf,
    0x8c, 0xa1, 0x89, 0x0d, 0xbf, 0xe6, 0x42, 0x68, 0x41, 0x99, 0x2d, 0x0f, 0xb0, 0x54, 0xbb, 0x16,
];

/// Round constants for key expansion (FIPS 197, Section 5.2).
///
/// Index 0 is unused (padding). RCON[i] = x^(i-1) in GF(2^8) with reduction
/// polynomial x^8 + x^4 + x^3 + x + 1.
pub const RCON: [u8; 11] = [
    0x00, 0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x1b, 0x36,
];

/// Multiply by x (i.e., by 0x02) in GF(2^8).
///
/// Performs a left shift and conditionally XORs with the reduction polynomial
/// 0x1b if the high bit was set.
#[inline]
pub fn xtime(a: u8) -> u8 {
    let shifted = (a as u16) << 1;
    // If the high bit of `a` was set, reduce modulo the AES polynomial
    (shifted as u8) ^ (if a & 0x80 != 0 { 0x1b } else { 0x00 })
}

/// Multiply two elements in GF(2^8) using the AES reduction polynomial.
///
/// Uses the "peasant multiplication" algorithm: for each set bit in `b`,
/// accumulate the corresponding power-of-two multiple of `a` (computed
/// via repeated `xtime`).
#[inline]
pub fn gmul(mut a: u8, mut b: u8) -> u8 {
    let mut result: u8 = 0;
    while b != 0 {
        if b & 1 != 0 {
            result ^= a;
        }
        a = xtime(a);
        b >>= 1;
    }
    result
}

/// Expand a 128-bit key into 11 round keys (FIPS 197, Section 5.2).
///
/// AES-128 uses Nk=4 (4 words = 16 bytes key), Nr=10 (10 rounds),
/// producing 11 round keys of 16 bytes each.
pub fn aes128_expand_key(key: &[u8; 16]) -> [[u8; 16]; 11] {
    // Total words needed: 4 * (10 + 1) = 44
    let mut w = [0u8; 44 * 4];

    // Copy original key into first Nk words
    w[..16].copy_from_slice(key);

    let nk = 4;
    for i in nk..44 {
        let mut temp = [0u8; 4];
        temp.copy_from_slice(&w[(i - 1) * 4..i * 4]);

        if i % nk == 0 {
            // RotWord: rotate left by one byte
            let t = temp[0];
            temp[0] = temp[1];
            temp[1] = temp[2];
            temp[2] = temp[3];
            temp[3] = t;
            // SubWord: apply S-Box
            for byte in &mut temp {
                *byte = SBOX[*byte as usize];
            }
            // XOR with round constant
            temp[0] ^= RCON[i / nk];
        }

        for j in 0..4 {
            w[i * 4 + j] = w[(i - nk) * 4 + j] ^ temp[j];
        }
    }

    // Pack into round key arrays
    let mut round_keys = [[0u8; 16]; 11];
    for (r, rk) in round_keys.iter_mut().enumerate() {
        rk.copy_from_slice(&w[r * 16..(r + 1) * 16]);
    }
    round_keys
}

/// Expand a 256-bit key into 15 round keys (FIPS 197, Section 5.2).
///
/// AES-256 uses Nk=8 (8 words = 32 bytes key), Nr=14 (14 rounds),
/// producing 15 round keys of 16 bytes each.
pub fn aes256_expand_key(key: &[u8; 32]) -> [[u8; 16]; 15] {
    // Total words needed: 4 * (14 + 1) = 60
    let mut w = [0u8; 60 * 4];

    // Copy original key into first Nk words
    w[..32].copy_from_slice(key);

    let nk = 8;
    for i in nk..60 {
        let mut temp = [0u8; 4];
        temp.copy_from_slice(&w[(i - 1) * 4..i * 4]);

        if i % nk == 0 {
            // RotWord
            let t = temp[0];
            temp[0] = temp[1];
            temp[1] = temp[2];
            temp[2] = temp[3];
            temp[3] = t;
            // SubWord
            for byte in &mut temp {
                *byte = SBOX[*byte as usize];
            }
            // XOR with round constant
            temp[0] ^= RCON[i / nk];
        } else if i % nk == 4 {
            // Extra SubWord step for AES-256 (FIPS 197, Section 5.2)
            for byte in &mut temp {
                *byte = SBOX[*byte as usize];
            }
        }

        for j in 0..4 {
            w[i * 4 + j] = w[(i - nk) * 4 + j] ^ temp[j];
        }
    }

    // Pack into round key arrays
    let mut round_keys = [[0u8; 16]; 15];
    for (r, rk) in round_keys.iter_mut().enumerate() {
        rk.copy_from_slice(&w[r * 16..(r + 1) * 16]);
    }
    round_keys
}

/// SubBytes transformation (FIPS 197, Section 5.1.1).
///
/// Replaces each byte in the state with its S-Box substitution.
pub fn sub_bytes(state: &mut [u8; 16]) {
    for byte in state.iter_mut() {
        *byte = SBOX[*byte as usize];
    }
}

/// ShiftRows transformation (FIPS 197, Section 5.1.2).
///
/// Cyclically shifts each row of the state matrix to the left by its row index.
/// Row 0 is unchanged, row 1 shifts by 1, row 2 by 2, row 3 by 3.
///
/// State layout is column-major: `state[row + 4*col]`.
pub fn shift_rows(state: &mut [u8; 16]) {
    // Row 0: no shift

    // Row 1: shift left by 1
    // Indices for row 1: [1, 5, 9, 13]
    let t = state[1];
    state[1] = state[5];
    state[5] = state[9];
    state[9] = state[13];
    state[13] = t;

    // Row 2: shift left by 2
    // Indices for row 2: [2, 6, 10, 14]
    let t0 = state[2];
    let t1 = state[6];
    state[2] = state[10];
    state[6] = state[14];
    state[10] = t0;
    state[14] = t1;

    // Row 3: shift left by 3 (equivalent to shift right by 1)
    // Indices for row 3: [3, 7, 11, 15]
    let t = state[15];
    state[15] = state[11];
    state[11] = state[7];
    state[7] = state[3];
    state[3] = t;
}

/// MixColumns transformation (FIPS 197, Section 5.1.3).
///
/// Treats each column of the state as a polynomial over GF(2^8) and
/// multiplies it by the fixed polynomial {03}x^3 + {01}x^2 + {01}x + {02}.
pub fn mix_columns(state: &mut [u8; 16]) {
    for col in 0..4 {
        let base = col * 4;
        let s0 = state[base];
        let s1 = state[base + 1];
        let s2 = state[base + 2];
        let s3 = state[base + 3];

        // The MixColumns matrix multiplication:
        // [2 3 1 1]   [s0]
        // [1 2 3 1] * [s1]
        // [1 1 2 3]   [s2]
        // [3 1 1 2]   [s3]
        state[base] = gmul(2, s0) ^ gmul(3, s1) ^ s2 ^ s3;
        state[base + 1] = s0 ^ gmul(2, s1) ^ gmul(3, s2) ^ s3;
        state[base + 2] = s0 ^ s1 ^ gmul(2, s2) ^ gmul(3, s3);
        state[base + 3] = gmul(3, s0) ^ s1 ^ s2 ^ gmul(2, s3);
    }
}

/// AddRoundKey transformation (FIPS 197, Section 5.1.4).
///
/// XORs the state with the round key.
pub fn add_round_key(state: &mut [u8; 16], round_key: &[u8; 16]) {
    for i in 0..16 {
        state[i] ^= round_key[i];
    }
}

/// AES-128 block cipher.
///
/// Encrypts individual 16-byte blocks using a 128-bit key. This provides the
/// raw ECB-mode primitive; use a higher-level mode (CBC, CTR, GCM) for actual
/// data encryption.
pub struct Aes128 {
    round_keys: [[u8; 16]; 11],
}

impl Aes128 {
    /// Create a new AES-128 cipher instance from a 128-bit (16-byte) key.
    pub fn new(key: &[u8; 16]) -> Self {
        Self {
            round_keys: aes128_expand_key(key),
        }
    }

    /// Encrypt a single 16-byte block in place.
    ///
    /// Performs the full AES-128 encryption: initial AddRoundKey, 9 main rounds
    /// (SubBytes, ShiftRows, MixColumns, AddRoundKey), and a final round
    /// (SubBytes, ShiftRows, AddRoundKey) without MixColumns.
    pub fn encrypt_block(&self, block: &mut [u8; 16]) {
        // Initial round key addition
        add_round_key(block, &self.round_keys[0]);

        // Rounds 1 through 9: full rounds
        for round in 1..10 {
            sub_bytes(block);
            shift_rows(block);
            mix_columns(block);
            add_round_key(block, &self.round_keys[round]);
        }

        // Final round (no MixColumns)
        sub_bytes(block);
        shift_rows(block);
        add_round_key(block, &self.round_keys[10]);
    }
}

/// AES-256 block cipher.
///
/// Encrypts individual 16-byte blocks using a 256-bit key. This provides the
/// raw ECB-mode primitive; use a higher-level mode (CBC, CTR, GCM) for actual
/// data encryption.
pub struct Aes256 {
    round_keys: [[u8; 16]; 15],
}

impl Aes256 {
    /// Create a new AES-256 cipher instance from a 256-bit (32-byte) key.
    pub fn new(key: &[u8; 32]) -> Self {
        Self {
            round_keys: aes256_expand_key(key),
        }
    }

    /// Encrypt a single 16-byte block in place.
    ///
    /// Performs the full AES-256 encryption: initial AddRoundKey, 13 main rounds
    /// (SubBytes, ShiftRows, MixColumns, AddRoundKey), and a final round
    /// (SubBytes, ShiftRows, AddRoundKey) without MixColumns.
    pub fn encrypt_block(&self, block: &mut [u8; 16]) {
        // Initial round key addition
        add_round_key(block, &self.round_keys[0]);

        // Rounds 1 through 13: full rounds
        for round in 1..14 {
            sub_bytes(block);
            shift_rows(block);
            mix_columns(block);
            add_round_key(block, &self.round_keys[round]);
        }

        // Final round (no MixColumns)
        sub_bytes(block);
        shift_rows(block);
        add_round_key(block, &self.round_keys[14]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to convert a hex string to a byte array.
    fn hex_to_bytes<const N: usize>(hex: &str) -> [u8; N] {
        let mut bytes = [0u8; N];
        for i in 0..N {
            bytes[i] = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap();
        }
        bytes
    }

    /// Helper to convert a byte array to a hex string.
    fn bytes_to_hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{:02x}", b)).collect()
    }

    #[test]
    fn test_xtime() {
        // xtime(0x57) = 0xae (no reduction, high bit not set)
        assert_eq!(xtime(0x57), 0xae);
        // xtime(0xae) = 0x47 (reduction: 0x5c ^ 0x1b = 0x47)
        assert_eq!(xtime(0xae), 0x47);
        // xtime(0x00) = 0x00
        assert_eq!(xtime(0x00), 0x00);
        // xtime(0x01) = 0x02
        assert_eq!(xtime(0x01), 0x02);
        // xtime(0x80) = 0x1b (just the reduction polynomial)
        assert_eq!(xtime(0x80), 0x1b);
    }

    #[test]
    fn test_gmul() {
        // gmul(0x57, 0x02) = xtime(0x57) = 0xae
        assert_eq!(gmul(0x57, 0x02), 0xae);
        // gmul(0x57, 0x04) = xtime(xtime(0x57)) = xtime(0xae) = 0x47
        assert_eq!(gmul(0x57, 0x04), 0x47);
        // gmul(0x57, 0x13) = 0xfe (FIPS 197, Section 4.2.1)
        assert_eq!(gmul(0x57, 0x13), 0xfe);
        // Identity: gmul(a, 1) = a
        assert_eq!(gmul(0x57, 0x01), 0x57);
        // Zero: gmul(a, 0) = 0
        assert_eq!(gmul(0x57, 0x00), 0x00);
    }

    #[test]
    fn test_sub_bytes() {
        let mut state = [0u8; 16];
        state[0] = 0x00;
        state[1] = 0x01;
        state[2] = 0x10;
        state[3] = 0xff;
        sub_bytes(&mut state);
        assert_eq!(state[0], 0x63); // SBOX[0x00]
        assert_eq!(state[1], 0x7c); // SBOX[0x01]
        assert_eq!(state[2], 0xca); // SBOX[0x10]
        assert_eq!(state[3], 0x16); // SBOX[0xff]
    }

    #[test]
    fn test_shift_rows() {
        // Column-major layout:
        // Before:
        //   col0     col1     col2     col3
        //   [0]=0    [4]=4    [8]=8    [12]=12
        //   [1]=1    [5]=5    [9]=9    [13]=13
        //   [2]=2    [6]=6    [10]=10  [14]=14
        //   [3]=3    [7]=7    [11]=11  [15]=15
        //
        // After ShiftRows (rotate row r left by r):
        // Row 0: 0,  4,  8,  12  (unchanged)
        // Row 1: 5,  9,  13, 1   (shift left 1)
        // Row 2: 10, 14, 2,  6   (shift left 2)
        // Row 3: 15, 3,  7,  11  (shift left 3)
        let mut state: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
        shift_rows(&mut state);

        // Column 0: row0=0, row1=5, row2=10, row3=15
        assert_eq!(state[0], 0);
        assert_eq!(state[1], 5);
        assert_eq!(state[2], 10);
        assert_eq!(state[3], 15);

        // Column 1: row0=4, row1=9, row2=14, row3=3
        assert_eq!(state[4], 4);
        assert_eq!(state[5], 9);
        assert_eq!(state[6], 14);
        assert_eq!(state[7], 3);

        // Column 2: row0=8, row1=13, row2=2, row3=7
        assert_eq!(state[8], 8);
        assert_eq!(state[9], 13);
        assert_eq!(state[10], 2);
        assert_eq!(state[11], 7);

        // Column 3: row0=12, row1=1, row2=6, row3=11
        assert_eq!(state[12], 12);
        assert_eq!(state[13], 1);
        assert_eq!(state[14], 6);
        assert_eq!(state[15], 11);
    }

    #[test]
    fn test_mix_columns() {
        // FIPS 197, Section 5.1.3 example:
        // Input column: [0xdb, 0x13, 0x53, 0x45]
        // Output column: [0x8e, 0x4d, 0xa1, 0xbc]
        let mut state = [0u8; 16];
        state[0] = 0xdb;
        state[1] = 0x13;
        state[2] = 0x53;
        state[3] = 0x45;
        // Fill other columns with zeros (they won't affect column 0)
        mix_columns(&mut state);
        assert_eq!(state[0], 0x8e);
        assert_eq!(state[1], 0x4d);
        assert_eq!(state[2], 0xa1);
        assert_eq!(state[3], 0xbc);
    }

    #[test]
    fn test_add_round_key() {
        let mut state = [0xffu8; 16];
        let key = [0xffu8; 16];
        add_round_key(&mut state, &key);
        assert_eq!(state, [0u8; 16]);

        let mut state = [0x00u8; 16];
        let key = [0xabu8; 16];
        add_round_key(&mut state, &key);
        assert_eq!(state, [0xabu8; 16]);
    }

    /// FIPS 197, Appendix B test vector.
    ///
    /// Key:        2b7e151628aed2a6abf7158809cf4f3c
    /// Plaintext:  3243f6a8885a308d313198a2e0370734
    /// Ciphertext: 3925841d02dc09fbdc118597196a0b32
    #[test]
    fn test_aes128_fips197_appendix_b() {
        let key: [u8; 16] = hex_to_bytes("2b7e151628aed2a6abf7158809cf4f3c");
        let plaintext: [u8; 16] = hex_to_bytes("3243f6a8885a308d313198a2e0370734");
        let expected: [u8; 16] = hex_to_bytes("3925841d02dc09fbdc118597196a0b32");

        let cipher = Aes128::new(&key);
        let mut block = plaintext;
        cipher.encrypt_block(&mut block);

        assert_eq!(
            bytes_to_hex(&block),
            bytes_to_hex(&expected),
            "AES-128 FIPS 197 Appendix B test vector failed"
        );
    }

    /// AES-256 test vector.
    ///
    /// Key:        000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f
    /// Plaintext:  00112233445566778899aabbccddeeff
    /// Ciphertext: 8ea2b7ca516745bfeafc49904b496089
    #[test]
    fn test_aes256_test_vector() {
        let key: [u8; 32] =
            hex_to_bytes("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f");
        let plaintext: [u8; 16] = hex_to_bytes("00112233445566778899aabbccddeeff");
        let expected: [u8; 16] = hex_to_bytes("8ea2b7ca516745bfeafc49904b496089");

        let cipher = Aes256::new(&key);
        let mut block = plaintext;
        cipher.encrypt_block(&mut block);

        assert_eq!(
            bytes_to_hex(&block),
            bytes_to_hex(&expected),
            "AES-256 test vector failed"
        );
    }

    /// Verify AES-128 key expansion produces correct first and last round keys.
    #[test]
    fn test_aes128_key_expansion() {
        let key: [u8; 16] = hex_to_bytes("2b7e151628aed2a6abf7158809cf4f3c");
        let round_keys = aes128_expand_key(&key);

        // Round key 0 should be the original key
        assert_eq!(bytes_to_hex(&round_keys[0]), "2b7e151628aed2a6abf7158809cf4f3c");

        // Round key 10 (last) - known from FIPS 197 Appendix A.1
        assert_eq!(bytes_to_hex(&round_keys[10]), "d014f9a8c9ee2589e13f0cc8b6630ca6");
    }

    /// Verify AES-256 key expansion produces correct first round keys.
    #[test]
    fn test_aes256_key_expansion() {
        let key: [u8; 32] =
            hex_to_bytes("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f");
        let round_keys = aes256_expand_key(&key);

        // Round key 0 should be the first half of the original key
        assert_eq!(bytes_to_hex(&round_keys[0]), "000102030405060708090a0b0c0d0e0f");

        // Round key 1 should be the second half of the original key
        assert_eq!(bytes_to_hex(&round_keys[1]), "101112131415161718191a1b1c1d1e1f");
    }

    /// Test that encrypting all zeros with an all-zero key produces the expected result.
    #[test]
    fn test_aes128_zero_key_zero_plaintext() {
        let key = [0u8; 16];
        let cipher = Aes128::new(&key);
        let mut block = [0u8; 16];
        cipher.encrypt_block(&mut block);

        // Known result for AES-128(zero_key, zero_plaintext)
        assert_eq!(
            bytes_to_hex(&block),
            "66e94bd4ef8a2c3b884cfa59ca342b2e"
        );
    }
}
