//! The storage lane's explicit hardware-disk writes on physical x86 boots
//! (docs/metal/README.md, "The disks").
//!
//! Physical x86 boots keep hardware disks read-only (issue #1702). Two records
//! in a disk's first sector lift that refusal, each for one disk and one
//! purpose. ext2 leaves its first 1024 bytes unused (the superblock starts at
//! byte 1024), so a record has the sector to itself.
//!
//! - `install-root <boot>` on the bootloader RAM disk asks the kernel to copy
//!   the RAM disk onto the machine's only hardware disk before root mounts. The
//!   boot loop writes it at staging, only for a boot it runs as an install.
//! - `root <boot> <bytes> <sha256>` on a hardware disk, written last by that
//!   copy, names the disk's ext2 root: a boot without a RAM disk mounts it as
//!   root, with writes allowed.
//!
//! A record is the line `BREENIX-STORAGE 1`, one record line, and NUL padding
//! to the end of the sector. The SHA-256 (FIPS 180-4) covers the image after its
//! first sector, so the record can name the bytes it precedes.

use alloc::{boxed::Box, format, string::String, vec, vec::Vec};

use super::{BlockDevice, BlockError};

#[path = "../../../libs/libbreenix/src/crypto/sha256.rs"]
pub mod sha256;

pub const MAGIC: &[u8] = b"BREENIX-STORAGE 1\n";
pub const SECTOR: usize = 512;
/// Sectors moved per read or write: one AHCI command's worth.
const CHUNK_SECTORS: usize = 128;
const BOOT_MAX: usize = 64;

/// A storage record from a disk's first sector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Record {
    Install { boot: String },
    Root { boot: String, bytes: u64, sha256: [u8; 32] },
}

fn valid_boot(boot: &str) -> bool {
    !boot.is_empty()
        && boot.len() <= BOOT_MAX
        && boot.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn digits(text: &str) -> Option<u64> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

fn unhex(text: &str) -> Option<[u8; 32]> {
    let bytes = text.as_bytes();
    if bytes.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (index, pair) in bytes.chunks(2).enumerate() {
        let nibble = |b: u8| match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            _ => None,
        };
        out[index] = (nibble(pair[0])? << 4) | nibble(pair[1])?;
    }
    Some(out)
}

/// Lowercase hexadecimal of a digest.
pub fn hex(digest: &[u8; 32]) -> String {
    digest.iter().map(|b| format!("{:02x}", b)).collect()
}

impl Record {
    /// The record a first sector holds, if it holds exactly one valid record.
    pub fn parse(sector: &[u8]) -> Option<Record> {
        let body = sector.get(..SECTOR)?.strip_prefix(MAGIC)?;
        let end = body.iter().position(|&b| b == b'\n')?;
        if body[end + 1..].iter().any(|&b| b != 0) {
            return None;
        }
        let line = core::str::from_utf8(&body[..end]).ok()?;
        let words: Vec<&str> = line.split(' ').collect();
        match words.as_slice() {
            ["install-root", boot] if valid_boot(boot) => Some(Record::Install { boot: String::from(*boot) }),
            ["root", boot, bytes, sha256] if valid_boot(boot) => {
                let bytes = digits(bytes).filter(|&n| n > SECTOR as u64 && n % SECTOR as u64 == 0)?;
                Some(Record::Root { boot: String::from(*boot), bytes, sha256: unhex(sha256)? })
            }
            _ => None,
        }
    }

    /// The record as a first sector.
    pub fn sector(&self) -> [u8; SECTOR] {
        let line = match self {
            Record::Install { boot } => format!("install-root {}\n", boot),
            Record::Root { boot, bytes, sha256 } => format!("root {} {} {}\n", boot, bytes, hex(sha256)),
        };
        let mut sector = [0u8; SECTOR];
        sector[..MAGIC.len()].copy_from_slice(MAGIC);
        sector[MAGIC.len()..MAGIC.len() + line.len()].copy_from_slice(line.as_bytes());
        sector
    }
}

/// What an install wrote, and whether reading it back found the same bytes.
pub struct Installed {
    pub bytes: u64,
    pub sha256: [u8; 32],
    pub verified: Result<(), &'static str>,
}

fn io(_: BlockError) -> &'static str {
    "disk I/O failed"
}

/// Copy `ram` onto `disk` from sector 0: sectors 1 onwards first, then the root
/// record in sector 0, then a flush, so an interrupted copy leaves no root
/// record. `writes(true)` is called just before the first write and
/// `writes(false)` once writing ends, whether it succeeded or not. Then the
/// copied sectors are read back and compared with the RAM disk and the record. An `Err`
/// means the copy was refused before its first write, or a write failed.
pub fn install(
    ram: &dyn BlockDevice,
    disk: &dyn BlockDevice,
    boot: &str,
    writes: &mut dyn FnMut(bool),
) -> Result<Installed, &'static str> {
    if !valid_boot(boot) {
        return Err("invalid boot id");
    }
    if ram.block_size() != SECTOR || disk.block_size() != SECTOR {
        return Err("both devices must have 512-byte sectors");
    }
    let sectors = ram.num_blocks();
    if sectors < 2 {
        return Err("the RAM disk is too small");
    }
    if disk.num_blocks() < sectors {
        return Err("the hardware disk is smaller than the RAM disk");
    }
    let mut source = vec![0u8; CHUNK_SECTORS * SECTOR];
    let mut hasher = sha256::Sha256::new();
    let bytes = sectors * SECTOR as u64;
    writes(true);
    let written = (|| {
        let mut sector = 1;
        while sector < sectors {
            let count = (sectors - sector).min(CHUNK_SECTORS as u64) as usize;
            let chunk = &mut source[..count * SECTOR];
            ram.read_blocks(sector, count, chunk).map_err(io)?;
            hasher.update(chunk);
            disk.write_blocks(sector, count, chunk).map_err(io)?;
            sector += count as u64;
        }
        let record = Record::Root { boot: String::from(boot), bytes, sha256: hasher.finalize() }.sector();
        disk.write_block(0, &record).map_err(io)?;
        disk.flush().map_err(io)?;
        Ok(record)
    })();
    writes(false);
    let record = written?;
    let Some(Record::Root { sha256, .. }) = Record::parse(&record) else {
        return Err("the root record does not parse");
    };
    let verified = verify(ram, disk, &record, &sha256);
    Ok(Installed { bytes, sha256, verified })
}

fn verify(ram: &dyn BlockDevice, disk: &dyn BlockDevice, record: &[u8; SECTOR], sha256: &[u8; 32]) -> Result<(), &'static str> {
    let mut first = [0u8; SECTOR];
    disk.read_block(0, &mut first).map_err(io)?;
    if &first != record {
        return Err("the record sector differs");
    }
    let mut expected = vec![0u8; CHUNK_SECTORS * SECTOR];
    let mut found = vec![0u8; CHUNK_SECTORS * SECTOR];
    let mut hasher = sha256::Sha256::new();
    let sectors = ram.num_blocks();
    let mut sector = 1;
    while sector < sectors {
        let count = (sectors - sector).min(CHUNK_SECTORS as u64) as usize;
        let length = count * SECTOR;
        ram.read_blocks(sector, count, &mut expected[..length]).map_err(io)?;
        disk.read_blocks(sector, count, &mut found[..length]).map_err(io)?;
        if expected[..length] != found[..length] {
            return Err("the image sectors differ");
        }
        hasher.update(&found[..length]);
        sector += count as u64;
    }
    if &hasher.finalize() != sha256 {
        return Err("the image digest differs");
    }
    Ok(())
}

/// The one disk whose first sector holds a root record, with that record.
pub fn find_root(disks: Vec<Box<dyn BlockDevice>>) -> Result<(Box<dyn BlockDevice>, Record), &'static str> {
    let mut found = None;
    for disk in disks {
        if disk.block_size() != SECTOR {
            continue;
        }
        let mut first = [0u8; SECTOR];
        if disk.read_block(0, &mut first).is_err() {
            continue;
        }
        if let Some(record @ Record::Root { .. }) = Record::parse(&first) {
            if found.is_some() {
                return Err("more than one hardware disk holds a root record");
            }
            found = Some((disk, record));
        }
    }
    found.ok_or("no hardware disk holds a root record")
}

/// Whether the image after the record sector still has the digest the record
/// names. Later boots that write the root change it, so this is a report, not a refusal.
pub fn matches(disk: &dyn BlockDevice, bytes: u64, sha256: &[u8; 32]) -> Result<bool, &'static str> {
    let sectors = bytes / SECTOR as u64;
    if disk.num_blocks() < sectors {
        return Ok(false);
    }
    let mut buffer = vec![0u8; CHUNK_SECTORS * SECTOR];
    let mut hasher = sha256::Sha256::new();
    let mut sector = 1;
    while sector < sectors {
        let count = (sectors - sector).min(CHUNK_SECTORS as u64) as usize;
        disk.read_blocks(sector, count, &mut buffer[..count * SECTOR]).map_err(io)?;
        hasher.update(&buffer[..count * SECTOR]);
        sector += count as u64;
    }
    Ok(&hasher.finalize() == sha256)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::sync::Arc;
    use spin::Mutex;

    struct Disk {
        bytes: Arc<Mutex<Vec<u8>>>,
        writable: Arc<Mutex<bool>>,
        writes: Arc<Mutex<usize>>,
        fail_write_at: Option<u64>,
    }

    impl Disk {
        fn new(sectors: usize) -> Self {
            Self {
                bytes: Arc::new(Mutex::new(vec![0; sectors * SECTOR])),
                writable: Arc::new(Mutex::new(false)),
                writes: Arc::new(Mutex::new(0)),
                fail_write_at: None,
            }
        }
    }

    impl BlockDevice for Disk {
        fn read_block(&self, block: u64, buf: &mut [u8]) -> Result<(), BlockError> {
            self.read_blocks(block, 1, buf)
        }
        fn read_blocks(&self, start: u64, count: usize, buf: &mut [u8]) -> Result<(), BlockError> {
            let at = start as usize * SECTOR;
            let bytes = self.bytes.lock();
            if at + count * SECTOR > bytes.len() {
                return Err(BlockError::OutOfBounds);
            }
            buf[..count * SECTOR].copy_from_slice(&bytes[at..at + count * SECTOR]);
            Ok(())
        }
        fn write_block(&self, block: u64, buf: &[u8]) -> Result<(), BlockError> {
            self.write_blocks(block, 1, buf)
        }
        fn write_blocks(&self, start: u64, count: usize, buf: &[u8]) -> Result<(), BlockError> {
            if !*self.writable.lock() || self.fail_write_at.is_some_and(|at| at >= start && at < start + count as u64) {
                return Err(BlockError::IoError);
            }
            *self.writes.lock() += 1;
            let at = start as usize * SECTOR;
            self.bytes.lock()[at..at + count * SECTOR].copy_from_slice(&buf[..count * SECTOR]);
            Ok(())
        }
        fn block_size(&self) -> usize { SECTOR }
        fn num_blocks(&self) -> u64 { (self.bytes.lock().len() / SECTOR) as u64 }
        fn flush(&self) -> Result<(), BlockError> { Ok(()) }
    }

    fn image(sectors: usize) -> Disk {
        let disk = Disk::new(sectors);
        for (index, byte) in disk.bytes.lock().iter_mut().enumerate() {
            *byte = (index * 7 + index / 512) as u8;
        }
        disk.bytes.lock()[..SECTOR].copy_from_slice(&Record::Install { boot: "20261017T000000Z-metal".into() }.sector());
        disk
    }

    fn gate(disk: &Disk) -> impl FnMut(bool) + '_ {
        move |granted| *disk.writable.lock() = granted
    }

    #[test]
    fn records_round_trip_and_reject_damage() {
        let root = Record::Root { boot: "20261017T000000Z-metal".into(), bytes: 4096, sha256: [0xab; 32] };
        for record in [Record::Install { boot: "20261017T000000Z-metal".into() }, root.clone()] {
            assert_eq!(Record::parse(&record.sector()), Some(record));
        }
        let mut sector = root.sector();
        sector[SECTOR - 1] = 1;
        assert_eq!(Record::parse(&sector), None);
        for line in ["root x 4096 ab", "root x 4097 ", "root x +4096 ", "root x/y 4096 ", "install-root", "other x"] {
            let mut sector = [0u8; SECTOR];
            sector[..MAGIC.len()].copy_from_slice(MAGIC);
            let text = if line.ends_with(' ') { format!("{}{}\n", line, hex(&[0xab; 32])) } else { format!("{}\n", line) };
            sector[MAGIC.len()..MAGIC.len() + text.len()].copy_from_slice(text.as_bytes());
            assert_eq!(Record::parse(&sector), None, "{line}");
        }
        assert_eq!(Record::parse(&[0u8; SECTOR]), None);
        assert_eq!(Record::parse(&root.sector()[..SECTOR - 1]), None);
    }

    #[test]
    fn install_copies_verifies_and_names_the_image() {
        let ram = image(300);
        let disk = Disk::new(400);
        let mut calls = Vec::new();
        let writable = disk.writable.clone();
        let done = install(&ram, &disk, "20261017T000000Z-metal", &mut |granted| {
            calls.push(granted);
            *writable.lock() = granted;
        }).unwrap();
        assert_eq!(calls, [true, false]);
        assert!(!*disk.writable.lock());
        assert_eq!(done.verified, Ok(()));
        assert_eq!(done.bytes, 300 * SECTOR as u64);
        assert_eq!(done.sha256, sha256::sha256(&ram.bytes.lock()[SECTOR..]));
        assert_eq!(disk.bytes.lock()[SECTOR..300 * SECTOR], ram.bytes.lock()[SECTOR..]);
        assert!(disk.bytes.lock()[300 * SECTOR..].iter().all(|&b| b == 0));
        let (_, record) = find_root(vec![Box::new(Disk::new(4)), Box::new(disk) as Box<dyn BlockDevice>]).unwrap();
        assert_eq!(record, Record::Root { boot: "20261017T000000Z-metal".into(), bytes: done.bytes, sha256: done.sha256 });
    }

    #[test]
    fn interrupted_install_leaves_no_root_record_and_revokes_writes() {
        let ram = image(300);
        let mut disk = Disk::new(400);
        disk.fail_write_at = Some(200);
        let writable = disk.writable.clone();
        assert!(install(&ram, &disk, "20261017T000000Z-metal", &mut |granted| *writable.lock() = granted).is_err());
        assert!(!*disk.writable.lock());
        assert!(disk.bytes.lock()[..SECTOR].iter().all(|&b| b == 0));
        assert!(find_root(vec![Box::new(disk) as Box<dyn BlockDevice>]).is_err());
    }

    #[test]
    fn install_refuses_before_writing() {
        let ram = image(300);
        let small = Disk::new(299);
        assert!(install(&ram, &small, "20261017T000000Z-metal", &mut gate(&small)).is_err());
        assert_eq!(*small.writes.lock(), 0);
        let disk = Disk::new(400);
        assert!(install(&ram, &disk, "bad/id", &mut gate(&disk)).is_err());
        assert_eq!(*disk.writes.lock(), 0);
    }

    #[test]
    fn find_root_needs_exactly_one_record_and_matches_reports_changes() {
        let ram = image(64);
        let (a, b) = (Disk::new(64), Disk::new(64));
        let done = install(&ram, &a, "a", &mut gate(&a)).unwrap();
        install(&ram, &b, "b", &mut gate(&b)).unwrap();
        assert!(matches(&a, done.bytes, &done.sha256).unwrap());
        a.bytes.lock()[40 * SECTOR] ^= 1;
        assert!(!matches(&a, done.bytes, &done.sha256).unwrap());
        assert!(find_root(vec![Box::new(a) as Box<dyn BlockDevice>, Box::new(b)]).is_err());
    }
}
