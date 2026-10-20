//! Block Device Abstraction Layer
//!
//! Provides a generic interface for block devices, allowing filesystems to work
//! with different underlying storage implementations (VirtIO, AHCI, etc.) through
//! a common trait.

use core::fmt;

#[cfg(target_arch = "x86_64")]
mod read_only;

#[cfg(all(target_arch = "x86_64", feature = "block_fence_inject"))]
pub mod fence_fault;
pub mod gpt;
#[cfg(target_arch = "x86_64")]
pub mod install;
#[cfg(target_arch = "x86_64")]
pub mod nvme;
#[cfg(target_arch = "x86_64")]
pub mod partition;
#[cfg(target_arch = "x86_64")]
pub mod ramdisk;
pub mod virtio;
#[cfg(all(feature = "testing", not(feature = "interactive")))]
pub mod testing_fixture;

mod write_policy;
pub use write_policy::{hardware_read_only, hardware_disk_read_only, hardware_write_allowed, HardwareDisk};
#[cfg(target_arch = "x86_64")]
pub use write_policy::{fence, fence_seal, init_hardware_policy, Fence};
#[cfg(target_arch = "x86_64")]
use write_policy::{fence_add, fence_clear};

/// Which partition of the installed disk this boot's root is (docs/metal/README.md, "Boot intents and root
/// selection"): not chosen yet, the test root, or the persistent install's system partition.
#[cfg(target_arch = "x86_64")]
static ROOT_ROLE: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
#[cfg(target_arch = "x86_64")]
const ROLE_TEST_ROOT: u8 = 1;
#[cfg(target_arch = "x86_64")]
const ROLE_SYSTEM: u8 = 2;

/// Prefer a supplied bootloader RAM disk; on physical x86 otherwise mount exactly the partition the boot intent
/// names, and refuse any other hardware root (issue #1702).
#[cfg(target_arch = "x86_64")]
pub fn root_devices() -> alloc::vec::Vec<alloc::boxed::Box<dyn BlockDevice>> {
    if let Some(device) = ramdisk::device() {
        alloc::vec![device]
    } else if hardware_read_only() {
        named_root().into_iter().collect()
    } else {
        devices()
    }
}

/// Whether root was taken from the test root partition the delivery stub installed.
#[cfg(target_arch = "x86_64")]
pub fn root_on_installed_disk() -> bool {
    ROOT_ROLE.load(core::sync::atomic::Ordering::Acquire) == ROLE_TEST_ROOT
}

/// Whether root was taken from the persistent install's system partition.
#[cfg(target_arch = "x86_64")]
pub fn root_on_system_partition() -> bool {
    ROOT_ROLE.load(core::sync::atomic::Ordering::Acquire) == ROLE_SYSTEM
}

/// The one partition on the attached hardware disks whose unique GUID is `text`, of one of `types`, as a block
/// device without the read-only barrier (the fence guards it), with its table entry and disk.
#[cfg(target_arch = "x86_64")]
fn named_partition(
    text: &str,
    types: &[gpt::Guid],
) -> Result<(partition::Partition, gpt::Partition, HardwareDisk), &'static str> {
    let unique = gpt::parse_guid(text).ok_or("the boot intent's GUID does not parse")?;
    let mut found = None;
    let mut matches = 0;
    for disk in discovered() {
        if disk.block_size() != gpt::SECTOR {
            continue;
        }
        let mut lba1 = [0u8; gpt::SECTOR];
        if disk.read_block(1, &mut lba1).is_err() {
            continue;
        }
        let Ok(header) = gpt::header(&lba1, disk.num_blocks()) else { continue };
        let entry = gpt::find(&header, &unique, |lba, sector| {
            disk.read_block(lba, sector).map_err(|_| "a partition table sector is unreadable")
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
    let hardware = disk.hardware_disk().ok_or("the partition's disk has no hardware identity")?;
    Ok((partition::Partition::new(disk, entry.first, entry.sectors())?, entry, hardware))
}

/// The root the boot intent names: the test root (a test boot, or an install's migration) or the system partition
/// (a persistent boot), with the fence opened for its range alone.
#[cfg(target_arch = "x86_64")]
fn named_root() -> Option<alloc::boxed::Box<dyn BlockDevice>> {
    let Some(text) = crate::boot::intent::root_partition() else {
        let why = crate::boot::intent::refused().unwrap_or("the boot intent names no root partition");
        log::warn!("[storage] root refused: {}", why);
        return None;
    };
    let (device, entry, hardware) = match named_partition(text, &[gpt::TEST_ROOT_TYPE, gpt::SYSTEM_TYPE]) {
        Ok(found) => found,
        Err(why) => {
            log::warn!("[storage] root refused: {}", why);
            return None;
        }
    };
    let test_root = entry.type_guid == gpt::TEST_ROOT_TYPE;
    log::info!(
        "[storage] root partition: {} {} LBA {}-{} on {}",
        if test_root { "test root" } else { "system" },
        text,
        entry.first,
        entry.last,
        hardware
    );
    if test_root {
        if let Err(why) = report_root_record(&device) {
            log::warn!("[storage] root refused: {}", why);
            return None;
        }
    }
    if let Err(why) = fence_add(hardware, entry.first, entry.last) {
        log::warn!("[storage] root refused: {}", why);
        return None;
    }
    if test_root {
        log::info!("[block] hardware disk writes granted: the root the storage lane installed");
    }
    ROOT_ROLE.store(if test_root { ROLE_TEST_ROOT } else { ROLE_SYSTEM }, core::sync::atomic::Ordering::Release);
    Some(alloc::boxed::Box::new(device))
}

/// The storage record in the test root partition's first sector, which the delivery stub wrote last.
#[cfg(target_arch = "x86_64")]
fn report_root_record(device: &dyn BlockDevice) -> Result<(), &'static str> {
    let install::Record::Root { boot, bytes, sha256 } = install::root_record(device)? else {
        return Err("the test root holds no valid root record");
    };
    log::info!("[storage] root record: installed by boot {}, {} bytes, sha256 {}", boot, bytes, install::hex(&sha256));
    match install::matches(device, bytes, &sha256) {
        Ok(true) => log::info!("[storage] root image matches its install record"),
        Ok(false) => log::info!("[storage] root image differs from its install record: written since"),
        Err(why) => return Err(why),
    }
    Ok(())
}

/// Whether the boot intent names a home partition.
#[cfg(target_arch = "x86_64")]
pub fn home_named() -> bool {
    crate::boot::intent::home_partition().is_some()
}

/// The home the boot intent names: the home partition beside the system root (a persistent boot), or the system
/// partition beside the test root (an install's migration, which fills it). Any other pairing is refused.
#[cfg(target_arch = "x86_64")]
pub fn named_home() -> Option<alloc::boxed::Box<dyn BlockDevice>> {
    let text = crate::boot::intent::home_partition()?;
    let wanted = match ROOT_ROLE.load(core::sync::atomic::Ordering::Acquire) {
        ROLE_SYSTEM => gpt::HOME_TYPE,
        ROLE_TEST_ROOT => gpt::SYSTEM_TYPE,
        _ => {
            log::warn!("[storage] home refused: no partition root was chosen");
            return None;
        }
    };
    if Some(text) == crate::boot::intent::root_partition() {
        log::warn!("[storage] home refused: it names the root partition");
        return None;
    }
    let (device, entry, hardware) = match named_partition(text, &[wanted]) {
        Ok(found) => found,
        Err(why) => {
            log::warn!("[storage] home refused: {}", why);
            return None;
        }
    };
    log::info!(
        "[storage] home partition: {} {} LBA {}-{} on {}",
        if wanted == gpt::HOME_TYPE { "home" } else { "system" },
        text,
        entry.first,
        entry.last,
        hardware
    );
    if let Err(why) = fence_add(hardware, entry.first, entry.last) {
        log::warn!("[storage] home refused: {}", why);
        return None;
    }
    Some(alloc::boxed::Box::new(device))
}

/// Before root mounts: when the RAM disk's first sector asks for a recovery install, copy the RAM disk into the
/// test root partition the request names, read it back, and close the fence again. A request that names no
/// partition is refused: this layout has no whole-disk root.
#[cfg(target_arch = "x86_64")]
pub fn install_requested_root() {
    let Some(ram) = ramdisk::device() else { return };
    let mut first = [0u8; install::SECTOR];
    if ram.read_block(0, &mut first).is_err() {
        return;
    }
    let Some(install::Record::Install { boot, partition }) = install::Record::parse(&first) else { return };
    let Some(partition) = partition else {
        log::warn!("[storage] install refused: the request names no partition");
        return;
    };
    let (target, entry, hardware) = match named_partition(&partition, &[gpt::TEST_ROOT_TYPE]) {
        Ok(found) => found,
        Err(why) => {
            log::warn!("[storage] install refused: {}", why);
            return;
        }
    };
    if let Err(why) = fence_add(hardware, entry.first, entry.last) {
        log::warn!("[storage] install refused: {}", why);
        return;
    }
    let bytes = ram.num_blocks() * install::SECTOR as u64;
    log::info!("[storage] install requested by boot {}: {} bytes onto the test root partition", boot, bytes);
    let started = crate::time::get_monotonic_time();
    // The fence holds the partition's range for the copy and is cleared once writing ends.
    let result = install::install(&*ram, &target, &boot, &mut |granted| {
        if !granted {
            fence_clear();
        }
    });
    let elapsed = crate::time::get_monotonic_time().saturating_sub(started);
    match result {
        Ok(done) => {
            log::info!("[storage] wrote {} bytes to the test root partition from its first sector", done.bytes);
            match done.verified {
                Ok(()) => log::info!(
                    "[storage] read back identical: {} bytes, sha256 after the record sector {} ({} ms)",
                    done.bytes,
                    install::hex(&done.sha256),
                    elapsed
                ),
                Err(why) => log::warn!("[storage] read back failed: {} bytes, {}", done.bytes, why),
            }
        }
        Err(why) => log::warn!("[storage] install failed: {}", why),
    }
    fence_clear();
}

/// The x86 disks the drivers attached, in backend order: VirtIO block
/// devices, AHCI SATA disks, then NVMe namespaces, without the physical-boot
/// write barrier.
#[cfg(target_arch = "x86_64")]
fn discovered() -> alloc::vec::Vec<alloc::boxed::Box<dyn BlockDevice>> {
    let mut devices: alloc::vec::Vec<alloc::boxed::Box<dyn BlockDevice>> = alloc::vec::Vec::new();
    let mut index = 0;
    while let Some(device) = virtio::VirtioBlockWrapper::new(index) {
        devices.push(alloc::boxed::Box::new(device));
        index += 1;
    }
    for index in 0..crate::drivers::ahci::sata_device_count() {
        if let Some(device) = crate::drivers::ahci::get_block_device_by_index(index) {
            devices.push(alloc::boxed::Box::new(device));
        }
    }
    for index in 0..crate::drivers::nvme::controller_count() {
        if let Some(device) = nvme::NvmeBlockDevice::new(index) {
            devices.push(alloc::boxed::Box::new(device));
        }
    }
    devices
}

/// Enumerate usable x86 disks in backend order: VirtIO block devices, AHCI
/// SATA disks, then NVMe namespaces. Consumers identify disk contents,
/// rather than relying on PCI slots or a fixed index shared across transports.
#[cfg(target_arch = "x86_64")]
pub fn devices() -> alloc::vec::Vec<alloc::boxed::Box<dyn BlockDevice>> {
    let devices = discovered();
    if hardware_read_only() {
        devices.into_iter().map(read_only::device).collect()
    } else {
        devices
    }
}

/// The x86-64 disk at `index` in `devices()` order.
///
/// The x86 launcher attaches every hardware profile's disks to one kind of
/// controller in a fixed order (0 = UEFI boot image, 1 = test binaries,
/// 2 = ext2 root, 3 = optional home disk), so the index names the same disk
/// whichever controller carries it. The root and test disks are found by
/// content through `devices()`; the home disk is an ext2 filesystem like the
/// root, so it is found by its place in that order.
#[cfg(target_arch = "x86_64")]
pub fn disk(index: usize) -> Option<alloc::boxed::Box<dyn BlockDevice>> {
    devices().into_iter().nth(index)
}

/// Generic block device interface
///
/// This trait provides a uniform interface for block-based storage devices.
/// Block sizes are device-specific (typically 512 bytes for raw sectors,
/// but filesystems may use 1024, 2048, or 4096 byte blocks).
#[allow(dead_code)] // Part of public block device API, will be used by ext2 filesystem
pub trait BlockDevice: Send + Sync {
    /// Whether the backing device refuses writes.
    fn is_read_only(&self) -> bool { false }

    /// Hardware backend used for a device-specific write grant.
    fn hardware_disk(&self) -> Option<HardwareDisk> { None }

    /// Stable allocation-quarantine identity, retained by operation-local wrappers.
    fn identity(&self) -> usize { self as *const Self as *const () as usize }

    /// Read a block into the provided buffer
    ///
    /// # Arguments
    /// * `block_num` - The block number to read (0-indexed)
    /// * `buf` - Buffer to read into (must be at least `block_size()` bytes)
    ///
    /// # Errors
    /// Returns `BlockError::OutOfBounds` if block_num >= num_blocks()
    /// Returns `BlockError::IoError` if the read operation fails
    fn read_block(&self, block_num: u64, buf: &mut [u8]) -> Result<(), BlockError>;

    /// Read consecutive blocks into the provided buffer.
    ///
    /// The default implementation preserves compatibility for simple block
    /// devices. Drivers with native multi-block commands should override this
    /// to avoid issuing one hardware command per sector.
    fn read_blocks(
        &self,
        start_block: u64,
        block_count: usize,
        buf: &mut [u8],
    ) -> Result<(), BlockError> {
        let block_size = self.block_size();
        if buf.len() < block_count.saturating_mul(block_size) {
            return Err(BlockError::IoError);
        }

        for i in 0..block_count {
            self.read_block(
                start_block + i as u64,
                &mut buf[i * block_size..(i + 1) * block_size],
            )?;
        }

        Ok(())
    }

    /// Write a block from the provided buffer
    ///
    /// # Arguments
    /// * `block_num` - The block number to write (0-indexed)
    /// * `buf` - Buffer to write from (must be at least `block_size()` bytes)
    ///
    /// # Errors
    /// Returns `BlockError::OutOfBounds` if block_num >= num_blocks()
    /// Returns `BlockError::IoError` if the write operation fails
    fn write_block(&self, block_num: u64, buf: &[u8]) -> Result<(), BlockError>;


    /// Write consecutive blocks, using one command per sector unless the
    /// backend supplies a native multi-block implementation.
    fn write_blocks(&self, start_block: u64, block_count: usize, buf: &[u8]) -> Result<(), BlockError> {
        let block_size = self.block_size();
        let byte_count = block_count.checked_mul(block_size).ok_or(BlockError::IoError)?;
        if buf.len() < byte_count { return Err(BlockError::IoError); }
        for i in 0..block_count {
            self.write_block(start_block + i as u64, &buf[i * block_size..(i + 1) * block_size])?;
        }
        Ok(())
    }

    /// Get the block size in bytes
    ///
    /// This is the native block size for this device. For raw sector devices,
    /// this is typically 512 bytes. Filesystems may use larger block sizes
    /// (1024, 2048, 4096) and perform multiple sector reads/writes as needed.
    fn block_size(&self) -> usize;

    /// Get the total number of blocks on the device
    fn num_blocks(&self) -> u64;

    /// Flush any cached writes to persistent storage
    ///
    /// This ensures all pending writes are committed to the physical device.
    /// Implementations that don't cache writes may return Ok(()) immediately.
    fn flush(&self) -> Result<(), BlockError>;
}

/// Errors that can occur during block device operations
#[allow(dead_code)] // Part of public block device API, will be used by ext2 filesystem
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockError {
    /// The filesystem has no free block for data or addressing.
    NoSpace,
    /// A fallible filesystem buffer allocation failed.
    OutOfMemory,
    /// I/O error occurred during operation
    IoError,
    /// Block number is out of bounds
    OutOfBounds,
    /// Device is not ready or not responding
    DeviceNotReady,
    /// Operation timed out
    Timeout,
    /// Deferred allocation metadata could not be published; inode links must not be published.
    MetadataPublishFailed,
}

impl fmt::Display for BlockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BlockError::NoSpace => write!(f, "no space left"),
            BlockError::OutOfMemory => write!(f, "out of memory"),
            BlockError::IoError => write!(f, "I/O error"),
            BlockError::OutOfBounds => write!(f, "block number out of bounds"),
            BlockError::DeviceNotReady => write!(f, "device not ready"),
            BlockError::Timeout => write!(f, "operation timed out"),
            BlockError::MetadataPublishFailed => write!(f, "allocation metadata publication failed"),
        }
    }
}

impl From<&'static str> for BlockError {
    fn from(s: &'static str) -> Self {
        // Map common error strings from VirtIO driver to BlockError variants
        match s {
            "Sector out of range" | "Start sector out of range" => BlockError::OutOfBounds,
            "Read request timed out" | "Write request timed out" => BlockError::Timeout,
            "Block device not initialized" => BlockError::DeviceNotReady,
            _ => BlockError::IoError,
        }
    }
}
