//! AES-128-CMAC as specified in RFC 4493.

use super::aes::Aes128;

fn double(block: [u8; 16]) -> [u8; 16] {
    let mut result = [0; 16];
    let mut carry = 0;
    for i in (0..16).rev() {
        result[i] = (block[i] << 1) | carry;
        carry = block[i] >> 7;
    }
    result[15] ^= 0x87 & 0u8.wrapping_sub(carry);
    result
}

fn derive_subkeys(cipher: &Aes128) -> ([u8; 16], [u8; 16]) {
    let mut zero = [0; 16];
    cipher.encrypt_block(&mut zero);
    let k1 = double(zero);
    (k1, double(k1))
}

#[cfg(test)]
pub(super) fn subkeys(key: &[u8; 16]) -> ([u8; 16], [u8; 16]) {
    derive_subkeys(&Aes128::new(key))
}

/// Streaming AES-128-CMAC computation.
pub struct AesCmac {
    cipher: Aes128,
    state: [u8; 16],
    buffer: [u8; 16],
    buffered: usize,
}

impl AesCmac {
    /// Start a MAC using a 128-bit key.
    pub fn new(key: &[u8; 16]) -> Self {
        Self {
            cipher: Aes128::new(key),
            state: [0; 16],
            buffer: [0; 16],
            buffered: 0,
        }
    }

    /// Add message bytes, retaining the final block for finalization.
    pub fn update(&mut self, mut input: &[u8]) {
        while !input.is_empty() {
            if self.buffered == 16 {
                for i in 0..16 {
                    self.state[i] ^= self.buffer[i];
                }
                self.cipher.encrypt_block(&mut self.state);
                self.buffered = 0;
            }
            let count = input.len().min(16 - self.buffered);
            self.buffer[self.buffered..self.buffered + count].copy_from_slice(&input[..count]);
            self.buffered += count;
            input = &input[count..];
        }
    }

    /// Finish the MAC and return its 128-bit tag.
    pub fn finalize(mut self) -> [u8; 16] {
        let (k1, k2) = derive_subkeys(&self.cipher);
        let subkey = if self.buffered == 16 {
            k1
        } else {
            self.buffer[self.buffered] = 0x80;
            self.buffer[self.buffered + 1..].fill(0);
            k2
        };
        for (i, byte) in subkey.iter().enumerate() {
            self.state[i] ^= self.buffer[i] ^ byte;
        }
        self.cipher.encrypt_block(&mut self.state);
        self.state
    }
}

/// Compute AES-128-CMAC in one call.
pub fn aes_cmac(key: &[u8; 16], message: &[u8]) -> [u8; 16] {
    let mut mac = AesCmac::new(key);
    mac.update(message);
    mac.finalize()
}
