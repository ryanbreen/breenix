#[cfg(any(feature = "alloc", test))]
extern crate alloc;
// Suite-only /dev/screen-evidence binary readback request (not a syscall 416 op).
pub const MAGIC: [u8; 8] = *b"SCREENRB";
// Suite endpoint only: a confirmed empty scanout, not a failed observation.
pub const NO_CASE: i64 = 61;

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

// On a refused request the suite endpoint publishes this status in the record
// buffer. Ordinary READBACK and SCANOUT_READBACK records are unchanged.
#[derive(Clone, Copy)]
pub struct Failure {
    pub errno: i64,
    pub reason: u32,
    pub failed_submission: u64,
    /// One for an end state the producer deliberately invalidated.
    pub declared: u32,
}
impl Failure {
    pub fn encode(self) -> [u8; 32] {
        let mut bytes = [0; 32];
        bytes[..8].copy_from_slice(b"SCREENER");
        bytes[8..16].copy_from_slice(&self.errno.to_le_bytes());
        bytes[16..20].copy_from_slice(&self.reason.to_le_bytes());
        bytes[20..24].copy_from_slice(&self.declared.to_le_bytes());
        bytes[24..32].copy_from_slice(&self.failed_submission.to_le_bytes());
        bytes
    }
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 32 || &bytes[..8] != b"SCREENER" { return None; }
        Some(Self {
            errno: i64::from_le_bytes(bytes[8..16].try_into().unwrap()),
            reason: u32::from_le_bytes(bytes[16..20].try_into().unwrap()),
            failed_submission: u64::from_le_bytes(bytes[24..32].try_into().unwrap()),
            declared: u32::from_le_bytes(bytes[20..24].try_into().unwrap()),
        })
    }
}

// A separate request tag preserves SCREENRB and ordinary observation records.
pub const CRC_MAGIC: [u8; 8] = *b"SCRCRCRB";
pub const CRC_HEADER: usize = 48;
pub const CRC_REQUEST_MAX: usize = CRC_HEADER + 3 * 255;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CrcRequest<'a> {
    pub pixels: u64,
    pub len: u64,
    pub observation: u64,
    /// Destination for a record shorter than 4096 bytes. Its header gives length.
    pub record: u64,
    pub suite: &'a str,
    pub case: &'a str,
    pub checkpoint: &'a str,
}
impl<'a> CrcRequest<'a> {
    #[cfg(any(feature = "alloc", test))]
    pub fn encode(&self) -> Result<alloc::vec::Vec<u8>, &'static str> {
        if !self.valid_names() { return Err("crc-request-identity"); }
        let mut bytes = alloc::vec![0; CRC_HEADER];
        bytes[..8].copy_from_slice(&CRC_MAGIC);
        for (i,value) in [self.pixels,self.len,self.observation,self.record].iter().enumerate() {
            bytes[8+i*8..16+i*8].copy_from_slice(&value.to_le_bytes());
        }
        for (i,name) in [self.suite,self.case,self.checkpoint].iter().enumerate() {
            bytes[40+i*2..42+i*2].copy_from_slice(&(name.len() as u16).to_le_bytes());
            bytes.extend_from_slice(name.as_bytes());
        }
        Ok(bytes)
    }
    pub fn decode(bytes: &'a [u8]) -> Option<Self> {
        if bytes.len() < CRC_HEADER || bytes.len() > CRC_REQUEST_MAX || bytes[..8] != CRC_MAGIC || bytes[46..48] != [0,0] { return None; }
        let word = |at| u64::from_le_bytes(bytes[at..at+8].try_into().unwrap());
        let mut at = CRC_HEADER;
        let mut name = |offset| -> Option<&'a str> {
            let len = u16::from_le_bytes(bytes[offset..offset+2].try_into().unwrap()) as usize;
            let text = core::str::from_utf8(bytes.get(at..at+len)?).ok()?;
            at += len; Some(text)
        };
        let request = Self {pixels:word(8),len:word(16),observation:word(24),record:word(32),
            suite:name(40)?,case:name(42)?,checkpoint:name(44)?};
        (at == bytes.len() && request.valid_names()).then_some(request)
    }
    fn valid_names(&self) -> bool {
        let token = |name: &str| !name.is_empty() && name.len() <= 255 && name.split('-').all(|word|
            !word.is_empty() && word.bytes().all(|b|b.is_ascii_lowercase() || b.is_ascii_digit()));
        matches!(self.suite,"gpu"|"breengel") && token(self.checkpoint)
            && self.case.len() <= 255 && self.case.split_once('/').is_some_and(|(a,b)|token(a) && token(b))
    }
}
