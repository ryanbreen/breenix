//! The ChaCha20 block function, written from RFC 8439 sections 2.1 to 2.3.
//!
//! Pure computation on byte arrays, so the host tests compile this file as is.

/// Bytes in a ChaCha20 key.
pub const KEY_LEN: usize = 32;
/// Bytes of keystream one block produces.
pub const BLOCK_LEN: usize = 64;

/// "expand 32-byte k" as four little-endian words (RFC 8439 section 2.3).
const CONSTANTS: [u32; 4] = [0x6170_7865, 0x3320_646e, 0x7962_2d32, 0x6b20_6574];

/// The quarter round on four words of the state (RFC 8439 section 2.2).
#[inline(always)]
pub fn quarter_round(state: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    state[a] = state[a].wrapping_add(state[b]);
    state[d] = (state[d] ^ state[a]).rotate_left(16);
    state[c] = state[c].wrapping_add(state[d]);
    state[b] = (state[b] ^ state[c]).rotate_left(12);
    state[a] = state[a].wrapping_add(state[b]);
    state[d] = (state[d] ^ state[a]).rotate_left(8);
    state[c] = state[c].wrapping_add(state[d]);
    state[b] = (state[b] ^ state[c]).rotate_left(7);
}

fn word(bytes: &[u8], index: usize) -> u32 {
    u32::from_le_bytes([
        bytes[4 * index],
        bytes[4 * index + 1],
        bytes[4 * index + 2],
        bytes[4 * index + 3],
    ])
}

/// One block of keystream for `key`, block `counter` and `nonce` (RFC 8439 section 2.3).
pub fn block(key: &[u8; KEY_LEN], counter: u32, nonce: &[u8; 12]) -> [u8; BLOCK_LEN] {
    let mut initial = [0u32; 16];
    initial[..4].copy_from_slice(&CONSTANTS);
    for i in 0..8 {
        initial[4 + i] = word(key, i);
    }
    initial[12] = counter;
    for i in 0..3 {
        initial[13 + i] = word(nonce, i);
    }

    let mut state = initial;
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

    let mut out = [0u8; BLOCK_LEN];
    for i in 0..16 {
        out[4 * i..4 * i + 4].copy_from_slice(&state[i].wrapping_add(initial[i]).to_le_bytes());
    }
    super::erase::wipe(&mut initial);
    super::erase::wipe(&mut state);
    out
}
