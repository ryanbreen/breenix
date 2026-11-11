//! SMB 3 preauthentication, key derivation and message signing.

use crate::Error;
use libbreenix::crypto::{cmac::AesCmac, ct::ct_eq, hmac::HmacSha256, sha512::Sha512};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    Smb302,
    Smb311,
}

/// The caller feeds complete SMB messages, excluding the final successful
/// SESSION_SETUP response, which is verified using the resulting signing key.
#[derive(Clone)]
pub struct PreauthHash([u8; 64]);

impl Default for PreauthHash {
    fn default() -> Self {
        Self::new()
    }
}

impl PreauthHash {
    pub fn new() -> Self {
        Self([0; 64])
    }

    pub fn update(&mut self, message: &[u8]) {
        let mut hash = Sha512::new();
        hash.update(&self.0);
        hash.update(message);
        self.0 = hash.finalize();
    }

    pub fn value(&self) -> &[u8; 64] {
        &self.0
    }
}

pub struct SessionKeys {
    pub signing: [u8; 16],
    pub application: [u8; 16],
    pub encryption: [u8; 16],
    pub decryption: [u8; 16],
}

fn kdf(key: &[u8; 16], label: &[u8], context: &[u8]) -> [u8; 16] {
    let mut mac = HmacSha256::new(key);
    mac.update(&1u32.to_be_bytes());
    mac.update(label);
    mac.update(&[0]);
    mac.update(context);
    mac.update(&128u32.to_be_bytes());
    let digest = mac.finalize();
    let mut result = [0; 16];
    result.copy_from_slice(&digest[..16]);
    result
}

/// Derive client-side AES-128 keys; encryption is client-to-server.
pub fn derive_keys(dialect: Dialect, key: &[u8; 16], preauth: &[u8; 64]) -> SessionKeys {
    match dialect {
        Dialect::Smb302 => SessionKeys {
            signing: kdf(key, b"SMB2AESCMAC\0", b"SmbSign\0"),
            application: kdf(key, b"SMB2APP\0", b"SmbRpc\0"),
            encryption: kdf(key, b"SMB2AESCCM\0", b"ServerIn \0"),
            decryption: kdf(key, b"SMB2AESCCM\0", b"ServerOut\0"),
        },
        Dialect::Smb311 => SessionKeys {
            signing: kdf(key, b"SMBSigningKey\0", preauth),
            application: kdf(key, b"SMBAppKey\0", preauth),
            encryption: kdf(key, b"SMBC2SCipherKey\0", preauth),
            decryption: kdf(key, b"SMBS2CCipherKey\0", preauth),
        },
    }
}

fn valid_header(message: &[u8]) -> bool {
    message.len() >= 64 && &message[..4] == b"\xfeSMB" && message[4..6] == 64u16.to_le_bytes()
}

fn signature(key: &[u8; 16], message: &[u8]) -> [u8; 16] {
    let mut mac = AesCmac::new(key);
    mac.update(&message[..48]);
    mac.update(&[0; 16]);
    mac.update(&message[64..]);
    mac.finalize()
}

pub fn sign(key: &[u8; 16], message: &mut [u8]) -> Result<(), Error> {
    if !valid_header(message) {
        return Err(Error);
    }
    message[16] |= 8;
    let tag = signature(key, message);
    message[48..64].copy_from_slice(&tag);
    Ok(())
}

pub fn verify(key: &[u8; 16], message: &[u8]) -> bool {
    valid_header(message)
        && message[16] & 8 != 0
        && ct_eq(&message[48..64], &signature(key, message))
}
