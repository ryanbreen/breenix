//! Cryptographic primitives for Breenix
//!
//! Pure Rust implementations with zero external dependencies.

#[cfg(feature = "std")]
pub mod aes;
#[cfg(feature = "std")]
pub mod bignum;
#[cfg(feature = "std")]
pub mod gcm;
#[cfg(feature = "std")]
pub mod hmac;
#[cfg(feature = "std")]
pub mod keccak;
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
