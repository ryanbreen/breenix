//! The persistent install's requests (docs/metal/README.md, "Install and upgrade"), from UEFI 2.10 sections 3.1.3,
//! 5.2.3, 5.3, 7.3, 8.2, 13.5 and 13.9.
//!
//! `persist.cfg` on the boot disk names an action. `install` (the one-time install) writes the GUID partition table
//! and the partitions' contents as extents onto the one fixed disk whose first sector is the recorded whole-disk root
//! record, invalidating the old layout first and writing the protective MBR last, then reconnects the disk so the
//! firmware reads the new table. `upgrade` checks the disk's GUID and writes files into the EFI system partition.
//! Either way it then writes `MANAGER.CFG` (the lab's lines, the HTTP boot port this stub was loaded through, and the
//! HTTP boot option) and puts the boot manager's load option first in `BootOrder`, the HTTP boot option second.
//! Each write is read back and compared. An ordinary delivery has no request and takes the existing boot path.
use core::ffi::c_void;
use core::ptr::{null, null_mut};

use metal_efi::bootdisk::SECTOR;
use metal_efi::files::{self, Root};
use metal_efi::say;
use metal_efi::uefi::*;

use crate::suite_disk::{partition_signature, Buffer};

const MAGIC: &str = "BREENIX-PERSIST 1";
const PARTITIONS: usize = 4;
const ENTRIES: usize = 128;
const ENTRY: usize = 128;
const ENTRY_SECTORS: u64 = (ENTRIES * ENTRY / SECTOR) as u64;
const CHUNK_SECTORS: usize = 128;
const CONFIG_LINES: usize = 16;
const MANAGER_FILE: &str = "\\EFI\\BREENIX\\BOOTMGR.EFI";
const CONFIG_FILE: &str = "\\EFI\\BREENIX\\MANAGER.CFG";

#[derive(Clone, Copy)]
struct Part {
    type_guid: [u8; 16],
    unique: [u8; 16],
    first: u64,
    last: u64,
    name: [u8; 36],
    name_len: usize,
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
    install: bool,
    expect_record: Option<[u8; SECTOR]>,
    expect_disk: Option<[u8; 16]>,
    disk_guid: [u8; 16],
    parts: [Option<Part>; PARTITIONS],
    esp: Option<[u8; 16]>,
    config: [&'t str; CONFIG_LINES],
    config_len: usize,
    load_option: Option<&'t str>,
}

fn parse<'t>(text: &'t str) -> Result<Request<'t>, Status> {
    let mut lines = text.lines();
    if lines.next() != Some(MAGIC) {
        return Err(INVALID_PARAMETER);
    }
    let mut request = Request {
        install: false,
        expect_record: None,
        expect_disk: None,
        disk_guid: [0; 16],
        parts: [None; PARTITIONS],
        esp: None,
        config: [""; CONFIG_LINES],
        config_len: 0,
        load_option: None,
    };
    let mut action = None;
    for line in lines {
        let (item, value) = line.split_once(' ').unwrap_or((line, ""));
        match item {
            "action" => action = Some(value),
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
            "expect-disk" => request.expect_disk = Some(guid(value).ok_or(INVALID_PARAMETER)?),
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
                if !(1..=PARTITIONS).contains(&index) || first > last || name_text.len() > 36 || !name_text.is_ascii() {
                    return Err(INVALID_PARAMETER);
                }
                let mut name = [0u8; 36];
                name[..name_text.len()].copy_from_slice(name_text.as_bytes());
                request.parts[index - 1] = Some(Part { type_guid, unique, first, last, name, name_len: name_text.len() });
            }
            "manager-config" if request.config_len < CONFIG_LINES => {
                request.config[request.config_len] = value;
                request.config_len += 1;
            }
            "load-option" => request.load_option = Some(value),
            "extent" | "esp-file" => {}
            _ => return Err(INVALID_PARAMETER),
        }
    }
    request.install = match action {
        Some("install") => true,
        Some("upgrade") => false,
        _ => return Err(INVALID_PARAMETER),
    };
    if request.install {
        if request.expect_record.is_none() || request.parts.iter().any(Option::is_none) {
            return Err(INVALID_PARAMETER);
        }
        request.esp = request.parts[0].map(|part| part.unique);
    } else if request.expect_disk.is_none() || request.esp.is_none() {
        return Err(INVALID_PARAMETER);
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
        if lba.checked_add(sectors as u64).map_or(true, |end| end > self.last + 1) {
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
    sector[40..48].copy_from_slice(&(2 + ENTRY_SECTORS).to_le_bytes());
    sector[48..56].copy_from_slice(&(last - 1 - ENTRY_SECTORS).to_le_bytes());
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
    let parts: [Part; PARTITIONS] = core::array::from_fn(|index| request.parts[index].unwrap());
    if parts[PARTITIONS - 1].last + 1 + ENTRY_SECTORS > disk.last {
        say!("persistent install refused: the layout does not fit the disk");
        return Err(BAD_BUFFER_SIZE);
    }
    // No old root record or table survives an interrupted install.
    disk.zero(0, 2 + ENTRY_SECTORS)?;
    disk.zero(disk.last - ENTRY_SECTORS, ENTRY_SECTORS + 1)?;
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
        match kind {
            "zero" => disk.zero(lba, count)?,
            "data" => {
                let mut offset = number(next()?)?;
                let mut done = 0;
                while done < count {
                    let sectors = (count - done).min(CHUNK_SECTORS as u64) as usize;
                    data(payload, offset, &mut disk.wire()[..sectors * SECTOR])?;
                    disk.write_wire(lba + done, sectors)?;
                    done += sectors as u64;
                    offset += (sectors * SECTOR) as u64;
                }
            }
            _ => return Err(INVALID_PARAMETER),
        }
        extents += 1;
    }
    // The entry array (section 5.3.3), then both headers, then the protective MBR.
    let entries = Buffer::new(boot, ENTRIES * ENTRY, 8)?;
    let array = core::slice::from_raw_parts_mut(entries.data, ENTRIES * ENTRY);
    array.fill(0);
    for (index, part) in parts.iter().enumerate() {
        let entry = &mut array[index * ENTRY..(index + 1) * ENTRY];
        entry[..16].copy_from_slice(&part.type_guid);
        entry[16..32].copy_from_slice(&part.unique);
        entry[32..40].copy_from_slice(&part.first.to_le_bytes());
        entry[40..48].copy_from_slice(&part.last.to_le_bytes());
        for (at, &byte) in part.name[..part.name_len].iter().enumerate() {
            entry[56 + 2 * at] = byte;
        }
    }
    let entries_crc = crc32(boot, array)?;
    let last = disk.last;
    disk.write(last - ENTRY_SECTORS, array)?;
    disk.write(last, &header(boot, request, last, last, 1, last - ENTRY_SECTORS, entries_crc)?)?;
    disk.write(2, array)?;
    disk.write(1, &header(boot, request, last, 1, last, 2, entries_crc)?)?;
    disk.write(0, &protective_mbr(last))?;
    say!(
        "persistent install: layout written: {} partitions, {} extents, {} bytes",
        PARTITIONS,
        extents,
        disk.written
    );
    Ok(extents)
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
    say!("persistent install request: {}", if request.install { "install" } else { "upgrade" });
    let payload = delivered.open("persist.dat", FILE_MODE_READ)?;
    let (disk_handle, io) = the_disk(boot)?;
    let mut disk = Disk::new(boot, io)?;
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
    // The HTTP boot option: the one this boot came through for the one-time install, the recorded one otherwise.
    let mut current = [0u8; 2];
    let boot_current = (variable(runtime, "BootCurrent", &mut current) == Ok(2)).then(|| u16::from_le_bytes(current));
    if request.install {
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
    let http_option = if request.install { boot_current } else { recorded_http_option(&root) };
    let mut written = 0;
    for line in text.lines().filter(|line| line.starts_with("esp-file ")) {
        let mut words = line[9..].split(' ');
        let name = words.next().ok_or(INVALID_PARAMETER)?;
        let offset = number(words.next().ok_or(INVALID_PARAMETER)?)?;
        let length = number(words.next().ok_or(INVALID_PARAMETER)?)? as usize;
        let pages = length.div_ceil(4096).max(1);
        let mut address = 0u64;
        ok((boot.allocate_pages)(ALLOCATE_ANY_PAGES, LOADER_DATA, pages, &mut address))?;
        let bytes = core::slice::from_raw_parts_mut(address as *mut u8, length);
        let result = data(payload.handle(), offset, bytes).and_then(|()| root.write(name, bytes));
        (boot.free_pages)(address, pages);
        result?;
        written += 1;
    }
    if written > 0 {
        say!("persistent install: EFI system partition files written: {}", written);
    }
    write_config(boot, &root, &request, image, http_option)?;
    if let Some(description) = request.load_option {
        load_option(boot, runtime, esp, description, http_option)?;
    }
    // Read embedded userland identities only after recovery and NEXT publication.
    // The host's disk.json is deliberately not an authority for reclamation.
    for slot in ["CURRENT", "PREVIOUS", "NEXT"] {
        let mut name = [0u8; 64];
        let prefix = b"\\EFI\\BREENIX\\";
        name[..prefix.len()].copy_from_slice(prefix);
        name[prefix.len()..prefix.len()+slot.len()].copy_from_slice(slot.as_bytes());
        name[prefix.len()+slot.len()..prefix.len()+slot.len()+4].copy_from_slice(b".IMG");
        let path = core::str::from_utf8(&name[..prefix.len()+slot.len()+4]).map_err(|_| INVALID_PARAMETER)?;
        match root.size(path) {
            Err(NOT_FOUND) => { say!("slot userland: {} empty", slot); continue; }
            Err(status) => return Err(status),
            Ok(size) if size > 512 << 20 => return Err(OUT_OF_RESOURCES),
            Ok(_) => (),
        }
        let (memory, size) = root.read_pages(boot, path)?;
        let disk = metal_efi::bootdisk::attach(boot, memory, size, "reference slot")?;
        let volume = metal_efi::bootdisk::connect(boot, disk, "reference slot")?;
        let mut device = volume;
        let mut handle = null_mut();
        ok((boot.locate_device_path)(&SIMPLE_FILE_SYSTEM, &mut device, &mut handle))?;
        let slot_root = files::root_of(boot, handle)?;
        let mut intent = [0u8; 1024];
        let len = slot_root.read("ramdisk", &mut intent)?;
        let text = core::str::from_utf8(&intent[..len]).map_err(|_| VOLUME_CORRUPTED)?;
        if !text.starts_with("BREENIX-BOOT 2\n") { return Err(VOLUME_CORRUPTED); }
        let mut ids = text.lines().filter_map(|line| line.strip_prefix("userland "));
        let id = ids.next().unwrap_or("flat");
        if ids.next().is_some() { return Err(VOLUME_CORRUPTED); }
        if id.is_empty() || id.len() > 64 || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return Err(VOLUME_CORRUPTED);
        }
        say!("slot userland: {} {}", slot, id);
        // The firmware's attached device may retain this memory until ExitBootServices.
    }
    say!("slot userlands complete");
    say!("persistent install request complete");
    Ok(())
}
