//! Pure validation for the protected power token and shutdown nonce.
#[derive(Debug, PartialEq, Eq)]
pub enum Invalid { Absent, Metadata, Content }

pub enum Kind { Token, Nonce }

pub struct File<'a> {
    pub regular: bool,
    pub uid: u32,
    pub mode: u16,
    pub size: u64,
    pub data: &'a [u8],
}

pub fn validate(kind: Kind, file: Option<File<'_>>) -> Result<(), Invalid> {
    let file = file.ok_or(Invalid::Absent)?;
    let (size, mode_ok) = match kind {
        Kind::Token => (64, file.mode & 0o7777 == 0o400),
        Kind::Nonce => (65, file.mode & 0o022 == 0),
    };
    if !file.regular || file.uid != 0 || !mode_ok || file.size != size {
        return Err(Invalid::Metadata);
    }
    if file.data.len() != size as usize
        || !file.data[..64].iter().all(|b| b.is_ascii_hexdigit())
        || (matches!(kind, Kind::Nonce) && file.data[64] != b'\n') {
        return Err(Invalid::Content);
    }
    Ok(())
}
