//! Host checks for the kernel random pool's primitives and generator, compiled
//! from the kernel's own files. Vectors are the published ones: RFC 8439
//! (ChaCha20) sections 2.1.1, 2.2.1, 2.3.2 and Appendix A.1, and RFC 7693
//! (BLAKE2s) Appendix B and the Appendix E self-test.
#![cfg(test)]

#[path = "../../../kernel/src/random/blake2s.rs"]
mod blake2s;
#[path = "../../../kernel/src/random/chacha20.rs"]
mod chacha20;
#[path = "../../../kernel/src/random/generator.rs"]
mod generator;

use blake2s::Blake2s;
use generator::{Generator, MAX_FILL};

fn hex(text: &str) -> Vec<u8> {
    let digits: Vec<u8> = text.bytes().filter(|b| b.is_ascii_hexdigit()).collect();
    digits
        .chunks(2)
        .map(|pair| u8::from_str_radix(core::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn blake2s(digest_len: usize, key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut hash = Blake2s::new(digest_len, key);
    hash.update(data);
    let mut out = [0u8; 32];
    hash.finalize(&mut out);
    out[..digest_len].to_vec()
}

#[test]
fn chacha20_quarter_round_matches_rfc8439_2_1_1() {
    let mut state = [0u32; 16];
    state[..4].copy_from_slice(&[0x1111_1111, 0x0102_0304, 0x9b8d_6f43, 0x0123_4567]);
    chacha20::quarter_round(&mut state, 0, 1, 2, 3);
    assert_eq!(state[..4], [0xea2a_92f4, 0xcb1c_f8ce, 0x4581_472e, 0x5881_c4bb]);
}

#[test]
fn chacha20_quarter_round_on_state_matches_rfc8439_2_2_1() {
    let mut state = [
        0x8795_31e0, 0xc5ec_f37d, 0x5164_61b1, 0xc9a6_2f8a, 0x44c2_0ef3, 0x3390_af7f, 0xd9fc_690b,
        0x2a5f_714c, 0x5337_2767, 0xb00a_5631, 0x974c_541a, 0x359e_9963, 0x5c97_1061, 0x3d63_1689,
        0x2098_d9d6, 0x91db_d320,
    ];
    chacha20::quarter_round(&mut state, 2, 7, 8, 13);
    assert_eq!(
        state,
        [
            0x8795_31e0, 0xc5ec_f37d, 0xbdb8_86dc, 0xc9a6_2f8a, 0x44c2_0ef3, 0x3390_af7f,
            0xd9fc_690b, 0xcfac_afd2, 0xe46b_ea80, 0xb00a_5631, 0x974c_541a, 0x359e_9963,
            0x5c97_1061, 0xccc0_7c79, 0x2098_d9d6, 0x91db_d320,
        ]
    );
}

#[test]
fn chacha20_block_matches_rfc8439_2_3_2() {
    let key: [u8; 32] = core::array::from_fn(|i| i as u8);
    let nonce: [u8; 12] = hex("000000090000004a00000000").try_into().unwrap();
    let expected = hex(
        "10f1e7e4d13b5915500fdd1fa32071c4c7d1f4c733c068030422aa9ac3d46c4e\
         d2826446079faa0914c2d705d98b02a2b5129cd1de164eb9cbd083e8a2503c4e",
    );
    assert_eq!(chacha20::block(&key, 1, &nonce).to_vec(), expected);
}

#[test]
fn chacha20_block_matches_rfc8439_a_1_vector_1() {
    let expected = hex(
        "76b8e0ada0f13d90405d6ae55386bd28bdd219b8a08ded1aa836efcc8b770dc7\
         da41597c5157488d7724e03fb8d84a376a43b8f41518a11cc387b669b2ee6586",
    );
    assert_eq!(chacha20::block(&[0; 32], 0, &[0; 12]).to_vec(), expected);
}

#[test]
fn blake2s_abc_matches_rfc7693_appendix_b() {
    let expected = hex("508c5e8c327c14e2e1a72ba34eeb452f37458b209ed63a294d999b4c86675982");
    assert_eq!(blake2s(32, &[], b"abc"), expected);
}

/// The Appendix E self-test: unkeyed and keyed hashes over the 4 listed
/// digest lengths and 6 input lengths, themselves hashed into one digest. It covers keyed
/// mode, short digests, the empty input and inputs ending on a block boundary.
#[test]
fn blake2s_self_test_matches_rfc7693_appendix_e() {
    fn sequence(len: usize, seed: u32) -> Vec<u8> {
        let mut a = 0xDEAD_4BADu32.wrapping_mul(seed);
        let mut b = 1u32;
        (0..len)
            .map(|_| {
                let t = a.wrapping_add(b);
                a = b;
                b = t;
                (t >> 24) as u8
            })
            .collect()
    }
    let mut all = Blake2s::new(32, &[]);
    for digest_len in [16, 20, 28, 32] {
        for input_len in [0, 3, 64, 65, 255, 1024] {
            let input = sequence(input_len, input_len as u32);
            all.update(&blake2s(digest_len, &[], &input));
            let key = sequence(digest_len, digest_len as u32);
            all.update(&blake2s(digest_len, &key, &input));
        }
    }
    let mut digest = [0u8; 32];
    all.finalize(&mut digest);
    let expected = hex("6a411f08ce25adcdfb02aba641451cec53c598b24f4fc787fbdc88797f4c1dfe");
    assert_eq!(digest.to_vec(), expected);
}

#[test]
fn blake2s_split_updates_match_one_update() {
    let data: Vec<u8> = (0..300u32).map(|i| (i * 7) as u8).collect();
    for split in [0, 1, 63, 64, 65, 128, 299, 300] {
        let mut hash = Blake2s::new(32, b"key");
        hash.update(&data[..split]);
        hash.update(&data[split..]);
        let mut digest = [0u8; 32];
        hash.finalize(&mut digest);
        assert_eq!(digest.to_vec(), blake2s(32, b"key", &data), "split at {split}");
    }
}

fn read(generator: &mut Generator, len: usize) -> Vec<u8> {
    let mut out = vec![0u8; len];
    generator.fill(&mut out);
    out
}

/// The output is RFC 8439 keystream under the current key: block 0 from byte
/// 32 on, then block 1, and so on. The next key is block 0's first 32 bytes,
/// which no read returns.
#[test]
fn fill_is_chacha20_keystream_with_the_key_replaced() {
    let mut generator = Generator::new();
    generator.mix(&[b"seed"]);
    let key = {
        // The key the mix produced, recomputed independently.
        let mut hash = Blake2s::new(32, &[0; 32]);
        hash.update(&4u64.to_le_bytes());
        hash.update(b"seed");
        let mut key = [0u8; 32];
        hash.finalize(&mut key);
        key
    };
    let out = read(&mut generator, 100);
    let block0 = chacha20::block(&key, 0, &[0; 12]);
    let block1 = chacha20::block(&key, 1, &[0; 12]);
    let block2 = chacha20::block(&key, 2, &[0; 12]);
    assert_eq!(out[..32], block0[32..]);
    assert_eq!(out[32..96], block1[..]);
    assert_eq!(out[96..], block2[..4]);

    let next_key: [u8; 32] = block0[..32].try_into().unwrap();
    let next = read(&mut generator, 32);
    assert_eq!(next[..], chacha20::block(&next_key, 0, &[0; 12])[32..]);
}

#[test]
fn consecutive_reads_differ() {
    let mut generator = Generator::new();
    generator.mix(&[b"seed"]);
    // Two-byte reads, the size of a DNS transaction ID: 10,000 uniform draws
    // from 65,536 values give about 9,260 distinct values and expect 0.15
    // equal neighbours.
    let ids: Vec<Vec<u8>> = (0..10_000).map(|_| read(&mut generator, 2)).collect();
    let distinct: std::collections::HashSet<_> = ids.iter().collect();
    assert!(distinct.len() > 9_000, "{} distinct", distinct.len());
    assert!(ids.windows(2).filter(|pair| pair[0] == pair[1]).count() <= 3);
    let reads: Vec<Vec<u8>> = (0..1000).map(|_| read(&mut generator, 16)).collect();
    let distinct: std::collections::HashSet<_> = reads.iter().collect();
    assert_eq!(distinct.len(), reads.len());
    assert!(reads.windows(2).all(|pair| pair[0] != pair[1]));
}

/// The defect in #2028: each read was a fixed function of the cycle counter.
/// Here two generators fold in the same counter reading before each read, and
/// still produce unrelated output because their earlier inputs differ; and one
/// generator given the same counter reading twice returns different bytes.
#[test]
fn output_does_not_depend_only_on_the_counter() {
    let counter = 0x0123_4567_89ab_cdefu64.to_le_bytes();

    let mut a = Generator::new();
    let mut b = Generator::new();
    a.mix(&[b"boot", &[1; 32]]);
    b.mix(&[b"boot", &[2; 32]]);
    a.mix(&[b"read", &counter]);
    b.mix(&[b"read", &counter]);
    assert_ne!(read(&mut a, 32), read(&mut b, 32));

    let mut c = Generator::new();
    c.mix(&[b"boot", &[1; 32]]);
    c.mix(&[b"read", &counter]);
    let first = read(&mut c, 32);
    c.mix(&[b"read", &counter]);
    assert_ne!(first, read(&mut c, 32));
}

#[test]
fn every_input_bit_changes_the_output() {
    let base = [0u8; 8];
    let mut reference = Generator::new();
    reference.mix(&[&base]);
    let expected = read(&mut reference, 32);
    for bit in 0..64 {
        let mut input = base;
        input[bit / 8] ^= 1 << (bit % 8);
        let mut generator = Generator::new();
        generator.mix(&[&input]);
        assert_ne!(read(&mut generator, 32), expected, "bit {bit}");
    }
}

#[test]
fn mix_separates_input_boundaries() {
    let mut a = Generator::new();
    let mut b = Generator::new();
    a.mix(&[b"ab", b"c"]);
    b.mix(&[b"a", b"bc"]);
    assert_ne!(read(&mut a, 32), read(&mut b, 32));
}

#[test]
fn every_fill_length_is_written_in_full() {
    for len in 0..=MAX_FILL {
        let mut generator = Generator::new();
        generator.mix(&[b"seed"]);
        let mut out = vec![0u8; len];
        generator.fill(&mut out);
        // A 32-byte run of 0 bytes would mean a block was skipped.
        if len >= 32 {
            assert!(out.chunks(32).all(|chunk| chunk.iter().any(|&b| b != 0)), "len {len}");
        }
    }
}
