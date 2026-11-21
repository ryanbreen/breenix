//! Append two partitions to the stub's 512-byte-sector GPT layout (UEFI 2.10 §5.3).

const SECTOR: usize = 512;
const HEADER: usize = 92;
pub(super) const ENTRIES: usize = 128;
pub(super) const ENTRY: usize = 128;
pub(super) const ARRAY_BYTES: usize = ENTRIES * ENTRY;
pub(super) const ARRAY_SECTORS: u64 = (ARRAY_BYTES / SECTOR) as u64;
pub(super) const ORIGINAL_PARTS: usize = 4;

type Error = &'static str;

#[derive(Clone, Copy)]
pub(super) struct Part {
    pub(super) type_guid: [u8; 16],
    pub(super) unique: [u8; 16],
    pub(super) first: u64,
    pub(super) last: u64,
    pub(super) name: [u8; 36],
}

impl Part {
    pub(super) fn encode(&self, entry: &mut [u8]) {
        entry[..16].copy_from_slice(&self.type_guid);
        entry[16..32].copy_from_slice(&self.unique);
        entry[32..40].copy_from_slice(&self.first.to_le_bytes());
        entry[40..48].copy_from_slice(&self.last.to_le_bytes());
        for (at, &byte) in self.name.iter().enumerate() {
            entry[56 + 2 * at] = byte;
        }
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

// The reflected IEEE polynomial used by EFI CalculateCrc32.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let low = crc & 1;
            crc = (crc >> 1) ^ (0xedb8_8320 * low);
        }
    }
    !crc
}

pub(super) struct Header {
    bytes: [u8; SECTOR],
}

impl Header {
    /// Check the header and the array's placement before using its LBA for I/O.
    pub(super) fn parse(bytes: &[u8], at: u64, last: u64) -> Result<Self, Error> {
        let mut header = Self {
            bytes: bytes
                .try_into()
                .map_err(|_| "wrong GPT header sector size")?,
        };
        if &bytes[..8] != b"EFI PART"
            || read_u32(bytes, 8) != 0x0001_0000
            || read_u32(bytes, 12) as usize != HEADER
            || bytes[20..24] != [0; 4]
            || bytes[HEADER..].iter().any(|&byte| byte != 0)
            || read_u32(bytes, 80) as usize != ENTRIES
            || read_u32(bytes, 84) as usize != ENTRY
        {
            return Err("unsupported GPT header or entry format");
        }
        let (mine, other) = match at {
            1 => (1, last),
            lba if lba == last => (last, 1),
            _ => return Err("unexpected GPT header location"),
        };
        if read_u64(bytes, 24) != mine || read_u64(bytes, 32) != other {
            return Err("invalid GPT header locations");
        }
        let stored_crc = read_u32(bytes, 16);
        if header.set_header_crc() != stored_crc {
            return Err("GPT header CRC mismatch");
        }

        let first_usable = read_u64(bytes, 40);
        let last_usable = read_u64(bytes, 48);
        let array = header.array_lba();
        let after_array = array
            .checked_add(ARRAY_SECTORS)
            .ok_or("GPT array LBA overflow")?;
        let placed = if at == 1 {
            array > 1 && after_array <= first_usable
        } else {
            array > last_usable && after_array <= last
        };
        if !placed || first_usable > last_usable || last_usable >= last {
            return Err("invalid GPT usable range or array location");
        }
        Ok(header)
    }

    pub(super) fn array_lba(&self) -> u64 {
        read_u64(&self.bytes, 72)
    }

    fn set_header_crc(&mut self) -> u32 {
        self.bytes[16..20].fill(0);
        let crc = crc32(&self.bytes[..HEADER]);
        self.bytes[16..20].copy_from_slice(&crc.to_le_bytes());
        crc
    }

    fn set_array_crc(&mut self, crc: u32) {
        self.bytes[88..92].copy_from_slice(&crc.to_le_bytes());
        self.set_header_crc();
    }

    fn validate_array(&self, array: &[u8]) -> Result<(), Error> {
        if array.len() != ARRAY_BYTES || crc32(array) != read_u32(&self.bytes, 88) {
            return Err("GPT entry-array CRC mismatch");
        }
        Ok(())
    }
}

pub(super) fn validate_pair(
    primary: &Header,
    backup: &Header,
    entries: &[u8],
    backup_entries: &[u8],
    expected: [u8; 16],
) -> Result<(), Error> {
    if expected == [0; 16] || primary.bytes[56..72] != expected || backup.bytes[56..72] != expected
    {
        return Err("wrong GPT disk GUID");
    }
    if read_u64(&primary.bytes, 24) != 1
        || read_u64(&backup.bytes, 32) != 1
        || read_u64(&primary.bytes, 32) != read_u64(&backup.bytes, 24)
        || primary.bytes[40..56] != backup.bytes[40..56]
    {
        return Err("GPT headers disagree");
    }
    primary.validate_array(entries)?;
    backup.validate_array(backup_entries)?;
    if entries != backup_entries {
        return Err("GPT entry arrays disagree");
    }

    Ok(())
}

pub(super) struct Append<'a> {
    entries: &'a [u8],
    primary: Header,
    backup: Header,
    write_gpt: bool,
}

/// Check both GPT copies, and append or recognize the exact requested entries 5 and 6.
pub(super) fn prepare<'a>(
    mut primary: Header,
    mut backup: Header,
    entries: &'a mut [u8],
    backup_entries: &[u8],
    expected: [u8; 16],
    parts: [Part; 2],
) -> Result<Append<'a>, Error> {
    validate_pair(&primary, &backup, entries, backup_entries, expected)?;

    let mut end = read_u64(&primary.bytes, 40) - 1;
    let last_usable = read_u64(&primary.bytes, 48);
    let mut guids = [[0; 16]; 1 + ORIGINAL_PARTS + 2];
    guids[0] = expected;
    let mut check = |index, type_guid, unique, first, last| {
        if type_guid == [0; 16] || first <= end || last < first || last > last_usable {
            return Err("partition overlaps or lies outside usable GPT space");
        }
        if unique == [0; 16] || guids[..index].contains(&unique) {
            return Err("empty or repeated partition GUID");
        }
        guids[index] = unique;
        end = last;
        Ok(())
    };
    let (existing, empty) = entries.split_at_mut(ORIGINAL_PARTS * ENTRY);
    for (index, entry) in existing.chunks_exact(ENTRY).enumerate() {
        check(
            index + 1,
            entry[..16].try_into().unwrap(),
            entry[16..32].try_into().unwrap(),
            read_u64(entry, 32),
            read_u64(entry, 40),
        )?;
    }
    for (index, part) in parts.iter().enumerate() {
        check(
            index + ORIGINAL_PARTS + 1,
            part.type_guid,
            part.unique,
            part.first,
            part.last,
        )?;
    }
    let mut appended = [0; 2 * ENTRY];
    for (part, entry) in parts.iter().zip(appended.chunks_exact_mut(ENTRY)) {
        part.encode(entry);
    }
    let write_gpt = empty.iter().all(|&byte| byte == 0);
    if write_gpt {
        empty[..appended.len()].copy_from_slice(&appended);
        let crc = crc32(entries);
        primary.set_array_crc(crc);
        backup.set_array_crc(crc);
    } else if empty[..appended.len()] != appended
        || empty[appended.len()..].iter().any(|&byte| byte != 0)
    {
        return Err("existing GPT entries differ from the add-breenfs request");
    }
    Ok(Append {
        entries,
        primary,
        backup,
        write_gpt,
    })
}

impl Append<'_> {
    /// Each writer call must flush and verify its data before returning success.
    pub(super) fn write<D, E, W>(
        &self,
        disk: &mut D,
        write_images: impl FnOnce(&mut D, &mut W) -> Result<(), E>,
        write: &mut W,
    ) -> Result<(), E>
    where
        W: FnMut(&mut D, u64, &[u8]) -> Result<(), E>,
    {
        // The old GPT hides the images. The old primary survives the backup update,
        // and the flushed new backup survives the primary update.
        write_images(disk, write)?;
        if self.write_gpt {
            for header in [&self.backup, &self.primary] {
                write(disk, header.array_lba(), self.entries)?;
                write(disk, read_u64(&header.bytes, 24), &header.bytes)?;
            }
        }
        Ok(())
    }
}
