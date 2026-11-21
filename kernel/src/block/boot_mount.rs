//! Device identity selection for boot mounts.

use super::gpt;
use crate::block::BlockDevice;
#[cfg(any(target_arch = "aarch64", test))]
use crate::fs::vfs::{format::detect, Format};
use alloc::boxed::Box;
#[cfg(any(target_arch = "x86_64", test))]
use {
    super::partition::Partition,
    crate::block::{write_policy::Fence, HardwareDisk},
};

#[cfg(any(target_arch = "x86_64", test))]
pub const BREENFS_SYSTEM_TYPE: gpt::Guid = gpt::guid(
    0xfad521d2,
    0x3783,
    0x43cc,
    [0xbe, 0x73, 0x60, 0xed, 0x52, 0x84, 0x54, 0xc9],
);
#[cfg(any(target_arch = "x86_64", test))]
pub const BREENFS_HOME_TYPE: gpt::Guid = gpt::guid(
    0xf93914ad,
    0x57e7,
    0x4feb,
    [0xa5, 0x2e, 0x29, 0x5a, 0x07, 0xfd, 0x7e, 0x35],
);

#[cfg(any(target_arch = "x86_64", test))]
pub fn partition_types(path: &str) -> &'static [gpt::Guid] {
    match path {
        "/" => &[gpt::TEST_ROOT_TYPE, gpt::SYSTEM_TYPE, BREENFS_SYSTEM_TYPE],
        "/home" => &[gpt::HOME_TYPE, BREENFS_HOME_TYPE],
        "/migrate/ext2-system" => &[gpt::SYSTEM_TYPE],
        "/migrate/ext2-home" => &[gpt::HOME_TYPE],
        "/migrate/system" => &[BREENFS_SYSTEM_TYPE],
        "/migrate/home" => &[BREENFS_HOME_TYPE],
        _ => &[],
    }
}

#[cfg(any(target_arch = "x86_64", test))]
pub fn find_partition(
    disks: impl IntoIterator<Item = Box<dyn BlockDevice>>,
    text: &str,
    types: &[gpt::Guid],
) -> Result<(Partition, gpt::Partition, HardwareDisk), &'static str> {
    let unique = gpt::parse_guid(text).ok_or("the boot intent's GUID does not parse")?;
    let mut found = None;
    let mut matches = 0;
    for disk in disks {
        if disk.block_size() != gpt::SECTOR {
            continue;
        }
        let mut lba1 = [0u8; gpt::SECTOR];
        if disk.read_block(1, &mut lba1).is_err() {
            continue;
        }
        let Ok(header) = gpt::header(&lba1, disk.num_blocks()) else {
            continue;
        };
        let entry = gpt::find(&header, &unique, |lba, sector| {
            disk.read_block(lba, sector)
                .map_err(|_| "a partition table sector is unreadable")
        })?;
        if let Some(entry) = entry {
            matches += 1;
            found = Some((disk, entry));
        }
    }
    let (disk, entry) = match (matches, found) {
        (1, Some(found)) => found,
        (0, _) => return Err("no attached disk has a partition with the boot intent's GUID"),
        _ => return Err("more than one attached disk has a partition with the boot intent's GUID"),
    };
    if !types.contains(&entry.type_guid) {
        return Err("the named partition's type is not one this role may have");
    }
    let hardware = disk
        .hardware_disk()
        .ok_or("the partition's disk has no hardware identity")?;
    Ok((
        Partition::new(disk, entry.first, entry.sectors())?,
        entry,
        hardware,
    ))
}

#[cfg(any(target_arch = "x86_64", test))]
pub fn grant_write(
    fence: &mut Fence,
    disk: HardwareDisk,
    entry: &gpt::Partition,
    writable: bool,
) -> Result<(), &'static str> {
    if writable {
        if !matches!(entry.type_guid, BREENFS_SYSTEM_TYPE | BREENFS_HOME_TYPE) {
            return Err("only BreenFS system and home partitions may receive a format-3 write grant");
        }
        fence.add(disk, entry.first, entry.last)?;
    }
    Ok(())
}

#[cfg(any(target_arch = "aarch64", test))]
pub fn find_filesystem(
    disks: impl IntoIterator<Item = Box<dyn BlockDevice>>,
    formats: &[Format],
    text: &str,
) -> Result<Box<dyn BlockDevice>, &'static str> {
    let mut uuid = gpt::parse_guid(text).ok_or("the boot intent's GUID does not parse")?;
    // Unlike GPT fields, filesystem UUIDs follow the displayed byte order.
    uuid[..4].reverse();
    uuid[4..6].reverse();
    uuid[6..8].reverse();

    let mut found = None;
    for disk in disks {
        let format = detect(formats, disk.as_ref()).map_err(|_| "a disk's format probe failed")?;
        if let Some(format) = format {
            let actual =
                (format.uuid)(disk.as_ref()).map_err(|_| "a filesystem UUID is unreadable")?;
            if actual == uuid {
                if found.is_some() {
                    return Err("more than one attached disk has the filesystem UUID");
                }
                found = Some(disk);
            }
        }
    }
    found.ok_or("no attached disk has the filesystem UUID")
}
