use std::io::{self, Write};
use std::path::Path;

pub fn ensure_boot_id(path: &Path, candidate: &str) -> io::Result<String> {
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
    match std::fs::OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut file) => writeln!(file, "{candidate}")?,
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(e),
    }
    let boot = std::fs::read_to_string(path)?;
    let boot = boot.trim();
    if boot.is_empty() { return Err(io::Error::new(io::ErrorKind::InvalidData, "empty boot ID")); }
    Ok(boot.to_owned())
}

pub fn write_records(out: &mut impl Write, nonce: &str, range: std::ops::Range<u32>) -> io::Result<()> {
    for n in range { out.write_all(format!("{nonce}-{n:02}\n").as_bytes())?; }
    Ok(())
}
