//! TLS 1.2 client implementation for Breenix
//!
//! Supports cipher suite TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256 (0xC02F).

pub mod handshake;
pub mod prf;
pub mod record;
pub mod stream;
