//! Pure validation for explicit breenfs disk mounts.
use crate::syscall::errno::{EINVAL, EPERM};

pub fn validate(euid: u32, flags: u64) -> Result<(), u64> {
    if euid != 0 {
        return Err(EPERM as u64);
    }
    if flags & !1 != 0 {
        return Err(EINVAL as u64);
    }
    Ok(())
}

pub fn parse(euid: u32, flags: u64, source: &[u8]) -> Result<usize, u64> {
    validate(euid, flags)?;
    let digits = source.strip_prefix(b"/dev/disk").ok_or(EINVAL as u64)?;
    if digits.is_empty() || digits.len() > 3 || digits.len() > 1 && digits[0] == b'0' {
        return Err(EINVAL as u64);
    }
    let mut number = 0u16;
    for &digit in digits {
        if !digit.is_ascii_digit() {
            return Err(EINVAL as u64);
        }
        number = number * 10 + u16::from(digit - b'0');
    }
    u8::try_from(number)
        .map(usize::from)
        .map_err(|_| EINVAL as u64)
}

pub fn has_magic(bytes: &[u8]) -> bool {
    bytes.starts_with(b"BREENFSS")
}

/// Parse optional mount data naming a filesystem root.
pub fn parse_root(data: Option<&[u8]>) -> Result<Option<&[u8]>, u64> {
    let data = match data {
        None | Some([]) => return Ok(None),
        Some(data) => data,
    };
    let name = data.strip_prefix(b"root=").ok_or(EINVAL as u64)?;
    if name.is_empty() || name.len() > 255 || name.iter().any(|b| matches!(b, 0 | b'/' | b',')) {
        return Err(EINVAL as u64);
    }
    Ok(Some(name))
}
