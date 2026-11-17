//! The host's root choice, independent of the mount and statfs calls under test.
use std::fs;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RootFs {
    Ext2,
    Breenfs,
}

impl RootFs {
    pub fn name(self) -> &'static str {
        match self {
            Self::Ext2 => "ext2",
            Self::Breenfs => "breenfs",
        }
    }

    pub fn magic(self) -> u64 {
        match self {
            Self::Ext2 => 0xef53,
            Self::Breenfs => u64::from_le_bytes(*b"BREENFSS"),
        }
    }
}

pub fn parse(text: Option<&str>) -> Result<RootFs, String> {
    // Boot paths without a declaration build ext2 (E24 coordinator line 278).
    let Some(text) = text else { return Ok(RootFs::Ext2) };
    match text.split_whitespace().collect::<Vec<_>>().as_slice() {
        ["root-fs", "ext2"] => Ok(RootFs::Ext2),
        ["root-fs", "breenfs"] => Ok(RootFs::Breenfs),
        _ => Err(format!("invalid host root filesystem declaration: {text:?}")),
    }
}

pub fn declared() -> Result<RootFs, String> {
    match fs::read_to_string("/etc/breenix/root-fs") {
        Ok(text) => parse(Some(&text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => parse(None),
        Err(error) => Err(format!("read host root filesystem declaration: {error}")),
    }
}
