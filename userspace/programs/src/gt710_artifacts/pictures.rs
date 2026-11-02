//! G28: failing pictures and one sample per category, in native RLE or canonical raw pixels.
use libbreenix::crypto::sha256::sha256;
use std::fmt::Write;

pub(super) const MAX_UPLOAD: usize = 2 << 20;
// G28 preserves the 64 MiB per-boot limit and 2 MiB upload slots.
pub(super) const MAX_TOTAL: usize = 64 << 20;
const MAX_PICTURES: usize = 401;

#[derive(Clone, Copy)]
struct Entry {
    name: [u8; 80],
    name_len: usize,
    sha256: [u8; 32],
    bytes: usize,
    encoding: &'static str,
    width: usize,
    height: usize,
}
impl Entry {
    const EMPTY: Self = Self { name: [0; 80], name_len: 0, sha256: [0; 32], bytes: 0,
        encoding: "", width: 0, height: 0 };
    fn case(&self) -> &str { std::str::from_utf8(&self.name[..self.name_len]).unwrap() }
}

pub(super) struct Pictures {
    entries: Vec<Entry>,
    used: usize,
    buffer: Vec<u8>,
    pub(super) bytes: usize,
    pub(super) unique: usize,
    bad: bool,
}

// Formatting a run must not allocate a one-page malloc block per token.
struct Token { bytes: [u8; 32], used: usize }
impl std::fmt::Write for Token {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        let end = self.used.checked_add(text.len()).filter(|&n| n <= self.bytes.len()).ok_or(std::fmt::Error)?;
        self.bytes[self.used..end].copy_from_slice(text.as_bytes());self.used = end;Ok(())
    }
}
fn append(buffer: &mut [u8], used: &mut usize, data: &[u8],
    send: &mut impl FnMut(&[u8]) -> Result<(), String>) -> Result<(), String> {
    if *used + data.len() > buffer.len() { send(&buffer[..*used])?;*used = 0; }
    buffer[*used..*used + data.len()].copy_from_slice(data);*used += data.len();Ok(())
}

impl Pictures {
    pub(super) const fn new() -> Self {
        Self { entries: Vec::new(), used: 0, buffer: Vec::new(), bytes: 0, unique: 0, bad: false }
    }
    /// Initialize the bounded owner before the memory baseline, including its
    /// first-touch pages. Reuse the encoding buffer instead of growing RLE strings.
    pub(super) fn prepare(&mut self) -> Result<(), String> {
        if self.used != 0 { return Err("picture preparation after recording".into()); }
        if self.entries.is_empty() {
            self.entries.try_reserve_exact(MAX_PICTURES).map_err(|_| "picture manifest allocation")?;
            self.entries.resize(MAX_PICTURES, Entry::EMPTY);
        }
        if self.buffer.is_empty() {
            self.buffer.try_reserve_exact(MAX_UPLOAD).map_err(|_| "picture encoding allocation")?;
            self.buffer.resize(MAX_UPLOAD, 0);
        }
        Ok(())
    }

    pub(super) fn record(
        &mut self, case: &str, failed: bool, width: usize, height: usize, pixels: &[u8], digest: [u8; 32],
        send: &mut impl FnMut(&[u8]) -> Result<(), String>,
    ) -> Result<(), String> {
        // A failed record remains a refusal, even if a caller tries to finish.
        if self.bad { return Err("picture sequence already refused".into()); }
        if self.entries.is_empty() || self.buffer.is_empty() {
            self.bad = true;self.prepare()?;self.bad = false;
        }
        let category = case.split('/').next().unwrap_or("");
        if !failed && self.entries[..self.used].iter().any(|e| e.case().split('/').next() == Some(category)) {
            return Ok(());
        }
        self.bad = true;
        if case.is_empty() || case.len() > 80
            || !case.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_/.".contains(&b))
            || self.used >= MAX_PICTURES || self.entries[..self.used].iter().any(|e| e.case() == case)
            || width == 0 || height == 0 || width > 16384 || height > 16384
            || width.checked_mul(height).and_then(|n| n.checked_mul(4)) != Some(pixels.len())
            || pixels.chunks_exact(4).any(|p| p[3] != 255) || sha256(pixels) != digest {
            return Err("invalid judged picture identity, dimensions or digest".into());
        }
        let (bytes, encoding) = if let Some(e) = self.entries[..self.used].iter().find(|e| e.sha256 == digest) {
            (e.bytes, e.encoding)
        } else {
            // Count without allocating. Stop once raw is no larger than RLE.
            let mut at = 0;let mut encoded_bytes = 0;
            while at < pixels.len() {
                let mut end = at + 4;
                while end < pixels.len() && pixels[end..end + 4] == pixels[at..at + 4] { end += 4; }
                let count = (end - at) / 4;
                let digits = (usize::BITS - count.leading_zeros()).div_ceil(4) as usize;
                encoded_bytes += digits + 10; // colon, word, separators and final newline
                if encoded_bytes >= pixels.len() { break; }
                at = end;
            }
            let rle = at == pixels.len() && encoded_bytes < pixels.len();
            let bytes = if rle { encoded_bytes } else { pixels.len() };
            if bytes > MAX_TOTAL.saturating_sub(self.bytes) {
                return Err(format!("picture sequence exceeds 64 MiB bound at {case}"));
            }
            if rle {
                let mut at = 0;let mut used = 0;
                while at < pixels.len() {
                    let word = u32::from_le_bytes(pixels[at..at + 4].try_into().unwrap());
                    let mut end = at + 4;
                    while end < pixels.len() && pixels[end..end + 4] == pixels[at..at + 4] { end += 4; }
                    let mut token = Token { bytes: [0; 32], used: 0 };
                    write!(token, "{}{:x}:{word:08x}", if at == 0 { "" } else { "," }, (end - at) / 4)
                        .map_err(|_| "native run token overflow")?;
                    append(&mut self.buffer, &mut used, &token.bytes[..token.used], send)?;
                    at = end;
                }
                append(&mut self.buffer, &mut used, b"\n", send)?;
                if used != 0 { send(&self.buffer[..used])?; }
            } else {
                for chunk in pixels.chunks(MAX_UPLOAD) { send(chunk)?; }
            }
            self.bytes += bytes;self.unique += 1;
            (bytes, if rle { "rle" } else { "raw" })
        };
        let e = &mut self.entries[self.used];
        e.name[..case.len()].copy_from_slice(case.as_bytes());e.name_len = case.len();
        (e.sha256, e.bytes, e.encoding, e.width, e.height) = (digest, bytes, encoding, width, height);
        self.used += 1;self.bad = false;Ok(())
    }

    pub(super) fn manifest(&self) -> Result<Vec<u8>, String> {
        if self.bad { return Err("incomplete judged picture evidence".into()); }
        let mut text = String::new();
        for (picture, e) in self.entries[..self.used].iter().enumerate() {
            let digest: String = e.sha256.iter().map(|b| format!("{b:02x}")).collect();
            writeln!(text, "GT710_BREENGL_PICTURE {{\"case\":\"{}\",\"picture\":{picture},\"sha256\":\"{digest}\",\"bytes\":{},\"encoding\":\"{}\",\"width\":{},\"height\":{}}}",
                e.case(), e.bytes, e.encoding, e.width, e.height).map_err(|_| "picture manifest formatting")?;
        }
        Ok(text.into_bytes())
    }
}
