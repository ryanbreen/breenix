//! Keccak-f[1600] permutation and SHA-3 / SHAKE hash functions (FIPS 202).
//!
//! Pure Rust implementation of the Keccak sponge construction with:
//! - SHA3-256 (256-bit hash)
//! - SHA3-512 (512-bit hash)
//! - SHAKE-128 (extensible-output function)
//! - SHAKE-256 (extensible-output function)
//!
//! Used internally by ML-KEM (FIPS 203) for matrix sampling, noise generation,
//! and key derivation.

// ---------------------------------------------------------------------------
// Keccak-f[1600] round constants
// ---------------------------------------------------------------------------

/// Round constants for Keccak-f[1600] (24 rounds).
const RC: [u64; 24] = [
    0x0000000000000001,
    0x0000000000008082,
    0x800000000000808A,
    0x8000000080008000,
    0x000000000000808B,
    0x0000000080000001,
    0x8000000080008081,
    0x8000000000008009,
    0x000000000000008A,
    0x0000000000000088,
    0x0000000080008009,
    0x000000008000000A,
    0x000000008000808B,
    0x800000000000008B,
    0x8000000000008089,
    0x8000000000008003,
    0x8000000000008002,
    0x8000000000000080,
    0x000000000000800A,
    0x800000008000000A,
    0x8000000080008081,
    0x8000000000008080,
    0x0000000080000001,
    0x8000000080008008,
];

/// Rotation offsets for the rho step.
const RHO_OFFSETS: [u32; 24] = [
    1, 3, 6, 10, 15, 21, 28, 36, 45, 55, 2, 14, 27, 41, 56, 8, 25, 43, 62, 18, 39, 61, 20, 44,
];

/// Lane permutation indices for the pi step.
/// pi_lane[t] gives the source index for destination (t+1) in the pi mapping.
const PI_LANE: [usize; 24] = [
    10, 7, 11, 17, 18, 3, 5, 16, 8, 21, 24, 4, 15, 23, 19, 13, 12, 2, 20, 14, 22, 9, 6, 1,
];

// ---------------------------------------------------------------------------
// Keccak-f[1600] permutation
// ---------------------------------------------------------------------------

/// The Keccak-f[1600] permutation operating on a 5x5 array of 64-bit lanes.
///
/// This is the core primitive underlying all SHA-3 and SHAKE functions.
/// Applies 24 rounds of theta, rho, pi, chi, and iota steps.
fn keccak_f1600(state: &mut [u64; 25]) {
    for round in 0..24 {
        // Theta step: compute column parities and XOR with neighbors
        let mut c = [0u64; 5];
        for x in 0..5 {
            c[x] = state[x] ^ state[x + 5] ^ state[x + 10] ^ state[x + 15] ^ state[x + 20];
        }
        let mut d = [0u64; 5];
        for x in 0..5 {
            d[x] = c[(x + 4) % 5] ^ c[(x + 1) % 5].rotate_left(1);
        }
        for x in 0..5 {
            for y in 0..5 {
                state[x + 5 * y] ^= d[x];
            }
        }

        // Rho and Pi steps combined
        let mut temp = [0u64; 25];
        temp[0] = state[0];
        let mut current = state[1];
        for t in 0..24 {
            let dest = PI_LANE[t];
            temp[dest] = current.rotate_left(RHO_OFFSETS[t]);
            current = state[dest];
        }

        // Chi step: non-linear mixing within each row
        for y in 0..5 {
            let base = 5 * y;
            let t0 = temp[base];
            let t1 = temp[base + 1];
            let t2 = temp[base + 2];
            let t3 = temp[base + 3];
            let t4 = temp[base + 4];
            state[base] = t0 ^ (!t1 & t2);
            state[base + 1] = t1 ^ (!t2 & t3);
            state[base + 2] = t2 ^ (!t3 & t4);
            state[base + 3] = t3 ^ (!t4 & t0);
            state[base + 4] = t4 ^ (!t0 & t1);
        }

        // Iota step: break symmetry with round constant
        state[0] ^= RC[round];
    }
}

// ---------------------------------------------------------------------------
// Keccak sponge construction
// ---------------------------------------------------------------------------

/// Keccak sponge state.
///
/// Accumulates input data and produces output using the Keccak-f[1600]
/// permutation. The rate (in bytes) determines how much data is absorbed
/// per permutation call.
struct KeccakState {
    state: [u64; 25],
    /// Current position within the rate portion of the state.
    offset: usize,
    /// Rate in bytes (r = 1600/8 - capacity/8).
    rate: usize,
}

impl KeccakState {
    /// Create a new Keccak sponge with the given rate (in bytes).
    fn new(rate: usize) -> Self {
        Self {
            state: [0u64; 25],
            offset: 0,
            rate,
        }
    }

    /// Absorb input data into the sponge.
    fn absorb(&mut self, data: &[u8]) {
        let mut pos = 0;
        while pos < data.len() {
            let remaining_rate = self.rate - self.offset;
            let to_absorb = core::cmp::min(remaining_rate, data.len() - pos);

            // XOR data into state bytes at current offset
            for i in 0..to_absorb {
                let byte_pos = self.offset + i;
                let lane = byte_pos / 8;
                let byte_in_lane = byte_pos % 8;
                self.state[lane] ^= (data[pos + i] as u64) << (8 * byte_in_lane);
            }

            self.offset += to_absorb;
            pos += to_absorb;

            if self.offset == self.rate {
                keccak_f1600(&mut self.state);
                self.offset = 0;
            }
        }
    }

    /// Finalize the sponge with the given domain separation suffix and
    /// squeeze `output_len` bytes of output.
    ///
    /// `suffix` is the domain separation byte:
    /// - 0x06 for SHA-3
    /// - 0x1F for SHAKE
    fn finalize(&mut self, suffix: u8, output: &mut [u8]) {
        // Pad: append suffix byte, then pad10*1
        let byte_pos = self.offset;
        let lane = byte_pos / 8;
        let byte_in_lane = byte_pos % 8;
        self.state[lane] ^= (suffix as u64) << (8 * byte_in_lane);

        // Set the last bit of the rate block
        let last_byte = self.rate - 1;
        let last_lane = last_byte / 8;
        let last_byte_in_lane = last_byte % 8;
        self.state[last_lane] ^= 0x80u64 << (8 * last_byte_in_lane);

        keccak_f1600(&mut self.state);

        // Squeeze phase
        let mut squeezed = 0;
        while squeezed < output.len() {
            let block_bytes = core::cmp::min(self.rate, output.len() - squeezed);
            for i in 0..block_bytes {
                let lane = i / 8;
                let byte_in_lane = i % 8;
                output[squeezed + i] = (self.state[lane] >> (8 * byte_in_lane)) as u8;
            }
            squeezed += block_bytes;
            if squeezed < output.len() {
                keccak_f1600(&mut self.state);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// SHA3-256
// ---------------------------------------------------------------------------

/// Compute the SHA3-256 digest of `data` (FIPS 202).
///
/// Returns a 32-byte hash. Uses capacity = 512 bits, rate = 1088 bits (136 bytes).
pub fn sha3_256(data: &[u8]) -> [u8; 32] {
    let mut sponge = KeccakState::new(136); // rate = (1600 - 512) / 8 = 136
    sponge.absorb(data);
    let mut output = [0u8; 32];
    sponge.finalize(0x06, &mut output);
    output
}

// ---------------------------------------------------------------------------
// SHA3-512
// ---------------------------------------------------------------------------

/// Compute the SHA3-512 digest of `data` (FIPS 202).
///
/// Returns a 64-byte hash. Uses capacity = 1024 bits, rate = 576 bits (72 bytes).
pub fn sha3_512(data: &[u8]) -> [u8; 64] {
    let mut sponge = KeccakState::new(72); // rate = (1600 - 1024) / 8 = 72
    sponge.absorb(data);
    let mut output = [0u8; 64];
    sponge.finalize(0x06, &mut output);
    output
}

// ---------------------------------------------------------------------------
// SHAKE-128
// ---------------------------------------------------------------------------

/// SHAKE-128 extensible-output function (FIPS 202).
///
/// Incremental interface: absorb data, then squeeze arbitrary-length output.
/// Uses capacity = 256 bits, rate = 1344 bits (168 bytes).
pub struct Shake128 {
    sponge: KeccakState,
    finalized: bool,
    squeeze_offset: usize,
}

impl Shake128 {
    /// Create a new SHAKE-128 instance.
    pub fn new() -> Self {
        Self {
            sponge: KeccakState::new(168), // rate = (1600 - 256) / 8 = 168
            finalized: false,
            squeeze_offset: 0,
        }
    }

    /// Absorb input data. Must be called before any squeeze operation.
    pub fn absorb(&mut self, data: &[u8]) {
        debug_assert!(!self.finalized, "cannot absorb after squeezing");
        self.sponge.absorb(data);
    }

    /// Squeeze `output.len()` bytes of output.
    ///
    /// On the first call, finalizes the sponge with SHAKE domain separation.
    /// Subsequent calls continue squeezing from where the previous call left off.
    pub fn squeeze(&mut self, output: &mut [u8]) {
        if !self.finalized {
            // Finalize the absorb phase with SHAKE suffix 0x1F
            let byte_pos = self.sponge.offset;
            let lane = byte_pos / 8;
            let byte_in_lane = byte_pos % 8;
            self.sponge.state[lane] ^= 0x1Fu64 << (8 * byte_in_lane);

            let last_byte = self.sponge.rate - 1;
            let last_lane = last_byte / 8;
            let last_byte_in_lane = last_byte % 8;
            self.sponge.state[last_lane] ^= 0x80u64 << (8 * last_byte_in_lane);

            keccak_f1600(&mut self.sponge.state);
            self.finalized = true;
            self.squeeze_offset = 0;
        }

        let rate = self.sponge.rate;
        let mut pos = 0;
        while pos < output.len() {
            if self.squeeze_offset == rate {
                keccak_f1600(&mut self.sponge.state);
                self.squeeze_offset = 0;
            }
            let available = rate - self.squeeze_offset;
            let to_squeeze = core::cmp::min(available, output.len() - pos);
            for i in 0..to_squeeze {
                let byte_idx = self.squeeze_offset + i;
                let lane_idx = byte_idx / 8;
                let byte_in_lane = byte_idx % 8;
                output[pos + i] = (self.sponge.state[lane_idx] >> (8 * byte_in_lane)) as u8;
            }
            self.squeeze_offset += to_squeeze;
            pos += to_squeeze;
        }
    }
}

// ---------------------------------------------------------------------------
// SHAKE-256
// ---------------------------------------------------------------------------

/// SHAKE-256 extensible-output function (FIPS 202).
///
/// Incremental interface: absorb data, then squeeze arbitrary-length output.
/// Uses capacity = 512 bits, rate = 1088 bits (136 bytes).
pub struct Shake256 {
    sponge: KeccakState,
    finalized: bool,
    squeeze_offset: usize,
}

impl Shake256 {
    /// Create a new SHAKE-256 instance.
    pub fn new() -> Self {
        Self {
            sponge: KeccakState::new(136), // rate = (1600 - 512) / 8 = 136
            finalized: false,
            squeeze_offset: 0,
        }
    }

    /// Absorb input data. Must be called before any squeeze operation.
    pub fn absorb(&mut self, data: &[u8]) {
        debug_assert!(!self.finalized, "cannot absorb after squeezing");
        self.sponge.absorb(data);
    }

    /// Squeeze `output.len()` bytes of output.
    ///
    /// On the first call, finalizes the sponge with SHAKE domain separation.
    /// Subsequent calls continue squeezing from where the previous call left off.
    pub fn squeeze(&mut self, output: &mut [u8]) {
        if !self.finalized {
            let byte_pos = self.sponge.offset;
            let lane = byte_pos / 8;
            let byte_in_lane = byte_pos % 8;
            self.sponge.state[lane] ^= 0x1Fu64 << (8 * byte_in_lane);

            let last_byte = self.sponge.rate - 1;
            let last_lane = last_byte / 8;
            let last_byte_in_lane = last_byte % 8;
            self.sponge.state[last_lane] ^= 0x80u64 << (8 * last_byte_in_lane);

            keccak_f1600(&mut self.sponge.state);
            self.finalized = true;
            self.squeeze_offset = 0;
        }

        let rate = self.sponge.rate;
        let mut pos = 0;
        while pos < output.len() {
            if self.squeeze_offset == rate {
                keccak_f1600(&mut self.sponge.state);
                self.squeeze_offset = 0;
            }
            let available = rate - self.squeeze_offset;
            let to_squeeze = core::cmp::min(available, output.len() - pos);
            for i in 0..to_squeeze {
                let byte_idx = self.squeeze_offset + i;
                let lane_idx = byte_idx / 8;
                let byte_in_lane = byte_idx % 8;
                output[pos + i] = (self.sponge.state[lane_idx] >> (8 * byte_in_lane)) as u8;
            }
            self.squeeze_offset += to_squeeze;
            pos += to_squeeze;
        }
    }
}

// ---------------------------------------------------------------------------
// Convenience functions
// ---------------------------------------------------------------------------

/// Compute SHAKE-128 output of the given length from input data.
pub fn shake128(data: &[u8], output: &mut [u8]) {
    let mut xof = Shake128::new();
    xof.absorb(data);
    xof.squeeze(output);
}

/// Compute SHAKE-256 output of the given length from input data.
pub fn shake256(data: &[u8], output: &mut [u8]) {
    let mut xof = Shake256::new();
    xof.absorb(data);
    xof.squeeze(output);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Convert a hex string to a byte vector.
    fn hex_to_bytes(hex: &str) -> Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect()
    }

    /// Convert bytes to a hex string.
    fn bytes_to_hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{:02x}", b)).collect()
    }

    #[test]
    fn test_sha3_256_empty() {
        // NIST CAVP: SHA3-256 of empty string
        let expected = "a7ffc6f8bf1ed76651c14756a061d662f580ff4de43b49fa82d80a4b80f8434a";
        let digest = sha3_256(b"");
        assert_eq!(bytes_to_hex(&digest), expected);
    }

    #[test]
    fn test_sha3_256_abc() {
        // NIST CAVP: SHA3-256("abc")
        let expected = "3a985da74fe225b2045c172d6bd390bd855f086e3e9d525b46bfe24511431532";
        let digest = sha3_256(b"abc");
        assert_eq!(bytes_to_hex(&digest), expected);
    }

    #[test]
    fn test_sha3_512_empty() {
        // NIST CAVP: SHA3-512 of empty string
        let expected = "a69f73cca23a9ac5c8b567dc185a756e97c982164fe25859e0d1dcc1475c80a6\
                        15b2123af1f5f94c11e3e9402c3ac558f500199d95b6d3e301758586281dcd26";
        let digest = sha3_512(b"");
        assert_eq!(bytes_to_hex(&digest), expected);
    }

    #[test]
    fn test_sha3_512_abc() {
        // NIST CAVP: SHA3-512("abc")
        let expected = "b751850b1a57168a5693cd924b6b096e08f621827444f70d884f5d0240d2712e\
                        10e116e9192af3c91a7ec57647e3934057340b4cf408d5a56592f8274eec53f0";
        let digest = sha3_512(b"abc");
        assert_eq!(bytes_to_hex(&digest), expected);
    }

    #[test]
    fn test_shake128_empty() {
        // NIST CAVP: SHAKE128("", 256 bits output)
        let expected = "7f9c2ba4e88f827d616045507605853ed73b8093f6efbc88eb1a6eacfa66ef26";
        let mut output = [0u8; 32];
        shake128(b"", &mut output);
        assert_eq!(bytes_to_hex(&output), expected);
    }

    #[test]
    fn test_shake256_empty() {
        // NIST CAVP: SHAKE256("", 256 bits output)
        let expected = "46b9dd2b0ba88d13233b3feb743eeb243fcd52ea62b81b82b50c27646ed5762f";
        let mut output = [0u8; 32];
        shake256(b"", &mut output);
        assert_eq!(bytes_to_hex(&output), expected);
    }

    #[test]
    fn test_shake128_incremental() {
        // Verify that incremental absorption matches one-shot
        let data = b"The quick brown fox jumps over the lazy dog";
        let mut oneshot = [0u8; 64];
        shake128(data, &mut oneshot);

        let mut xof = Shake128::new();
        xof.absorb(&data[..10]);
        xof.absorb(&data[10..]);
        let mut incremental = [0u8; 64];
        xof.squeeze(&mut incremental);

        assert_eq!(oneshot, incremental);
    }

    #[test]
    fn test_shake128_long_squeeze() {
        // Squeeze more than one rate block (168 bytes) to test multi-block squeeze
        let mut output = [0u8; 512];
        shake128(b"test", &mut output);

        // Verify by squeezing in two separate calls
        let mut xof = Shake128::new();
        xof.absorb(b"test");
        let mut part1 = [0u8; 200];
        let mut part2 = [0u8; 312];
        xof.squeeze(&mut part1);
        xof.squeeze(&mut part2);

        assert_eq!(&output[..200], &part1[..]);
        assert_eq!(&output[200..], &part2[..]);
    }

    #[test]
    fn test_sha3_256_longer_message() {
        // SHA3-256 of "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
        let expected = "41c0dba2a9d6240849100376a8235e2c82e1b9998a999e21db32dd97496d3376";
        let digest = sha3_256(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq");
        assert_eq!(bytes_to_hex(&digest), expected);
    }

    #[test]
    fn test_shake256_abc() {
        // SHAKE256("abc", 512 bits)
        let expected_hex = hex_to_bytes(
            "483366601360a8771c6863080cc4114d8d\
             b44530f8f1e1ee4f94ea37e78b5739d5a1\
             5bef186a5386c75744c0527e1faa9f8726\
             e462a12a4feb06bd8801e751e4",
        );
        let mut output = [0u8; 64];
        shake256(b"abc", &mut output);
        assert_eq!(&output[..expected_hex.len()], &expected_hex[..]);
    }
}
