//! AES must not index memory by secret bytes: a table lookup's cache line
//! depends on the key and the data. The cipher computes the S-box as a
//! Boolean circuit instead, so its timing does not depend on either.

use libbreenix::crypto::aes::sub_word;

/// FIPS 197 section 5.1.1: the multiplicative inverse in GF(2^8), then the
/// affine transformation. Computed here from the definition, not a table.
fn sbox_by_definition(value: u8) -> u8 {
    let multiply = |mut a: u8, mut b: u8| {
        let mut product = 0;
        while b != 0 {
            if b & 1 != 0 {
                product ^= a;
            }
            a = (a << 1) ^ if a & 0x80 != 0 { 0x1b } else { 0 };
            b >>= 1;
        }
        product
    };
    let inverse = (1..=255u8).find(|&x| multiply(value, x) == 1).unwrap_or(0);
    let mut out = 0x63;
    for shift in 0..5 {
        out ^= inverse.rotate_left(shift);
    }
    out
}

#[test]
fn sub_word_matches_the_fips_197_definition_for_every_byte_in_every_lane() {
    for value in 0..=255u8 {
        let lanes = [value, value.wrapping_add(85), value.wrapping_add(170), !value];
        let substituted = sub_word(u32::from_le_bytes(lanes)).to_le_bytes();
        for (lane, (&input, &output)) in lanes.iter().zip(&substituted).enumerate() {
            assert_eq!(output, sbox_by_definition(input), "lane {lane}, input {input:#04x}");
        }
    }
}

#[test]
fn aes_source_has_no_lookup_tables_indexed_by_data() {
    let source = include_str!("../libs/libbreenix/src/crypto/aes.rs");
    let code = source.split("#[cfg(test)]").next().unwrap();
    for forbidden in ["[u8; 256]", "as usize]"] {
        assert!(
            !code.contains(forbidden),
            "crypto/aes.rs indexes a table by data ({forbidden:?})"
        );
    }
}
