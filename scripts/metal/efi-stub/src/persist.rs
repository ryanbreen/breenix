//! The persistent install's requests (docs/metal/README.md, "Install and upgrade"), from UEFI 2.10 sections 3.1.3,
//! 5.2.3, 5.3, 7.3, 8.2, 13.5 and 13.9.
//!
//! `persist.cfg` on the boot disk names an action. `install` (the one-time install) writes the GUID partition table
//! and the partitions' contents as extents onto the one fixed disk whose first sector is the recorded whole-disk root
//! record, invalidating the old layout first and writing the protective MBR last, then reconnects the disk so the
//! firmware reads the new table. `add-breenfs` validates both GPT copies, writes two images and appends their
//! entries. `upgrade` checks the disk's GUID and writes files into the EFI system partition.
//! Install and upgrade also write `MANAGER.CFG` (the lab's lines, the HTTP boot port this stub was loaded through, and the
//! HTTP boot option) and puts the boot manager's load option first in `BootOrder`, the HTTP boot option second.
//! Each write is read back and compared. An ordinary delivery has no request and takes the existing boot path.
use core::ffi::c_void;
use core::ptr::{null, null_mut};

use metal_efi::bootdisk::SECTOR;
use metal_efi::files::{self, Root};
use metal_efi::say;
use metal_efi::uefi::*;

use crate::suite_disk::{partition_signature, Buffer};

#[path = "gpt_append.rs"]
mod gpt_append;
use gpt_append::{Header, Part, ARRAY_BYTES, ARRAY_SECTORS, ENTRIES, ENTRY, ORIGINAL_PARTS};

const MAGIC: &str = "BREENIX-PERSIST 1";
const PARTITIONS: usize = ORIGINAL_PARTS + 2;
const CHUNK_SECTORS: usize = 128;
const CONFIG_LINES: usize = 16;
const MANAGER_FILE: &str = "\\EFI\\BREENIX\\BOOTMGR.EFI";
const CONFIG_FILE: &str = "\\EFI\\BREENIX\\MANAGER.CFG";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Install,
    Upgrade,
    AddBreenfs,
}

impl Action {
    fn name(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Upgrade => "upgrade",
            Self::AddBreenfs => "add-breenfs",
        }
    }
}

/// The GUID's on-disk bytes from its text form (the first three fields little-endian).
fn guid(text: &str) -> Option<[u8; 16]> {
    let bytes = text.as_bytes();
    if bytes.len() != 36 || [8, 13, 18, 23].iter().any(|&at| bytes[at] != b'-') {
        return None;
    }
    let mut digits = [0u8; 32];
    let mut count = 0;
    for &byte in bytes.iter().filter(|&&b| b != b'-') {
        digits[count] = (byte as char).to_digit(16)? as u8;
        count += 1;
    }
    if count != digits.len() {
        return None;
    }
    let byte = |index: usize| digits[2 * index] << 4 | digits[2 * index + 1];
    let order = [3usize, 2, 1, 0, 5, 4, 7, 6, 8, 9, 10, 11, 12, 13, 14, 15];
    let mut out = [0u8; 16];
    for (at, &from) in order.iter().enumerate() {
        out[at] = byte(from);
    }
    Some(out)
}

fn number(text: &str) -> Result<u64, Status> {
    text.parse().map_err(|_| INVALID_PARAMETER)
}

fn hex_digit(nibble: u8) -> u8 {
    b"0123456789abcdef"[nibble as usize]
}

/// The request's header: what it does and to which disk.
struct Request<'t> {
    action: Action,
    expect_record: Option<[u8; SECTOR]>,
    expect_disk: Option<[u8; 16]>,
    disk_guid: [u8; 16],
    parts: [Option<Part>; PARTITIONS],
    esp: Option<[u8; 16]>,
    config: [&'t str; CONFIG_LINES],
    config_len: usize,
    load_option: Option<&'t str>,
    migration_slots: bool,
    breenfs_upgrade: Option<[[u8; 16]; 4]>,
}

fn parse<'t>(text: &'t str) -> Result<Request<'t>, Status> {
    let mut lines = text.lines();
    if lines.next() != Some(MAGIC) {
        return Err(INVALID_PARAMETER);
    }
    let mut request = Request {
        action: Action::Upgrade,
        expect_record: None,
        expect_disk: None,
        disk_guid: [0; 16],
        parts: [None; PARTITIONS],
        esp: None,
        config: [""; CONFIG_LINES],
        config_len: 0,
        load_option: None,
        migration_slots: false,
        breenfs_upgrade: None,
    };
    let mut action = None;
    for line in lines {
        let (item, value) = line.split_once(' ').unwrap_or((line, ""));
        match item {
            "action" if action.is_none() => action = Some(value),
            "expect-record" => {
                if value.len() != 2 * SECTOR {
                    return Err(INVALID_PARAMETER);
                }
                let mut record = [0u8; SECTOR];
                for (index, pair) in value.as_bytes().chunks(2).enumerate() {
                    let text = core::str::from_utf8(pair).map_err(|_| INVALID_PARAMETER)?;
                    record[index] = u8::from_str_radix(text, 16).map_err(|_| INVALID_PARAMETER)?;
                }
                request.expect_record = Some(record);
            }
            "expect-disk" if request.expect_disk.is_none() => {
                request.expect_disk = Some(guid(value).ok_or(INVALID_PARAMETER)?);
            }
            "disk" => request.disk_guid = guid(value).ok_or(INVALID_PARAMETER)?,
            "esp" => request.esp = Some(guid(value).ok_or(INVALID_PARAMETER)?),
            "partition" => {
                let mut words = value.splitn(6, ' ');
                let mut next = || words.next().ok_or(INVALID_PARAMETER);
                let index = number(next()?)? as usize;
                let type_guid = guid(next()?).ok_or(INVALID_PARAMETER)?;
                let unique = guid(next()?).ok_or(INVALID_PARAMETER)?;
                let first = number(next()?)?;
                let last = number(next()?)?;
                let name_text = next()?;
                if !(1..=PARTITIONS).contains(&index)
                    || request.parts[index - 1].is_some()
                    || first > last
                    || name_text.len() > 36
                    || !name_text.is_ascii()
                {
                    return Err(INVALID_PARAMETER);
                }
                let mut name = [0u8; 36];
                name[..name_text.len()].copy_from_slice(name_text.as_bytes());
                request.parts[index - 1] = Some(Part { type_guid, unique, first, last, name });
            }
            "manager-config" if request.config_len < CONFIG_LINES => {
                request.config[request.config_len] = value;
                request.config_len += 1;
            }
            "load-option" => request.load_option = Some(value),
            "migration-slots" if value.is_empty() && !request.migration_slots => request.migration_slots = true,
            "breenfs-upgrade" if request.breenfs_upgrade.is_none() => {
                let mut words = value.split(' ');
                let mut guids = [[0; 16]; 4];
                for index in 0..guids.len() {
                    let unique = words.next().and_then(guid).ok_or(INVALID_PARAMETER)?;
                    if unique == [0; 16] || guids[..index].contains(&unique) {
                        return Err(INVALID_PARAMETER);
                    }
                    guids[index] = unique;
                }
                if words.next().is_some() {
                    return Err(INVALID_PARAMETER);
                }
                request.breenfs_upgrade = Some(guids);
            }
            "rescue-file" if !value.is_empty() => {}
            "extent" | "esp-file" if !value.is_empty() => {}
            _ => return Err(INVALID_PARAMETER),
        }
    }
    request.action = match action {
        Some("install") => Action::Install,
        Some("upgrade") => Action::Upgrade,
        Some("add-breenfs") => Action::AddBreenfs,
        _ => return Err(INVALID_PARAMETER),
    };
    let rescue_files = text.lines().filter_map(|line| line.strip_prefix("rescue-file "));
    if request.migration_slots {
        let files = rescue_files.map(|line| line.split(' ').next().unwrap_or(""));
        let candidate = text.lines().filter_map(|line| line.strip_prefix("esp-file "))
            .map(|line| line.split(' ').next().unwrap_or(""));
        if request.action != Action::Upgrade || request.config_len != 0 || request.load_option.is_some()
            || candidate.ne(["\\EFI\\BREENIX\\NEXT.IMG", "\\EFI\\BREENIX\\NEXT.TXT"])
            || files.ne([
            "\\EFI\\BREENIX\\NEXT.IMG", "\\EFI\\BREENIX\\NEXT.TXT",
        ]) {
            return Err(INVALID_PARAMETER);
        }
    } else if rescue_files.count() != 0 {
        return Err(INVALID_PARAMETER);
    }
    if request.breenfs_upgrade.is_some()
        && (request.action != Action::Upgrade || request.migration_slots
            || request.expect_record.is_some() || request.parts.iter().any(Option::is_some)
            || text.lines().any(|line| line.starts_with("extent ")))
    {
        return Err(INVALID_PARAMETER);
    }
    let (original, additions) = request.parts.split_at(ORIGINAL_PARTS);
    match request.action {
        Action::Install => {
            if request.expect_record.is_none()
                || original.iter().any(Option::is_none)
                || additions.iter().any(Option::is_some)
            {
                return Err(INVALID_PARAMETER);
            }
            request.esp = request.parts[0].map(|part| part.unique);
        }
        Action::Upgrade => {
            if request.expect_disk.is_none()
                || request.esp.is_none()
                || additions.iter().any(Option::is_some)
            {
                return Err(INVALID_PARAMETER);
            }
        }
        Action::AddBreenfs if request.expect_disk.is_none()
            || original.iter().any(Option::is_some)
            || additions.iter().any(Option::is_none)
            || request.expect_record.is_some()
            || request.esp.is_some()
            || request.disk_guid != [0; 16]
            || request.config_len != 0
            || request.load_option.is_some()
            || text.lines().any(|line| line.starts_with("esp-file")) =>
        {
            return Err(INVALID_PARAMETER);
        }
        Action::AddBreenfs => {}
    }
    Ok(request)
}

/// The one fixed, writable, 512-byte-sector whole disk: its handle and Block I/O.
unsafe fn the_disk(boot: &BootServices) -> Result<(Handle, *mut BlockIo), Status> {
    let mut count = 0;
    let mut handles = null_mut();
    ok((boot.locate_handle_buffer)(BY_PROTOCOL, &BLOCK_IO, null(), &mut count, &mut handles))?;
    let mut found = None;
    let mut matches = 0;
    for index in 0..count {
        let handle = *handles.add(index);
        let mut protocol = null_mut();
        if (boot.handle_protocol)(handle, &BLOCK_IO, &mut protocol) != SUCCESS {
            continue;
        }
        let disk = protocol.cast::<BlockIo>();
        let media = &*(*disk).media;
        if media.logical_partition == 0
            && media.removable_media == 0
            && media.media_present != 0
            && media.read_only == 0
            && media.block_size == SECTOR as u32
        {
            matches += 1;
            found = Some((handle, disk));
        }
    }
    (boot.free_pool)(handles.cast());
    match (matches, found) {
        (1, Some(found)) => Ok(found),
        _ => {
            say!("persistent install refused: {} fixed writable disks, expected exactly one", matches);
            Err(NOT_FOUND)
        }
    }
}

struct Disk<'b> {
    io: *mut BlockIo,
    media_id: u32,
    last: u64,
    wire: Buffer<'b>,
    check: Buffer<'b>,
    written: u64,
}

impl<'b> Disk<'b> {
    unsafe fn new(boot: &'b BootServices, io: *mut BlockIo) -> Result<Self, Status> {
        let media = &*(*io).media;
        Ok(Self {
            io,
            media_id: media.media_id,
            last: media.last_block,
            wire: Buffer::new(boot, CHUNK_SECTORS * SECTOR, media.io_align as usize)?,
            check: Buffer::new(boot, CHUNK_SECTORS * SECTOR, media.io_align as usize)?,
            written: 0,
        })
    }

    unsafe fn read(&self, lba: u64, sectors: usize) -> Result<&[u8], Status> {
        ok(((*self.io).read_blocks)(self.io, self.media_id, lba, sectors * SECTOR, self.check.data.cast()))?;
        Ok(core::slice::from_raw_parts(self.check.data, sectors * SECTOR))
    }

    /// Writes the wire buffer's first `sectors` sectors at `lba`, then reads them back and compares.
    unsafe fn write_wire(&mut self, lba: u64, sectors: usize) -> Result<(), Status> {
        if sectors == 0 || lba.checked_add(sectors as u64 - 1).map_or(true, |end| end > self.last) {
            return Err(INVALID_PARAMETER);
        }
        ok(((*self.io).write_blocks)(self.io, self.media_id, lba, sectors * SECTOR, self.wire.data.cast()))?;
        ok(((*self.io).flush_blocks)(self.io))?;
        let wanted = core::slice::from_raw_parts(self.wire.data, sectors * SECTOR);
        if self.read(lba, sectors)? != wanted {
            say!("persistent install: LBA {} read back differently", lba);
            return Err(DEVICE_ERROR);
        }
        let before = self.written;
        self.written += (sectors * SECTOR) as u64;
        if before >> 26 != self.written >> 26 {
            say!("persistent install: {} MiB written and read back", self.written >> 20);
        }
        Ok(())
    }

    fn wire(&mut self) -> &mut [u8] {
        // SAFETY: the wire buffer is CHUNK_SECTORS sectors this disk owns.
        unsafe { core::slice::from_raw_parts_mut(self.wire.data, CHUNK_SECTORS * SECTOR) }
    }

    /// Writes `bytes` (whole sectors) at `lba`, a chunk at a time.
    unsafe fn write(&mut self, lba: u64, bytes: &[u8]) -> Result<(), Status> {
        let mut at = 0;
        while at < bytes.len() {
            let length = (bytes.len() - at).min(CHUNK_SECTORS * SECTOR);
            self.wire()[..length].copy_from_slice(&bytes[at..at + length]);
            self.write_wire(lba + (at / SECTOR) as u64, length / SECTOR)?;
            at += length;
        }
        Ok(())
    }

    unsafe fn zero(&mut self, lba: u64, sectors: u64) -> Result<(), Status> {
        self.wire().fill(0);
        let mut done = 0;
        while done < sectors {
            let count = (sectors - done).min(CHUNK_SECTORS as u64) as usize;
            self.write_wire(lba + done, count)?;
            done += count as u64;
        }
        Ok(())
    }

    unsafe fn write_extent(&mut self, lba: u64, count: u64, offset: Option<u64>, payload: *mut File) -> Result<(), Status> {
        let Some(mut offset) = offset else {
            return self.zero(lba, count);
        };
        let mut done = 0;
        while done < count {
            let sectors = (count - done).min(CHUNK_SECTORS as u64) as usize;
            data(payload, offset, &mut self.wire()[..sectors * SECTOR])?;
            self.write_wire(lba + done, sectors)?;
            done += sectors as u64;
            offset += (sectors * SECTOR) as u64;
        }
        Ok(())
    }
}

unsafe fn crc32(boot: &BootServices, bytes: &[u8]) -> Result<u32, Status> {
    let mut crc = 0u32;
    ok((boot.calculate_crc32)(bytes.as_ptr().cast(), bytes.len(), &mut crc))?;
    Ok(crc)
}

/// A GPT header (section 5.3.2) for one copy: `mine` its own LBA, `other` the other copy's, `entries` where its
/// entry array lies.
unsafe fn header(boot: &BootServices, request: &Request, last: u64, mine: u64, other: u64, entries: u64, entries_crc: u32) -> Result<[u8; SECTOR], Status> {
    let mut sector = [0u8; SECTOR];
    sector[..8].copy_from_slice(b"EFI PART");
    sector[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
    sector[12..16].copy_from_slice(&92u32.to_le_bytes());
    sector[24..32].copy_from_slice(&mine.to_le_bytes());
    sector[32..40].copy_from_slice(&other.to_le_bytes());
    sector[40..48].copy_from_slice(&(2 + ARRAY_SECTORS).to_le_bytes());
    sector[48..56].copy_from_slice(&(last - 1 - ARRAY_SECTORS).to_le_bytes());
    sector[56..72].copy_from_slice(&request.disk_guid);
    sector[72..80].copy_from_slice(&entries.to_le_bytes());
    sector[80..84].copy_from_slice(&(ENTRIES as u32).to_le_bytes());
    sector[84..88].copy_from_slice(&(ENTRY as u32).to_le_bytes());
    sector[88..92].copy_from_slice(&entries_crc.to_le_bytes());
    let crc = crc32(boot, &sector[..92])?;
    sector[16..20].copy_from_slice(&crc.to_le_bytes());
    Ok(sector)
}

/// The protective MBR (section 5.2.3): one partition of type 0xEE from LBA 1 over the disk, or 2^32 - 1 sectors.
fn protective_mbr(last: u64) -> [u8; SECTOR] {
    let mut sector = [0u8; SECTOR];
    let record = &mut sector[446..462];
    record[1..4].copy_from_slice(&[0x00, 0x02, 0x00]);
    record[4] = 0xee;
    record[5..8].copy_from_slice(&[0xff, 0xff, 0xff]);
    record[8..12].copy_from_slice(&1u32.to_le_bytes());
    record[12..16].copy_from_slice(&(last.min(0xffff_ffff) as u32).to_le_bytes());
    sector[510] = 0x55;
    sector[511] = 0xaa;
    sector
}

/// Reads `length` bytes of persist.dat from `offset` into `out`.
unsafe fn data(file: *mut File, offset: u64, out: &mut [u8]) -> Result<(), Status> {
    ok(((*file).set_position)(file, offset))?;
    let mut at = 0;
    while at < out.len() {
        let mut size = out.len() - at;
        ok(((*file).read)(file, &mut size, out[at..].as_mut_ptr().cast()))?;
        if size == 0 {
            return Err(VOLUME_CORRUPTED);
        }
        at += size;
    }
    Ok(())
}

/// The one-time install: invalidate the old layout, write each extent, then the table copies and the MBR last.
unsafe fn install(boot: &BootServices, disk: &mut Disk, request: &Request, text: &str, payload: *mut File) -> Result<u64, Status> {
    let parts: [Part; ORIGINAL_PARTS] = core::array::from_fn(|index| request.parts[index].unwrap());
    if parts[ORIGINAL_PARTS - 1].last + 1 + ARRAY_SECTORS > disk.last {
        say!("persistent install refused: the layout does not fit the disk");
        return Err(BAD_BUFFER_SIZE);
    }
    // No old root record or table survives an interrupted install.
    disk.zero(0, 2 + ARRAY_SECTORS)?;
    disk.zero(disk.last - ARRAY_SECTORS, ARRAY_SECTORS + 1)?;
    let mut extents = 0u64;
    for line in text.lines().filter(|line| line.starts_with("extent ")) {
        let mut words = line[7..].split(' ');
        let mut next = || words.next().ok_or(INVALID_PARAMETER);
        let index = number(next()?)? as usize;
        let first = number(next()?)?;
        let count = number(next()?)?;
        let kind = next()?;
        let part = parts.get(index.wrapping_sub(1)).ok_or(INVALID_PARAMETER)?;
        if first.checked_add(count).map_or(true, |end| end > part.last - part.first + 1) {
            return Err(INVALID_PARAMETER);
        }
        let lba = part.first + first;
        let offset = match kind {
            "zero" => None,
            "data" => Some(number(next()?)?),
            _ => return Err(INVALID_PARAMETER),
        };
        disk.write_extent(lba, count, offset, payload)?;
        extents += 1;
    }
    // The entry array (section 5.3.3), then both headers, then the protective MBR.
    let entries = Buffer::new(boot, ARRAY_BYTES, 8)?;
    let array = core::slice::from_raw_parts_mut(entries.data, ARRAY_BYTES);
    array.fill(0);
    for (part, entry) in parts.iter().zip(array.chunks_exact_mut(ENTRY)) {
        part.encode(entry);
    }
    let entries_crc = crc32(boot, array)?;
    let last = disk.last;
    disk.write(last - ARRAY_SECTORS, array)?;
    disk.write(last, &header(boot, request, last, last, 1, last - ARRAY_SECTORS, entries_crc)?)?;
    disk.write(2, array)?;
    disk.write(1, &header(boot, request, last, 1, last, 2, entries_crc)?)?;
    disk.write(0, &protective_mbr(last))?;
    say!(
        "persistent install: layout written: {} partitions, {} extents, {} bytes",
        ORIGINAL_PARTS,
        extents,
        disk.written
    );
    Ok(extents)
}

struct Extent {
    part: usize,
    first: u64,
    count: u64,
    offset: Option<u64>,
}

fn append_extent(line: &str, parts: &[Part; 2], payload_size: u64) -> Result<Extent, Status> {
    let mut words = line.split(' ');
    let mut next = || words.next().ok_or(INVALID_PARAMETER);
    let part = number(next()?)?
        .checked_sub(ORIGINAL_PARTS as u64 + 1)
        .filter(|&part| part < parts.len() as u64)
        .ok_or(INVALID_PARAMETER)? as usize;
    let first = number(next()?)?;
    let count = number(next()?)?;
    let offset = match next()? {
        "zero" => None,
        "data" => Some(number(next()?)?),
        _ => return Err(INVALID_PARAMETER),
    };
    let partition = parts[part];
    if words.next().is_some()
        || count == 0
        || first.checked_add(count).map_or(true, |end| end > partition.last - partition.first + 1)
    {
        return Err(INVALID_PARAMETER);
    }
    if let Some(offset) = offset {
        let payload_end = count.checked_mul(SECTOR as u64).and_then(|size| offset.checked_add(size));
        if payload_end.map_or(true, |end| end > payload_size) {
            return Err(INVALID_PARAMETER);
        }
    }
    Ok(Extent { part, first, count, offset })
}

fn refuse_append(reason: &str) -> Status {
    say!("add-breenfs refused: {}", reason);
    VOLUME_CORRUPTED
}

unsafe fn add_breenfs(
    boot: &BootServices,
    disk: &mut Disk,
    request: &Request,
    text: &str,
    payload: *mut File,
    payload_size: u64,
) -> Result<(), Status> {
    let primary = Header::parse(disk.read(1, 1)?, 1, disk.last).map_err(refuse_append)?;
    let backup = Header::parse(disk.read(disk.last, 1)?, disk.last, disk.last).map_err(refuse_append)?;
    let entries = Buffer::new(boot, ARRAY_BYTES, 8)?;
    let array = core::slice::from_raw_parts_mut(entries.data, ARRAY_BYTES);
    array.copy_from_slice(disk.read(primary.array_lba(), ARRAY_SECTORS as usize)?);
    let backup_array = disk.read(backup.array_lba(), ARRAY_SECTORS as usize)?;
    let parts = core::array::from_fn(|index| request.parts[index + ORIGINAL_PARTS].unwrap());
    let update = gpt_append::prepare(primary, backup, array, backup_array, request.expect_disk.unwrap(), parts)
        .map_err(refuse_append)?;

    // Check the entire extent list before the first write. Gaps in the sparse images are allowed.
    let extents = || {
        text.lines().filter_map(|line| line.strip_prefix("extent "))
            .map(|line| append_extent(line, &parts, payload_size))
    };
    let mut end = [0; 2];
    for extent in extents() {
        let extent = extent?;
        if extent.first < end[extent.part] {
            return Err(refuse_append("partition extents overlap or are out of order"));
        }
        end[extent.part] = extent.first + extent.count;
    }
    if end.contains(&0) {
        return Err(refuse_append("missing partition extents"));
    }
    update.write(
        disk,
        |disk, _| {
            for extent in extents() {
                let extent = extent?;
                let lba = parts[extent.part].first + extent.first;
                disk.write_extent(lba, extent.count, extent.offset, payload)?;
            }
            Ok(())
        },
        &mut |disk, lba, bytes| disk.write(lba, bytes),
    )?;
    say!("add-breenfs: images and both GPT copies written and read back");
    Ok(())
}

/// The partition handle whose unique GUID is `unique`.
unsafe fn partition_handle(boot: &BootServices, unique: &[u8; 16]) -> Result<Handle, Status> {
    let mut count = 0;
    let mut handles = null_mut();
    ok((boot.locate_handle_buffer)(BY_PROTOCOL, &BLOCK_IO, null(), &mut count, &mut handles))?;
    let mut found = None;
    for index in 0..count {
        let handle = *handles.add(index);
        if partition_signature(boot, handle).as_ref() == Some(unique) {
            found = Some(handle);
        }
    }
    (boot.free_pool)(handles.cast());
    found.ok_or(NOT_FOUND)
}

/// The device path this stub was loaded from (the firmware's HTTP boot port and URI), into `out`; its length.
unsafe fn own_path(boot: &BootServices, image: Handle, out: &mut [u8]) -> Result<usize, Status> {
    let mut loaded = null_mut();
    ok((boot.handle_protocol)(image, &LOADED_IMAGE, &mut loaded))?;
    let mut path: *mut c_void = null_mut();
    ok((boot.handle_protocol)((*loaded.cast::<LoadedImage>()).device_handle, &DEVICE_PATH, &mut path))?;
    let len = path_length(path.cast()).ok_or(INVALID_PARAMETER)? + END_NODE.len();
    if len > out.len() {
        return Err(BAD_BUFFER_SIZE);
    }
    out[..len].copy_from_slice(core::slice::from_raw_parts(path.cast::<u8>(), len));
    Ok(len)
}

unsafe fn variable(runtime: &RuntimeServices, name: &str, out: &mut [u8]) -> Result<usize, Status> {
    let wide = Wide::<16>::new(name);
    let mut size = out.len();
    let mut attributes = 0u32;
    ok((runtime.get_variable)(wide.as_ptr(), &GLOBAL_VARIABLE, &mut attributes, &mut size, out.as_mut_ptr().cast()))?;
    Ok(size)
}

unsafe fn set_variable(runtime: &RuntimeServices, name: &str, bytes: &[u8]) -> Result<(), Status> {
    let wide = Wide::<16>::new(name);
    let attributes = VARIABLE_NON_VOLATILE | VARIABLE_BOOTSERVICE_ACCESS | VARIABLE_RUNTIME_ACCESS;
    ok((runtime.set_variable)(wide.as_ptr(), &GLOBAL_VARIABLE, attributes, bytes.len(), bytes.as_ptr().cast()))
}

fn option_name(number: u16, out: &mut [u8; 8]) -> &str {
    out[..4].copy_from_slice(b"Boot");
    for index in 0..4 {
        out[4 + index] = hex_digit(((number >> (12 - 4 * index)) & 0xf) as u8).to_ascii_uppercase();
    }
    core::str::from_utf8(&out[..]).unwrap_or("Boot????")
}

/// The HTTP boot option recorded in the existing MANAGER.CFG, for an upgrade booted through the manager.
unsafe fn recorded_http_option(root: &Root) -> Option<u16> {
    let mut text = [0u8; 4096];
    let len = root.read(CONFIG_FILE, &mut text).ok()?;
    core::str::from_utf8(&text[..len]).ok()?.lines().find_map(|line| {
        line.strip_prefix("http-option ").and_then(|hex| u16::from_str_radix(hex, 16).ok())
    })
}

/// MANAGER.CFG: the lab's lines, the HTTP boot port in hex and the HTTP boot option.
unsafe fn write_config(boot: &BootServices, root: &Root, request: &Request, image: Handle, http_option: Option<u16>) -> Result<(), Status> {
    let mut port = [0u8; 512];
    let port_len = own_path(boot, image, &mut port)?;
    let text = Buffer::new(boot, 8192, 8)?;
    let out = core::slice::from_raw_parts_mut(text.data, 8192);
    let mut len = 0;
    let mut put = |bytes: &[u8]| -> Result<(), Status> {
        if len + bytes.len() > out.len() {
            return Err(BAD_BUFFER_SIZE);
        }
        out[len..len + bytes.len()].copy_from_slice(bytes);
        len += bytes.len();
        Ok(())
    };
    for line in &request.config[..request.config_len] {
        put(line.as_bytes())?;
        put(b"\n")?;
    }
    put(b"port ")?;
    for &byte in &port[..port_len] {
        put(&[hex_digit(byte >> 4), hex_digit(byte & 0xf)])?;
    }
    put(b"\n")?;
    if let Some(number) = http_option {
        let mut name = [0u8; 8];
        put(b"http-option ")?;
        put(&option_name(number, &mut name).as_bytes()[4..])?;
        put(b"\n")?;
    }
    root.write(CONFIG_FILE, &out[..len])?;
    let mut readback = [0u8; 8192];
    let read = root.read(CONFIG_FILE, &mut readback)?;
    if readback[..read] != out[..len] {
        return Err(VOLUME_CORRUPTED);
    }
    say!("persistent install: MANAGER.CFG written: {} lines", request.config_len + 1 + http_option.is_some() as usize);
    Ok(())
}

/// The manager's load option, first in BootOrder, the HTTP boot option second (sections 3.1.2 and 3.1.3).
unsafe fn load_option(boot: &BootServices, runtime: &RuntimeServices, esp: Handle, description: &str, http_option: Option<u16>) -> Result<(), Status> {
    let mut path: *mut c_void = null_mut();
    ok((boot.handle_protocol)(esp, &DEVICE_PATH, &mut path))?;
    let prefix = path_length(path.cast()).ok_or(INVALID_PARAMETER)?;
    let mut option = [0u8; 1024];
    let mut len = 0;
    option[..4].copy_from_slice(&LOAD_OPTION_ACTIVE.to_le_bytes());
    len += 6;
    for unit in description.bytes().map(u16::from).chain([0]) {
        option[len..len + 2].copy_from_slice(&unit.to_le_bytes());
        len += 2;
    }
    let path_start = len;
    option[len..len + prefix].copy_from_slice(core::slice::from_raw_parts(path.cast::<u8>(), prefix));
    len += prefix;
    let node = 4 + 2 * (MANAGER_FILE.len() + 1);
    option[len..len + 4].copy_from_slice(&[node::MEDIA, node::FILE_PATH, node as u8, (node >> 8) as u8]);
    len += 4;
    for unit in MANAGER_FILE.bytes().map(u16::from).chain([0]) {
        option[len..len + 2].copy_from_slice(&unit.to_le_bytes());
        len += 2;
    }
    option[len..len + 4].copy_from_slice(&END_NODE);
    len += 4;
    option[4..6].copy_from_slice(&((len - path_start) as u16).to_le_bytes());
    // Reuse the manager's earlier option, else the lowest free number.
    let mut chosen = None;
    let mut free = None;
    for number in 0..=0xffu16 {
        let mut name = [0u8; 8];
        let mut bytes = [0u8; 1024];
        match variable(runtime, option_name(number, &mut name), &mut bytes) {
            Ok(size) => {
                let units = bytes[6..size].chunks(2).take_while(|pair| pair != &[0, 0]).count();
                let matches = units == description.len()
                    && description.bytes().enumerate().all(|(at, b)| bytes[6 + 2 * at] == b && bytes[7 + 2 * at] == 0);
                if matches && chosen.is_none() {
                    chosen = Some(number);
                }
            }
            Err(_) if free.is_none() => free = Some(number),
            Err(_) => {}
        }
    }
    let number = chosen.or(free).ok_or(OUT_OF_RESOURCES)?;
    let mut name = [0u8; 8];
    let option_name_text = option_name(number, &mut name);
    set_variable(runtime, option_name_text, &option[..len])?;
    let mut check = [0u8; 1024];
    if variable(runtime, option_name_text, &mut check)? != len || check[..len] != option[..len] {
        return Err(DEVICE_ERROR);
    }
    let mut order_bytes = [0u8; 256];
    let order_len = variable(runtime, "BootOrder", &mut order_bytes).unwrap_or(0);
    let old: [u16; 128] = core::array::from_fn(|at| {
        if 2 * at + 1 < order_len { u16::from_le_bytes([order_bytes[2 * at], order_bytes[2 * at + 1]]) } else { 0 }
    });
    let old = &old[..order_len / 2];
    let mut order = [0u16; 128];
    let mut count = 0;
    order[count] = number;
    count += 1;
    if let Some(http) = http_option.filter(|http| old.contains(http) && *http != number) {
        order[count] = http;
        count += 1;
    }
    for &entry in old {
        if entry != number && Some(entry) != http_option && count < order.len() {
            order[count] = entry;
            count += 1;
        }
    }
    let mut new_bytes = [0u8; 256];
    for (at, entry) in order[..count].iter().enumerate() {
        new_bytes[2 * at..2 * at + 2].copy_from_slice(&entry.to_le_bytes());
    }
    set_variable(runtime, "BootOrder", &new_bytes[..2 * count])?;
    let mut check = [0u8; 256];
    if variable(runtime, "BootOrder", &mut check)? != 2 * count || check[..2 * count] != new_bytes[..2 * count] {
        return Err(DEVICE_ERROR);
    }
    say!("persistent install: load option {} first in the boot order of {}", option_name_text, count);
    Ok(())
}

unsafe fn write_esp_files(
    boot: &BootServices, root: &files::Root, payload: *mut File, text: &str, prefix: &str,
) -> Result<usize, Status> {
    let mut written = 0;
    for line in text.lines().filter_map(|line| line.strip_prefix(prefix)) {
        let mut words = line.split(' ');
        let name = words.next().ok_or(INVALID_PARAMETER)?;
        let offset = number(words.next().ok_or(INVALID_PARAMETER)?)?;
        let length = number(words.next().ok_or(INVALID_PARAMETER)?)? as usize;
        let pages = length.div_ceil(4096).max(1);
        let mut address = 0u64;
        ok((boot.allocate_pages)(ALLOCATE_ANY_PAGES, LOADER_DATA, pages, &mut address))?;
        let bytes = core::slice::from_raw_parts_mut(address as *mut u8, length);
        let result = data(payload, offset, bytes).and_then(|()| root.write(name, bytes));
        (boot.free_pages)(address, pages);
        result?;
        written += 1;
    }
    Ok(written)
}

/// A request for the upgraded layout must agree with both physical GPT copies.
unsafe fn check_upgrade_layout(
    boot: &BootServices, disk: &mut Disk, expected: [u8; 16], guids: [[u8; 16]; 4],
) -> Result<(), Status> {
    let primary = Header::parse(disk.read(1, 1)?, 1, disk.last).map_err(refuse_append)?;
    let backup = Header::parse(disk.read(disk.last, 1)?, disk.last, disk.last).map_err(refuse_append)?;
    let entries = Buffer::new(boot, ARRAY_BYTES, 8)?;
    let array = core::slice::from_raw_parts_mut(entries.data, ARRAY_BYTES);
    array.copy_from_slice(disk.read(primary.array_lba(), ARRAY_SECTORS as usize)?);
    let backup_array = disk.read(backup.array_lba(), ARRAY_SECTORS as usize)?;
    gpt_append::validate_pair(&primary, &backup, array, backup_array, expected).map_err(refuse_append)?;
    for (entry, (unique, kind)) in array.chunks_exact(ENTRY).skip(2).zip([
        (guids[2], "2ef14a34-abbd-48f8-83d6-f2a8c7b4a9fe"),
        (guids[3], "9bfeaef9-4f51-45de-8bcf-5cec1603b862"),
        (guids[0], "fad521d2-3783-43cc-be73-60ed528454c9"),
        (guids[1], "f93914ad-57e7-4feb-a52e-295a07fd7e35"),
    ]) {
        if entry[..16] != guid(kind).ok_or(INVALID_PARAMETER)? || entry[16..32] != unique {
            return Err(VOLUME_CORRUPTED);
        }
    }
    if array[PARTITIONS * ENTRY..].iter().any(|&byte| byte != 0) {
        return Err(VOLUME_CORRUPTED);
    }
    Ok(())
}

fn slot_intent<'a>(
    text: &'a str, migration: bool, upgrade: Option<[[u8; 16]; 4]>, slot: &str,
) -> Result<(&'a str, bool), Status> {
    let mut rescue = false;
    if let Some(guids) = upgrade {
        let mut lines = text.lines();
        if lines.next() != Some("BREENIX-BOOT 3") || lines.next() != Some("target persistent") {
            return Err(VOLUME_CORRUPTED);
        }
        let root = lines.next().and_then(|line| line.strip_prefix("mount "))
            .and_then(|line| line.split_once(' ')).ok_or(VOLUME_CORRUPTED)?;
        let home = lines.next().and_then(|line| line.strip_prefix("mount "))
            .and_then(|line| line.split_once(' ')).ok_or(VOLUME_CORRUPTED)?;
        let root = (guid(root.0), root.1);
        let home = (guid(home.0), home.1);
        rescue = slot == "PREVIOUS" && root == (Some(guids[2]), "/ ro")
            && home == (Some(guids[3]), "/home ro");
        if !rescue && (root != (Some(guids[0]), "/ rw") || home != (Some(guids[1]), "/home rw")) {
            return Err(VOLUME_CORRUPTED);
        }
        if !lines.next().is_some_and(|line| line.starts_with("userland ")) || lines.next().is_some() {
            return Err(VOLUME_CORRUPTED);
        }
    } else if !(text.starts_with("BREENIX-BOOT 2\n") || migration && text.starts_with("BREENIX-BOOT 3\n")) {
        return Err(VOLUME_CORRUPTED);
    }
    let mut ids = text.lines().filter_map(|line| line.strip_prefix("userland "));
    let id = ids.next().unwrap_or("flat");
    if ids.next().is_some() || id.is_empty() || id.len() > 64
        || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err(VOLUME_CORRUPTED);
    }
    Ok((id, rescue))
}

unsafe fn report_slot(
    boot: &BootServices, root: &Root, request: &Request<'_>, slot: &str,
) -> Result<(), Status> {
    let mut name = [0u8; 64];
    let prefix = br"\EFI\BREENIX\";
    name[..prefix.len()].copy_from_slice(prefix);
    name[prefix.len()..prefix.len()+slot.len()].copy_from_slice(slot.as_bytes());
    name[prefix.len()+slot.len()..prefix.len()+slot.len()+4].copy_from_slice(b".IMG");
    let path = core::str::from_utf8(&name[..prefix.len()+slot.len()+4]).map_err(|_| INVALID_PARAMETER)?;
    match root.size(path) {
        Err(NOT_FOUND) if request.breenfs_upgrade.is_none() || slot == "PREVIOUS" => {
            say!("slot userland: {} empty", slot);
            return Ok(());
        }
        Err(status) => return Err(status),
        Ok(size) if size > 512 << 20 => return Err(OUT_OF_RESOURCES),
        Ok(_) => (),
    }
    let (memory, size) = root.read_pages(boot, path)?;
    let disk = metal_efi::bootdisk::attach(boot, memory, size, "reference slot")?;
    let mut volume = metal_efi::bootdisk::connect(boot, disk, "reference slot")?;
    let mut handle = null_mut();
    ok((boot.locate_device_path)(&SIMPLE_FILE_SYSTEM, &mut volume, &mut handle))?;
    let slot_root = files::root_of(boot, handle)?;
    let mut intent = [0u8; 1024];
    let len = slot_root.read("ramdisk", &mut intent)?;
    let text = core::str::from_utf8(&intent[..len]).map_err(|_| VOLUME_CORRUPTED)?;
    let (id, rescue) = slot_intent(text, request.migration_slots, request.breenfs_upgrade, slot)?;
    if rescue {
        // Ordinary promotion replaces PREVIOUS. Keep this read-only pair for
        // the explicit retirement request, separate from the BreenFS rollback.
        let mut note = [0u8; 256];
        let len = root.read(r"\EFI\BREENIX\PREVIOUS.TXT", &mut note)?;
        root.write(r"\EFI\BREENIX\RESCUE.IMG", core::slice::from_raw_parts(memory, size))?;
        root.write(r"\EFI\BREENIX\RESCUE.TXT", &note[..len])?;
        say!("BreenFS upgrade: ext2 rescue pair preserved");
    }
    say!("slot userland: {} {}", slot, id);
    // The firmware's attached device may retain its memory until ExitBootServices.
    Ok(())
}

/// Carries out persist.cfg when the boot disk has one.
pub(super) unsafe fn run(boot: &BootServices, runtime: &RuntimeServices, volume: *const u8, image: Handle) -> Result<(), Status> {
    let mut path = volume;
    let mut handle = null_mut();
    ok((boot.locate_device_path)(&SIMPLE_FILE_SYSTEM, &mut path, &mut handle))?;
    let delivered = files::root_of(boot, handle)?;
    if !delivered.exists("persist.cfg") {
        return Ok(());
    }
    // An install takes minutes; the firmware's watchdog gets as long as a whole attempt.
    (boot.set_watchdog_timer)(1200, 0, 0, null());
    let (text_pages, text_len) = delivered.read_pages(boot, "persist.cfg")?;
    let text = core::str::from_utf8(core::slice::from_raw_parts(text_pages, text_len)).map_err(|_| INVALID_PARAMETER)?;
    let request = parse(text)?;
    say!("persistent install request: {}", request.action.name());
    let payload = delivered.open("persist.dat", FILE_MODE_READ)?;
    let (disk_handle, io) = the_disk(boot)?;
    let mut disk = Disk::new(boot, io)?;
    if request.action == Action::AddBreenfs {
        add_breenfs(boot, &mut disk, &request, text, payload.handle(), delivered.size("persist.dat")?)?;
        say!("persistent install request complete");
        metal_efi::com1::halt();
    }
    if let Some(expected) = request.expect_record {
        if disk.read(0, 1)? != &expected[..] {
            say!("persistent install refused: the disk's first sector is not the recorded root record");
            return Err(NOT_FOUND);
        }
    }
    if let Some(expected) = request.expect_disk {
        let header = disk.read(1, 1)?;
        if &header[..8] != b"EFI PART" || header[56..72] != expected {
            say!("persistent install refused: the disk is not the recorded persistent layout");
            return Err(NOT_FOUND);
        }
    }
    if let Some(guids) = request.breenfs_upgrade {
        check_upgrade_layout(boot, &mut disk, request.expect_disk.ok_or(INVALID_PARAMETER)?, guids)?;
    }
    // The HTTP boot option: the one this boot came through for the one-time install, the recorded one otherwise.
    let mut current = [0u8; 2];
    let boot_current = (variable(runtime, "BootCurrent", &mut current) == Ok(2)).then(|| u16::from_le_bytes(current));
    let installing = request.action == Action::Install;
    if installing {
        install(boot, &mut disk, &request, text, payload.handle())?;
        // The partition driver reads the new table once the disk's drivers are connected again (section 7.3).
        (boot.disconnect_controller)(disk_handle, null_mut(), null_mut());
        let status = (boot.connect_controller)(disk_handle, null(), null(), 1);
        say!("persistent install: reconnected the disk's drivers ({:#x})", status);
    }
    let esp = partition_handle(boot, request.esp.as_ref().ok_or(INVALID_PARAMETER)?).map_err(|status| {
        say!("persistent install: the EFI system partition did not appear ({:#x})", status);
        status
    })?;
    let root = files::root_of(boot, esp)?;
    // Distinguish absence from an I/O refusal before the transaction's Boolean
    // existence interface is used. An unreadable reference cannot authorize GC.
    for name in ["SLOT.TXN", "SLOT0.OK", "SLOT1.OK", "SLOT2.OK", "CURRENT.IMG", "CURRENT.TXT",
                 "PREVIOUS.IMG", "PREVIOUS.TXT", "NEXT.IMG", "NEXT.TXT", "SWAP.IMG", "SWAP.TXT"] {
        let mut path = [0u8; 96];
        let prefix = b"\\EFI\\BREENIX\\";
        path[..prefix.len()].copy_from_slice(prefix);
        path[prefix.len()..prefix.len()+name.len()].copy_from_slice(name.as_bytes());
        match root.size(core::str::from_utf8(&path[..prefix.len()+name.len()]).map_err(|_| INVALID_PARAMETER)?) {
            Ok(_) | Err(NOT_FOUND) => (),
            Err(status) => return Err(status),
        }
    }
    metal_efi::slot_transaction::recover(&metal_efi::slot_store::SlotStore(&root))?;
    let http_option = if installing { boot_current } else { recorded_http_option(&root) };
    if request.migration_slots {
        write_esp_files(boot, &root, payload.handle(), text, "rescue-file ")?;
        metal_efi::slot_transaction::begin(&metal_efi::slot_store::SlotStore(&root), false)?;
        say!("migration rescue slot installed");
    }
    if request.breenfs_upgrade.is_some() {
        for slot in ["CURRENT", "PREVIOUS"] {
            report_slot(boot, &root, &request, slot)?;
        }
    }
    let written = write_esp_files(boot, &root, payload.handle(), text, "esp-file ")?;
    if written > 0 {
        say!("persistent install: EFI system partition files written: {}", written);
    }
    if !request.migration_slots {
        write_config(boot, &root, &request, image, http_option)?;
        if let Some(description) = request.load_option {
            load_option(boot, runtime, esp, description, http_option)?;
        }
    }
    // The host's disk.json is deliberately not an authority for reclamation.
    if request.breenfs_upgrade.is_none() {
        for slot in ["CURRENT", "PREVIOUS"] {
            report_slot(boot, &root, &request, slot)?;
        }
    }
    report_slot(boot, &root, &request, "NEXT")?;
    say!("slot userlands complete");
    say!("persistent install request complete");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_breenfs_upgrade_request_without_enabling_migration_slots() {
        const SYSTEM: &str = "33333333-3333-3333-3333-333333333333";
        const HOME: &str = "44444444-4444-4444-4444-444444444444";
        const EXT_SYSTEM: &str = "55555555-5555-5555-5555-555555555555";
        const EXT_HOME: &str = "66666666-6666-6666-6666-666666666666";
        let base = concat!(
            "BREENIX-PERSIST 1\naction upgrade\n",
            "expect-disk 11111111-1111-1111-1111-111111111111\n",
            "esp 22222222-2222-2222-2222-222222222222\n",
        );
        let base_request = parse(base).unwrap();
        let marked = format!("{base}breenfs-upgrade {SYSTEM} {HOME} {EXT_SYSTEM} {EXT_HOME}\n");
        let request = parse(&marked).ok().unwrap();
        assert!(!base_request.migration_slots);
        assert!(!request.migration_slots);
        assert!(parse(&format!("{base}breenfs-upgrade {SYSTEM} {SYSTEM} {EXT_SYSTEM} {EXT_HOME}\n")).is_err());

        let cutover = format!("BREENIX-BOOT 3\ntarget persistent\nmount {SYSTEM} / rw\nmount {HOME} /home rw\nuserland gen\n");
        let rescue = format!("BREENIX-BOOT 3\ntarget persistent\nmount {EXT_SYSTEM} / ro\nmount {EXT_HOME} /home ro\nuserland gen\n");
        assert_eq!(slot_intent(&cutover, false, None, "CURRENT"), Err(VOLUME_CORRUPTED));
        assert_eq!(slot_intent(&cutover, false, request.breenfs_upgrade, "CURRENT"), Ok(("gen", false)));
        assert_eq!(slot_intent(&rescue, false, request.breenfs_upgrade, "PREVIOUS"), Ok(("gen", true)));
        assert_eq!(slot_intent(&rescue, false, request.breenfs_upgrade, "CURRENT"), Err(VOLUME_CORRUPTED));
    }
}
