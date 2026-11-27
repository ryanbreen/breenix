//! Poly1305 one-time authenticator (RFC 8439 section 2.5).
//!
//! The accumulator is kept in five 26-bit limbs so every product fits in a
//! u64. Blocks are processed with the same operations whatever their
//! contents, and the final reduction selects between h and h - p with a
//! mask rather than a branch, so timing depends only on the message length.

const MASK26: u64 = (1 << 26) - 1;
const CLAMP: u128 = 0x0fff_fffc_0fff_fffc_0fff_fffc_0fff_ffff;

fn limbs(value: u128) -> [u64; 5] {
    [
        (value as u64) & MASK26,
        ((value >> 26) as u64) & MASK26,
        ((value >> 52) as u64) & MASK26,
        ((value >> 78) as u64) & MASK26,
        (value >> 104) as u64,
    ]
}

/// Compute the 16-byte tag of `message` under the one-time `key` (r || s).
pub fn poly1305(key: &[u8; 32], message: &[u8]) -> [u8; 16] {
    let r = limbs(u128::from_le_bytes(key[..16].try_into().unwrap()) & CLAMP);
    let s = u128::from_le_bytes(key[16..].try_into().unwrap());
    let r5 = [0, r[1] * 5, r[2] * 5, r[3] * 5, r[4] * 5];
    let mut h = [0u64; 5];

    for chunk in message.chunks(16) {
        let mut block = [0u8; 16];
        block[..chunk.len()].copy_from_slice(chunk);
        // A full block's appended 0x01 byte is bit 128, which is bit 24 of
        // limb 4. A short final block carries it inside its 128 bits.
        let mut high_bit = 1 << 24;
        if chunk.len() < 16 {
            block[chunk.len()] = 1;
            high_bit = 0;
        }
        let mut m = limbs(u128::from_le_bytes(block));
        m[4] |= high_bit;
        for (acc, add) in h.iter_mut().zip(m) {
            *acc += add;
        }

        let d0 = h[0] * r[0] + h[1] * r5[4] + h[2] * r5[3] + h[3] * r5[2] + h[4] * r5[1];
        let d1 = h[0] * r[1] + h[1] * r[0] + h[2] * r5[4] + h[3] * r5[3] + h[4] * r5[2];
        let d2 = h[0] * r[2] + h[1] * r[1] + h[2] * r[0] + h[3] * r5[4] + h[4] * r5[3];
        let d3 = h[0] * r[3] + h[1] * r[2] + h[2] * r[1] + h[3] * r[0] + h[4] * r5[4];
        let d4 = h[0] * r[4] + h[1] * r[3] + h[2] * r[2] + h[3] * r[1] + h[4] * r[0];

        let mut carry;
        h[0] = d0 & MASK26;
        carry = d0 >> 26;
        let d1 = d1 + carry;
        h[1] = d1 & MASK26;
        carry = d1 >> 26;
        let d2 = d2 + carry;
        h[2] = d2 & MASK26;
        carry = d2 >> 26;
        let d3 = d3 + carry;
        h[3] = d3 & MASK26;
        carry = d3 >> 26;
        let d4 = d4 + carry;
        h[4] = d4 & MASK26;
        carry = d4 >> 26;
        // 2^130 = 5 (mod 2^130 - 5).
        h[0] += carry * 5;
        carry = h[0] >> 26;
        h[0] &= MASK26;
        h[1] += carry;
    }

    // Fully propagate the carries, so every limb is below 2^26.
    let mut carry = h[1] >> 26;
    h[1] &= MASK26;
    for i in 2..5 {
        h[i] += carry;
        carry = h[i] >> 26;
        h[i] &= MASK26;
    }
    h[0] += carry * 5;
    carry = h[0] >> 26;
    h[0] &= MASK26;
    h[1] += carry;

    // g = h + 5 - 2^130. If it does not borrow, h was at least p.
    let mut g = [0u64; 5];
    g[0] = h[0] + 5;
    carry = g[0] >> 26;
    g[0] &= MASK26;
    for i in 1..4 {
        g[i] = h[i] + carry;
        carry = g[i] >> 26;
        g[i] &= MASK26;
    }
    g[4] = h[4].wrapping_add(carry).wrapping_sub(1 << 26);
    let use_g = (g[4] >> 63).wrapping_sub(1);
    for i in 0..5 {
        h[i] = (h[i] & !use_g) | (g[i] & use_g);
    }
    h[4] &= MASK26;

    let value = h[0] as u128
        | (h[1] as u128) << 26
        | (h[2] as u128) << 52
        | (h[3] as u128) << 78
        | (h[4] as u128) << 104;
    value.wrapping_add(s).to_le_bytes()
}
