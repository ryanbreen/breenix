//! SipHash-2-4 for keyed directory-name hashes.

fn round(v: &mut [u64; 4]) {
    v[0] = v[0].wrapping_add(v[1]);
    v[1] = v[1].rotate_left(13) ^ v[0];
    v[0] = v[0].rotate_left(32);
    v[2] = v[2].wrapping_add(v[3]);
    v[3] = v[3].rotate_left(16) ^ v[2];
    v[0] = v[0].wrapping_add(v[3]);
    v[3] = v[3].rotate_left(21) ^ v[0];
    v[2] = v[2].wrapping_add(v[1]);
    v[1] = v[1].rotate_left(17) ^ v[2];
    v[2] = v[2].rotate_left(32);
}

fn compress(v: &mut [u64; 4], word: u64) {
    v[3] ^= word;
    round(v);
    round(v);
    v[0] ^= word;
}

/// Hash `message` using the 16-byte SipHash key.
pub fn hash(key: &[u8; 16], message: &[u8]) -> u64 {
    let k0 = u64::from_le_bytes(key[..8].try_into().unwrap());
    let k1 = u64::from_le_bytes(key[8..].try_into().unwrap());
    let mut v = [
        0x736f_6d65_7073_6575 ^ k0,
        0x646f_7261_6e64_6f6d ^ k1,
        0x6c79_6765_6e65_7261 ^ k0,
        0x7465_6462_7974_6573 ^ k1,
    ];
    let mut chunks = message.chunks_exact(8);
    for chunk in &mut chunks {
        compress(&mut v, u64::from_le_bytes(chunk.try_into().unwrap()));
    }
    let mut last = (message.len() as u64) << 56;
    for (i, &byte) in chunks.remainder().iter().enumerate() {
        last |= u64::from(byte) << (8 * i);
    }
    compress(&mut v, last);
    v[2] ^= 0xff;
    for _ in 0..4 {
        round(&mut v);
    }
    v[0] ^ v[1] ^ v[2] ^ v[3]
}
