//! Md4 message digest, specified in RFC 1320.
//! Provided for protocol interoperability.

pub struct Md4 {
    state: [u32; 4],
    buffer: [u8; 64],
    used: usize,
    length: u64,
}

impl Default for Md4 {
    fn default() -> Self {
        Self::new()
    }
}

impl Md4 {
    pub fn new() -> Self {
        Self {
            state: [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476],
            buffer: [0; 64],
            used: 0,
            length: 0,
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.length = self.length.wrapping_add(data.len() as u64);
        while !data.is_empty() {
            let count = (64 - self.used).min(data.len());
            self.buffer[self.used..self.used + count].copy_from_slice(&data[..count]);
            self.used += count;
            data = &data[count..];
            if self.used == 64 {
                let block = self.buffer;
                self.compress(&block);
                self.used = 0;
            }
        }
    }

    pub fn finalize(mut self) -> [u8; 16] {
        let bits = self.length.wrapping_mul(8).to_le_bytes();
        let mut padding = [0u8; 64];
        padding[0] = 0x80;
        let pad_len = if self.used < 56 {
            56 - self.used
        } else {
            120 - self.used
        };
        self.update(&padding[..pad_len]);
        self.update(&bits);
        let mut result = [0u8; 16];
        for (out, word) in result.chunks_exact_mut(4).zip(self.state) {
            out.copy_from_slice(&word.to_le_bytes());
        }
        result
    }

    fn compress(&mut self, block: &[u8; 64]) {
        let mut words = [0u32; 16];
        for (word, chunk) in words.iter_mut().zip(block.chunks_exact(4)) {
            *word = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }
        let [mut a, mut b, mut c, mut d] = self.state;
        const ORDER3: [usize; 16] = [0, 8, 4, 12, 2, 10, 6, 14, 1, 9, 5, 13, 3, 11, 7, 15];
        for i in 0..48 {
            let round = i / 16;
            let j = i % 16;
            let (f, k, shift, constant) = match round {
                0 => ((b & c) | (!b & d), j, [3, 7, 11, 19][j % 4], 0),
                1 => (
                    (b & c) | (b & d) | (c & d),
                    (j % 4) * 4 + j / 4,
                    [3, 5, 9, 13][j % 4],
                    0x5a827999,
                ),
                _ => (b ^ c ^ d, ORDER3[j], [3, 9, 11, 15][j % 4], 0x6ed9eba1),
            };
            let next = a
                .wrapping_add(f)
                .wrapping_add(words[k])
                .wrapping_add(constant)
                .rotate_left(shift);
            (a, b, c, d) = (d, next, b, c);
        }
        for (state, value) in self.state.iter_mut().zip([a, b, c, d]) {
            *state = state.wrapping_add(value);
        }
    }
}

pub fn md4(data: &[u8]) -> [u8; 16] {
    let mut digest = Md4::new();
    digest.update(data);
    digest.finalize()
}
