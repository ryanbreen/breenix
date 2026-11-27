//! One-shot boot publication protocol. The guest writes the image before the
//! manifest and can acknowledge only the attempt identified by the manager.
use crate::sha256::sha256;

pub const MAX_IMAGE: usize = 512 << 20;
pub type Token = [u8; 32];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub token: Token,
    pub commit: [u8; 40],
    pub size: usize,
    pub digest: [u8; 32],
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

pub fn decode_hex<const N: usize>(text: &[u8]) -> Option<[u8; N]> {
    if text.len() != N * 2 { return None; }
    let mut digest = [0; N];
    for (byte, pair) in digest.iter_mut().zip(text.chunks_exact(2)) {
        *byte = hex(pair[0])? << 4 | hex(pair[1])?;
    }
    Some(digest)
}

pub fn token(text: &[u8]) -> Option<Token> {
    let bytes: Token = text.try_into().ok()?;
    bytes.iter().all(|&b| hex(b).is_some()).then_some(bytes)
}

impl Manifest {
    pub fn parse(text: &[u8]) -> Option<Self> {
        let mut lines = text.strip_suffix(b"\n")?.split(|&b| b == b'\n');
        if lines.next()? != b"BREENIX-FLASH 1" { return None; }
        let token = token(lines.next()?)?;
        let commit: [u8; 40] = lines.next()?.try_into().ok()?;
        decode_hex::<20>(&commit)?;
        let length = lines.next()?;
        if length.is_empty() || length[0] == b'0' || !length.iter().all(u8::is_ascii_digit) {
            return None;
        }
        let size = core::str::from_utf8(length).ok()?.parse::<usize>().ok()?;
        if size > MAX_IMAGE { return None; }
        let digest = decode_hex(lines.next()?)?;
        if lines.next().is_some() { return None; }
        Some(Self { token, commit, size, digest })
    }

    pub fn verifies(&self, image: &[u8]) -> bool {
        image.len() == self.size && sha256(image) == self.digest
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Action { Current, Try, Promote }

pub fn choose(pending: Option<Token>, attempted: Option<Token>, acknowledged: Option<Token>, retained: bool) -> Action {
    if retained && attempted.is_some() && attempted == acknowledged {
        Action::Promote
    } else if pending.is_some() && pending != attempted {
        Action::Try
    } else {
        Action::Current
    }
}
