//! The GUID partition table (UEFI 2.10 section 5.3), read to find the partitions a boot intent names
//! (docs/metal/README.md, "Boot intents and root selection").
//!
//! Only the primary table is read: its header must have the `EFI PART` signature, revision 1.0, a header size of 92
//! to 512 bytes, its own LBA 1, and a matching header CRC32; its entries must be 128 bytes or a larger power of two
//! and match the header's entry-array CRC32. A partition is named by its unique GUID, in the on-disk (mixed-endian)
//! byte order. No allocation: entries are read one sector at a time into the caller's buffer.

pub const SECTOR: usize = 512;
pub type Guid = [u8; 16];

/// The partition roles of a Breenix disk, by their type GUIDs.
pub const TEST_ROOT_TYPE: Guid = guid(0xdac951c3, 0x27df, 0x4e3d, [0x8b, 0x9e, 0x48, 0x79, 0x06, 0xac, 0x2c, 0x81]);
pub const SYSTEM_TYPE: Guid = guid(0x2ef14a34, 0xabbd, 0x48f8, [0x83, 0xd6, 0xf2, 0xa8, 0xc7, 0xb4, 0xa9, 0xfe]);
pub const HOME_TYPE: Guid = guid(0x9bfeaef9, 0x4f51, 0x45de, [0x8b, 0xcf, 0x5c, 0xec, 0x16, 0x03, 0xb8, 0x62]);

/// A GUID's on-disk bytes from its fields (the first three little-endian).
pub const fn guid(a: u32, b: u16, c: u16, d: [u8; 8]) -> Guid {
    let a = a.to_le_bytes();
    let b = b.to_le_bytes();
    let c = c.to_le_bytes();
    [a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]]
}

/// A GUID from its text form, `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx` (either case).
pub fn parse_guid(text: &str) -> Option<Guid> {
    let bytes = text.as_bytes();
    if bytes.len() != 36 || [8, 13, 18, 23].iter().any(|&at| bytes[at] != b'-') {
        return None;
    }
    let mut digits = [0u8; 32];
    let mut count = 0;
    for &byte in bytes {
        if byte == b'-' {
            continue;
        }
        digits[count] = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            _ => return None,
        };
        count += 1;
    }
    let value = |from: usize, len: usize| digits[from..from + len].iter().fold(0u64, |acc, &d| acc << 4 | d as u64);
    let mut tail = [0u8; 8];
    for (index, byte) in tail.iter_mut().enumerate() {
        *byte = value(16 + 2 * index, 2) as u8;
    }
    Some(guid(value(0, 8) as u32, value(8, 4) as u16, value(12, 4) as u16, tail))
}

/// A GUID's text form, lowercase.
pub fn format_guid(guid: &Guid, out: &mut [u8; 36]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let order = [3usize, 2, 1, 0, 5, 4, 7, 6, 8, 9, 10, 11, 12, 13, 14, 15];
    let mut len = 0;
    for (index, &at) in order.iter().enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) {
            out[len] = b'-';
            len += 1;
        }
        out[len] = HEX[(guid[at] >> 4) as usize];
        out[len + 1] = HEX[(guid[at] & 0xf) as usize];
        len += 2;
    }
}

/// CRC32 as UEFI uses it (the ISO 3309 polynomial, reflected, with the initial value and final XOR 0xffffffff), over
/// several pieces in turn.
#[derive(Clone, Copy)]
pub struct Crc32(u32);

impl Crc32 {
    pub fn new() -> Self {
        Crc32(0xffff_ffff)
    }

    pub fn update(&mut self, bytes: &[u8]) {
        let mut crc = self.0;
        for &byte in bytes {
            crc ^= byte as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
            }
        }
        self.0 = crc;
    }

    pub fn finish(self) -> u32 {
        !self.0
    }
}

pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = Crc32::new();
    crc.update(bytes);
    crc.finish()
}

/// One partition entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Partition {
    pub type_guid: Guid,
    pub unique: Guid,
    pub first: u64,
    pub last: u64,
}

impl Partition {
    pub fn sectors(&self) -> u64 {
        self.last - self.first + 1
    }
}

/// The primary header's fields this module uses.
#[derive(Clone, Copy, Debug)]
pub struct Header {
    pub first_usable: u64,
    pub last_usable: u64,
    pub disk_guid: Guid,
    pub entries_lba: u64,
    pub entries: u32,
    pub entry_size: u32,
    pub entries_crc: u32,
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    let mut value = [0u8; 8];
    value.copy_from_slice(&bytes[at..at + 8]);
    u64::from_le_bytes(value)
}

/// The primary header from LBA 1 of a disk of `sectors` sectors.
pub fn header(lba1: &[u8], sectors: u64) -> Result<Header, &'static str> {
    if lba1.len() < SECTOR || &lba1[..8] != b"EFI PART" {
        return Err("no GPT header in LBA 1");
    }
    if u32_at(lba1, 8) != 0x0001_0000 {
        return Err("the GPT header revision is not 1.0");
    }
    let size = u32_at(lba1, 12) as usize;
    if !(92..=SECTOR).contains(&size) {
        return Err("the GPT header size is out of range");
    }
    let mut copy = [0u8; SECTOR];
    copy[..size].copy_from_slice(&lba1[..size]);
    copy[16..20].fill(0);
    if crc32(&copy[..size]) != u32_at(lba1, 16) {
        return Err("the GPT header CRC32 does not match");
    }
    if u64_at(lba1, 24) != 1 {
        return Err("the GPT header is not the primary one");
    }
    let header = Header {
        first_usable: u64_at(lba1, 40),
        last_usable: u64_at(lba1, 48),
        disk_guid: lba1[56..72].try_into().unwrap(),
        entries_lba: u64_at(lba1, 72),
        entries: u32_at(lba1, 80),
        entry_size: u32_at(lba1, 84),
        entries_crc: u32_at(lba1, 88),
    };
    let entry_size = header.entry_size as usize;
    if entry_size < 128 || !entry_size.is_power_of_two() || entry_size > SECTOR {
        return Err("the GPT entry size is not supported");
    }
    if header.entries == 0 || header.entries > 1024 {
        return Err("the GPT entry count is out of range");
    }
    let entry_sectors = (header.entries as u64 * entry_size as u64).div_ceil(SECTOR as u64);
    if header.entries_lba < 2 || header.entries_lba.saturating_add(entry_sectors) > sectors
        || header.first_usable > header.last_usable || header.last_usable >= sectors
    {
        return Err("the GPT header's ranges do not fit the disk");
    }
    Ok(header)
}

/// The partition whose unique GUID is `unique`, reading entry sectors through `read` (`read(lba, buffer)`), with the
/// whole entry array checked against the header's CRC32. `Ok` without a partition when no entry has that GUID.
pub fn find(
    header: &Header,
    unique: &Guid,
    mut read: impl FnMut(u64, &mut [u8; SECTOR]) -> Result<(), &'static str>,
) -> Result<Option<Partition>, &'static str> {
    let entry_size = header.entry_size as usize;
    let per_sector = SECTOR / entry_size;
    let total = header.entries as usize * entry_size;
    let sectors = total.div_ceil(SECTOR);
    let mut crc = Crc32::new();
    let mut found = None;
    let mut matches = 0;
    let mut sector = [0u8; SECTOR];
    for index in 0..sectors {
        read(header.entries_lba + index as u64, &mut sector)?;
        let covered = (total - index * SECTOR).min(SECTOR);
        crc.update(&sector[..covered]);
        for slot in 0..per_sector {
            let at = slot * entry_size;
            if index * SECTOR + at >= total {
                break;
            }
            let entry = &sector[at..at + 128];
            if entry[..16] == [0u8; 16] || &entry[16..32] != unique {
                continue;
            }
            let partition = Partition {
                type_guid: entry[..16].try_into().unwrap(),
                unique: entry[16..32].try_into().unwrap(),
                first: u64_at(entry, 32),
                last: u64_at(entry, 40),
            };
            matches += 1;
            found = Some(partition);
        }
    }
    if crc.finish() != header.entries_crc {
        return Err("the GPT entry array CRC32 does not match");
    }
    if matches > 1 {
        return Err("more than one GPT entry has the GUID");
    }
    if let Some(partition) = found {
        if partition.first < header.first_usable || partition.last > header.last_usable || partition.first > partition.last {
            return Err("the partition lies outside the disk's usable sectors");
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A disk image with a valid primary GPT holding `parts`.
    fn disk(sectors: usize, parts: &[Partition]) -> Vec<u8> {
        let mut bytes = vec![0u8; sectors * SECTOR];
        let mut entries = vec![0u8; 128 * 128];
        for (index, part) in parts.iter().enumerate() {
            let entry = &mut entries[index * 128..index * 128 + 128];
            entry[..16].copy_from_slice(&part.type_guid);
            entry[16..32].copy_from_slice(&part.unique);
            entry[32..40].copy_from_slice(&part.first.to_le_bytes());
            entry[40..48].copy_from_slice(&part.last.to_le_bytes());
        }
        bytes[2 * SECTOR..2 * SECTOR + entries.len()].copy_from_slice(&entries);
        let header = &mut bytes[SECTOR..2 * SECTOR];
        header[..8].copy_from_slice(b"EFI PART");
        header[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
        header[12..16].copy_from_slice(&92u32.to_le_bytes());
        header[24..32].copy_from_slice(&1u64.to_le_bytes());
        header[32..40].copy_from_slice(&(sectors as u64 - 1).to_le_bytes());
        header[40..48].copy_from_slice(&34u64.to_le_bytes());
        header[48..56].copy_from_slice(&(sectors as u64 - 34).to_le_bytes());
        header[72..80].copy_from_slice(&2u64.to_le_bytes());
        header[80..84].copy_from_slice(&128u32.to_le_bytes());
        header[84..88].copy_from_slice(&128u32.to_le_bytes());
        header[88..92].copy_from_slice(&crc32(&entries).to_le_bytes());
        let crc = crc32(&header[..92]);
        header[16..20].copy_from_slice(&crc.to_le_bytes());
        bytes
    }

    fn reader(bytes: &[u8]) -> impl FnMut(u64, &mut [u8; SECTOR]) -> Result<(), &'static str> + '_ {
        move |lba, out| {
            let at = lba as usize * SECTOR;
            out.copy_from_slice(bytes.get(at..at + SECTOR).ok_or("past the end")?);
            Ok(())
        }
    }

    const UNIQUE: Guid = guid(0x11112222, 0x3333, 0x4444, [5, 5, 6, 6, 6, 6, 6, 6]);

    #[test]
    fn crc32_matches_the_published_check_value() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }

    #[test]
    fn guid_text_round_trips_in_the_on_disk_order() {
        let parsed = parse_guid("DAC951C3-27DF-4E3D-8B9E-487906AC2C81").unwrap();
        assert_eq!(parsed, TEST_ROOT_TYPE);
        assert_eq!(&parsed[..4], &[0xc3, 0x51, 0xc9, 0xda]);
        let mut text = [0u8; 36];
        format_guid(&parsed, &mut text);
        assert_eq!(&text, b"dac951c3-27df-4e3d-8b9e-487906ac2c81");
        for bad in ["dac951c3-27df-4e3d-8b9e-487906ac2c8", "dac951c3x27df-4e3d-8b9e-487906ac2c81", "gac951c3-27df-4e3d-8b9e-487906ac2c81"] {
            assert_eq!(parse_guid(bad), None, "{bad}");
        }
    }

    #[test]
    fn finds_the_named_partition_and_only_it() {
        let part = Partition { type_guid: SYSTEM_TYPE, unique: UNIQUE, first: 100, last: 199 };
        let other = Partition { type_guid: HOME_TYPE, unique: guid(9, 9, 9, [9; 8]), first: 200, last: 299 };
        let bytes = disk(400, &[other, part]);
        let header = header(&bytes[SECTOR..2 * SECTOR], 400).unwrap();
        assert_eq!(find(&header, &UNIQUE, reader(&bytes)).unwrap(), Some(part));
        assert_eq!(find(&header, &guid(1, 2, 3, [4; 8]), reader(&bytes)).unwrap(), None);
    }

    #[test]
    fn damage_and_ambiguity_are_refused() {
        let part = Partition { type_guid: SYSTEM_TYPE, unique: UNIQUE, first: 100, last: 199 };
        let mut bytes = disk(400, &[part]);
        // A changed entry byte breaks the entry array's CRC.
        bytes[2 * SECTOR + 40] ^= 1;
        let header_ok = header(&bytes[SECTOR..2 * SECTOR], 400).unwrap();
        assert!(find(&header_ok, &UNIQUE, reader(&bytes)).is_err());
        // A changed header byte breaks the header CRC.
        let mut bytes = disk(400, &[part]);
        bytes[SECTOR + 40] ^= 1;
        assert!(header(&bytes[SECTOR..2 * SECTOR], 400).is_err());
        // Two entries with one unique GUID.
        let twin = Partition { first: 200, last: 210, ..part };
        let bytes = disk(400, &[part, twin]);
        let header_twin = header(&bytes[SECTOR..2 * SECTOR], 400).unwrap();
        assert!(find(&header_twin, &UNIQUE, reader(&bytes)).is_err());
        // A partition past the last usable sector.
        let outside = Partition { first: 100, last: 390, ..part };
        let bytes = disk(400, &[outside]);
        let header_out = header(&bytes[SECTOR..2 * SECTOR], 400).unwrap();
        assert!(find(&header_out, &UNIQUE, reader(&bytes)).is_err());
        // No table, or a disk too small for the header's entries.
        assert!(header(&[0u8; SECTOR], 400).is_err());
        let bytes = disk(400, &[part]);
        assert!(header(&bytes[SECTOR..2 * SECTOR], 30).is_err());
    }
}
