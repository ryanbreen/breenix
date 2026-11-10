//! Cryptographic primitives for Breenix
//!
//! Pure Rust implementations with zero external dependencies.

#[cfg(feature = "std")]
pub mod aes;
#[cfg(feature = "std")]
pub mod bignum;
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
