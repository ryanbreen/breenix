//! Action 3: cold export of the two recorded persistent partitions.
//! GPT hard-drive device paths and partition-relative Block I/O follow UEFI 2.10 sections 10.3.5 and 13.9.

use core::ffi::c_void;
use core::ptr::{null, null_mut};

use metal_efi::{bootdisk::SECTOR, com1, say, sha256::Sha256, uefi::*};

use crate::suite_disk::{self, Buffer, CHUNK};

const MAGIC: &[u8; 16] = b"BREENIX-EXPORT2\0";
const ACTION: u64 = 3;
const RANGE_SIZE: usize = 32;
const HEADER_SIZE: usize = 24 + 3 * RANGE_SIZE;
const PART_BYTES: usize = 1 << 30;
const PART_SUFFIX: usize = 1 + 16; // Slash and the hexadecimal part index.
const MAX_URL: usize = 2046;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Range {
    guid: [u8; 16],
    first: u64,
    last: u64,
}

impl Range {
    fn parse(wire: &[u8]) -> Self {
        Self {
            guid: wire[..16].try_into().unwrap(),
            first: number(&wire[16..24]),
            last: number(&wire[24..32]),
        }
    }

    fn disjoint(self, other: Self) -> bool {
        self.guid != other.guid && (self.last < other.first || other.last < self.first)
    }

    fn sectors(self) -> Option<u64> {
        self.last.checked_sub(self.first)?.checked_add(1)
    }
}

fn number(wire: &[u8]) -> u64 {
    u64::from_le_bytes(wire.try_into().unwrap())
}

pub(super) unsafe fn run(
    boot: &BootServices,
    volume: *const u8,
    image: Handle,
) -> Result<(), Status> {
    let root = suite_disk::open_root(boot, volume)?;
    let mut wire = [0; HEADER_SIZE + 2 * 2048];
    let name = Wide::<24>::new("export.cfg");
    let result = suite_disk::read_file(root, &name.units, &mut wire);
    ((*root).close)(root);
    let length = match result {
        Err(NOT_FOUND) => return Ok(()),
        result => result?,
    };
    if length < HEADER_SIZE
        || length >= wire.len()
        || &wire[..16] != MAGIC
        || number(&wire[16..24]) != ACTION
    {
        return Err(INVALID_PARAMETER);
    }
    let wanted = core::array::from_fn(|i| {
        Range::parse(&wire[24 + i * RANGE_SIZE..24 + (i + 1) * RANGE_SIZE])
    });
    let [test, system, home] = wanted;
    if wanted
        .iter()
        .any(|range| range.guid == [0; 16] || range.first == 0 || range.sectors().is_none())
        || !system.disjoint(test)
        || !home.disjoint(test)
        || !system.disjoint(home)
    {
        return Err(INVALID_PARAMETER);
    }
    let mut urls = wire[HEADER_SIZE..length]
        .strip_suffix(&[0])
        .ok_or(INVALID_PARAMETER)?
        .split(|&b| b == 0);
    let mut url = || {
        urls.next()
            .filter(|value| {
                value.len() + PART_SUFFIX <= MAX_URL
                    && value.starts_with(b"http://")
                    && value.is_ascii()
            })
            .ok_or(INVALID_PARAMETER)
    };
    let (system_url, home_url) = (url()?, url()?);
    if urls.next().is_some() || system_url == home_url {
        return Err(INVALID_PARAMETER);
    }
    // UEFI 2.10 §7.5.1 allows this long-running export to disable the boot-option watchdog.
    ok((boot.set_watchdog_timer)(0, 0, 0, null()))?;
    let [system_disk, home_disk] = select(boot, &wanted)?;
    export(boot, image, "system", system_disk, system, system_url)?;
    export(boot, image, "home", home_disk, home, home_url)?;
    say!("persistent export complete");
    com1::halt()
}

/// The last GPT hard-drive node supplies the absolute range; its path prefix identifies the parent disk.
unsafe fn partition_path(boot: &BootServices, handle: Handle) -> Option<(&[u8], Range)> {
    let mut path: *mut c_void = null_mut();
    if (boot.handle_protocol)(handle, &DEVICE_PATH, &mut path) != SUCCESS || path.is_null() {
        return None;
    }
    let bytes = core::slice::from_raw_parts(path.cast::<u8>(), path_length(path.cast())?);
    let (kind, subtype, hd) = Nodes::new(bytes).last()?;
    if kind != node::MEDIA || subtype != node::HARD_DRIVE || hd.len() != 42 || hd[40..42] != [2, 2]
    {
        return None;
    }
    let first = number(&hd[8..16]);
    let last = first.checked_add(number(&hd[16..24]).checked_sub(1)?)?;
    Some((
        &bytes[..bytes.len() - hd.len()],
        Range {
            guid: hd[24..40].try_into().unwrap(),
            first,
            last,
        },
    ))
}

#[derive(Clone, Copy)]
struct Partition {
    disk: *mut BlockIo,
    media_id: u32,
    io_align: usize,
}

unsafe fn select(boot: &BootServices, wanted: &[Range; 3]) -> Result<[Partition; 2], Status> {
    let mut handles = null_mut();
    let mut count = 0;
    ok((boot.locate_handle_buffer)(
        BY_PROTOCOL,
        &BLOCK_IO,
        null(),
        &mut count,
        &mut handles,
    ))?;
    let result = select_handles(boot, wanted, handles, count);
    (boot.free_pool)(handles.cast());
    result
}

unsafe fn select_handles(
    boot: &BootServices,
    wanted: &[Range; 3],
    handles: *mut Handle,
    count: usize,
) -> Result<[Partition; 2], Status> {
    let mut disks = [None; 3];
    let mut parent = None;
    for index in 0..count {
        let handle = *handles.add(index);
        let Some((path, found)) = partition_path(boot, handle) else {
            continue;
        };
        let Some(slot) = wanted.iter().position(|range| range.guid == found.guid) else {
            continue;
        };
        if found != wanted[slot]
            || disks[slot].is_some()
            || parent.is_some_and(|previous| previous != path)
        {
            return Err(INVALID_PARAMETER);
        }

        let mut protocol = null_mut();
        ok((boot.handle_protocol)(handle, &BLOCK_IO, &mut protocol))?;
        let disk = protocol.cast::<BlockIo>();
        let media = &*(*disk).media;
        if media.logical_partition == 0
            || media.media_present == 0
            || media.removable_media != 0
            || media.block_size != SECTOR as u32
            || media.last_block != found.last - found.first
        {
            return Err(INVALID_PARAMETER);
        }
        parent = Some(path);
        disks[slot] = Some(Partition {
            disk,
            media_id: media.media_id,
            io_align: media.io_align as usize,
        });
    }
    let [Some(_), Some(system), Some(home)] = disks else {
        say!("persistent export refused: the recorded partition GUIDs were not all found");
        return Err(NOT_FOUND);
    };
    Ok([system, home])
}

unsafe fn export(
    boot: &BootServices,
    image: Handle,
    role: &str,
    partition: Partition,
    range: Range,
    url: &[u8],
) -> Result<(), Status> {
    let size = range
        .sectors()
        .and_then(|sectors| sectors.checked_mul(SECTOR as u64))
        .and_then(|bytes| usize::try_from(bytes).ok())
        .ok_or(INVALID_PARAMETER)?;
    let Partition {
        disk,
        media_id,
        io_align,
    } = partition;
    let buffer = Buffer::new(boot, CHUNK, io_align)?;
    let mut digest = Sha256::new();
    let mut count = 0;
    let mut part_url = [0; MAX_URL];
    let end = url.len() + PART_SUFFIX;
    part_url[..url.len()].copy_from_slice(url);
    part_url[url.len()] = b'/';
    for (index, start) in (0..size).step_by(PART_BYTES).enumerate() {
        let size = PART_BYTES.min(size - start);
        encode_hex(
            &(index as u64).to_be_bytes(),
            &mut part_url[url.len() + 1..end],
        );
        let mut part_digest = Sha256::new();
        let mut part_count = 0;
        suite_disk::upload(
            boot,
            image,
            size,
            &buffer,
            &part_url[..end],
            |offset, len| {
                ok(((*disk).read_blocks)(
                    disk,
                    media_id,
                    ((start + offset) / SECTOR) as u64,
                    len,
                    buffer.data.cast(),
                ))?;
                let bytes = core::slice::from_raw_parts(buffer.data, len);
                part_digest.update(bytes);
                digest.update(bytes);
                part_count += len;
                Ok(())
            },
        )?;
        count += part_count;
        let mut hex = [0; 64];
        encode_hex(&part_digest.finalize(), &mut hex);
        say!(
            "persistent export {} part={} bytes={} sha256={}",
            role,
            index,
            part_count,
            core::str::from_utf8(&hex).unwrap()
        );
    }
    let mut hex = [0u8; 64];
    encode_hex(&digest.finalize(), &mut hex);
    say!(
        "persistent export {} bytes={} sha256={}",
        role,
        count,
        core::str::from_utf8(&hex).unwrap()
    );
    Ok(())
}

fn encode_hex(bytes: &[u8], hex: &mut [u8]) {
    let alphabet = b"0123456789abcdef";
    for (pair, &byte) in hex.chunks_exact_mut(2).zip(bytes) {
        pair[0] = alphabet[(byte >> 4) as usize];
        pair[1] = alphabet[(byte & 15) as usize];
    }
}
