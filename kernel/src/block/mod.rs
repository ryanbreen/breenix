//! Block Device Abstraction Layer
//!
//! Provides a generic interface for block devices, allowing filesystems to work
//! with different underlying storage implementations (VirtIO, AHCI, etc.) through
//! a common trait.

use core::fmt;

#[cfg(target_arch = "x86_64")]
mod read_only;

#[cfg(target_arch = "x86_64")]
pub mod install;
#[cfg(target_arch = "x86_64")]
pub mod nvme;
#[cfg(target_arch = "x86_64")]
pub mod ramdisk;
pub mod virtio;

// Fail closed until early boot identifies the platform, before any driver starts.
#[cfg(target_arch = "x86_64")]
static HARDWARE_READ_ONLY: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);

/// Whether this boot's root is the installed hardware disk (`install`).
#[cfg(target_arch = "x86_64")]
static ROOT_ON_INSTALLED_DISK: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

#[cfg(target_arch = "x86_64")]
pub fn init_hardware_policy(qemu: bool) {
    HARDWARE_READ_ONLY.store(!qemu, core::sync::atomic::Ordering::Release);
}

/// Hardware writes are reserved for the storage lane on physical x86 boots.
/// RAM-disk writes do not pass through hardware drivers.
pub fn hardware_read_only() -> bool {
    #[cfg(target_arch = "x86_64")]
    { HARDWARE_READ_ONLY.load(core::sync::atomic::Ordering::Acquire) }
    #[cfg(not(target_arch = "x86_64"))]
    { false }
}

/// The storage lane's explicit lift of the physical-boot write refusal, and
/// its return. Only `install_requested_root` and `installed_root` call it,
/// each after checking that one disk is the subject (`install`).
#[cfg(target_arch = "x86_64")]
fn hardware_writes(granted: bool, purpose: &str) {
    HARDWARE_READ_ONLY.store(!granted, core::sync::atomic::Ordering::Release);
    if granted {
        log::info!("[block] hardware disk writes granted: {}", purpose);
    } else {
        log::info!("[block] hardware disk writes revoked: {}", purpose);
    }
}

/// Prefer a supplied bootloader RAM disk; refuse hardware root fallback on physical x86 (issue #1702),
/// except for the one disk the storage lane installed a root on.
#[cfg(target_arch = "x86_64")]
pub fn root_devices() -> alloc::vec::Vec<alloc::boxed::Box<dyn BlockDevice>> {
    if let Some(device) = ramdisk::device() {
        alloc::vec![device]
    } else if hardware_read_only() {
        installed_root().into_iter().collect()
    } else {
        devices()
    }
}

/// Whether root was taken from the installed hardware disk.
#[cfg(target_arch = "x86_64")]
pub fn root_on_installed_disk() -> bool {
    ROOT_ON_INSTALLED_DISK.load(core::sync::atomic::Ordering::Acquire)
}

/// On a physical boot without a RAM disk, the one hardware disk holding a root
/// record, with writes granted for it.
#[cfg(target_arch = "x86_64")]
fn installed_root() -> Option<alloc::boxed::Box<dyn BlockDevice>> {
    let (disk, record) = match install::find_root(discovered()) {
        Ok(found) => found,
        Err(why) => {
            log::info!("[storage] no installed root: {}", why);
            return None;
        }
    };
    let install::Record::Root { boot, bytes, sha256 } = record else { return None };
    log::info!(
        "[storage] root record: installed by boot {}, {} bytes, sha256 {}",
        boot,
        bytes,
        install::hex(&sha256)
    );
    match install::matches(&*disk, bytes, &sha256) {
        Ok(true) => log::info!("[storage] root image matches its install record"),
        Ok(false) => log::info!("[storage] root image differs from its install record: written since"),
        Err(why) => log::warn!("[storage] root image unreadable: {}", why),
    }
    hardware_writes(true, "the root the storage lane installed");
    ROOT_ON_INSTALLED_DISK.store(true, core::sync::atomic::Ordering::Release);
    Some(disk)
}

/// Before root mounts: when the RAM disk's first sector asks for an install,
/// copy the RAM disk onto the machine's only hardware disk, read it back, and
/// return the disks to read-only.
#[cfg(target_arch = "x86_64")]
pub fn install_requested_root() {
    let Some(ram) = ramdisk::device() else { return };
    let mut first = [0u8; install::SECTOR];
    if ram.read_block(0, &mut first).is_err() {
        return;
    }
    let Some(install::Record::Install { boot }) = install::Record::parse(&first) else { return };
    let disks = discovered();
    if disks.len() != 1 {
        log::warn!("[storage] install refused: {} hardware disks, expected exactly one", disks.len());
        return;
    }
    let bytes = ram.num_blocks() * install::SECTOR as u64;
    log::info!("[storage] install requested by boot {}: {} bytes onto the hardware disk", boot, bytes);
    let started = crate::time::get_monotonic_time();
    let result = install::install(&*ram, &*disks[0], &boot, &mut |granted| {
        hardware_writes(granted, "install the RAM-disk root")
    });
    let elapsed = crate::time::get_monotonic_time().saturating_sub(started);
    match result {
        Ok(done) => {
            log::info!("[storage] wrote {} bytes to the hardware disk from LBA 0", done.bytes);
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
