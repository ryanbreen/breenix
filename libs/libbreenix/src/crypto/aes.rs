//! AES-128/256 block encryption (FIPS 197), bitsliced and constant-time.
//!
//! A byte-oriented AES looks up its S-box by state and key bytes, so which
//! cache lines it touches depends on secrets. This implementation never
//! indexes memory by data. Four blocks are processed together as eight
//! 64-bit bit-planes: plane `i` holds bit `i` of all 64 bytes, and byte `k`
//! of the four blocks is bit `k` of every plane. SubBytes is then a Boolean
//! circuit evaluated on whole planes (the 113-gate circuit of Boyar and
//! Peralta, "A depth-16 circuit for the AES S-box", 2012), and ShiftRows
//! and MixColumns are fixed masks and shifts.
//!
//! # Column-major state layout
//!
//! ```text
//! state[0]  state[4]  state[8]  state[12]
//! state[1]  state[5]  state[9]  state[13]
//! state[2]  state[6]  state[10] state[14]
//! state[3]  state[7]  state[11] state[15]
//! ```
//!
//! Within a block, byte `r + 4c` is row `r` of column `c`, so in a plane a
//! block owns 16 bits, each column owns 4 of them and the row is the bit's
//! position within its group of 4.
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

/// Eight bit-planes of four blocks.
type Planes = [u64; 8];

/// Round constants for key expansion (FIPS 197, Section 5.2). Index 0 is unused.
const RCON: [u8; 11] = [0x00, 0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x1b, 0x36];

/// A 16-bit pattern repeated in each of the four blocks.
const fn lanes(pattern: u64) -> u64 {
    pattern * 0x0001_0001_0001_0001
}

/// Transpose an 8x8 bit matrix: bit `8 * row + col` moves to `8 * col + row`.
fn transpose8(mut x: u64) -> u64 {
    let t = (x ^ (x >> 7)) & 0x00aa_00aa_00aa_00aa;
    x ^= t ^ (t << 7);
    let t = (x ^ (x >> 14)) & 0x0000_cccc_0000_cccc;
    x ^= t ^ (t << 14);
    let t = (x ^ (x >> 28)) & 0x0000_0000_f0f0_f0f0;
    x ^= t ^ (t << 28);
    x
}

fn pack(bytes: &[u8; 64]) -> Planes {
    let mut planes = [0u64; 8];
    for (group, chunk) in bytes.chunks_exact(8).enumerate() {
        // Byte i of the transposed word holds bit i of each of the 8 bytes.
        let transposed = transpose8(u64::from_le_bytes(chunk.try_into().unwrap()));
        for (bit, plane) in planes.iter_mut().enumerate() {
            *plane |= ((transposed >> (8 * bit)) & 0xff) << (8 * group);
        }
    }
    planes
}

fn unpack(planes: &Planes) -> [u8; 64] {
    let mut bytes = [0u8; 64];
    for (group, chunk) in bytes.chunks_exact_mut(8).enumerate() {
        let mut transposed = 0;
        for (bit, plane) in planes.iter().enumerate() {
            transposed |= ((plane >> (8 * group)) & 0xff) << (8 * bit);
        }
        chunk.copy_from_slice(&transpose8(transposed).to_le_bytes());
    }
    bytes
}

/// SubBytes on every byte of the planes. U0 and S0 are the most
/// significant bits, as in the circuit's description.
fn sub_bytes(planes: &mut Planes) {
    let [u7, u6, u5, u4, u3, u2, u1, u0] = *planes;

    // Top linear transformation.
    let t1 = u0 ^ u3;
    let t2 = u0 ^ u5;
    let t3 = u0 ^ u6;
    let t4 = u3 ^ u5;
    let t5 = u4 ^ u6;
    let t6 = t1 ^ t5;
    let t7 = u1 ^ u2;
    let t8 = u7 ^ t6;
    let t9 = u7 ^ t7;
    let t10 = t6 ^ t7;
    let t11 = u1 ^ u5;
    let t12 = u2 ^ u5;
    let t13 = t3 ^ t4;
    let t14 = t6 ^ t11;
    let t15 = t5 ^ t11;
    let t16 = t5 ^ t12;
    let t17 = t9 ^ t16;
    let t18 = u3 ^ u7;
    let t19 = t7 ^ t18;
    let t20 = t1 ^ t19;
    let t21 = u6 ^ u7;
    let t22 = t7 ^ t21;
    let t23 = t2 ^ t22;
    let t24 = t2 ^ t10;
    let t25 = t20 ^ t17;
    let t26 = t3 ^ t16;
    let t27 = t1 ^ t12;

    // Shared non-linear middle: the inversion in GF(2^8).
    let m1 = t13 & t6;
    let m2 = t23 & t8;
    let m3 = t14 ^ m1;
    let m4 = t19 & u7;
    let m5 = m4 ^ m1;
    let m6 = t3 & t16;
    let m7 = t22 & t9;
    let m8 = t26 ^ m6;
    let m9 = t20 & t17;
    let m10 = m9 ^ m6;
    let m11 = t1 & t15;
    let m12 = t4 & t27;
    let m13 = m12 ^ m11;
    let m14 = t2 & t10;
    let m15 = m14 ^ m11;
    let m16 = m3 ^ m2;
    let m17 = m5 ^ t24;
    let m18 = m8 ^ m7;
    let m19 = m10 ^ m15;
    let m20 = m16 ^ m13;
    let m21 = m17 ^ m15;
    let m22 = m18 ^ m13;
    let m23 = m19 ^ t25;
    let m24 = m22 ^ m23;
    let m25 = m22 & m20;
    let m26 = m21 ^ m25;
    let m27 = m20 ^ m21;
    let m28 = m23 ^ m25;
    let m29 = m28 & m27;
    let m30 = m26 & m24;
    let m31 = m20 & m23;
    let m32 = m27 & m31;
    let m33 = m27 ^ m25;
    let m34 = m21 & m22;
    let m35 = m24 & m34;
    let m36 = m24 ^ m25;
    let m37 = m21 ^ m29;
    let m38 = m32 ^ m33;
    let m39 = m23 ^ m30;
    let m40 = m35 ^ m36;
    let m41 = m38 ^ m40;
    let m42 = m37 ^ m39;
    let m43 = m37 ^ m38;
    let m44 = m39 ^ m40;
    let m45 = m42 ^ m41;
    let m46 = m44 & t6;
    let m47 = m40 & t8;
    let m48 = m39 & u7;
    let m49 = m43 & t16;
    let m50 = m38 & t9;
    let m51 = m37 & t17;
    let m52 = m42 & t15;
    let m53 = m45 & t27;
    let m54 = m41 & t10;
    let m55 = m44 & t13;
    let m56 = m40 & t23;
    let m57 = m39 & t19;
    let m58 = m43 & t3;
    let m59 = m38 & t22;
    let m60 = m37 & t20;
    let m61 = m42 & t1;
    let m62 = m45 & t4;
    let m63 = m41 & t2;

    // Bottom linear transformation, including the affine constant.
    let l0 = m61 ^ m62;
    let l1 = m50 ^ m56;
    let l2 = m46 ^ m48;
    let l3 = m47 ^ m55;
    let l4 = m54 ^ m58;
    let l5 = m49 ^ m61;
    let l6 = m62 ^ l5;
    let l7 = m46 ^ l3;
    let l8 = m51 ^ m59;
    let l9 = m52 ^ m53;
    let l10 = m53 ^ l4;
    let l11 = m60 ^ l2;
    let l12 = m48 ^ m51;
    let l13 = m50 ^ l0;
    let l14 = m52 ^ m61;
    let l15 = m55 ^ l1;
    let l16 = m56 ^ l0;
    let l17 = m57 ^ l1;
    let l18 = m58 ^ l8;
    let l19 = m63 ^ l4;
    let l20 = l0 ^ l1;
    let l21 = l1 ^ l7;
    let l22 = l3 ^ l12;
    let l23 = l18 ^ l2;
    let l24 = l15 ^ l9;
    let l25 = l6 ^ l10;
    let l26 = l7 ^ l9;
    let l27 = l8 ^ l10;
    let l28 = l11 ^ l14;
    let l29 = l11 ^ l17;
    let s0 = l6 ^ l24;
    let s1 = !(l16 ^ l26);
    let s2 = !(l19 ^ l28);
    let s3 = l6 ^ l21;
    let s4 = l20 ^ l22;
    let s5 = l25 ^ l29;
    let s6 = !(l13 ^ l27);
    let s7 = !(l6 ^ l23);

    *planes = [s7, s6, s5, s4, s3, s2, s1, s0];
}

/// ShiftRows: row `r` of each block rotates left by `r` columns.
fn shift_rows(planes: &mut Planes) {
    for plane in planes.iter_mut() {
        let x = *plane;
        *plane = (x & lanes(0x1111))
            | ((x & lanes(0x2220)) >> 4)
            | ((x & lanes(0x0002)) << 12)
            | ((x & lanes(0x4400)) >> 8)
            | ((x & lanes(0x0044)) << 8)
            | ((x & lanes(0x8000)) >> 12)
            | ((x & lanes(0x0888)) << 4);
    }
}

/// Row `r` of each column takes the value of row `r + 1`.
fn rotate_rows1(x: u64) -> u64 {
    ((x >> 1) & lanes(0x7777)) | ((x << 3) & lanes(0x8888))
}

/// Row `r` of each column takes the value of row `r + 2`.
fn rotate_rows2(x: u64) -> u64 {
    ((x >> 2) & lanes(0x3333)) | ((x << 2) & lanes(0xcccc))
}

/// MixColumns. With t[r] = s[r] ^ s[r+1], each output row is
/// {02}t[r] ^ s[r+1] ^ t[r+2], which equals
/// {02}s[r] ^ {03}s[r+1] ^ s[r+2] ^ s[r+3] (FIPS 197, Section 5.1.3).
fn mix_columns(planes: &mut Planes) {
    let s = *planes;
    let mut t = [0u64; 8];
    for i in 0..8 {
        t[i] = s[i] ^ rotate_rows1(s[i]);
    }
    // Multiplication by {02}: shift up one bit and reduce by x^8 = x^4 + x^3 + x + 1.
    let doubled = [t[7], t[0] ^ t[7], t[1], t[2] ^ t[7], t[3] ^ t[7], t[4], t[5], t[6]];
    for i in 0..8 {
        planes[i] = doubled[i] ^ rotate_rows1(s[i]) ^ rotate_rows2(t[i]);
    }
}

fn add_round_key(planes: &mut Planes, round_key: &Planes) {
    for (plane, key) in planes.iter_mut().zip(round_key) {
        *plane ^= key;
    }
}

/// Apply the S-box to each byte of a key-schedule word.
pub fn sub_word(word: u32) -> u32 {
    let mut bytes = [0u8; 64];
    bytes[..4].copy_from_slice(&word.to_le_bytes());
    let mut planes = pack(&bytes);
    sub_bytes(&mut planes);
    u32::from_le_bytes(unpack(&planes)[..4].try_into().unwrap())
}

/// FIPS 197 Section 5.2 for Nk key words, giving `ROUNDS + 1` round keys.
fn expand_key<const NK: usize, const KEYS: usize>(key: &[u8]) -> [[u8; 16]; KEYS] {
    let total = 4 * KEYS;
    let mut words = [0u32; 60];
    for (word, chunk) in words.iter_mut().zip(key.chunks_exact(4)) {
        *word = u32::from_le_bytes(chunk.try_into().unwrap());
    }
    for i in NK..total {
        let mut temp = words[i - 1];
        if i % NK == 0 {
            // RotWord moves byte 0 to byte 3; bytes are little-endian here.
            temp = sub_word(temp.rotate_right(8)) ^ RCON[i / NK] as u32;
        } else if NK > 6 && i % NK == 4 {
            temp = sub_word(temp);
        }
        words[i] = words[i - NK] ^ temp;
    }
    let mut round_keys = [[0u8; 16]; KEYS];
    for (round, round_key) in round_keys.iter_mut().enumerate() {
        for (column, chunk) in round_key.chunks_exact_mut(4).enumerate() {
            chunk.copy_from_slice(&words[4 * round + column].to_le_bytes());
        }
    }
    round_keys
}

/// Expand a 128-bit key into 11 round keys (FIPS 197, Section 5.2).
pub fn aes128_expand_key(key: &[u8; 16]) -> [[u8; 16]; 11] {
    expand_key::<4, 11>(key)
}

/// Expand a 256-bit key into 15 round keys (FIPS 197, Section 5.2).
pub fn aes256_expand_key(key: &[u8; 32]) -> [[u8; 16]; 15] {
    expand_key::<8, 15>(key)
}

/// Each round key repeated in all four block positions.
fn round_key_planes<const KEYS: usize>(round_keys: &[[u8; 16]; KEYS]) -> [Planes; KEYS] {
    let mut planes = [[0u64; 8]; KEYS];
    for (planes, round_key) in planes.iter_mut().zip(round_keys) {
        let mut bytes = [0u8; 64];
        for block in bytes.chunks_exact_mut(16) {
            block.copy_from_slice(round_key);
        }
        *planes = pack(&bytes);
    }
    planes
}

/// Encrypt four blocks in place with the given round keys.
fn encrypt4(round_keys: &[Planes], blocks: &mut [u8; 64]) {
    let rounds = round_keys.len() - 1;
    let mut state = pack(blocks);
    add_round_key(&mut state, &round_keys[0]);
    for round_key in &round_keys[1..rounds] {
        sub_bytes(&mut state);
        shift_rows(&mut state);
        mix_columns(&mut state);
        add_round_key(&mut state, round_key);
    }
    sub_bytes(&mut state);
    shift_rows(&mut state);
    add_round_key(&mut state, &round_keys[rounds]);
    *blocks = unpack(&state);
}

/// Encrypt any number of blocks, four at a time.
fn encrypt_many(round_keys: &[Planes], blocks: &mut [[u8; 16]]) {
    for group in blocks.chunks_mut(4) {
        let mut bytes = [0u8; 64];
        for (slot, block) in bytes.chunks_exact_mut(16).zip(group.iter()) {
            slot.copy_from_slice(block);
        }
        encrypt4(round_keys, &mut bytes);
        for (block, slot) in group.iter_mut().zip(bytes.chunks_exact(16)) {
            block.copy_from_slice(slot);
        }
    }
}

/// AES-128 block cipher (encryption only; CTR, GCM, CCM and CMAC need no
/// inverse cipher).
pub struct Aes128 {
    round_keys: [Planes; 11],
}

impl Aes128 {
    /// Create a new AES-128 cipher instance from a 128-bit (16-byte) key.
    pub fn new(key: &[u8; 16]) -> Self {
        Self { round_keys: round_key_planes(&aes128_expand_key(key)) }
    }

    /// Encrypt a single 16-byte block in place.
    pub fn encrypt_block(&self, block: &mut [u8; 16]) {
        encrypt_many(&self.round_keys, core::slice::from_mut(block));
    }

    /// Encrypt several independent blocks in place, four per pass.
    pub fn encrypt_blocks(&self, blocks: &mut [[u8; 16]]) {
        encrypt_many(&self.round_keys, blocks);
    }
}

/// AES-256 block cipher (encryption only).
pub struct Aes256 {
    round_keys: [Planes; 15],
}

impl Aes256 {
    /// Create a new AES-256 cipher instance from a 256-bit (32-byte) key.
    pub fn new(key: &[u8; 32]) -> Self {
        Self { round_keys: round_key_planes(&aes256_expand_key(key)) }
    }

    /// Encrypt a single 16-byte block in place.
    pub fn encrypt_block(&self, block: &mut [u8; 16]) {
        encrypt_many(&self.round_keys, core::slice::from_mut(block));
    }

    /// Encrypt several independent blocks in place, four per pass.
    pub fn encrypt_blocks(&self, blocks: &mut [[u8; 16]]) {
        encrypt_many(&self.round_keys, blocks);
    }
}

/// AES-128 in CTR mode (NIST SP 800-38A).
///
/// Encrypts and decrypts data by XORing with an AES-encrypted counter block.
/// The counter is a full 128-bit big-endian integer that increments by 1
/// for each block, matching the SSH (RFC 4344) counter mode specification.
pub struct Aes128Ctr {
    cipher: Aes128,
    counter: u128,
    /// Keystream left over from a block that a call ended inside.
    spare: [u8; 16],
    spare_len: usize,
}

impl Aes128Ctr {
    /// Create a new AES-128-CTR instance.
    ///
    /// # Arguments
    /// * `key` - 128-bit encryption key
    /// * `iv` - 128-bit initial counter value
    pub fn new(key: &[u8; 16], iv: &[u8; 16]) -> Self {
        Self {
            cipher: Aes128::new(key),
            counter: u128::from_be_bytes(*iv),
            spare: [0; 16],
            spare_len: 0,
        }
    }

    /// Encrypt or decrypt data in place.
    ///
    /// CTR mode is its own inverse: encrypt and decrypt are the same operation.
    /// Consecutive calls continue one keystream, as SSH's length-first
    /// decryption needs.
    pub fn process(&mut self, mut data: &mut [u8]) {
        let used = self.spare_len.min(data.len());
        let start = 16 - self.spare_len;
        for (byte, key) in data[..used].iter_mut().zip(&self.spare[start..]) {
            *byte ^= key;
        }
        self.spare_len -= used;
        data = &mut data[used..];

        let mut keystream = [[0u8; 16]; 4];
        while !data.is_empty() {
            let blocks = data.len().div_ceil(16).min(4);
            for block in &mut keystream[..blocks] {
                *block = self.counter.to_be_bytes();
                self.counter = self.counter.wrapping_add(1);
            }
            self.cipher.encrypt_blocks(&mut keystream[..blocks]);
            let take = data.len().min(16 * blocks);
            for (index, byte) in data[..take].iter_mut().enumerate() {
                *byte ^= keystream[index / 16][index % 16];
            }
            if take % 16 != 0 {
                self.spare = keystream[take / 16];
                self.spare_len = 16 - take % 16;
            }
            data = &mut data[take..];
        }
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

    /// The four-block path must give each block the result one block gets,
    /// including in a short final group.
    #[test]
    fn multi_block_matches_single_block() {
        let cipher = Aes256::new(&[0x42; 32]);
        let mut blocks: Vec<[u8; 16]> = (0..7u8).map(|i| [i.wrapping_mul(37); 16]).collect();
        let expected: Vec<[u8; 16]> = blocks
            .iter()
            .map(|block| {
                let mut block = *block;
                cipher.encrypt_block(&mut block);
                block
            })
            .collect();
        cipher.encrypt_blocks(&mut blocks);
        assert_eq!(blocks, expected);
    }

    /// CTR keystream continues across calls that split a block.
    #[test]
    fn ctr_continues_across_calls() {
        let mut whole = [0u8; 80];
        Aes128Ctr::new(&[1; 16], &[0xff; 16]).process(&mut whole);
        let mut split = [0u8; 80];
        let mut ctr = Aes128Ctr::new(&[1; 16], &[0xff; 16]);
        let (head, tail) = split.split_at_mut(21);
        ctr.process(head);
        ctr.process(tail);
        assert_eq!(split, whole);
    }
}
