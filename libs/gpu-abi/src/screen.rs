// Suite-only /dev/screen-evidence binary readback request (not a syscall 416 op).
pub const MAGIC: [u8; 8] = *b"SCREENRB";

pub struct Request {
    pub pixels: u64,
    pub len: u64,
    pub record: u64,
}
impl Request {
    pub fn encode(&self) -> [u8; 32] {
        let mut bytes = [0; 32];
        bytes[..8].copy_from_slice(&MAGIC);
        for (i, value) in [self.pixels, self.len, self.record].iter().enumerate() {
            bytes[8 + i * 8..16 + i * 8].copy_from_slice(&value.to_le_bytes());
        }
        bytes
    }
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != 32 || bytes[..8] != MAGIC { return None; }
        let word = |at| u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
        Some(Self { pixels: word(8), len: word(16), record: word(24) })
    }
}

// A known binding is insufficient if its scan origin is unknown or the mode does
// not fit. Callers supply the origin established by their scanout programming.
pub fn rectangle(mode: [u32; 2], binding: [u32; 2], origin: Option<[u32; 2]>) -> Option<[u32; 4]> {
    let [x, y] = origin?;
    let [w, h] = mode;
    (w != 0 && h != 0 && x.checked_add(w)? <= binding[0] && y.checked_add(h)? <= binding[1])
        .then_some([x, y, w, h])
}
