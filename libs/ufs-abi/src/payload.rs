//! Operation payload primitives; see docs/fs/ufs.md for the bridge contract.
use crate::{Error, NAME_MAX};

pub const ROOT: u64 = 1;
pub const S_IFDIR: u32 = 0o040000;
pub const S_IFREG: u32 = 0o100000;
pub const MAGIC: i64 = 0x55465331;
pub const REQUEST_TIMEOUT_MS: u64 = 5_000;
/// A write's handle and offset occupy the first 16 payload bytes.
pub const MAX_WRITE: usize = crate::MAX_DATA - 16;

pub struct Reader<'a> {
    bytes: &'a [u8],
}
impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self { Self { bytes } }
    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        if n > self.bytes.len() { return Err(Error); }
        let (head, tail) = self.bytes.split_at(n);
        self.bytes = tail;
        Ok(head)
    }
    pub fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().map_err(|_| Error)?))
    }
    pub fn u64(&mut self) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().map_err(|_| Error)?))
    }
    pub fn name(&mut self) -> Result<&'a [u8], Error> {
        let n = u16::from_le_bytes(self.take(2)?.try_into().map_err(|_| Error)?) as usize;
        let name = self.take(n)?;
        valid_name(name)?;
        Ok(name)
    }
    pub fn rest(&mut self) -> &'a [u8] {
        let rest = self.bytes;
        self.bytes = &[];
        rest
    }
    pub fn finish(self) -> Result<(), Error> {
        if self.bytes.is_empty() { Ok(()) } else { Err(Error) }
    }
}

pub struct Writer<'a> {
    bytes: &'a mut [u8],
    used: usize,
}
impl<'a> Writer<'a> {
    pub fn new(bytes: &'a mut [u8]) -> Self { Self { bytes, used: 0 } }
    pub fn len(&self) -> usize { self.used }
    pub fn is_empty(&self) -> bool { self.used == 0 }
    pub fn bytes(&mut self, bytes: &[u8]) -> Result<(), Error> {
        if bytes.len() > self.bytes.len() - self.used { return Err(Error); }
        self.bytes[self.used..self.used + bytes.len()].copy_from_slice(bytes);
        self.used += bytes.len();
        Ok(())
    }
    pub fn u32(&mut self, n: u32) -> Result<(), Error> { self.bytes(&n.to_le_bytes()) }
    pub fn u64(&mut self, n: u64) -> Result<(), Error> { self.bytes(&n.to_le_bytes()) }
    pub fn name(&mut self, name: &[u8]) -> Result<(), Error> {
        valid_name(name)?;
        if name.len() + 2 > self.bytes.len() - self.used { return Err(Error); }
        self.bytes(&(name.len() as u16).to_le_bytes())?;
        self.bytes(name)
    }
}
fn valid_name(name: &[u8]) -> Result<(), Error> {
    if name.is_empty() || name.len() > NAME_MAX || name == b"." || name == b".."
        || name.contains(&0) || name.contains(&b'/') { Err(Error) } else { Ok(()) }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Attr {
    pub node: u64,
    pub size: u64,
    pub mode: u32,
    pub nlink: u32,
    pub uid: u32,
    pub gid: u32,
    pub atime_ns: u64,
    pub mtime_ns: u64,
}
impl Attr {
    pub const SIZE: usize = 48;
    pub fn encode(self, out: &mut [u8]) -> Result<usize, Error> {
        let mut w = Writer::new(out);
        w.u64(self.node)?;
        w.u64(self.size)?;
        w.u32(self.mode)?;
        w.u32(self.nlink)?;
        w.u32(self.uid)?;
        w.u32(self.gid)?;
        w.u64(self.atime_ns)?;
        w.u64(self.mtime_ns)?;
        Ok(w.len())
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Reader::new(bytes);
        let attr = Self { node: r.u64()?, size: r.u64()?, mode: r.u32()?, nlink: r.u32()?,
            uid: r.u32()?, gid: r.u32()?, atime_ns: r.u64()?, mtime_ns: r.u64()? };
        r.finish()?;
        Ok(attr)
    }
}

/// Setattr timestamp form. The mask leaves omitted times untouched by the daemon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetTimes {
    pub atime_ns: Option<u64>,
    pub mtime_ns: Option<u64>,
}
impl SetTimes {
    pub const SIZE: usize = 20;
    pub fn encode(self, out: &mut [u8]) -> Result<usize, Error> {
        let mut w = Writer::new(out);
        w.u32(u32::from(self.atime_ns.is_some()) | (u32::from(self.mtime_ns.is_some()) << 1))?;
        w.u64(self.atime_ns.unwrap_or(0))?;
        w.u64(self.mtime_ns.unwrap_or(0))?;
        Ok(w.len())
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Reader::new(bytes);
        let mask = r.u32()?;
        let atime = r.u64()?;
        let mtime = r.u64()?;
        r.finish()?;
        if mask & !3 != 0 { return Err(Error); }
        Ok(Self { atime_ns: (mask & 1 != 0).then_some(atime), mtime_ns: (mask & 2 != 0).then_some(mtime) })
    }
}
