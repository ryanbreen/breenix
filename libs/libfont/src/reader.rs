//! Zero-copy big-endian binary reader over `&[u8]`.

use alloc::string::String;

#[derive(Clone)]
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn at(data: &'a [u8], offset: usize) -> Self {
        Self { data, pos: offset }
    }

    pub fn position(&self) -> usize {
        self.pos
    }

    pub fn set_position(&mut self, pos: usize) {
        self.pos = pos;
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    pub fn read_u8(&mut self) -> Result<u8, String> {
        if self.pos >= self.data.len() {
            return Err(String::from("unexpected end of data reading u8"));
        }
        let v = self.data[self.pos];
        self.pos += 1;
        Ok(v)
    }

    pub fn read_i8(&mut self) -> Result<i8, String> {
        self.read_u8().map(|v| v as i8)
    }

    pub fn read_u16(&mut self) -> Result<u16, String> {
        if self.pos + 2 > self.data.len() {
            return Err(String::from("unexpected end of data reading u16"));
        }
        let v = u16::from_be_bytes([self.data[self.pos], self.data[self.pos + 1]]);
        self.pos += 2;
        Ok(v)
    }

    pub fn read_i16(&mut self) -> Result<i16, String> {
        self.read_u16().map(|v| v as i16)
    }

    pub fn read_u32(&mut self) -> Result<u32, String> {
        if self.pos + 4 > self.data.len() {
            return Err(String::from("unexpected end of data reading u32"));
        }
        let v = u32::from_be_bytes([
            self.data[self.pos],
            self.data[self.pos + 1],
            self.data[self.pos + 2],
            self.data[self.pos + 3],
        ]);
        self.pos += 4;
        Ok(v)
    }

    pub fn read_i32(&mut self) -> Result<i32, String> {
        self.read_u32().map(|v| v as i32)
    }

    pub fn read_i64(&mut self) -> Result<i64, String> {
        if self.pos + 8 > self.data.len() {
            return Err(String::from("unexpected end of data reading i64"));
        }
        let v = i64::from_be_bytes([
            self.data[self.pos],
            self.data[self.pos + 1],
            self.data[self.pos + 2],
            self.data[self.pos + 3],
            self.data[self.pos + 4],
            self.data[self.pos + 5],
            self.data[self.pos + 6],
            self.data[self.pos + 7],
        ]);
        self.pos += 8;
        Ok(v)
    }

    pub fn read_tag(&mut self) -> Result<[u8; 4], String> {
        if self.pos + 4 > self.data.len() {
            return Err(String::from("unexpected end of data reading tag"));
        }
        let tag = [
            self.data[self.pos],
            self.data[self.pos + 1],
            self.data[self.pos + 2],
            self.data[self.pos + 3],
        ];
        self.pos += 4;
        Ok(tag)
    }

    pub fn skip(&mut self, n: usize) -> Result<(), String> {
        if self.pos + n > self.data.len() {
            return Err(String::from("unexpected end of data during skip"));
        }
        self.pos += n;
        Ok(())
    }

    pub fn read_bytes(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.pos + n > self.data.len() {
            return Err(String::from("unexpected end of data reading bytes"));
        }
        let slice = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }
}

// Standalone read functions for random access
pub fn read_u16_at(data: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes([data[offset], data[offset + 1]])
}

pub fn read_i16_at(data: &[u8], offset: usize) -> i16 {
    read_u16_at(data, offset) as i16
}

pub fn read_u32_at(data: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}
