//! Little-endian, sector-aligned SCR1 records on a disk marked SCRDISK1.
#![no_std]
extern crate alloc;
use alloc::{string::String, vec::Vec};

pub const HEADER: usize = 80;
pub const DISK_MAGIC: &[u8; 8] = b"SCRDISK1";
pub const REQUEST_MAGIC: &[u8; 8] = b"SCRWRITE";
pub const MAX_PIXELS: usize = 2560 * 1600 * 4;
pub const MAX_RECORD: usize = MAX_PIXELS + 4096;

pub struct Record<'a> {
    pub suite: &'a str, pub case: &'a str, pub checkpoint: &'a str,
    pub width: u32, pub height: u32, pub digest: [u8; 32], pub pixels: &'a [u8],
}
#[derive(Debug)]
pub struct Decoded {
    pub suite: String, pub case: String, pub checkpoint: String,
    pub width: u32, pub height: u32, pub digest: [u8; 32], pub pixels: Vec<u8>,
}
fn u32_at(b: &[u8], n: usize) -> u32 { u32::from_le_bytes(b[n..n+4].try_into().unwrap()) }
fn dimensions(w: u32, h: u32) -> Result<usize, &'static str> {
    (w as usize).checked_mul(h as usize).and_then(|n| n.checked_mul(4))
        .filter(|&n| w > 0 && h > 0 && n <= MAX_PIXELS).ok_or("capture-dimensions")
}
fn text(s: &str, slash: bool) -> bool {
    !s.is_empty() && s.len() <= 255 && s.bytes().all(|b|
        b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || (slash && b == b'/'))
}
pub fn encode(r: &Record<'_>) -> Result<Vec<u8>, &'static str> {
    if dimensions(r.width,r.height)? != r.pixels.len()
        || !text(r.suite,false) || !text(r.case,true) || !text(r.checkpoint,false) {
        return Err("capture-record");
    }
    if r.pixels.chunks_exact(4).any(|p| p[3] != 255) || libpixcheck::sha256(r.pixels) != r.digest {
        return Err("capture-digest");
    }
    let mut runs = 0usize;
    let mut previous: Option<&[u8]> = None;
    for p in r.pixels.chunks_exact(4) {
        if previous != Some(p) { runs += 1; previous = Some(p); }
    }
    let rle = runs * 8 < r.pixels.len();
    let payload = if rle { runs * 8 } else { r.pixels.len() };
    let length = (HEADER + r.suite.len() + r.case.len() + r.checkpoint.len() + payload).div_ceil(512) * 512;
    let mut bytes = alloc::vec![0; length];
    bytes[..4].copy_from_slice(b"SCR1");
    for (at,value) in [(4,length as u32),(8,u32::from(rle)),(12,payload as u32),
        (16,r.width),(20,r.height),(64,r.pixels.len() as u32)] {
        bytes[at..at+4].copy_from_slice(&value.to_le_bytes());
    }
    bytes[32..64].copy_from_slice(&r.digest);
    let mut at = HEADER;
    for (slot,s) in [(24,r.suite),(26,r.case),(28,r.checkpoint)] {
        bytes[slot..slot+2].copy_from_slice(&(s.len() as u16).to_le_bytes());
        bytes[at..at+s.len()].copy_from_slice(s.as_bytes()); at += s.len();
    }
    if rle {
        let mut i = 0;
        while i < r.pixels.len() {
            let p = &r.pixels[i..i+4];
            let mut end = i+4;
            while end < r.pixels.len() && &r.pixels[end..end+4] == p { end += 4; }
            bytes[at..at+4].copy_from_slice(&((end-i) as u32/4).to_le_bytes());
            bytes[at+4..at+8].copy_from_slice(p); at += 8; i = end;
        }
    } else { bytes[at..at+payload].copy_from_slice(r.pixels); }
    Ok(bytes)
}
pub fn decode(bytes: &[u8]) -> Result<(Decoded,usize), &'static str> {
    if bytes.len() < HEADER || &bytes[..4] != b"SCR1" { return Err("capture-magic"); }
    let length = u32_at(bytes,4) as usize;
    let payload = u32_at(bytes,12) as usize;
    let encoding = u32_at(bytes,8);
    let (width,height) = (u32_at(bytes,16),u32_at(bytes,20));
    let raw = dimensions(width,height)?;
    if length < HEADER || length > MAX_RECORD || length % 512 != 0 || bytes.len() < length
        || u32_at(bytes,64) as usize != raw || bytes[30..32] != [0;2]
        || bytes[68..HEADER].iter().any(|&b| b != 0) { return Err("capture-length"); }
    let mut at = HEADER;
    let mut name = |slot,slash| -> Result<String,&'static str> {
        let n = u16::from_le_bytes(bytes[slot..slot+2].try_into().unwrap()) as usize;
        let end = at.checked_add(n).filter(|&end| end <= length).ok_or("capture-text")?;
        let s = core::str::from_utf8(&bytes[at..end]).map_err(|_| "capture-text")?;
        if !text(s,slash) { return Err("capture-text"); } at = end; Ok(s.into())
    };
    let suite = name(24,false)?; let case = name(26,true)?; let checkpoint = name(28,false)?;
    let end = at.checked_add(payload).filter(|&n| n <= length).ok_or("capture-length")?;
    if end.div_ceil(512)*512 != length || bytes[end..length].iter().any(|&b| b != 0) { return Err("capture-padding"); }
    let mut pixels = Vec::new(); pixels.try_reserve_exact(raw).map_err(|_| "capture-allocation")?;
    match encoding {
        0 if payload == raw => pixels.extend_from_slice(&bytes[at..end]),
        1 if payload % 8 == 0 && payload < raw => {
            for run in bytes[at..end].chunks_exact(8) {
                let count = u32_at(run,0) as usize;
                if count == 0 || count > (raw - pixels.len())/4 { return Err("capture-run"); }
                for _ in 0..count { pixels.extend_from_slice(&run[4..8]); }
            }
        }
        _ => return Err("capture-encoding"),
    }
    let digest: [u8;32] = bytes[32..64].try_into().unwrap();
    if pixels.len() != raw || pixels.chunks_exact(4).any(|p| p[3] != 255)
        || libpixcheck::sha256(&pixels) != digest { return Err("capture-digest"); }
    Ok((Decoded { suite,case,checkpoint,width,height,digest,pixels },length))
}

pub struct CaptureLog { next: u64, seen: Vec<[u8;32]>, failed: bool }
impl Default for CaptureLog { fn default() -> Self { Self::new() } }
impl CaptureLog {
    pub const fn new() -> Self { Self { next: 512, seen: Vec::new(), failed: false } }
    pub fn append(&mut self, bytes: &[u8], capacity: u64,
        mut write: impl FnMut(u64,&[u8])->Result<(),&'static str>) -> Result<bool,&'static str> {
        if self.failed { return Err("capture-disk-failed"); }
        let length = bytes.len();
        let digest = validate_stream(length,|at,buf| {
            buf.copy_from_slice(&bytes[at..at+buf.len()]); Ok(())
        },|_,_| Ok(()))?;
        if self.seen.contains(&digest) { return Ok(false); }
        let end = self.next.checked_add(length as u64).filter(|&n| n <= capacity).ok_or("capture-disk-full")?;
        self.seen.try_reserve(1).map_err(|_| "capture-allocation")?;
        if let Err(e) = write(self.next,bytes) { self.failed = true; return Err(e); }
        self.seen.push(digest); self.next = end; Ok(true)
    }
}

/// Validate sector bytes without constructing a second framebuffer.
fn validate_stream(length: usize,
    mut read: impl FnMut(usize,&mut [u8])->Result<(),&'static str>,
    mut consume: impl FnMut(usize,&[u8])->Result<(),&'static str>) -> Result<[u8;32],&'static str> {
    if !(512..=MAX_RECORD).contains(&length) || length % 512 != 0 { return Err("capture-length"); }
    let mut sector = [0;512]; read(0,&mut sector)?;
    if &sector[..4] != b"SCR1" { return Err("capture-magic"); }
    let raw = dimensions(u32_at(&sector,16),u32_at(&sector,20))?;
    let payload = u32_at(&sector,12) as usize;
    let encoding = u32_at(&sector,8);
    let sizes: [usize;3] = [24,26,28].map(|at| u16::from_le_bytes(sector[at..at+2].try_into().unwrap()) as usize);
    if sizes.iter().any(|&n| n == 0 || n > 255) { return Err("capture-text"); }
    let meta_end = HEADER + sizes.iter().sum::<usize>();
    let payload_end = meta_end.checked_add(payload).ok_or("capture-length")?;
    if u32_at(&sector,4) as usize != length || u32_at(&sector,64) as usize != raw
        || sector[30..32] != [0;2] || sector[68..HEADER].iter().any(|&b| b != 0)
        || payload_end > length || payload_end.div_ceil(512)*512 != length {
        return Err("capture-length");
    }
    if !matches!(encoding,0|1) || (encoding == 0 && payload != raw)
        || (encoding == 1 && (payload % 8 != 0 || payload >= raw)) { return Err("capture-encoding"); }
    let expected: [u8;32] = sector[32..64].try_into().unwrap();
    let mut metadata = [0;HEADER+3*255];
    let mut sha = libpixcheck::Sha256::new();
    let mut produced = 0usize;
    let mut run = [0;8]; let mut pending = 0;
    let mut repeated = [0;4096];
    for at in (0..length).step_by(512) {
        if at != 0 { read(at,&mut sector)?; }
        let end = (at+512).min(meta_end);
        if at < end { metadata[at..end].copy_from_slice(&sector[..end-at]); }
        let (start,end) = (at.max(meta_end),(at+512).min(payload_end));
        if start < end {
            let part = &sector[start-at..end-at];
            if encoding == 0 {
                if part.iter().enumerate().any(|(i,&b)| (start-meta_end+i)%4 == 3 && b != 255) {
                    return Err("capture-digest");
                }
                sha.update(part); produced += part.len();
            } else {
                for &byte in part {
                    run[pending] = byte; pending += 1;
                    if pending != 8 { continue; }
                    let count = u32_at(&run,0) as usize;
                    if count == 0 || count > (raw-produced)/4 || run[7] != 255 { return Err("capture-run"); }
                    let tile = count.min(1024)*4;
                    for pixel in repeated[..tile].chunks_exact_mut(4) { pixel.copy_from_slice(&run[4..]); }
                    let mut left = count*4;
                    while left > 0 { let n=left.min(tile); sha.update(&repeated[..n]); left -= n; }
                    produced += count*4; pending = 0;
                }
            }
        }
        let padding = at.max(payload_end);
        if padding < at+512 && sector[padding-at..].iter().any(|&b| b != 0) { return Err("capture-padding"); }
        consume(at,&sector)?;
    }
    let mut at = HEADER;
    for (n,slash) in sizes.into_iter().zip([false,true,false]) {
        let name = core::str::from_utf8(&metadata[at..at+n]).map_err(|_| "capture-text")?;
        if !text(name,slash) { return Err("capture-text"); } at += n;
    }
    if pending != 0 || produced != raw || sha.finish() != expected { return Err("capture-digest"); }
    Ok(expected)
}

impl CaptureLog {
    /// Write payload sectors before committing their first sector and index entry.
    pub fn append_stream(&mut self, length: usize, capacity: u64,
        mut read: impl FnMut(usize,&mut [u8])->Result<(),&'static str>,
        mut write: impl FnMut(u64,&[u8])->Result<(),&'static str>,
        mut flush: impl FnMut()->Result<(),&'static str>) -> Result<bool,&'static str> {
        if self.failed { return Err("capture-disk-failed"); }
        if !(512..=MAX_RECORD).contains(&length) || length%512 != 0 { return Err("capture-length"); }
        let mut first = [0;512]; read(0,&mut first)?;
        let digest: [u8;32] = first[32..64].try_into().unwrap();
        let duplicate = self.seen.contains(&digest);
        let end = self.next.checked_add(length as u64).filter(|&n| duplicate || n <= capacity).ok_or("capture-disk-full")?;
        self.seen.try_reserve(1).map_err(|_| "capture-allocation")?;
        let result = validate_stream(length,|at,buf| {
            if at == 0 { buf.copy_from_slice(&first); Ok(()) } else { read(at,buf) }
        },|at,buf| {
            if duplicate || at == 0 { Ok(()) } else { write(self.next+at as u64,buf) }
        }).and_then(|_| {
            if !duplicate { write(self.next,&first)?; flush()?; }
            Ok(())
        });
        if let Err(e) = result { self.failed = true; return Err(e); }
        if duplicate { return Ok(false); }
        self.seen.push(digest); self.next = end; Ok(true)
    }
}
