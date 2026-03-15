//! Breenix SSH (BSSH) — clean-room SSH-2 protocol implementation
//!
//! Implements the SSH-2 protocol (RFC 4253, 4252, 4254) with:
//! - Key exchange: curve25519-sha256 (RFC 8731)
//! - Host key: rsa-sha2-256 (RFC 8332)
//! - Cipher: aes128-ctr (RFC 4344)
//! - MAC: hmac-sha2-256 (RFC 6668)
//! - Compression: none

pub mod auth;
pub mod channel;
pub mod cipher;
pub mod kex;
pub mod keys;
pub mod packet;
pub mod transport;

// SSH message type constants (RFC 4253, 4252, 4254)
pub const SSH_MSG_DISCONNECT: u8 = 1;
pub const SSH_MSG_IGNORE: u8 = 2;
pub const SSH_MSG_UNIMPLEMENTED: u8 = 3;
pub const SSH_MSG_DEBUG: u8 = 4;
pub const SSH_MSG_SERVICE_REQUEST: u8 = 5;
pub const SSH_MSG_SERVICE_ACCEPT: u8 = 6;
pub const SSH_MSG_KEXINIT: u8 = 20;
pub const SSH_MSG_NEWKEYS: u8 = 21;
pub const SSH_MSG_KEX_ECDH_INIT: u8 = 30;
pub const SSH_MSG_KEX_ECDH_REPLY: u8 = 31;
pub const SSH_MSG_USERAUTH_REQUEST: u8 = 50;
pub const SSH_MSG_USERAUTH_FAILURE: u8 = 51;
pub const SSH_MSG_USERAUTH_SUCCESS: u8 = 52;
pub const SSH_MSG_USERAUTH_BANNER: u8 = 53;
pub const SSH_MSG_GLOBAL_REQUEST: u8 = 80;
pub const SSH_MSG_REQUEST_SUCCESS: u8 = 81;
pub const SSH_MSG_REQUEST_FAILURE: u8 = 82;
pub const SSH_MSG_CHANNEL_OPEN: u8 = 90;
pub const SSH_MSG_CHANNEL_OPEN_CONFIRMATION: u8 = 91;
pub const SSH_MSG_CHANNEL_OPEN_FAILURE: u8 = 92;
pub const SSH_MSG_CHANNEL_WINDOW_ADJUST: u8 = 93;
pub const SSH_MSG_CHANNEL_DATA: u8 = 94;
pub const SSH_MSG_CHANNEL_EXTENDED_DATA: u8 = 95;
pub const SSH_MSG_CHANNEL_EOF: u8 = 96;
pub const SSH_MSG_CHANNEL_CLOSE: u8 = 97;
pub const SSH_MSG_CHANNEL_REQUEST: u8 = 98;
pub const SSH_MSG_CHANNEL_SUCCESS: u8 = 99;
pub const SSH_MSG_CHANNEL_FAILURE: u8 = 100;

/// SSH version string for BSSH
pub const BSSH_VERSION: &str = "SSH-2.0-bssh_1.0";

/// SSH disconnect reason codes
pub const SSH_DISCONNECT_HOST_NOT_ALLOWED_TO_CONNECT: u32 = 1;
pub const SSH_DISCONNECT_PROTOCOL_ERROR: u32 = 2;
pub const SSH_DISCONNECT_KEY_EXCHANGE_FAILED: u32 = 3;
pub const SSH_DISCONNECT_MAC_ERROR: u32 = 5;
pub const SSH_DISCONNECT_SERVICE_NOT_AVAILABLE: u32 = 7;
pub const SSH_DISCONNECT_BY_APPLICATION: u32 = 11;

/// Errors that can occur during SSH operations.
#[derive(Debug)]
pub enum SshError {
    Io,
    Protocol(&'static str),
    Mac,
    KeyExchange,
    Auth,
    ChannelNotFound,
    Disconnected,
}

/// SSH string encoding helpers (RFC 4251 §5)
pub struct SshBuf;

impl SshBuf {
    /// Encode a u32 in big-endian (network byte order).
    pub fn put_u32(buf: &mut Vec<u8>, val: u32) {
        buf.extend_from_slice(&val.to_be_bytes());
    }

    /// Encode a byte string with a uint32 length prefix.
    pub fn put_string(buf: &mut Vec<u8>, data: &[u8]) {
        Self::put_u32(buf, data.len() as u32);
        buf.extend_from_slice(data);
    }

    /// Encode an SSH mpint (RFC 4251 §5).
    ///
    /// The value is encoded as a big-endian byte string with a uint32 length
    /// prefix. A leading 0x00 byte is prepended if the MSB of the first byte
    /// is set (to distinguish from negative numbers).
    pub fn put_mpint(buf: &mut Vec<u8>, val: &[u8]) {
        // Skip leading zeros
        let start = val.iter().position(|&b| b != 0).unwrap_or(val.len());
        let trimmed = &val[start..];

        if trimmed.is_empty() {
            Self::put_u32(buf, 0);
        } else if trimmed[0] & 0x80 != 0 {
            // Need leading zero to indicate positive
            Self::put_u32(buf, (trimmed.len() + 1) as u32);
            buf.push(0);
            buf.extend_from_slice(trimmed);
        } else {
            Self::put_u32(buf, trimmed.len() as u32);
            buf.extend_from_slice(trimmed);
        }
    }

    /// Encode a boolean.
    pub fn put_bool(buf: &mut Vec<u8>, val: bool) {
        buf.push(if val { 1 } else { 0 });
    }

    /// Read a u32 from a byte slice, advancing the position.
    pub fn get_u32(data: &[u8], pos: &mut usize) -> Option<u32> {
        if *pos + 4 > data.len() {
            return None;
        }
        let val = u32::from_be_bytes([
            data[*pos],
            data[*pos + 1],
            data[*pos + 2],
            data[*pos + 3],
        ]);
        *pos += 4;
        Some(val)
    }

    /// Read an SSH string from a byte slice, advancing the position.
    pub fn get_string<'a>(data: &'a [u8], pos: &mut usize) -> Option<&'a [u8]> {
        let len = Self::get_u32(data, pos)? as usize;
        if *pos + len > data.len() {
            return None;
        }
        let result = &data[*pos..*pos + len];
        *pos += len;
        Some(result)
    }

    /// Read a boolean from a byte slice.
    pub fn get_bool(data: &[u8], pos: &mut usize) -> Option<bool> {
        if *pos >= data.len() {
            return None;
        }
        let val = data[*pos] != 0;
        *pos += 1;
        Some(val)
    }

    /// Read a single byte from a byte slice.
    pub fn get_u8(data: &[u8], pos: &mut usize) -> Option<u8> {
        if *pos >= data.len() {
            return None;
        }
        let val = data[*pos];
        *pos += 1;
        Some(val)
    }

    /// Build a name-list string from a single algorithm name.
    pub fn name_list(names: &[&str]) -> Vec<u8> {
        let joined: Vec<u8> = names.join(",").into_bytes();
        let mut buf = Vec::with_capacity(4 + joined.len());
        Self::put_string(&mut buf, &joined);
        buf
    }
}
