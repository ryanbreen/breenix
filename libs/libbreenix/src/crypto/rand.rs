//! ChaCha20-based cryptographically secure pseudorandom number generator
//!
//! Provides a CSPRNG seeded from the kernel's `getrandom` syscall. The ChaCha20
//! stream cipher is used as the core PRNG, following the construction described
//! in RFC 7539.
//!
//! # Usage
//!
//! ```rust,ignore
//! use libbreenix::crypto::rand::Csprng;
//!
//! let mut rng = Csprng::new();
//! let value = rng.random_u64();
//!
//! let mut buf = [0u8; 128];
//! rng.fill(&mut buf);
//! ```

use crate::syscall::{nr, raw};

/// Request random bytes from the kernel via the `getrandom` syscall.
///
/// # Arguments
/// * `buf` - Buffer to fill with random bytes
///
/// # Returns
/// `Ok(n)` where `n` is the number of bytes filled, or `Err(errno)` on failure.
pub fn getrandom_bytes(buf: &mut [u8]) -> Result<usize, i64> {
    let ret = unsafe {
        raw::syscall3(
            nr::GETRANDOM,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
            0, // flags
        )
    };
    let ret = ret as i64;
    if ret < 0 {
        Err(ret)
    } else {
        Ok(ret as usize)
    }
}

/// Perform a ChaCha20 quarter round on four elements of the state array.
///
/// This is the fundamental operation of ChaCha20: add-rotate-xor applied
/// in a specific pattern to mix the state. Takes the state array and four
/// distinct indices to avoid multiple mutable borrow issues.
#[inline]
fn quarter_round(state: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    state[a] = state[a].wrapping_add(state[b]);
    state[d] ^= state[a];
    state[d] = state[d].rotate_left(16);

    state[c] = state[c].wrapping_add(state[d]);
    state[b] ^= state[c];
    state[b] = state[b].rotate_left(12);

    state[a] = state[a].wrapping_add(state[b]);
    state[d] ^= state[a];
    state[d] = state[d].rotate_left(8);

    state[c] = state[c].wrapping_add(state[d]);
    state[b] ^= state[c];
    state[b] = state[b].rotate_left(7);
}

/// Generate a single 64-byte ChaCha20 block.
///
/// # Arguments
/// * `key` - 256-bit key as 8 little-endian u32 words
/// * `counter` - 32-bit block counter
/// * `nonce` - 96-bit nonce as 3 little-endian u32 words
///
/// # Returns
/// 64 bytes of ChaCha20 keystream output.
fn chacha20_block(key: &[u32; 8], counter: u32, nonce: &[u32; 3]) -> [u8; 64] {
    // "expand 32-byte k" as four little-endian u32 constants
    const CONSTANTS: [u32; 4] = [0x6170_7865, 0x3320_646e, 0x7962_2d32, 0x6b20_6574];

    // Initialize 16-word state
    let mut state: [u32; 16] = [
        CONSTANTS[0],
        CONSTANTS[1],
        CONSTANTS[2],
        CONSTANTS[3],
        key[0],
        key[1],
        key[2],
        key[3],
        key[4],
        key[5],
        key[6],
        key[7],
        counter,
        nonce[0],
        nonce[1],
        nonce[2],
    ];

    // Save initial state for final addition
    let initial = state;

    // 20 rounds = 10 double rounds (column rounds + diagonal rounds)
    for _ in 0..10 {
        // Column rounds
        quarter_round(&mut state, 0, 4, 8, 12);
        quarter_round(&mut state, 1, 5, 9, 13);
        quarter_round(&mut state, 2, 6, 10, 14);
        quarter_round(&mut state, 3, 7, 11, 15);

        // Diagonal rounds
        quarter_round(&mut state, 0, 5, 10, 15);
        quarter_round(&mut state, 1, 6, 11, 12);
        quarter_round(&mut state, 2, 7, 8, 13);
        quarter_round(&mut state, 3, 4, 9, 14);
    }

    // Add initial state to final state
    for i in 0..16 {
        state[i] = state[i].wrapping_add(initial[i]);
    }

    // Serialize as little-endian bytes
    let mut output = [0u8; 64];
    for (i, word) in state.iter().enumerate() {
        let bytes = word.to_le_bytes();
        output[i * 4] = bytes[0];
        output[i * 4 + 1] = bytes[1];
        output[i * 4 + 2] = bytes[2];
        output[i * 4 + 3] = bytes[3];
    }

    output
}

/// A cryptographically secure pseudorandom number generator based on ChaCha20.
///
/// Seeded from the kernel's entropy source via the `getrandom` syscall. Each
/// instance maintains an independent ChaCha20 stream, generating 64-byte blocks
/// on demand and buffering unused bytes for subsequent requests.
pub struct Csprng {
    key: [u32; 8],
    counter: u32,
    nonce: [u32; 3],
    buffer: [u8; 64],
    buf_pos: usize,
}

impl Csprng {
    /// Create a new CSPRNG instance, seeded from the kernel's entropy source.
    ///
    /// Requests 32 bytes for the key and 12 bytes for the nonce via `getrandom`,
    /// then generates the first block of output.
    ///
    /// # Panics
    /// Panics if the `getrandom` syscall fails or returns fewer bytes than requested.
    pub fn new() -> Self {
        // Seed the key (32 bytes = 8 x u32)
        let mut key_bytes = [0u8; 32];
        let n = getrandom_bytes(&mut key_bytes).expect("getrandom failed for key");
        assert_eq!(n, 32, "getrandom returned fewer than 32 bytes for key");

        let mut key = [0u32; 8];
        for i in 0..8 {
            key[i] = u32::from_le_bytes([
                key_bytes[i * 4],
                key_bytes[i * 4 + 1],
                key_bytes[i * 4 + 2],
                key_bytes[i * 4 + 3],
            ]);
        }

        // Seed the nonce (12 bytes = 3 x u32)
        let mut nonce_bytes = [0u8; 12];
        let n = getrandom_bytes(&mut nonce_bytes).expect("getrandom failed for nonce");
        assert_eq!(n, 12, "getrandom returned fewer than 12 bytes for nonce");

        let mut nonce = [0u32; 3];
        for i in 0..3 {
            nonce[i] = u32::from_le_bytes([
                nonce_bytes[i * 4],
                nonce_bytes[i * 4 + 1],
                nonce_bytes[i * 4 + 2],
                nonce_bytes[i * 4 + 3],
            ]);
        }

        let counter = 0;
        let buffer = chacha20_block(&key, counter, &nonce);

        Csprng {
            key,
            counter,
            nonce,
            buffer,
            buf_pos: 0,
        }
    }

    /// Generate the next block of ChaCha20 output.
    fn next_block(&mut self) {
        self.counter = self.counter.wrapping_add(1);
        self.buffer = chacha20_block(&self.key, self.counter, &self.nonce);
        self.buf_pos = 0;
    }

    /// Fill a buffer with cryptographically secure random bytes.
    ///
    /// Generates new ChaCha20 blocks as needed to fill the entire output buffer.
    ///
    /// # Arguments
    /// * `buf` - The buffer to fill with random bytes
    pub fn fill(&mut self, buf: &mut [u8]) {
        let mut offset = 0;
        while offset < buf.len() {
            if self.buf_pos >= 64 {
                self.next_block();
            }

            let available = 64 - self.buf_pos;
            let needed = buf.len() - offset;
            let to_copy = if available < needed { available } else { needed };

            buf[offset..offset + to_copy]
                .copy_from_slice(&self.buffer[self.buf_pos..self.buf_pos + to_copy]);
            self.buf_pos += to_copy;
            offset += to_copy;
        }
    }

    /// Generate a random 32-bit unsigned integer.
    pub fn random_u32(&mut self) -> u32 {
        let mut bytes = [0u8; 4];
        self.fill(&mut bytes);
        u32::from_le_bytes(bytes)
    }

    /// Generate a random 64-bit unsigned integer.
    pub fn random_u64(&mut self) -> u64 {
        let mut bytes = [0u8; 8];
        self.fill(&mut bytes);
        u64::from_le_bytes(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify the ChaCha20 block function against the RFC 7539 Section 2.3.2 test vector.
    #[test]
    fn test_chacha20_rfc7539_test_vector() {
        // Key: 00:01:02:...:1f
        let key: [u32; 8] = [
            0x0302_0100,
            0x0706_0504,
            0x0b0a_0908,
            0x0f0e_0d0c,
            0x1312_1110,
            0x1716_1514,
            0x1b1a_1918,
            0x1f1e_1d1c,
        ];

        // Nonce: 00:00:00:09:00:00:00:4a:00:00:00:00
        let nonce: [u32; 3] = [0x0900_0000, 0x4a00_0000, 0x0000_0000];

        // Counter: 1
        let output = chacha20_block(&key, 1, &nonce);

        // RFC 7539 Section 2.3.2 expected output (first 16 bytes):
        // 10 f1 e7 e4 d1 3b 59 15 50 0f dd 1f a3 20 71 c4
        assert_eq!(output[0], 0x10);
        assert_eq!(output[1], 0xf1);
        assert_eq!(output[2], 0xe7);
        assert_eq!(output[3], 0xe4);
        assert_eq!(output[4], 0xd1);
        assert_eq!(output[5], 0x3b);
        assert_eq!(output[6], 0x59);
        assert_eq!(output[7], 0x15);
        assert_eq!(output[8], 0x50);
        assert_eq!(output[9], 0x0f);
        assert_eq!(output[10], 0xdd);
        assert_eq!(output[11], 0x1f);
        assert_eq!(output[12], 0xa3);
        assert_eq!(output[13], 0x20);
        assert_eq!(output[14], 0x71);
        assert_eq!(output[15], 0xc4);
    }

    /// Verify the quarter round operation with RFC 7539 Section 2.1.1 test vector.
    #[test]
    fn test_quarter_round() {
        let mut state: [u32; 16] = [0; 16];
        state[0] = 0x1111_1111;
        state[1] = 0x0102_0304;
        state[2] = 0x9b8d_6f43;
        state[3] = 0x0123_4567;

        quarter_round(&mut state, 0, 1, 2, 3);

        assert_eq!(state[0], 0xea2a_92f4);
        assert_eq!(state[1], 0xcb1c_f8ce);
        assert_eq!(state[2], 0x4581_472e);
        assert_eq!(state[3], 0x5881_c4bb);
    }

    /// Verify that the ChaCha20 block function produces exactly 64 bytes of output.
    #[test]
    fn test_chacha20_block_output_length() {
        let key = [0u32; 8];
        let nonce = [0u32; 3];
        let output = chacha20_block(&key, 0, &nonce);
        assert_eq!(output.len(), 64);
    }

    /// Verify that different counter values produce different output.
    #[test]
    fn test_chacha20_different_counters() {
        let key = [0u32; 8];
        let nonce = [0u32; 3];
        let block0 = chacha20_block(&key, 0, &nonce);
        let block1 = chacha20_block(&key, 1, &nonce);
        assert_ne!(block0, block1);
    }

    /// Verify that fill() produces non-zero bytes for a reasonably sized buffer.
    /// Note: This test requires a running kernel with getrandom support.
    #[test]
    #[ignore]
    fn test_csprng_fill_nonzero() {
        let mut rng = Csprng::new();
        let mut buf = [0u8; 64];
        rng.fill(&mut buf);

        // It is statistically impossible for 64 random bytes to all be zero.
        let all_zero = buf.iter().all(|&b| b == 0);
        assert!(!all_zero, "fill() produced all-zero output");
    }

    /// Verify that two independent CSPRNG instances produce different output.
    /// Note: This test requires a running kernel with getrandom support.
    #[test]
    #[ignore]
    fn test_csprng_different_instances() {
        let mut rng1 = Csprng::new();
        let mut rng2 = Csprng::new();

        let v1 = rng1.random_u64();
        let v2 = rng2.random_u64();

        assert_ne!(v1, v2, "two CSPRNG instances produced identical output");
    }

    /// Verify that fill() works across block boundaries.
    #[test]
    #[ignore]
    fn test_csprng_fill_across_blocks() {
        let mut rng = Csprng::new();

        // Request more than one 64-byte block
        let mut buf = [0u8; 200];
        rng.fill(&mut buf);

        let all_zero = buf.iter().all(|&b| b == 0);
        assert!(!all_zero, "fill() produced all-zero output for 200 bytes");
    }
}
