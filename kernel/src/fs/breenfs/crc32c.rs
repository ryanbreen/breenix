//! CRC-32C, with a streaming interface.

const fn table() -> [u32; 256] {
    let mut entries = [0; 256];
    let mut i = 0;
    while i < entries.len() {
        let mut value = i as u32;
        let mut bit = 0;
        while bit < 8 {
            value = (value >> 1) ^ if value & 1 != 0 { 0x82f6_3b78 } else { 0 };
            bit += 1;
        }
        entries[i] = value;
        i += 1;
    }
    entries
}

const TABLE: [u32; 256] = table();

/// An unfinished CRC-32C calculation.
pub struct Crc32c(u32);

impl Crc32c {
    pub const fn new() -> Self {
        Self(0xffff_ffff)
    }

    pub fn update(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 = TABLE[((self.0 ^ u32::from(byte)) & 0xff) as usize] ^ (self.0 >> 8);
        }
    }

    pub const fn finish(&self) -> u32 {
        self.0 ^ 0xffff_ffff
    }
}

impl Default for Crc32c {
    fn default() -> Self {
        Self::new()
    }
}

pub fn checksum(bytes: &[u8]) -> u32 {
    let mut crc = Crc32c::new();
    crc.update(bytes);
    crc.finish()
}
