//! Blocking client for bwm's clipboard socket.
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;

const MAX_PAYLOAD: usize = 16 * 1024 * 1024;

fn request<S: Read + Write>(stream: &mut S, window: u64, op: u32, payload: &[u8]) -> io::Result<Vec<u8>> {
    if payload.len() > MAX_PAYLOAD {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "clipboard payload too large"));
    }
    let mut header = [0u8; 24];
    header[..8].copy_from_slice(b"BCLIP001");
    header[8..12].copy_from_slice(&op.to_le_bytes());
    header[12..16].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    header[16..].copy_from_slice(&window.to_le_bytes());
    stream.write_all(&header)?;
    stream.write_all(payload)?;
    let mut reply = [0u8; 16];
    stream.read_exact(&mut reply)?;
    let status = i32::from_le_bytes(reply[..4].try_into().unwrap());
    if status != 0 { return Err(io::Error::from_raw_os_error(status.saturating_neg())); }
    let len = u32::from_le_bytes(reply[4..8].try_into().unwrap()) as usize;
    if len > MAX_PAYLOAD { return Err(io::Error::new(io::ErrorKind::InvalidData, "clipboard reply too large")); }
    let mut result = vec![0; len];
    stream.read_exact(&mut result)?;
    Ok(result)
}

pub fn set_on<S: Read + Write>(stream: &mut S, window: u64, payload: &[u8]) -> io::Result<()> {
    request(stream, window, 1, payload).map(|_| ())
}
pub fn get_on<S: Read + Write>(stream: &mut S, window: u64) -> io::Result<Vec<u8>> {
    request(stream, window, 2, &[])
}
pub fn set(window: u64, payload: &[u8]) -> io::Result<()> {
    set_on(&mut UnixStream::connect("/run/bwm/clipboard")?, window, payload)
}
pub fn get(window: u64) -> io::Result<Vec<u8>> {
    get_on(&mut UnixStream::connect("/run/bwm/clipboard")?, window)
}
