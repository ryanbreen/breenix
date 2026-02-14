//! Pure Rust SHA-256 implementation (FIPS 180-4).
//!
//! Provides both incremental (`Sha256::new()` / `.update()` / `.finalize()`)
//! and one-shot (`sha256()`) interfaces. Uses only core operations with no
//! external crate dependencies.

/// SHA-256 round constants (first 32 bits of the fractional parts of the
/// cube roots of the first 64 primes).
const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
    0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
    0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
    0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
    0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
    0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
    0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
    0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
    0xc67178f2,
];

/// Initial hash values (first 32 bits of the fractional parts of the
/// square roots of the first 8 primes).
const H_INIT: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
    0x5be0cd19,
];

// --- Helper functions (FIPS 180-4 section 4.1.2) ---

#[inline(always)]
fn ch(x: u32, y: u32, z: u32) -> u32 {
    (x & y) ^ (!x & z)
}

#[inline(always)]
fn maj(x: u32, y: u32, z: u32) -> u32 {
    (x & y) ^ (x & z) ^ (y & z)
}

#[inline(always)]
fn big_sigma0(x: u32) -> u32 {
    x.rotate_right(2) ^ x.rotate_right(13) ^ x.rotate_right(22)
}

#[inline(always)]
fn big_sigma1(x: u32) -> u32 {
    x.rotate_right(6) ^ x.rotate_right(11) ^ x.rotate_right(25)
}

#[inline(always)]
fn small_sigma0(x: u32) -> u32 {
    x.rotate_right(7) ^ x.rotate_right(18) ^ (x >> 3)
}

#[inline(always)]
fn small_sigma1(x: u32) -> u32 {
    x.rotate_right(17) ^ x.rotate_right(19) ^ (x >> 10)
}

/// Process a single 512-bit (64-byte) block, updating the hash state in place.
fn compress(state: &mut [u32; 8], block: &[u8; 64]) {
    // Prepare the message schedule W[0..64].
    let mut w = [0u32; 64];

    // W[0..16]: big-endian decode from the block.
    for t in 0..16 {
        w[t] = u32::from_be_bytes([
            block[4 * t],
            block[4 * t + 1],
            block[4 * t + 2],
            block[4 * t + 3],
        ]);
    }

    // W[16..64]: expansion.
    for t in 16..64 {
        w[t] = small_sigma1(w[t - 2])
            .wrapping_add(w[t - 7])
            .wrapping_add(small_sigma0(w[t - 15]))
            .wrapping_add(w[t - 16]);
    }

    // Initialize working variables.
    let mut a = state[0];
    let mut b = state[1];
    let mut c = state[2];
    let mut d = state[3];
    let mut e = state[4];
    let mut f = state[5];
    let mut g = state[6];
    let mut h = state[7];

    // 64 rounds.
    for t in 0..64 {
        let t1 = h
            .wrapping_add(big_sigma1(e))
            .wrapping_add(ch(e, f, g))
            .wrapping_add(K[t])
            .wrapping_add(w[t]);
        let t2 = big_sigma0(a).wrapping_add(maj(a, b, c));

        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }

    // Add the compressed chunk to the current hash value.
    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
    state[4] = state[4].wrapping_add(e);
    state[5] = state[5].wrapping_add(f);
    state[6] = state[6].wrapping_add(g);
    state[7] = state[7].wrapping_add(h);
}

/// Incremental SHA-256 hasher.
///
/// # Example
///
/// ```
/// use libbreenix::crypto::sha256::Sha256;
///
/// let mut hasher = Sha256::new();
/// hasher.update(b"hello ");
/// hasher.update(b"world");
/// let digest = hasher.finalize();
/// ```
pub struct Sha256 {
    state: [u32; 8],
    buffer: [u8; 64],
    buffer_len: usize,
    total_len: u64,
}

impl Sha256 {
    /// Create a new SHA-256 hasher initialized to the standard IV.
    pub fn new() -> Self {
        Self {
            state: H_INIT,
            buffer: [0u8; 64],
            buffer_len: 0,
            total_len: 0,
        }
    }

    /// Feed data into the hasher. May be called repeatedly with arbitrary
    /// chunk sizes.
    pub fn update(&mut self, data: &[u8]) {
        self.total_len += data.len() as u64;
        let mut offset = 0;

        // If there is leftover data in the buffer, try to fill it up first.
        if self.buffer_len > 0 {
            let space = 64 - self.buffer_len;
            let to_copy = if data.len() < space { data.len() } else { space };
            self.buffer[self.buffer_len..self.buffer_len + to_copy]
                .copy_from_slice(&data[..to_copy]);
            self.buffer_len += to_copy;
            offset += to_copy;

            if self.buffer_len == 64 {
                let block: [u8; 64] = self.buffer;
                compress(&mut self.state, &block);
                self.buffer_len = 0;
            }
        }

        // Process full 64-byte blocks directly from the input slice.
        while offset + 64 <= data.len() {
            let block: &[u8; 64] = data[offset..offset + 64].try_into().unwrap();
            compress(&mut self.state, block);
            offset += 64;
        }

        // Buffer any remaining bytes.
        let remaining = data.len() - offset;
        if remaining > 0 {
            self.buffer[..remaining].copy_from_slice(&data[offset..]);
            self.buffer_len = remaining;
        }
    }

    /// Finalize the hash and return the 32-byte digest.
    ///
    /// Consumes `self`; to hash more data, create a new `Sha256`.
    pub fn finalize(mut self) -> [u8; 32] {
        // Total message length in bits.
        let bit_len = self.total_len * 8;

        // Append the 0x80 byte.
        self.buffer[self.buffer_len] = 0x80;
        self.buffer_len += 1;

        // If there is not enough room for the 8-byte length field, pad and
        // compress the current block, then start a new one.
        if self.buffer_len > 56 {
            // Zero-fill the rest of this block.
            for b in &mut self.buffer[self.buffer_len..64] {
                *b = 0;
            }
            let block: [u8; 64] = self.buffer;
            compress(&mut self.state, &block);
            self.buffer_len = 0;
            self.buffer = [0u8; 64];
        }

        // Zero-fill up to the length field.
        for b in &mut self.buffer[self.buffer_len..56] {
            *b = 0;
        }

        // Append the 64-bit message length in bits (big-endian).
        self.buffer[56..64].copy_from_slice(&bit_len.to_be_bytes());

        compress(&mut self.state, &self.buffer);

        // Produce the final 32-byte digest from the state (big-endian).
        let mut digest = [0u8; 32];
        for (i, word) in self.state.iter().enumerate() {
            digest[4 * i..4 * i + 4].copy_from_slice(&word.to_be_bytes());
        }
        digest
    }
}

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute the SHA-256 digest of `data` in one shot.
///
/// # Example
///
/// ```ignore
/// use libbreenix::crypto::sha256::sha256;
///
/// let digest = sha256(b"abc");
/// // ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
/// ```
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Convert a hex string to a 32-byte array.
    fn hex_to_bytes(hex: &str) -> [u8; 32] {
        assert_eq!(hex.len(), 64);
        let mut out = [0u8; 32];
        for i in 0..32 {
            out[i] = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap();
        }
        out
    }

    #[test]
    fn test_empty_string() {
        let expected =
            hex_to_bytes("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(sha256(b""), expected);
    }

    #[test]
    fn test_abc() {
        let expected =
            hex_to_bytes("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(sha256(b"abc"), expected);
    }

    #[test]
    fn test_448_bit() {
        let expected =
            hex_to_bytes("248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1");
        assert_eq!(
            sha256(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            expected
        );
    }

    #[test]
    fn test_incremental_matches_oneshot() {
        // Feed "abc" one byte at a time via the incremental API.
        let mut hasher = Sha256::new();
        hasher.update(b"a");
        hasher.update(b"b");
        hasher.update(b"c");
        let incremental = hasher.finalize();

        assert_eq!(incremental, sha256(b"abc"));
    }

    #[test]
    fn test_exactly_one_block() {
        // 55 bytes of data + 1 byte 0x80 + 8 bytes length = exactly 64 bytes
        // (one block after padding). Verify incremental matches oneshot.
        let data = [0x61u8; 55]; // 55 'a' characters
        let oneshot = sha256(&data);
        let mut hasher = Sha256::new();
        hasher.update(&data[..30]);
        hasher.update(&data[30..]);
        assert_eq!(hasher.finalize(), oneshot);
    }

    #[test]
    fn test_two_block_boundary() {
        // 56 bytes of data forces the padding into a second block.
        let data = [0x62u8; 56];
        let oneshot = sha256(&data);
        let mut hasher = Sha256::new();
        hasher.update(&data[..10]);
        hasher.update(&data[10..40]);
        hasher.update(&data[40..]);
        assert_eq!(hasher.finalize(), oneshot);
    }

    #[test]
    fn test_large_input() {
        // 1000 bytes fed in various chunk sizes.
        let data: Vec<u8> = (0..1000).map(|i| (i % 256) as u8).collect();
        let oneshot = sha256(&data);

        let mut hasher = Sha256::new();
        for chunk in data.chunks(63) {
            hasher.update(chunk);
        }
        assert_eq!(hasher.finalize(), oneshot);
    }
}
