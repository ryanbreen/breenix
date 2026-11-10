//! AES-128-CCM authenticated encryption (NIST SP 800-38C).

use super::{aes::Aes128, ct::ct_eq};

/// AES-128-CCM with a fixed authentication tag length.
pub struct AesCcm {
    cipher: Aes128,
    tag_len: usize,
}

impl AesCcm {
    /// Create a CCM cipher. Tag lengths must be even and between 4 and 16 bytes.
    pub fn new(key: &[u8; 16], tag_len: usize) -> Self {
        assert!((4..=16).contains(&tag_len) && tag_len % 2 == 0);
        Self { cipher: Aes128::new(key), tag_len }
    }

    fn check(&self, nonce: &[u8], len: usize) -> usize {
        assert!((7..=13).contains(&nonce.len()));
        let q = 15 - nonce.len();
        assert!(q >= 8 || (len as u64) < (1u64 << (8 * q)), "message too long for nonce");
        q
    }

    fn mac(&self, nonce: &[u8], aad: &[u8], plain: &[u8], q: usize) -> [u8; 16] {
        let mut state = [0; 16];
        let mut b0 = [0; 16];
        b0[0] = (if aad.is_empty() { 0 } else { 0x40 }) | (((self.tag_len - 2) / 2) as u8) << 3 | (q - 1) as u8;
        b0[1..1 + nonce.len()].copy_from_slice(nonce);
        b0[16 - q..].copy_from_slice(&(plain.len() as u64).to_be_bytes()[8 - q..]);
        self.absorb(&mut state, &b0);
        if !aad.is_empty() {
            let len = aad.len() as u64;
            let mut prefix = [0; 10];
            let n = if len < 0xff00 {
                prefix[..2].copy_from_slice(&(len as u16).to_be_bytes()); 2
            } else if len <= u32::MAX as u64 {
                prefix[..2].copy_from_slice(&[0xff, 0xfe]);
                prefix[2..6].copy_from_slice(&(len as u32).to_be_bytes()); 6
            } else {
                prefix[..2].copy_from_slice(&[0xff, 0xff]);
                prefix[2..10].copy_from_slice(&len.to_be_bytes()); 10
            };
            let mut block = [0; 16];
            block[..n].copy_from_slice(&prefix[..n]);
            let first = aad.len().min(16 - n);
            block[n..n + first].copy_from_slice(&aad[..first]);
            self.absorb(&mut state, &block);
            self.absorb_bytes(&mut state, &aad[first..]);
        }
        self.absorb_bytes(&mut state, plain);
        state
    }

    fn absorb(&self, state: &mut [u8; 16], block: &[u8; 16]) {
        for (a, b) in state.iter_mut().zip(block) { *a ^= b; }
        self.cipher.encrypt_block(state);
    }

    fn absorb_bytes(&self, state: &mut [u8; 16], bytes: &[u8]) {
        for chunk in bytes.chunks(16) {
            let mut block = [0; 16];
            block[..chunk.len()].copy_from_slice(chunk);
            self.absorb(state, &block);
        }
    }

    fn counter(&self, nonce: &[u8], q: usize, counter: u64) -> [u8; 16] {
        let mut block = [0; 16];
        block[0] = (q - 1) as u8;
        block[1..1 + nonce.len()].copy_from_slice(nonce);
        block[16 - q..].copy_from_slice(&counter.to_be_bytes()[8 - q..]);
        self.cipher.encrypt_block(&mut block);
        block
    }

    fn crypt(&self, nonce: &[u8], q: usize, buf: &mut [u8]) {
        for (i, chunk) in buf.chunks_mut(16).enumerate() {
            let stream = self.counter(nonce, q, i as u64 + 1);
            for (byte, mask) in chunk.iter_mut().zip(stream) { *byte ^= mask; }
        }
    }

    /// Encrypt in place and return the authentication tag. Nonces must be unique per key.
    pub fn encrypt(&self, nonce: &[u8], aad: &[u8], buf: &mut [u8]) -> Vec<u8> {
        let q = self.check(nonce, buf.len());
        let mac = self.mac(nonce, aad, buf, q);
        let s0 = self.counter(nonce, q, 0);
        let tag = (0..self.tag_len).map(|i| mac[i] ^ s0[i]).collect();
        self.crypt(nonce, q, buf);
        tag
    }

    /// Decrypt in place, clearing the buffer if authentication fails.
    pub fn decrypt(&self, nonce: &[u8], aad: &[u8], buf: &mut [u8], tag: &[u8]) -> bool {
        let q = self.check(nonce, buf.len());
        self.crypt(nonce, q, buf);
        let mac = self.mac(nonce, aad, buf, q);
        let s0 = self.counter(nonce, q, 0);
        let mut expected = [0; 16];
        for i in 0..self.tag_len { expected[i] = mac[i] ^ s0[i]; }
        if !ct_eq(&expected[..self.tag_len], tag) {
            buf.fill(0);
            return false;
        }
        true
    }
}
