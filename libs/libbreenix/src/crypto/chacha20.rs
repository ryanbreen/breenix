//! ChaCha20 stream cipher (RFC 8439 section 2.3 and 2.4).
//!
//! The cipher uses only 32-bit additions, rotations and XORs, so its timing
//! does not depend on the key or the data.
//!
//! RFC 8439 lays out the last four state words as a 32-bit block counter and
//! a 96-bit nonce. The original layout, used by SSH's
//! chacha20-poly1305@openssh.com, has a 64-bit counter and a 64-bit nonce.
//! While the counter stays below 2^32 the two agree when the 96-bit nonce is
//! four 0x00 bytes followed by the 64-bit nonce, which is how
//! [`ssh_nonce`] builds it.

const CONSTANTS: [u32; 4] = [0x6170_7865, 0x3320_646e, 0x7962_2d32, 0x6b20_6574];

#[inline(always)]
fn quarter_round(state: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    state[a] = state[a].wrapping_add(state[b]);
    state[d] = (state[d] ^ state[a]).rotate_left(16);
    state[c] = state[c].wrapping_add(state[d]);
    state[b] = (state[b] ^ state[c]).rotate_left(12);
    state[a] = state[a].wrapping_add(state[b]);
    state[d] = (state[d] ^ state[a]).rotate_left(8);
    state[c] = state[c].wrapping_add(state[d]);
    state[b] = (state[b] ^ state[c]).rotate_left(7);
}

fn initial_state(key: &[u8; 32], counter: u32, nonce: &[u8; 12]) -> [u32; 16] {
    let word = |bytes: &[u8]| u32::from_le_bytes(bytes.try_into().unwrap());
    let mut state = [0u32; 16];
    state[..4].copy_from_slice(&CONSTANTS);
    for (slot, chunk) in state[4..12].iter_mut().zip(key.chunks_exact(4)) {
        *slot = word(chunk);
    }
    state[12] = counter;
    for (slot, chunk) in state[13..].iter_mut().zip(nonce.chunks_exact(4)) {
        *slot = word(chunk);
    }
    state
}

fn block_words(input: &[u32; 16]) -> [u32; 16] {
    let mut state = *input;
    for _ in 0..10 {
        quarter_round(&mut state, 0, 4, 8, 12);
        quarter_round(&mut state, 1, 5, 9, 13);
        quarter_round(&mut state, 2, 6, 10, 14);
        quarter_round(&mut state, 3, 7, 11, 15);
        quarter_round(&mut state, 0, 5, 10, 15);
        quarter_round(&mut state, 1, 6, 11, 12);
        quarter_round(&mut state, 2, 7, 8, 13);
        quarter_round(&mut state, 3, 4, 9, 14);
    }
    for (out, original) in state.iter_mut().zip(input) {
        *out = out.wrapping_add(*original);
    }
    state
}

/// The ChaCha20 block function: one 64-byte keystream block.
pub fn chacha20_block(key: &[u8; 32], counter: u32, nonce: &[u8; 12]) -> [u8; 64] {
    let words = block_words(&initial_state(key, counter, nonce));
    let mut out = [0u8; 64];
    for (chunk, word) in out.chunks_exact_mut(4).zip(words) {
        chunk.copy_from_slice(&word.to_le_bytes());
    }
    out
}

/// XOR `data` with the keystream that starts at block `counter`.
/// Encryption and decryption are the same operation.
pub fn chacha20_xor(key: &[u8; 32], counter: u32, nonce: &[u8; 12], data: &mut [u8]) {
    let mut state = initial_state(key, counter, nonce);
    for chunk in data.chunks_mut(64) {
        let words = block_words(&state);
        for (bytes, word) in chunk.chunks_mut(4).zip(words) {
            for (byte, key_byte) in bytes.iter_mut().zip(word.to_le_bytes()) {
                *byte ^= key_byte;
            }
        }
        state[12] = state[12].wrapping_add(1);
    }
}

/// The 96-bit nonce equivalent to SSH's 64-bit big-endian sequence nonce.
pub fn ssh_nonce(sequence: u32) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[8..].copy_from_slice(&sequence.to_be_bytes());
    nonce
}
