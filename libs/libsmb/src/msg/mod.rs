//! Sans-I/O client codecs from Microsoft's MS-SMB2 message syntax.
//!
//! Encoders accept a synchronous header containing the caller's identifiers and
//! signing fields; they set Command and require request direction/status. Sign
//! the returned bytes afterwards. Decoders take one complete SMB message without
//! Direct TCP framing. Compounds and asynchronous headers are outside this API.
//! Authentication tokens and extension payloads remain opaque.

use crate::{u16_at, u32_at, Error, SyncHeader};

mod create;
mod error;
mod ioctl;
mod session;
mod tree;
pub use create::*;
pub use error::*;
pub use ioctl::*;
pub use session::*;
pub use tree::*;

fn request(header: &SyncHeader, command: u16, size: usize) -> Result<Vec<u8>, Error> {
    if header.flags & 3 != 0 || header.status != 0 || header.next_command != 0 {
        return Err(Error);
    }
    let mut header = header.clone();
    header.command = command;
    let mut b = header.encode().to_vec();
    b.resize(64 + size, 0);
    Ok(b)
}
fn response(b: &[u8], command: u16, structure: u16, fixed: usize) -> Result<SyncHeader, Error> {
    let header = SyncHeader::decode(b)?;
    if header.command != command
        || header.flags & 1 == 0
        || header.flags & 0x0fff_ff80 != 0
        || header.next_command != 0
        || b.len() < 64 + fixed
        || u16_at(b, 64)? != structure
    {
        return Err(Error);
    }
    Ok(header)
}
fn success(b: &[u8], command: u16, structure: u16, fixed: usize) -> Result<SyncHeader, Error> {
    let header = response(b, command, structure, fixed)?;
    if header.status != 0 {
        return Err(Error);
    }
    Ok(header)
}
// Empty fields accept offset 0 or an in-buffer position. Validate the latter
// against the supplied message, including for an empty range.
fn buffer(b: &[u8], offset: usize, len: usize, minimum: usize) -> Result<&[u8], Error> {
    if len == 0 && offset == 0 {
        return Ok(&[]);
    }
    if offset < minimum {
        return Err(Error);
    }
    b.get(offset..offset.checked_add(len).ok_or(Error)?)
        .ok_or(Error)
}
fn put16(b: &mut [u8], at: usize, n: u16) {
    b[at..at + 2].copy_from_slice(&n.to_le_bytes());
}
fn put32(b: &mut [u8], at: usize, n: u32) {
    b[at..at + 4].copy_from_slice(&n.to_le_bytes());
}
fn put64(b: &mut [u8], at: usize, n: u64) {
    b[at..at + 8].copy_from_slice(&n.to_le_bytes());
}
fn len16(n: usize) -> Result<u16, Error> {
    n.try_into().map_err(|_| Error)
}
fn len32(n: usize) -> Result<u32, Error> {
    n.try_into().map_err(|_| Error)
}
fn align8(n: usize) -> Result<usize, Error> {
    Ok(n.checked_add(7).ok_or(Error)? & !7)
}
fn utf16(s: &str) -> Result<Vec<u8>, Error> {
    let n = s.encode_utf16().count().checked_mul(2).ok_or(Error)?;
    len16(n)?;
    let mut b = Vec::with_capacity(n);
    for ch in s.encode_utf16() {
        b.extend_from_slice(&ch.to_le_bytes());
    }
    Ok(b)
}
fn empty_request(header: &SyncHeader, command: u16) -> Result<Vec<u8>, Error> {
    let mut b = request(header, command, 4)?;
    put16(&mut b, 64, 4);
    Ok(b)
}
fn empty_response(b: &[u8], command: u16) -> Result<SyncHeader, Error> {
    success(b, command, 4, 4)
}
