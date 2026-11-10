//! SMB 3 encrypted transform (MS-SMB2 section 2.2.41).
use crate::Error;
use libbreenix::crypto::{ccm::AesCcm, gcm::AesGcm};

const HEADER_LEN: usize = 52;

#[derive(Debug, Clone, Copy)]
pub enum Cipher {
    Aes128Gcm,
    Aes128Ccm,
}

/// One sending connection's nonce sequence and session keys.
///
/// The caller supplies a distinct prefix for each connection sharing a key,
/// including reconnections, to avoid repeating a nonce under that key.
pub struct Transform {
    cipher: Cipher,
    key: [u8; 16],
    session_id: u64,
    prefix: [u8; 3],
    next: Option<u64>,
}

impl Transform {
    pub fn new(cipher: Cipher, key: [u8; 16], session_id: u64, prefix: [u8; 3]) -> Self {
        Self {
            cipher,
            key,
            session_id,
            prefix,
            next: Some(0),
        }
    }

    pub fn encrypt(&mut self, message: &[u8]) -> Result<Vec<u8>, Error> {
        let size = u32::try_from(message.len()).map_err(|_| Error)?;
        if size == 0 {
            return Err(Error);
        }
        let capacity = HEADER_LEN.checked_add(message.len()).ok_or(Error)?;
        let count = self.next.ok_or(Error)?;
        self.next = count.checked_add(1);
        let mut header = [0u8; HEADER_LEN];
        header[..4].copy_from_slice(b"\xfdSMB");
        header[20..23].copy_from_slice(&self.prefix);
        header[23..31].copy_from_slice(&count.to_le_bytes());
        header[36..40].copy_from_slice(&size.to_le_bytes());
        header[42..44].copy_from_slice(&1u16.to_le_bytes());
        header[44..52].copy_from_slice(&self.session_id.to_le_bytes());
        let mut encrypted = vec![0; message.len()];
        let mut tag = [0; 16];
        match self.cipher {
            Cipher::Aes128Gcm => AesGcm::new(&self.key).encrypt(
                header[20..32].try_into().map_err(|_| Error)?,
                &header[20..],
                message,
                &mut encrypted,
                &mut tag,
            ),
            Cipher::Aes128Ccm => {
                encrypted.copy_from_slice(message);
                tag.copy_from_slice(&AesCcm::new(&self.key, 16).encrypt(
                    &header[20..31],
                    &header[20..],
                    &mut encrypted,
                ));
            }
        }
        header[4..20].copy_from_slice(&tag);
        let mut result = Vec::with_capacity(capacity);
        result.extend_from_slice(&header);
        result.extend_from_slice(&encrypted);
        Ok(result)
    }

    pub fn decrypt(&self, frame: &[u8]) -> Result<Vec<u8>, Error> {
        if frame.len() <= HEADER_LEN || &frame[..4] != b"\xfdSMB" {
            return Err(Error);
        }
        let size = u32::from_le_bytes(frame[36..40].try_into().map_err(|_| Error)?) as usize;
        if size != frame.len() - HEADER_LEN
            || frame[42..44] != [1, 0]
            || u64::from_le_bytes(frame[44..52].try_into().map_err(|_| Error)?) != self.session_id
        {
            return Err(Error);
        }
        let tag: &[u8; 16] = frame[4..20].try_into().map_err(|_| Error)?;
        let mut plain = vec![0; size];
        let valid = match self.cipher {
            Cipher::Aes128Gcm => AesGcm::new(&self.key).decrypt(
                frame[20..32].try_into().map_err(|_| Error)?,
                &frame[20..52],
                &frame[52..],
                tag,
                &mut plain,
            ),
            Cipher::Aes128Ccm => {
                plain.copy_from_slice(&frame[52..]);
                AesCcm::new(&self.key, 16).decrypt(&frame[20..31], &frame[20..52], &mut plain, tag)
            }
        };
        if valid {
            Ok(plain)
        } else {
            Err(Error)
        }
    }
}
