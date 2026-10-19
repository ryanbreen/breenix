//! BLAKE2s, written from RFC 7693 (sections 2 and 3), with its optional key.
//!
//! Pure computation on byte arrays, so the host tests compile this file as is.

/// Bytes in one message block.
const BLOCK: usize = 64;
/// Longest key and digest, in bytes.
pub const MAX_LEN: usize = 32;

/// The initialization vector (RFC 7693 section 2.6).
const IV: [u32; 8] = [
    0x6A09_E667, 0xBB67_AE85, 0x3C6E_F372, 0xA54F_F53A, 0x510E_527F, 0x9B05_688C, 0x1F83_D9AB,
    0x5BE0_CD19,
];

/// The message schedule permutations (RFC 7693 section 2.7).
const SIGMA: [[usize; 16]; 10] = [
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
    [14, 10, 4, 8, 9, 15, 13, 6, 1, 12, 0, 2, 11, 7, 5, 3],
    [11, 8, 12, 0, 5, 2, 15, 13, 10, 14, 3, 6, 7, 1, 9, 4],
    [7, 9, 3, 1, 13, 12, 11, 14, 2, 6, 5, 10, 4, 0, 15, 8],
    [9, 0, 5, 7, 2, 4, 10, 15, 14, 1, 11, 12, 6, 8, 3, 13],
    [2, 12, 6, 10, 0, 11, 8, 3, 4, 13, 7, 5, 15, 14, 1, 9],
    [12, 5, 1, 15, 14, 13, 4, 10, 0, 7, 6, 3, 9, 2, 8, 11],
    [13, 11, 7, 14, 12, 1, 3, 9, 5, 0, 15, 4, 8, 6, 2, 10],
    [6, 15, 14, 9, 11, 3, 0, 8, 12, 2, 13, 7, 1, 4, 10, 5],
    [10, 2, 8, 4, 7, 6, 1, 5, 15, 11, 9, 14, 3, 12, 13, 0],
];

/// The mixing function G with BLAKE2s's rotation distances (RFC 7693 section 3.1).
#[inline(always)]
fn mix(v: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize, x: u32, y: u32) {
    v[a] = v[a].wrapping_add(v[b]).wrapping_add(x);
    v[d] = (v[d] ^ v[a]).rotate_right(16);
    v[c] = v[c].wrapping_add(v[d]);
    v[b] = (v[b] ^ v[c]).rotate_right(12);
    v[a] = v[a].wrapping_add(v[b]).wrapping_add(y);
    v[d] = (v[d] ^ v[a]).rotate_right(8);
    v[c] = v[c].wrapping_add(v[d]);
    v[b] = (v[b] ^ v[c]).rotate_right(7);
}

/// The compression function F (RFC 7693 section 3.2).
fn compress(h: &mut [u32; 8], block: &[u8; BLOCK], bytes: u64, last: bool) {
    let mut m = [0u32; 16];
    for (i, word) in m.iter_mut().enumerate() {
        *word = u32::from_le_bytes([
            block[4 * i],
            block[4 * i + 1],
            block[4 * i + 2],
            block[4 * i + 3],
        ]);
    }
    let mut v = [0u32; 16];
    v[..8].copy_from_slice(h);
    v[8..].copy_from_slice(&IV);
    v[12] ^= bytes as u32;
    v[13] ^= (bytes >> 32) as u32;
    if last {
        v[14] = !v[14];
    }
    for s in &SIGMA {
        mix(&mut v, 0, 4, 8, 12, m[s[0]], m[s[1]]);
        mix(&mut v, 1, 5, 9, 13, m[s[2]], m[s[3]]);
        mix(&mut v, 2, 6, 10, 14, m[s[4]], m[s[5]]);
        mix(&mut v, 3, 7, 11, 15, m[s[6]], m[s[7]]);
        mix(&mut v, 0, 5, 10, 15, m[s[8]], m[s[9]]);
        mix(&mut v, 1, 6, 11, 12, m[s[10]], m[s[11]]);
        mix(&mut v, 2, 7, 8, 13, m[s[12]], m[s[13]]);
        mix(&mut v, 3, 4, 9, 14, m[s[14]], m[s[15]]);
    }
    for i in 0..8 {
        h[i] ^= v[i] ^ v[i + 8];
    }
}

/// An incremental BLAKE2s computation.
pub struct Blake2s {
    h: [u32; 8],
    buffer: [u8; BLOCK],
    buffered: usize,
    bytes: u64,
    digest_len: usize,
}

impl Blake2s {
    /// A hash of `digest_len` bytes (1 to 32) keyed with `key` (0 to 32 bytes; empty for an unkeyed hash).
    pub fn new(digest_len: usize, key: &[u8]) -> Self {
        assert!((1..=MAX_LEN).contains(&digest_len) && key.len() <= MAX_LEN);
        let mut h = IV;
        h[0] ^= 0x0101_0000 ^ ((key.len() as u32) << 8) ^ digest_len as u32;
        let mut state = Self { h, buffer: [0; BLOCK], buffered: 0, bytes: 0, digest_len };
        if !key.is_empty() {
            // The key, zero-padded, is the first message block (RFC 7693 section 3.3).
            state.buffer[..key.len()].copy_from_slice(key);
            state.buffered = BLOCK;
        }
        state
    }

    pub fn update(&mut self, mut data: &[u8]) {
        while !data.is_empty() {
            // The final block is compressed with the last-block flag, so a full
            // buffer is compressed only once more input shows it is not final.
            if self.buffered == BLOCK {
                self.bytes += BLOCK as u64;
                compress(&mut self.h, &self.buffer, self.bytes, false);
                self.buffered = 0;
            }
            let take = (BLOCK - self.buffered).min(data.len());
            self.buffer[self.buffered..self.buffered + take].copy_from_slice(&data[..take]);
            self.buffered += take;
            data = &data[take..];
        }
    }

    /// Write the digest into `out[..digest_len]`.
    pub fn finalize(mut self, out: &mut [u8]) {
        self.bytes += self.buffered as u64;
        self.buffer[self.buffered..].fill(0);
        compress(&mut self.h, &self.buffer, self.bytes, true);
        let mut digest = [0u8; MAX_LEN];
        for (i, word) in self.h.iter().enumerate() {
            digest[4 * i..4 * i + 4].copy_from_slice(&word.to_le_bytes());
        }
        out[..self.digest_len].copy_from_slice(&digest[..self.digest_len]);
    }
}
