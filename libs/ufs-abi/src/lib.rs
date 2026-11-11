//! Allocation-free wire framing for requests and replies on `/dev/ufs`.
//!
//! Integers are little endian, with no padding. Request headers contain id (u64),
//! operation (u16), node (u64), and payload length (u32); reply headers contain
//! id (u64), errno (i32), and payload length (u32). Operation-specific payloads
//! are opaque to this framing layer.
#![no_std]

pub mod payload;

pub const MAX_DATA: usize = 64 * 1024;
pub const REQUEST_HEADER: usize = 22;
pub const REPLY_HEADER: usize = 16;
pub const MAX_MESSAGE: usize = REQUEST_HEADER + MAX_DATA;
pub const NAME_MAX: usize = 255;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum Op {
    Lookup = 1,
    Getattr,
    Setattr,
    Readdir,
    Open,
    Read,
    Write,
    Create,
    Mkdir,
    Unlink,
    Rmdir,
    Rename,
    Fsync,
    Statfs,
    Release,
    Parent,
}

impl TryFrom<u16> for Op {
    type Error = Error;
    fn try_from(value: u16) -> Result<Self, Error> {
        match value {
            1 => Ok(Self::Lookup),
            2 => Ok(Self::Getattr),
            3 => Ok(Self::Setattr),
            4 => Ok(Self::Readdir),
            5 => Ok(Self::Open),
            6 => Ok(Self::Read),
            7 => Ok(Self::Write),
            8 => Ok(Self::Create),
            9 => Ok(Self::Mkdir),
            10 => Ok(Self::Unlink),
            11 => Ok(Self::Rmdir),
            12 => Ok(Self::Rename),
            13 => Ok(Self::Fsync),
            14 => Ok(Self::Statfs),
            15 => Ok(Self::Release),
            16 => Ok(Self::Parent),
            _ => Err(Error),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Request<'a> {
    pub id: u64,
    pub op: Op,
    pub node: u64,
    pub payload: &'a [u8],
}
#[derive(Debug, PartialEq, Eq)]
pub struct Reply<'a> {
    pub id: u64,
    pub errno: i32,
    pub payload: &'a [u8],
}
#[derive(Debug, PartialEq, Eq)]
pub struct Error;

fn check_length(input: &[u8], header: usize, length_offset: usize) -> Result<&[u8], Error> {
    if input.len() < header || input.len() > header + MAX_DATA {
        return Err(Error);
    }
    let length = u32::from_le_bytes(
        input[length_offset..length_offset + 4]
            .try_into()
            .map_err(|_| Error)?,
    ) as usize;
    if length != input.len() - header {
        return Err(Error);
    }
    Ok(&input[header..])
}

impl Request<'_> {
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, Error> {
        if self.payload.len() > MAX_DATA || out.len() < REQUEST_HEADER + self.payload.len() {
            return Err(Error);
        }
        let len = REQUEST_HEADER + self.payload.len();
        out[..8].copy_from_slice(&self.id.to_le_bytes());
        out[8..10].copy_from_slice(&(self.op as u16).to_le_bytes());
        out[10..18].copy_from_slice(&self.node.to_le_bytes());
        out[18..22].copy_from_slice(&(self.payload.len() as u32).to_le_bytes());
        out[REQUEST_HEADER..len].copy_from_slice(self.payload);
        Ok(len)
    }
    pub fn decode(input: &[u8]) -> Result<Request<'_>, Error> {
        let payload = check_length(input, REQUEST_HEADER, 18)?;
        Ok(Request {
            id: u64::from_le_bytes(input[..8].try_into().map_err(|_| Error)?),
            op: Op::try_from(u16::from_le_bytes(
                input[8..10].try_into().map_err(|_| Error)?,
            ))?,
            node: u64::from_le_bytes(input[10..18].try_into().map_err(|_| Error)?),
            payload,
        })
    }
}
impl Reply<'_> {
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, Error> {
        if self.payload.len() > MAX_DATA || out.len() < REPLY_HEADER + self.payload.len() {
            return Err(Error);
        }
        let len = REPLY_HEADER + self.payload.len();
        out[..8].copy_from_slice(&self.id.to_le_bytes());
        out[8..12].copy_from_slice(&self.errno.to_le_bytes());
        out[12..16].copy_from_slice(&(self.payload.len() as u32).to_le_bytes());
        out[REPLY_HEADER..len].copy_from_slice(self.payload);
        Ok(len)
    }
    pub fn decode(input: &[u8]) -> Result<Reply<'_>, Error> {
        let payload = check_length(input, REPLY_HEADER, 12)?;
        Ok(Reply {
            id: u64::from_le_bytes(input[..8].try_into().map_err(|_| Error)?),
            errno: i32::from_le_bytes(input[8..12].try_into().map_err(|_| Error)?),
            payload,
        })
    }
}
