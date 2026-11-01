//! G28: failing pictures and one sample per category, in native RLE or canonical raw pixels.
use libbreenix::crypto::sha256::sha256;
use std::fmt::Write;

pub(super) const MAX_UPLOAD: usize = 2 << 20;
// G28 preserves the 64 MiB per-boot limit and 2 MiB upload slots.
pub(super) const MAX_TOTAL: usize = 64 << 20;

struct Entry {
    case: String,
    sha256: [u8; 32],
    bytes: usize,
    encoding: &'static str,
    width: usize,
    height: usize,
}

pub(super) struct Pictures {
    entries: Vec<Entry>,
    pub(super) bytes: usize,
    pub(super) unique: usize,
    bad: bool,
}

impl Pictures {
    pub(super) const fn new() -> Self { Self { entries: Vec::new(), bytes: 0, unique: 0, bad: false } }

    pub(super) fn record(
        &mut self, case: &str, failed: bool, width: usize, height: usize, pixels: &[u8], digest: [u8; 32],
        send: &mut impl FnMut(&[u8]) -> Result<(), String>,
    ) -> Result<(), String> {
        // A failed record remains a refusal, even if a caller tries to finish.
        if self.bad { return Err("picture sequence already refused".into()); }
        let category = case.split('/').next().unwrap_or("");
        if !failed && self.entries.iter().any(|e| e.case.split('/').next() == Some(category)) {
            return Ok(());
        }
        self.bad = true;
        if case.is_empty() || case.len() > 80
            || !case.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_/.".contains(&b))
            || self.entries.len() >= 401 || self.entries.iter().any(|e| e.case == case)
            || width == 0 || height == 0 || width > 16384 || height > 16384
            || width.checked_mul(height).and_then(|n| n.checked_mul(4)) != Some(pixels.len())
            || pixels.chunks_exact(4).any(|p| p[3] != 255) || sha256(pixels) != digest {
            return Err("invalid judged picture identity, dimensions or digest".into());
        }
        let (bytes, encoding) = if let Some(e) = self.entries.iter().find(|e| e.sha256 == digest) {
            (e.bytes, e.encoding)
        } else {
            let mut encoded = String::new();
            let mut at = 0;
            while at < pixels.len() {
                let word = u32::from_le_bytes(pixels[at..at + 4].try_into().unwrap());
                let mut end = at + 4;
                while end < pixels.len() && pixels[end..end + 4] == pixels[at..at + 4] { end += 4; }
                let token = format!("{}{:x}:{word:08x}", if at == 0 { "" } else { "," }, (end - at) / 4);
                // Once RLE cannot be smaller, use the existing canonical raw bytes.
                if encoded.len() + token.len() + 1 >= pixels.len() { break; }
                encoded.try_reserve(token.len()).map_err(|_| "picture encoding allocation")?;
                encoded.push_str(&token);
                at = end;
            }
            let (payload, encoding) = if at == pixels.len() {
                encoded.push('\n');
                (encoded.as_bytes(), "rle")
            } else { (pixels, "raw") };
            if payload.len() > MAX_TOTAL.saturating_sub(self.bytes) {
                return Err(format!("picture sequence exceeds 64 MiB bound at {case}"));
            }
            for chunk in payload.chunks(MAX_UPLOAD) { send(chunk)?; }
            self.bytes += payload.len();
            self.unique += 1;
            (payload.len(), encoding)
        };
        self.entries.try_reserve(1).map_err(|_| "picture manifest allocation")?;
        self.entries.push(Entry { case: case.into(), sha256: digest, bytes, encoding, width, height });
        self.bad = false;
        Ok(())
    }

    pub(super) fn manifest(&self) -> Result<Vec<u8>, String> {
        if self.bad { return Err("incomplete judged picture evidence".into()); }
        let mut text = String::new();
        for (picture, e) in self.entries.iter().enumerate() {
            let digest: String = e.sha256.iter().map(|b| format!("{b:02x}")).collect();
            writeln!(text, "GT710_BREENGL_PICTURE {{\"case\":\"{}\",\"picture\":{picture},\"sha256\":\"{digest}\",\"bytes\":{},\"encoding\":\"{}\",\"width\":{},\"height\":{}}}",
                e.case, e.bytes, e.encoding, e.width, e.height).map_err(|_| "picture manifest formatting")?;
        }
        Ok(text.into_bytes())
    }
}
