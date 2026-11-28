//! RFC 8439 test vectors for the ChaCha20 stream cipher and the Poly1305
//! one-time authenticator (sections 2.3.2, 2.4.2, 2.5.2, 2.6.2 and A.3).

use libbreenix::crypto::{
    chacha20::{chacha20_block, chacha20_xor},
    poly1305::poly1305,
};

fn hex(text: &str) -> Vec<u8> {
    let digits: Vec<u8> = text.bytes().filter(u8::is_ascii_hexdigit).collect();
    digits
        .chunks(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn array<const N: usize>(text: &str) -> [u8; N] {
    hex(text).try_into().unwrap()
}

const KEY_00_1F: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
const SUNSCREEN: &[u8] = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.";

#[test]
fn block_function_section_2_3_2() {
    let block = chacha20_block(&array(KEY_00_1F), 1, &array("000000090000004a00000000"));
    assert_eq!(
        block.to_vec(),
        hex("10f1e7e4d13b5915500fdd1fa32071c4c7d1f4c733c068030422aa9ac3d46c4e
             d2826446079faa0914c2d705d98b02a2b5129cd1de164eb9cbd083e8a2503c4e")
    );
}

#[test]
fn encryption_section_2_4_2() {
    let mut text = SUNSCREEN.to_vec();
    chacha20_xor(&array(KEY_00_1F), 1, &array("000000000000004a00000000"), &mut text);
    assert_eq!(
        text,
        hex("6e2e359a2568f98041ba0728dd0d6981e97e7aec1d4360c20a27afccfd9fae0b
             f91b65c5524733ab8f593dabcd62b3571639d624e65152ab8f530c359f0861d8
             07ca0dbf500d6a6156a38e088a22b65e52bc514d16ccf806818ce91ab7793736
             5af90bbf74a35be6b40b8eedf2785e42874d")
    );
    // The same call decrypts.
    chacha20_xor(&array(KEY_00_1F), 1, &array("000000000000004a00000000"), &mut text);
    assert_eq!(text, SUNSCREEN);
}

#[test]
fn poly1305_section_2_5_2() {
    let key = array("85d6be7857556d337f4452fe42d506a80103808afb0db2fd4abff6af4149f51b");
    assert_eq!(
        poly1305(&key, b"Cryptographic Forum Research Group").to_vec(),
        hex("a8061dc1305136c6c22b8baf0c0127a9")
    );
}

#[test]
fn poly1305_key_generation_section_2_6_2() {
    let key = array("808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9f");
    let block = chacha20_block(&key, 0, &array("000000000001020304050607"));
    assert_eq!(
        block[..32].to_vec(),
        hex("8ad5a08b905f81cc815040274ab29471a833b637e3fd0da508dbb8e2fdd1a646")
    );
}

/// Appendix A.3 vectors 5 to 11 exercise the final carry and reduction,
/// where a non-constant-time or partially reduced implementation goes wrong.
#[test]
fn poly1305_reduction_edge_cases_appendix_a_3() {
    let cases: [(&str, &str, &str, &str); 7] = [
        (
            "02000000000000000000000000000000",
            "00000000000000000000000000000000",
            "ffffffffffffffffffffffffffffffff",
            "03000000000000000000000000000000",
        ),
        (
            "02000000000000000000000000000000",
            "ffffffffffffffffffffffffffffffff",
            "02000000000000000000000000000000",
            "03000000000000000000000000000000",
        ),
        (
            "01000000000000000000000000000000",
            "00000000000000000000000000000000",
            "ffffffffffffffffffffffffffffffff f0ffffffffffffffffffffffffffffff
             11000000000000000000000000000000",
            "05000000000000000000000000000000",
        ),
        (
            "01000000000000000000000000000000",
            "00000000000000000000000000000000",
            "ffffffffffffffffffffffffffffffff fbfefefefefefefefefefefefefefefe
             01010101010101010101010101010101",
            "00000000000000000000000000000000",
        ),
        (
            "02000000000000000000000000000000",
            "00000000000000000000000000000000",
            "fdffffffffffffffffffffffffffffff",
            "faffffffffffffffffffffffffffffff",
        ),
        (
            "01000000000000000400000000000000",
            "00000000000000000000000000000000",
            "e33594d7505e43b90000000000000000 3394d7505e4379cd0100000000000000
             00000000000000000000000000000000 01000000000000000000000000000000",
            "14000000000000005500000000000000",
        ),
        (
            "01000000000000000400000000000000",
            "00000000000000000000000000000000",
            "e33594d7505e43b90000000000000000 3394d7505e4379cd0100000000000000
             00000000000000000000000000000000",
            "13000000000000000000000000000000",
        ),
    ];
    for (index, (r, s, data, tag)) in cases.iter().enumerate() {
        let key: [u8; 32] = [hex(r), hex(s)].concat().try_into().unwrap();
        assert_eq!(poly1305(&key, &hex(data)).to_vec(), hex(tag), "vector #{}", index + 5);
    }
}
