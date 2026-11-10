//! Md5 message digest, specified in RFC 1321.
//! Provided for protocol interoperability.

pub struct Md5 {
    state: [u32; 4],
    buffer: [u8; 64],
    used: usize,
    length: u64,
}

impl Default for Md5 {
    fn default() -> Self {
        Self::new()
    }
}

impl Md5 {
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
        const T: [u32; 64] = [
            0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
            0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
            0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
            0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
            0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
            0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
            0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
            0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
            0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
            0xeb86d391,
        ];
        for i in 0..64 {
            let j = i % 16;
            let (f, k, shift) = match i / 16 {
                0 => ((b & c) | (!b & d), j, [7, 12, 17, 22][j % 4]),
                1 => ((b & d) | (c & !d), (5 * j + 1) % 16, [5, 9, 14, 20][j % 4]),
                2 => (b ^ c ^ d, (3 * j + 5) % 16, [4, 11, 16, 23][j % 4]),
                _ => (c ^ (b | !d), (7 * j) % 16, [6, 10, 15, 21][j % 4]),
            };
            let next = b.wrapping_add(
                a.wrapping_add(f)
                    .wrapping_add(words[k])
                    .wrapping_add(T[i])
                    .rotate_left(shift),
            );
            (a, b, c, d) = (d, next, b, c);
        }
        for (state, value) in self.state.iter_mut().zip([a, b, c, d]) {
            *state = state.wrapping_add(value);
        }
    }
}

pub fn md5(data: &[u8]) -> [u8; 16] {
    let mut digest = Md5::new();
    digest.update(data);
    digest.finalize()
}
