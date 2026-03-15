//! SSH cipher operations after key exchange
//!
//! Wraps AES-128-CTR encryption and HMAC-SHA256 MAC computation for
//! the SSH binary packet protocol.

use crate::crypto::aes::Aes128Ctr;
use crate::crypto::hmac::hmac_sha256;

/// Encryption and MAC state for one direction of an SSH connection.
pub struct SshCipher {
    /// AES-128-CTR cipher instance.
    ctr: Aes128Ctr,
    /// HMAC-SHA256 integrity key.
    mac_key: [u8; 32],
}

impl SshCipher {
    /// Create a new cipher instance.
    ///
    /// # Arguments
    /// * `enc_key` - 16-byte AES-128 encryption key
    /// * `iv` - 16-byte initial counter value
    /// * `mac_key` - 32-byte HMAC-SHA256 integrity key
    pub fn new(enc_key: &[u8; 16], iv: &[u8; 16], mac_key: &[u8; 32]) -> Self {
        Self {
            ctr: Aes128Ctr::new(enc_key, iv),
            mac_key: *mac_key,
        }
    }

    /// Encrypt data in place using AES-128-CTR.
    pub fn encrypt(&mut self, data: &mut [u8]) {
        self.ctr.process(data);
    }

    /// Decrypt data in place using AES-128-CTR.
    ///
    /// CTR mode is its own inverse, so this is the same as encrypt.
    pub fn decrypt(&mut self, data: &mut [u8]) {
        self.ctr.process(data);
    }

    /// Compute MAC for an outgoing packet.
    ///
    /// MAC = HMAC-SHA256(mac_key, sequence_number || unencrypted_packet)
    pub fn compute_mac(&self, seq: u32, unencrypted_packet: &[u8]) -> Vec<u8> {
        let mut mac_data = Vec::with_capacity(4 + unencrypted_packet.len());
        mac_data.extend_from_slice(&seq.to_be_bytes());
        mac_data.extend_from_slice(unencrypted_packet);
        hmac_sha256(&self.mac_key, &mac_data).to_vec()
    }

    /// Compute expected MAC for an incoming packet (for verification).
    ///
    /// Same computation as compute_mac — the caller compares the result
    /// with the received MAC.
    pub fn verify_mac(&self, seq: u32, unencrypted_packet: &[u8]) -> Vec<u8> {
        self.compute_mac(seq, unencrypted_packet)
    }
}
