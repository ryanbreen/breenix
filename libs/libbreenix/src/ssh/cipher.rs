//! SSH AES-CTR/HMAC (RFC 4253) and AES-256-GCM (RFC 5647).

use crate::crypto::{aes::Aes128Ctr, gcm::AesGcm, hmac::hmac_sha256};
use crate::{errno::Errno, error::Error};

/// Encryption state for one direction; GCM authenticates with its encryption key.
pub enum SshCipher {
    Ctr { ctr: Aes128Ctr, mac_key: [u8; 32] },
    Gcm { cipher: AesGcm, nonce: [u8; 12], exhausted: bool },
}

impl SshCipher {
    pub fn new(key: &[u8; 16], iv: &[u8; 16], mac_key: &[u8; 32]) -> Self {
        Self::Ctr { ctr: Aes128Ctr::new(key, iv), mac_key: *mac_key }
    }

    pub fn new_gcm(key: &[u8; 32], iv: &[u8; 12]) -> Self {
        Self::Gcm { cipher: AesGcm::new256(key), nonce: *iv, exhausted: false }
    }

    pub fn is_gcm(&self) -> bool { matches!(self, Self::Gcm { .. }) }

    /// CTR operates on complete blocks, including the encrypted length field.
    pub fn decrypt(&mut self, data: &mut [u8]) {
        if let Self::Ctr { ctr, .. } = self { ctr.process(data); }
    }

    /// Encrypt one whole packet and return its integrity bytes.
    pub fn seal(&mut self, seq: u32, packet: &mut [u8]) -> Result<Vec<u8>, Error> {
        match self {
            Self::Ctr { ctr, mac_key } => {
                let tag = ctr_mac(mac_key, seq, packet);
                ctr.process(packet);
                Ok(tag)
            }
            Self::Gcm { cipher, nonce, exhausted } => {
                if *exhausted { return Err(Error::Os(Errno::EIO)); }
                let mut ciphertext = vec![0; packet.len() - 4];
                let mut tag = [0; 16];
                cipher.encrypt(nonce, &packet[..4], &packet[4..], &mut ciphertext, &mut tag);
                packet[4..].copy_from_slice(&ciphertext);
                advance_nonce(nonce, exhausted);
                Ok(tag.to_vec())
            }
        }
    }

    /// Authenticate before exposing GCM plaintext. CTR bytes are already decrypted.
    pub fn open(&mut self, seq: u32, packet: &mut [u8], tag: &[u8]) -> Result<(), Error> {
        let valid = match self {
            Self::Ctr { mac_key, .. } => constant_eq(&ctr_mac(mac_key, seq, packet), tag),
            Self::Gcm { cipher, nonce, exhausted } => {
                if *exhausted { return Err(Error::Os(Errno::EIO)); }
                let tag: &[u8; 16] = tag.try_into().map_err(|_| Error::Os(Errno::EBADMSG))?;
                let mut plaintext = vec![0; packet.len() - 4];
                if !cipher.decrypt(nonce, &packet[..4], &packet[4..], tag, &mut plaintext) {
                    return Err(Error::Os(Errno::EBADMSG));
                }
                packet[4..].copy_from_slice(&plaintext);
                advance_nonce(nonce, exhausted);
                true
            }
        };
        if valid { Ok(()) } else { Err(Error::Os(Errno::EBADMSG)) }
    }
}

fn ctr_mac(key: &[u8; 32], seq: u32, packet: &[u8]) -> Vec<u8> {
    let mut data = Vec::with_capacity(4 + packet.len());
    data.extend_from_slice(&seq.to_be_bytes());
    data.extend_from_slice(packet);
    hmac_sha256(key, &data).to_vec()
}

fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |diff, (a, b)| diff | (a ^ b)) == 0
}

/// RFC 5647 section 7.1: increment only the 64-bit invocation field.
/// Refuse further packets when it wraps, rather than ever reuse a nonce.
fn advance_nonce(nonce: &mut [u8; 12], exhausted: &mut bool) {
    let counter = u64::from_be_bytes(nonce[4..].try_into().unwrap());
    *exhausted = counter == u64::MAX;
    nonce[4..].copy_from_slice(&counter.wrapping_add(1).to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gcm_authenticates_length_ciphertext_and_nonce() {
        let key = [7; 32];
        let iv = [9; 12];
        let mut sender = SshCipher::new_gcm(&key, &iv);
        let mut receiver = SshCipher::new_gcm(&key, &iv);
        for seq in [3, 4] {
            let plain = [0, 0, 0, 16, 4, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
            let mut packet = plain;
            let tag = sender.seal(seq, &mut packet).unwrap();
            assert_eq!(&packet[..4], &plain[..4]);
            for index in [0, 8] {
                let mut corrupt = packet;
                corrupt[index] ^= 1;
                assert!(receiver.open(seq, &mut corrupt, &tag).is_err());
            }
            let mut bad_tag = tag.clone();
            bad_tag[0] ^= 1;
            assert!(receiver.open(seq, &mut packet.clone(), &bad_tag).is_err());
            receiver.open(seq, &mut packet, &tag).unwrap();
            assert_eq!(packet, plain);
        }
        let mut exhausted = SshCipher::new_gcm(&key, &[255; 12]);
        let mut packet = [0; 20];
        exhausted.seal(0, &mut packet).unwrap();
        assert!(exhausted.seal(1, &mut packet).is_err());
    }
}
