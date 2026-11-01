//! Judged pictures use the native hex run-count:32-bit-pixel encoding, unchanged.
use libbreenix::crypto::sha256::sha256;
use std::fmt::Write;

pub(super) const MAX_UPLOAD: usize = 2 << 20;
// R6 amended: provisional per-boot transport bound. The retained ARM capture has
// 163 pictured cases, without a corpus; eight panel fixtures measured 4.35 MB in
// native RLE. 64 MiB gives about 15.4 times that sample, not a full-suite estimate.
// The first native boot records bytes/uploads; revisit the bound before 25.4.
pub(super) const MAX_TOTAL: usize = 64 << 20;

struct Entry {
    case: String,
    sha256: [u8; 32],
    bytes: usize,
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
        &mut self, case: &str, width: usize, height: usize, pixels: &[u8], digest: [u8; 32],
        send: &mut impl FnMut(&[u8]) -> Result<(), String>,
    ) -> Result<(), String> {
        // A failed record remains a refusal, even if a caller tries to finish.
        if self.bad { return Err("picture sequence already refused".into()); }
        self.bad = true;
        if case.is_empty() || case.len() > 80
            || !case.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_/.".contains(&b))
            || self.entries.len() >= 401 || self.entries.iter().any(|e| e.case == case)
            || width == 0 || height == 0 || width > 16384 || height > 16384
            || width.checked_mul(height).and_then(|n| n.checked_mul(4)) != Some(pixels.len())
            || pixels.chunks_exact(4).any(|p| p[3] != 255) || sha256(pixels) != digest {
            return Err("invalid judged picture identity, dimensions or digest".into());
        }
        let bytes = if let Some(e) = self.entries.iter().find(|e| e.sha256 == digest) {
            e.bytes
        } else {
            let remaining = MAX_TOTAL - self.bytes;
            let mut encoded = String::new();
            let mut at = 0;
            while at < pixels.len() {
                let word = u32::from_le_bytes(pixels[at..at + 4].try_into().unwrap());
                let mut end = at + 4;
                while end < pixels.len() && pixels[end..end + 4] == pixels[at..at + 4] { end += 4; }
                // Check the encoded length before appending; no prefix is
                // uploaded when this picture exceeds the declared bound.
                let token = format!("{}{:x}:{word:08x}", if at == 0 { "" } else { "," }, (end - at) / 4);
                if encoded.len() + token.len() + 1 > remaining {
                    return Err(format!("picture sequence exceeds 64 MiB bound at {case}"));
                }
                encoded.try_reserve(token.len()).map_err(|_| "picture encoding allocation")?;
                encoded.push_str(&token);
                at = end;
            }
            encoded.push('\n');
            for chunk in encoded.as_bytes().chunks(MAX_UPLOAD) { send(chunk)?; }
            self.bytes += encoded.len();
            self.unique += 1;
            encoded.len()
        };
        self.entries.try_reserve(1).map_err(|_| "picture manifest allocation")?;
        self.entries.push(Entry { case: case.into(), sha256: digest, bytes, width, height });
        self.bad = false;
        Ok(())
    }

    pub(super) fn manifest(&self) -> Result<Vec<u8>, String> {
        if self.bad { return Err("incomplete judged picture evidence".into()); }
        let mut text = String::new();
        for (picture, e) in self.entries.iter().enumerate() {
            let digest: String = e.sha256.iter().map(|b| format!("{b:02x}")).collect();
            writeln!(text, "GT710_BREENGL_PICTURE {{\"case\":\"{}\",\"picture\":{picture},\"sha256\":\"{digest}\",\"bytes\":{},\"width\":{},\"height\":{}}}",
                e.case, e.bytes, e.width, e.height).map_err(|_| "picture manifest formatting")?;
        }
        Ok(text.into_bytes())
    }
}
