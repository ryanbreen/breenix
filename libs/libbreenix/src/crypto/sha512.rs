//! Pure Rust SHA-512 implementation (FIPS 180-4).
//!
//! Provides both incremental (`Sha512::new()` / `.update()` / `.finalize()`)
//! and one-shot (`sha512()`) interfaces. Uses only core operations with no
//! external crate dependencies.

/// SHA-512 round constants (first 64 bits of the fractional parts of the
/// cube roots of the first 80 primes).
const K: [u64; 80] = [
    0x428a2f98d728ae22, 0x7137449123ef65cd, 0xb5c0fbcfec4d3b2f, 0xe9b5dba58189dbbc,
    0x3956c25bf348b538, 0x59f111f1b605d019, 0x923f82a4af194f9b, 0xab1c5ed5da6d8118,
    0xd807aa98a3030242, 0x12835b0145706fbe, 0x243185be4ee4b28c, 0x550c7dc3d5ffb4e2,
    0x72be5d74f27b896f, 0x80deb1fe3b1696b1, 0x9bdc06a725c71235, 0xc19bf174cf692694,
    0xe49b69c19ef14ad2, 0xefbe4786384f25e3, 0x0fc19dc68b8cd5b5, 0x240ca1cc77ac9c65,
    0x2de92c6f592b0275, 0x4a7484aa6ea6e483, 0x5cb0a9dcbd41fbd4, 0x76f988da831153b5,
    0x983e5152ee66dfab, 0xa831c66d2db43210, 0xb00327c898fb213f, 0xbf597fc7beef0ee4,
    0xc6e00bf33da88fc2, 0xd5a79147930aa725, 0x06ca6351e003826f, 0x142929670a0e6e70,
    0x27b70a8546d22ffc, 0x2e1b21385c26c926, 0x4d2c6dfc5ac42aed, 0x53380d139d95b3df,
    0x650a73548baf63de, 0x766a0abb3c77b2a8, 0x81c2c92e47edaee6, 0x92722c851482353b,
    0xa2bfe8a14cf10364, 0xa81a664bbc423001, 0xc24b8b70d0f89791, 0xc76c51a30654be30,
    0xd192e819d6ef5218, 0xd69906245565a910, 0xf40e35855771202a, 0x106aa07032bbd1b8,
    0x19a4c116b8d2d0c8, 0x1e376c085141ab53, 0x2748774cdf8eeb99, 0x34b0bcb5e19b48a8,
    0x391c0cb3c5c95a63, 0x4ed8aa4ae3418acb, 0x5b9cca4f7763e373, 0x682e6ff3d6b2b8a3,
    0x748f82ee5defb2fc, 0x78a5636f43172f60, 0x84c87814a1f0ab72, 0x8cc702081a6439ec,
    0x90befffa23631e28, 0xa4506cebde82bde9, 0xbef9a3f7b2c67915, 0xc67178f2e372532b,
    0xca273eceea26619c, 0xd186b8c721c0c207, 0xeada7dd6cde0eb1e, 0xf57d4f7fee6ed178,
    0x06f067aa72176fba, 0x0a637dc5a2c898a6, 0x113f9804bef90dae, 0x1b710b35131c471b,
    0x28db77f523047d84, 0x32caab7b40c72493, 0x3c9ebe0a15c9bebc, 0x431d67c49c100d4c,
    0x4cc5d4becb3e42b6, 0x597f299cfc657e2a, 0x5fcb6fab3ad6faec, 0x6c44198c4a475817,
];

/// Initial hash values (first 64 bits of the fractional parts of the
/// square roots of the first 8 primes).
const H_INIT: [u64; 8] = [
    0x6a09e667f3bcc908, 0xbb67ae8584caa73b, 0x3c6ef372fe94f82b, 0xa54ff53a5f1d36f1,
    0x510e527fade682d1, 0x9b05688c2b3e6c1f, 0x1f83d9abfb41bd6b, 0x5be0cd19137e2179,
];

// --- Helper functions (FIPS 180-4 section 4.1.3) ---

#[inline(always)]
fn ch(x: u64, y: u64, z: u64) -> u64 {
    (x & y) ^ (!x & z)
}

#[inline(always)]
fn maj(x: u64, y: u64, z: u64) -> u64 {
    (x & y) ^ (x & z) ^ (y & z)
}

#[inline(always)]
fn big_sigma0(x: u64) -> u64 {
    x.rotate_right(28) ^ x.rotate_right(34) ^ x.rotate_right(39)
}

#[inline(always)]
fn big_sigma1(x: u64) -> u64 {
    x.rotate_right(14) ^ x.rotate_right(18) ^ x.rotate_right(41)
}

#[inline(always)]
fn small_sigma0(x: u64) -> u64 {
    x.rotate_right(1) ^ x.rotate_right(8) ^ (x >> 7)
}

#[inline(always)]
fn small_sigma1(x: u64) -> u64 {
    x.rotate_right(19) ^ x.rotate_right(61) ^ (x >> 6)
}

/// Process a single 1024-bit (128-byte) block, updating the hash state in place.
fn compress(state: &mut [u64; 8], block: &[u8; 128]) {
    // Prepare the message schedule W[0..80].
    let mut w = [0u64; 80];

    // W[0..16]: big-endian decode from the block.
    for t in 0..16 {
        w[t] = u64::from_be_bytes([
            block[8 * t],
            block[8 * t + 1],
            block[8 * t + 2],
            block[8 * t + 3],
            block[8 * t + 4],
            block[8 * t + 5],
            block[8 * t + 6],
            block[8 * t + 7],
        ]);
    }

    // W[16..80]: expansion.
    for t in 16..80 {
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

    // 80 rounds.
    for t in 0..80 {
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

/// Incremental SHA-512 hasher.
///
/// # Example
///
/// ```
/// use libbreenix::crypto::sha512::Sha512;
///
/// let mut hasher = Sha512::new();
/// hasher.update(b"hello ");
/// hasher.update(b"world");
/// let digest = hasher.finalize();
/// ```
pub struct Sha512 {
    state: [u64; 8],
    buffer: [u8; 128],
    buffer_len: usize,
    total_len: u128,
}

impl Sha512 {
    /// Create a new SHA-512 hasher initialized to the standard IV.
    pub fn new() -> Self {
        Self {
            state: H_INIT,
            buffer: [0u8; 128],
            buffer_len: 0,
            total_len: 0,
        }
    }

    /// Feed data into the hasher. May be called repeatedly with arbitrary
    /// chunk sizes.
    pub fn update(&mut self, data: &[u8]) {
        self.total_len += data.len() as u128;
        let mut offset = 0;

        // If there is leftover data in the buffer, try to fill it up first.
        if self.buffer_len > 0 {
            let space = 128 - self.buffer_len;
            let to_copy = if data.len() < space { data.len() } else { space };
            self.buffer[self.buffer_len..self.buffer_len + to_copy]
                .copy_from_slice(&data[..to_copy]);
            self.buffer_len += to_copy;
            offset += to_copy;

            if self.buffer_len == 128 {
                let block: [u8; 128] = self.buffer;
                compress(&mut self.state, &block);
                self.buffer_len = 0;
            }
        }

        // Process full 128-byte blocks directly from the input slice.
        while offset + 128 <= data.len() {
            let block: &[u8; 128] = data[offset..offset + 128].try_into().unwrap();
            compress(&mut self.state, block);
            offset += 128;
        }

        // Buffer any remaining bytes.
        let remaining = data.len() - offset;
        if remaining > 0 {
            self.buffer[..remaining].copy_from_slice(&data[offset..]);
            self.buffer_len = remaining;
        }
    }

    /// Finalize the hash and return the 64-byte digest.
    ///
    /// Consumes `self`; to hash more data, create a new `Sha512`.
    pub fn finalize(mut self) -> [u8; 64] {
        // Total message length in bits (128-bit value).
        let bit_len = self.total_len * 8;

        // Append the 0x80 byte.
        self.buffer[self.buffer_len] = 0x80;
        self.buffer_len += 1;

        // If there is not enough room for the 16-byte length field, pad and
        // compress the current block, then start a new one.
        if self.buffer_len > 112 {
            // Zero-fill the rest of this block.
            for b in &mut self.buffer[self.buffer_len..128] {
                *b = 0;
            }
            let block: [u8; 128] = self.buffer;
            compress(&mut self.state, &block);
            self.buffer_len = 0;
            self.buffer = [0u8; 128];
        }

        // Zero-fill up to the length field.
        for b in &mut self.buffer[self.buffer_len..112] {
            *b = 0;
        }

        // Append the 128-bit message length in bits (big-endian).
        self.buffer[112..128].copy_from_slice(&bit_len.to_be_bytes());

        compress(&mut self.state, &self.buffer);

        // Produce the final 64-byte digest from the state (big-endian).
        let mut digest = [0u8; 64];
        for (i, word) in self.state.iter().enumerate() {
            digest[8 * i..8 * i + 8].copy_from_slice(&word.to_be_bytes());
        }
        digest
    }
}

impl Default for Sha512 {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute the SHA-512 digest of `data` in one shot.
///
/// # Example
///
/// ```ignore
/// use libbreenix::crypto::sha512::sha512;
///
/// let digest = sha512(b"abc");
/// // ddaf35a193617aba...
/// ```
pub fn sha512(data: &[u8]) -> [u8; 64] {
    let mut hasher = Sha512::new();
    hasher.update(data);
    hasher.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Convert a hex string to a 64-byte array.
    fn hex_to_bytes(hex: &str) -> [u8; 64] {
        assert_eq!(hex.len(), 128);
        let mut out = [0u8; 64];
        for i in 0..64 {
            out[i] = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap();
        }
        out
    }

    #[test]
    fn test_empty_string() {
        let expected = hex_to_bytes(
            "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce\
             47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e",
        );
        assert_eq!(sha512(b""), expected);
    }

    #[test]
    fn test_abc() {
        let expected = hex_to_bytes(
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a\
             2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f",
        );
        assert_eq!(sha512(b"abc"), expected);
    }

    #[test]
    fn test_896_bit() {
        // FIPS 180-4 test vector: "abcdefghbcdefghicdefghijdefghijkefghijkl
        //   fghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu"
        let expected = hex_to_bytes(
            "8e959b75dae313da8cf4f72814fc143f8f7779c6eb9f7fa17299aeadb6889018\
             501d289e4900f7e4331b99dec4b5433ac7d329eeb6dd26545e96e55b874be909",
        );
        assert_eq!(
            sha512(b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu"),
            expected
        );
    }

    #[test]
    fn test_incremental_matches_oneshot() {
        // Feed "abc" one byte at a time via the incremental API.
        let mut hasher = Sha512::new();
        hasher.update(b"a");
        hasher.update(b"b");
        hasher.update(b"c");
        let incremental = hasher.finalize();

        assert_eq!(incremental, sha512(b"abc"));
    }

    #[test]
    fn test_exactly_one_block() {
        // 111 bytes of data + 1 byte 0x80 + 16 bytes length = exactly 128 bytes
        // (one block after padding). Verify incremental matches oneshot.
        let data = [0x61u8; 111]; // 111 'a' characters
        let oneshot = sha512(&data);
        let mut hasher = Sha512::new();
        hasher.update(&data[..60]);
        hasher.update(&data[60..]);
        assert_eq!(hasher.finalize(), oneshot);
    }

    #[test]
    fn test_two_block_boundary() {
        // 112 bytes of data forces the padding into a second block.
        let data = [0x62u8; 112];
        let oneshot = sha512(&data);
        let mut hasher = Sha512::new();
        hasher.update(&data[..10]);
        hasher.update(&data[10..80]);
        hasher.update(&data[80..]);
        assert_eq!(hasher.finalize(), oneshot);
    }

    #[test]
    fn test_large_input() {
        // 1000 bytes fed in various chunk sizes.
        let data: Vec<u8> = (0..1000).map(|i| (i % 256) as u8).collect();
        let oneshot = sha512(&data);

        let mut hasher = Sha512::new();
        for chunk in data.chunks(127) {
            hasher.update(chunk);
        }
        assert_eq!(hasher.finalize(), oneshot);
    }
}
