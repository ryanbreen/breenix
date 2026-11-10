//! Cryptographic primitives for Breenix
//!
//! Pure Rust implementations with zero external dependencies.

#[cfg(feature = "std")]
pub mod aes;
#[cfg(feature = "std")]
pub mod bignum;
#[cfg(feature = "std")]
pub mod cmac;
pub mod ct;
#[cfg(feature = "std")]
pub mod ed25519;
#[cfg(feature = "std")]
pub mod gcm;
#[cfg(feature = "std")]
pub mod hmac;
#[cfg(feature = "std")]
pub mod keccak;
#[cfg(feature = "std")]
pub mod md4;
#[cfg(feature = "std")]
pub mod md5;
#[cfg(feature = "std")]
pub mod mlkem;
#[cfg(feature = "std")]
pub mod rand;
#[cfg(feature = "std")]
pub mod rsa;
#[cfg(feature = "std")]
pub mod sha256;
pub mod sha512;
#[cfg(feature = "std")]
pub mod x25519;
pub mod sha512_crypt;

#[cfg(all(test, feature = "std"))]
mod md_tests {
    use super::{md4::{md4, Md4}, md5::{md5, Md5}};

    const INPUTS: [&[u8]; 7] = [
        b"", b"a", b"abc", b"message digest", b"abcdefghijklmnopqrstuvwxyz",
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789",
        b"12345678901234567890123456789012345678901234567890123456789012345678901234567890",
    ];

    fn hex(bytes: [u8; 16]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    #[test]
    fn md4_rfc_1320_appendix_a5() {
        let expected = [
            "31d6cfe0d16ae931b73c59d7e0c089c0", "bde52cb31de33e46245e05fbdbd6fb24",
            "a448017aaf21d8525fc10ae87aa6729d", "d9130a8164549fe818874806e1c7014b",
            "d79e1c308aa5bbcdeea8ed63df412da9", "043f8582f241db351ce627e153e7f0e4",
            "e33b4ddc9c38f2199c3e7b164fcc0536",
        ];
        for (input, expected) in INPUTS.into_iter().zip(expected) {
            assert_eq!(hex(md4(input)), expected);
            let mut digest = Md4::new();
            for byte in input { digest.update(core::slice::from_ref(byte)); }
            assert_eq!(hex(digest.finalize()), expected);
        }
    }

    #[test]
    fn md5_rfc_1321_appendix_a5() {
        let expected = [
            "d41d8cd98f00b204e9800998ecf8427e", "0cc175b9c0f1b6a831c399e269772661",
            "900150983cd24fb0d6963f7d28e17f72", "f96b697d7cb7938d525a2f31aaf161d0",
            "c3fcd3d76192e4007dfb496cca67e13b", "d174ab98d277d9f5a5611c2c9f419d9f",
            "57edf4a22be3c955ac49da2e2107b67a",
        ];
        for (input, expected) in INPUTS.into_iter().zip(expected) {
            assert_eq!(hex(md5(input)), expected);
            let mut digest = Md5::new();
            for byte in input { digest.update(core::slice::from_ref(byte)); }
            assert_eq!(hex(digest.finalize()), expected);
        }
    }
}

#[cfg(all(test, feature = "std"))]
mod cmac_tests {
    use super::cmac::{aes_cmac, subkeys, AesCmac};

    fn bytes<const N: usize>(s: &str) -> [u8; N] {
        assert_eq!(s.len(), N * 2);
        core::array::from_fn(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap())
    }

    #[test]
    fn rfc_4493_subkeys() {
        let key = bytes("2b7e151628aed2a6abf7158809cf4f3c");
        let (k1, k2) = subkeys(&key);
        assert_eq!(k1, bytes("fbeed618357133667c85e08f7236a8de"));
        assert_eq!(k2, bytes("f7ddac306ae266ccf90bc11ee46d513b"));
    }

    #[test]
    fn rfc_4493_messages() {
        let key = bytes("2b7e151628aed2a6abf7158809cf4f3c");
        let message: [u8; 64] = bytes(concat!(
            "6bc1bee22e409f96e93d7e117393172a",
            "ae2d8a571e03ac9c9eb76fac45af8e51",
            "30c81c46a35ce411e5fbc1191a0a52ef",
            "f69f2445df4f9b17ad2b417be66c3710",
        ));
        for (len, expected) in [
            (0, "bb1d6929e95937287fa37d129b756746"),
            (16, "070a16b46b4d4144f79bdd9dd04a287c"),
            (40, "dfa66747de9ae63030ca32611497c827"),
            (64, "51f0bebf7e3b9d92fc49741779363cfe"),
        ] {
            let expected = bytes::<16>(expected);
            let input = &message[..len];
            assert_eq!(aes_cmac(&key, input), expected);
            let mut mac = AesCmac::new(&key);
            for chunk in input.chunks(7) { mac.update(chunk); }
            mac.update(&[]);
            assert_eq!(mac.finalize(), expected);
        }
    }
}

#[cfg(all(test, feature = "std"))]
mod ccm_tests {
    use super::ccm::AesCcm;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn nist_sp800_38c_appendix_c() {
        let key: [u8; 16] = hex("404142434445464748494a4b4c4d4e4f").try_into().unwrap();
        let vectors = [
            ("10111213141516", "0001020304050607", "20212223", "7162015b4dac255d", 4),
            ("1011121314151617", "000102030405060708090a0b0c0d0e0f", "202122232425262728292a2b2c2d2e2f", "d2a1f0e051ea5f62081a7792073d593d1fc64fbfaccd", 6),
            ("101112131415161718191a1b", "000102030405060708090a0b0c0d0e0f10111213", "202122232425262728292a2b2c2d2e2f3031323334353637", "e3b201a9f5b71a7a9b1ceaeccd97e70b6176aad9a4428aa5484392fbc1b09951", 8),
            ("101112131415161718191a1b1c", "", "202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f", "69915dad1e84c6376a68c2967e4dab615ae0fd1faec44cc484828529463ccf72b4ac6bec93e8598e7f0dadbcea5b", 14),
        ];
        for (index, (nonce, aad, plain, combined, tag_len)) in vectors.into_iter().enumerate() {
            let nonce = hex(nonce);
            let aad = if index == 3 { (0..65536).map(|i| i as u8).collect() } else { hex(aad) };
            let plain = hex(plain);
            let expected = hex(combined);
            let ccm = AesCcm::new(&key, tag_len);
            let mut buf = plain.clone();
            let tag = ccm.encrypt(&nonce, &aad, &mut buf);
            assert_eq!(buf, expected[..plain.len()]);
            assert_eq!(tag, expected[plain.len()..]);
            assert!(ccm.decrypt(&nonce, &aad, &mut buf, &tag));
            assert_eq!(buf, plain);
        }
    }

    #[test]
    fn rejects_modified_inputs_and_zeroes_plaintext() {
        let ccm = AesCcm::new(&[0; 16], 16);
        let nonce = [0; 12];
        let aad = [1; 4];
        let mut ciphertext = [2; 20];
        let tag = ccm.encrypt(&nonce, &aad, &mut ciphertext);
        for changed in 0..3 {
            let mut buf = ciphertext;
            let mut bad_aad = aad;
            let mut bad_tag = tag.clone();
            match changed { 0 => bad_tag[0] ^= 1, 1 => buf[0] ^= 1, _ => bad_aad[0] ^= 1 }
            assert!(!ccm.decrypt(&nonce, &bad_aad, &mut buf, &bad_tag));
            assert_eq!(buf, [0; 20]);
        }
    }
}
