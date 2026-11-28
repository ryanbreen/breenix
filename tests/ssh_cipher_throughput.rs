use libbreenix::crypto::gcm::ghash_multiply;
use std::{
    hint::black_box,
    time::{Duration, Instant},
};

type Block = [u8; 16];

fn blocks() -> Vec<Block> {
    let mut seed = 0xa151_89d1_4d1f_ae17u64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed.to_be_bytes()
    };
    (0..4096)
        .map(|_| {
            let mut block = [0; 16];
            block[..8].copy_from_slice(&next());
            block[8..].copy_from_slice(&next());
            block
        })
        .collect()
}

// NIST SP 800-38D Algorithm 1, expressed byte by byte.
fn ghash_reference(x: &Block, h: &Block) -> Block {
    let mut product = [0; 16];
    let mut shifted = *h;
    for bit in 0..128 {
        if x[bit / 8] & (0x80 >> (bit % 8)) != 0 {
            for (out, byte) in product.iter_mut().zip(shifted) {
                *out ^= byte;
            }
        }
        let reduce = shifted[15] & 1 != 0;
        for i in (1..16).rev() {
            shifted[i] = (shifted[i] >> 1) | (shifted[i - 1] << 7);
        }
        shifted[0] >>= 1;
        if reduce {
            shifted[0] ^= 0xe1;
        }
    }
    product
}

fn best_time(blocks: &[Block], op: impl Fn(Block) -> Block) -> Duration {
    (0..3)
        .map(|_| {
            let start = Instant::now();
            for block in blocks {
                black_box(op(black_box(*block)));
            }
            start.elapsed()
        })
        .min()
        .unwrap()
}

fn check_work(blocks: &[Block], reference: impl Fn(Block) -> Block, fast: impl Fn(Block) -> Block) {
    for &block in blocks {
        assert_eq!(fast(block), reference(block));
    }
    // Compare sustained work in the same process to keep the check independent of CPU speed.
    let reference = best_time(blocks, reference);
    let fast = best_time(blocks, fast);
    assert!(
        fast * 2 < reference,
        "block operation took {fast:?}; expected at least 2x the reference rate ({reference:?})"
    );
}

#[test]
fn ghash_avoids_bytewise_work_per_bit() {
    let h = black_box([0x5a; 16]);
    check_work(
        &blocks(),
        |block| ghash_reference(&block, &h),
        |block| ghash_multiply(&block, &h),
    );
}
