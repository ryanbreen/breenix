//! Optional suite-panel transport; case assertions remain independent.
use libbreenix::crypto::sha256::sha256;
use std::io::Write;

pub(super) const MAX_ROUTE: usize = 2048;
// G28: complete canonical pictures share the fixed per-boot transport budget.
pub(super) const MAX_UPLOAD: usize = MAX_TOTAL;
pub(super) const MAX_TOTAL: usize = 64 << 20;

pub(super) struct Route {
    pub host: String,
    pub port: u16,
    pub authority: String,
    pub path: String,
}
impl Route {
    pub fn parse(text: &str, suite: &str, sequence: bool) -> Result<Self, String> {
        if text.len() > MAX_ROUTE { return Err("route-bound".into()); }
        let text = text.trim();
        if text.bytes().any(|b| !b.is_ascii() || b <= b' ' || b == 127)
            || text.contains(['@', '?', '#', '\\']) {
            return Err("route-syntax".into());
        }
        let rest = text.strip_prefix("http://").ok_or("route-http")?;
        let (authority, path) = rest.split_once('/').ok_or("route-path")?;
        if path.is_empty() { return Err("route-path".into()); }
        let (host, port) = match authority.split_once(':') {
            Some((host, port)) => (host, port.parse::<u16>().ok().filter(|&p| p != 0).ok_or("route-port")?),
            None => (authority, 80),
        };
        if host.is_empty() || !host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-') {
            return Err("route-host".into());
        }
        let mut path = format!("/{path}");
        if sequence {
            if suite.is_empty() || !suite.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') {
                return Err("route-suite".into());
            }
            path.push_str("/suites/"); path.push_str(suite);
        }
        Ok(Self { host: host.into(), port, authority: authority.into(), path })
    }
}

pub(super) struct Observation {
    pub width: u32,
    pub height: u32,
    pub record: [u8; 1024],
    pub pixels: Vec<u8>,
}

pub(super) fn pixel_size(width: u32, height: u32) -> Result<usize, String> {
    (width as usize).checked_mul(height as usize).and_then(|n| n.checked_mul(4))
        .filter(|&n| width != 0 && height != 0 && n <= MAX_TOTAL).ok_or_else(|| "dimensions".into())
}

fn hex_digits(mut n: usize) -> usize {
    let mut digits = 1;
    while n >= 16 { n >>= 4; digits += 1; }
    digits
}

// The existing native text RLE is count-in-hex:canonical-word-in-hex, separated
// by commas and terminated by a newline. Raw is used when that RLE is larger.
fn encode(o: &Observation, remaining: usize) -> Result<Vec<u8>, String> {
    if pixel_size(o.width, o.height)? != o.pixels.len() || o.pixels.chunks_exact(4).any(|p| p[3] != 255) {
        return Err("pixels".into());
    }
    let mut rle_len = 0usize;
    let mut at = 0;
    while at < o.pixels.len() {
        let mut end = at + 4;
        while end < o.pixels.len() && o.pixels[end..end+4] == o.pixels[at..at+4] { end += 4; }
        rle_len = rle_len.checked_add(hex_digits((end-at)/4) + 10).ok_or("upload-bound")?;
        at = end;
    }
    let rle = rle_len < o.pixels.len();
    let header = format!("SUITE_SCANOUT v=1 width={} height={} encoding={} record=1024\n",
        o.width, o.height, if rle { "rle" } else { "raw" });
    let size = header.len().checked_add(o.record.len())
        .and_then(|n| n.checked_add(if rle { rle_len } else { o.pixels.len() })).ok_or("upload-bound")?;
    if size > MAX_UPLOAD { return Err("upload-bound".into()); }
    if size > remaining { return Err("total-bound".into()); }
    let mut data = Vec::new();
    data.try_reserve_exact(size).map_err(|_| "allocation")?;
    data.extend_from_slice(header.as_bytes()); data.extend_from_slice(&o.record);
    if rle {
        let mut at = 0;
        while at < o.pixels.len() {
            let mut end = at + 4;
            while end < o.pixels.len() && o.pixels[end..end+4] == o.pixels[at..at+4] { end += 4; }
            let p = &o.pixels[at..at+4];
            let word = u32::from_le_bytes([p[0], p[1], p[2], p[3]]);
            write!(data, "{:x}:{word:08x}{}", (end-at)/4, if end == o.pixels.len() { '\n' } else { ',' })
                .map_err(|_| "encoding")?;
            at = end;
        }
    } else { data.extend_from_slice(&o.pixels); }
    Ok(data)
}

pub(super) struct Upload {
    suite: &'static str,
    route: Result<Option<Route>, String>,
    bytes: usize,
}
impl Upload {
    pub fn new(suite: &'static str, route: Result<Option<Route>, String>) -> Self {
        Self { suite, route, bytes: 0 }
    }
    /// Picture buffers are local to this call and drop before it returns.
    pub fn capture(&mut self, label: &str,
        capture: impl FnOnce() -> Result<Observation, String>,
        send: impl FnOnce(&Route, &str, &[u8], [u8; 32]) -> Result<(), String>) -> String {
        let prefix = format!("SUITE_EVIDENCE {} {label}", self.suite);
        let route = match &self.route {
            Ok(Some(route)) => route,
            Ok(None) => return format!("{prefix} skipped=no-route"),
            Err(error) => return format!("{prefix} refused={error}"),
        };
        let result = (|| {
            let observation = capture()?;
            let data = encode(&observation, MAX_TOTAL - self.bytes)?;
            let digest = sha256(&data);
            let path = if label == "suite-final-panel" { format!("{}/final", route.path) } else { route.path.clone() };
            // Count attempted bytes, including ambiguous HTTP failures,
            // against the boot transport bound.
            self.bytes += data.len();
            send(route, &path, &data, digest)?;
            let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
            Ok::<_, String>(format!("{prefix} bytes={} sha256={hex}", data.len()))
        })();
        result.unwrap_or_else(|error| format!("{prefix} refused={error}"))
    }
}
