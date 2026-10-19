//! The generator behind getrandom: one secret key, replaced by each input and
//! by each read.
//!
//! - Input: the key becomes BLAKE2s-256 (RFC 7693) keyed with the current key,
//!   over the input. Without the current key, knowing or choosing the input
//!   does not reveal or fix the new key, and an input with any unpredictable
//!   bits makes the key unpredictable.
//! - Output: ChaCha20 (RFC 8439) keystream under the key, a 12-byte nonce of
//!   0s, block counter from 0. The first 32 bytes of block 0 become the next
//!   key and `fill` does not return them; the output starts at byte 32. Each
//!   read therefore leaves a key that cannot be run backwards to the bytes it
//!   returned, and two reads do not share a key.
//!
//! Pure computation on byte arrays, so the host tests compile this file as is.

use super::blake2s::Blake2s;
use super::chacha20::{self, BLOCK_LEN, KEY_LEN};

/// Longest output one `fill` produces; larger reads take several, each with a
/// fresh key.
pub const MAX_FILL: usize = 256;

pub struct Generator {
    key: [u8; KEY_LEN],
}

impl Generator {
    pub const fn new() -> Self {
        Self { key: [0; KEY_LEN] }
    }

    /// Fold the parts of `input`, in order and each with its length, into the key.
    pub fn mix(&mut self, input: &[&[u8]]) {
        let mut hash = Blake2s::new(KEY_LEN, &self.key);
        for part in input {
            hash.update(&(part.len() as u64).to_le_bytes());
            hash.update(part);
        }
        hash.finalize(&mut self.key);
    }

    /// Fill `out` (at most `MAX_FILL` bytes) and replace the key.
    pub fn fill(&mut self, out: &mut [u8]) {
        assert!(out.len() <= MAX_FILL);
        let nonce = [0u8; 12];
        let mut first = chacha20::block(&self.key, 0, &nonce);
        let mut written = (BLOCK_LEN - KEY_LEN).min(out.len());
        out[..written].copy_from_slice(&first[KEY_LEN..KEY_LEN + written]);
        let mut counter = 1;
        while written < out.len() {
            let mut block = chacha20::block(&self.key, counter, &nonce);
            let take = BLOCK_LEN.min(out.len() - written);
            out[written..written + take].copy_from_slice(&block[..take]);
            super::erase::wipe(&mut block);
            written += take;
            counter += 1;
        }
        self.key.copy_from_slice(&first[..KEY_LEN]);
        super::erase::wipe(&mut first);
    }
}
