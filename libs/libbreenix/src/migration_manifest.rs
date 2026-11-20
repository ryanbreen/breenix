//! Canonical TSV encoder shared by the migrator and the BreenFS image reader.
//! See docs/fs/migration-manifest.md.
use std::collections::BTreeMap;
use std::io::{self, Write};

pub struct Row {
    pub path: Vec<u8>,
    pub identity: (u64, u64),
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub rdev: (u64, u64),
    pub size: u64,
    pub mtime: (i64, u32),
    pub digest: String,
}

pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(out, "{byte:02x}").unwrap();
    }
    out
}

pub fn kind(mode: u32) -> io::Result<&'static str> {
    match mode & 0o170000 {
        0o100000 => Ok("reg"),
        0o040000 => Ok("dir"),
        0o120000 => Ok("symlink"),
        0o010000 => Ok("fifo"),
        0o140000 => Ok("sock"),
        0o020000 => Ok("chr"),
        0o060000 => Ok("blk"),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unknown inode type",
        )),
    }
}

/// Decode a device number from the on-disk and Breenix stat representation.
pub fn device_parts(raw: u64) -> (u64, u64) {
    ((raw >> 8) & 0xfff, (raw & 0xff) | ((raw >> 12) & 0xfff00))
}

pub fn write(mut rows: Vec<Row>, mut out: impl Write) -> io::Result<()> {
    rows.sort_by(|a, b| a.path.cmp(&b.path));
    let mut groups = BTreeMap::new();
    for row in &rows {
        if kind(row.mode)? != "dir" {
            let group = groups
                .entry(row.identity)
                .or_insert_with(|| (0, hex(&row.path)));
            group.0 += 1;
        }
    }
    for row in &rows {
        let group = groups.get(&row.identity).filter(|group| group.0 > 1);
        let group = group.map_or("-", |group| group.1.as_str());
        let kind = kind(row.mode)?;
        write!(out, "{}\t{kind}", hex(&row.path))?;
        if matches!(kind, "chr" | "blk") {
            let (major, minor) = row.rdev;
            write!(out, ":{major}:{minor}")?;
        }
        let size = if matches!(kind, "reg" | "symlink") {
            row.size
        } else {
            0
        };
        let (sec, nsec) = row.mtime;
        writeln!(
            out,
            "\t{:04o}\t{}\t{}\t{group}\t{size}\t{sec}.{nsec:09}\t{}",
            row.mode & 0o7777,
            row.uid,
            row.gid,
            row.digest
        )?;
    }
    Ok(())
}
