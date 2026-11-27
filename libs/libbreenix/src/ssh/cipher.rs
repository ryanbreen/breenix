//! SSH packet protection for one direction:
//! - chacha20-poly1305@openssh.com (draft-ietf-sshm-chacha20-poly1305),
//! - aes256-gcm@openssh.com (RFC 5647 as profiled by OpenSSH),
//! - aes128-ctr with hmac-sha2-256 (RFC 4344, RFC 6668).
//!
//! Every operation works on the packet buffer in place: `seal` turns
//! `length || padding_length || payload || padding` into wire bytes and
//! writes the tag into the space after it, and `open` reverses that after
//! checking the tag.

use crate::crypto::{
    aes::Aes128Ctr,
    chacha20::{chacha20_block, chacha20_xor, ssh_nonce},
    ct::ct_eq,
    gcm::AesGcm,
    hmac::HmacSha256,
    poly1305::poly1305,
};
use crate::{errno::Errno, error::Error};

/// The negotiated encryption algorithm for one direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CipherKind {
    ChaCha20Poly1305,
    Aes256Gcm,
    Aes128Ctr,
}

impl CipherKind {
    /// Algorithm names in our preference order.
    pub const ALL: [CipherKind; 3] = [Self::ChaCha20Poly1305, Self::Aes256Gcm, Self::Aes128Ctr];

    pub fn name(self) -> &'static str {
        match self {
            Self::ChaCha20Poly1305 => "chacha20-poly1305@openssh.com",
            Self::Aes256Gcm => "aes256-gcm@openssh.com",
            Self::Aes128Ctr => "aes128-ctr",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }

    /// Whether the cipher authenticates the packet itself, so the
    /// negotiated MAC is ignored.
    pub fn is_aead(self) -> bool {
        !matches!(self, Self::Aes128Ctr)
    }

    /// Bytes of encryption key, IV and MAC key taken from the key exchange.
    pub fn key_sizes(self) -> (usize, usize, usize) {
        match self {
            // K_1 (payload) then K_2 (length), 32 bytes each.
            Self::ChaCha20Poly1305 => (64, 0, 0),
            Self::Aes256Gcm => (32, 12, 0),
            Self::Aes128Ctr => (16, 16, 32),
        }
    }
}

/// Encryption state for one direction.
pub enum SshCipher {
    Ctr { ctr: Aes128Ctr, mac_key: [u8; 32] },
    Gcm { cipher: AesGcm, nonce: [u8; 12], exhausted: bool },
    ChaCha { main: [u8; 32], header: [u8; 32] },
}

impl SshCipher {
    /// `material` is the encryption key, then the IV, then the MAC key, in
    /// the sizes `kind.key_sizes()` gives.
    pub fn new(kind: CipherKind, material: &[u8]) -> Self {
        let (key, iv, mac) = kind.key_sizes();
        assert!(material.len() >= key + iv + mac, "short SSH key material");
        let part = |start: usize, len: usize| &material[start..start + len];
        match kind {
            CipherKind::ChaCha20Poly1305 => Self::ChaCha {
                main: part(0, 32).try_into().unwrap(),
                header: part(32, 32).try_into().unwrap(),
            },
            CipherKind::Aes256Gcm => Self::Gcm {
                cipher: AesGcm::new256(part(0, 32).try_into().unwrap()),
                nonce: part(key, 12).try_into().unwrap(),
                exhausted: false,
            },
            CipherKind::Aes128Ctr => Self::Ctr {
                ctr: Aes128Ctr::new(part(0, 16).try_into().unwrap(), part(key, 16).try_into().unwrap()),
                mac_key: part(key + iv, 32).try_into().unwrap(),
            },
        }
    }

    /// The padding multiple. AEAD ciphers leave the length field out of it.
    pub fn block_size(&self) -> usize {
        match self {
            Self::ChaCha { .. } => 8,
            Self::Gcm { .. } | Self::Ctr { .. } => 16,
        }
    }

    /// Whether the 4-byte length field is outside the padded, aligned body.
    pub fn length_is_aad(&self) -> bool {
        !matches!(self, Self::Ctr { .. })
    }

    /// Bytes that must arrive before the packet length can be read.
    pub fn length_bytes(&self) -> usize {
        match self {
            Self::Ctr { .. } => 16,
            Self::Gcm { .. } | Self::ChaCha { .. } => 4,
        }
    }

    pub fn tag_len(&self) -> usize {
        match self {
            Self::Ctr { .. } => 32,
            Self::Gcm { .. } | Self::ChaCha { .. } => 16,
        }
    }

    /// Read the packet length from the first `length_bytes()` received
    /// bytes. Call once per packet: CTR decrypts this block in place and
    /// continues its keystream from it.
    pub fn packet_length(&mut self, seq: u32, head: &mut [u8]) -> u32 {
        let mut length = [head[0], head[1], head[2], head[3]];
        match self {
            Self::Ctr { ctr, .. } => {
                ctr.process(head);
                length.copy_from_slice(&head[..4]);
            }
            Self::Gcm { .. } => {}
            // The ciphertext stays in place: the MAC covers it.
            Self::ChaCha { header, .. } => chacha20_xor(header, 0, &ssh_nonce(seq), &mut length),
        }
        u32::from_be_bytes(length)
    }

    /// Encrypt a whole packet in place and write its tag.
    pub fn seal(&mut self, seq: u32, packet: &mut [u8], tag: &mut [u8]) -> Result<(), Error> {
        match self {
            Self::Ctr { ctr, mac_key } => {
                tag.copy_from_slice(&ctr_mac(mac_key, seq, packet));
                ctr.process(packet);
            }
            Self::Gcm { cipher, nonce, exhausted } => {
                if *exhausted {
                    return Err(Error::Os(Errno::EIO));
                }
                let (length, body) = packet.split_at_mut(4);
                tag.copy_from_slice(&cipher.encrypt_in_place(nonce, length, body));
                advance_nonce(nonce, exhausted);
            }
            Self::ChaCha { main, header } => {
                let nonce = ssh_nonce(seq);
                chacha20_xor(header, 0, &nonce, &mut packet[..4]);
                chacha20_xor(main, 1, &nonce, &mut packet[4..]);
                tag.copy_from_slice(&chacha_mac(main, &nonce, packet));
            }
        }
        Ok(())
    }

    /// Check the tag, then decrypt the packet in place. For CTR the first
    /// `length_bytes()` were already decrypted by `packet_length`.
    pub fn open(&mut self, seq: u32, packet: &mut [u8], tag: &[u8]) -> Result<(), Error> {
        let valid = match self {
            Self::Ctr { ctr, mac_key } => {
                ctr.process(&mut packet[16..]);
                ct_eq(&ctr_mac(mac_key, seq, packet), tag)
            }
            Self::Gcm { cipher, nonce, exhausted } => {
                if *exhausted {
                    return Err(Error::Os(Errno::EIO));
                }
                let tag: &[u8; 16] = tag.try_into().map_err(|_| Error::Os(Errno::EBADMSG))?;
                let (length, body) = packet.split_at_mut(4);
                let valid = cipher.decrypt_in_place(nonce, length, body, tag);
                if valid {
                    advance_nonce(nonce, exhausted);
                }
                valid
            }
            Self::ChaCha { main, header } => {
                let nonce = ssh_nonce(seq);
                let valid = ct_eq(&chacha_mac(main, &nonce, packet), tag);
                if valid {
                    chacha20_xor(header, 0, &nonce, &mut packet[..4]);
                    chacha20_xor(main, 1, &nonce, &mut packet[4..]);
                }
                valid
            }
        };
        if valid { Ok(()) } else { Err(Error::Os(Errno::EBADMSG)) }
    }
}

fn ctr_mac(key: &[u8; 32], seq: u32, packet: &[u8]) -> [u8; 32] {
    let mut mac = HmacSha256::new(key);
    mac.update(&seq.to_be_bytes());
    mac.update(packet);
    mac.finalize()
}

/// Poly1305 keyed by K_1's block 0, over the encrypted length and body.
fn chacha_mac(main: &[u8; 32], nonce: &[u8; 12], packet: &[u8]) -> [u8; 16] {
    let block = chacha20_block(main, 0, nonce);
    poly1305(block[..32].try_into().unwrap(), packet)
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
        let material = [[7u8; 32].as_slice(), &[9u8; 12]].concat();
        let mut sender = SshCipher::new(CipherKind::Aes256Gcm, &material);
        let mut receiver = SshCipher::new(CipherKind::Aes256Gcm, &material);
        for seq in [3, 4] {
            let plain = [0, 0, 0, 16, 4, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
            let mut packet = plain;
            let mut tag = [0u8; 16];
            sender.seal(seq, &mut packet, &mut tag).unwrap();
            assert_eq!(&packet[..4], &plain[..4]);
            for index in [0, 8] {
                let mut corrupt = packet;
                corrupt[index] ^= 1;
                assert!(receiver.open(seq, &mut corrupt, &tag).is_err());
            }
            let mut bad_tag = tag;
            bad_tag[0] ^= 1;
            assert!(receiver.open(seq, &mut packet.clone(), &bad_tag).is_err());
            receiver.open(seq, &mut packet, &tag).unwrap();
            assert_eq!(packet, plain);
        }
        let material = [[7u8; 32].as_slice(), &[255u8; 12]].concat();
        let mut exhausted = SshCipher::new(CipherKind::Aes256Gcm, &material);
        let mut packet = [0; 20];
        let mut tag = [0; 16];
        exhausted.seal(0, &mut packet, &mut tag).unwrap();
        assert!(exhausted.seal(1, &mut packet, &mut tag).is_err());
    }
}
