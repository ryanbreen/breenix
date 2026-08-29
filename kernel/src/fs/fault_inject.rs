//! ext2/VFS fault-injection leg — TEST PROFILE ONLY.
//!
//! Every boot gate this kernel has proves the filesystem works when the block
//! device answers correctly. None of them proves what happens when it does not.
//! This module is the missing half: it drives the ext2 read path through three
//! block-layer fault shapes and holds each one to a stated expectation.
//!
//! # The three shapes and what each one must do
//!
//! | shape             | injected at the block layer                              | required behaviour |
//! |-------------------|----------------------------------------------------------|--------------------|
//! | `short_read`      | device returns `Ok` having filled only the first N bytes  | the superblock parse must REJECT the truncated image (`Ext2Fs::new` → `Err`), not build a filesystem out of a half-read block |
//! | `eio_data_block`  | device returns `Err(IoError)` for one file data block     | the error must propagate to the caller of the read, and the same read must succeed again once the device recovers |
//! | `corrupt_inode`   | device returns `Ok` with the inode record overwritten     | the corrupt inode must produce `Err`, whether the corruption is an implausible size or wild block pointers |
//!
//! Common to all three, and the reason this is a leg rather than three unit
//! tests: **no panic, no hang, and the kernel is still live afterwards** — the
//! leg runs during boot, and the boot that follows it is the liveness proof.
//!
//! # Production cleanliness
//!
//! The whole module is behind `#[cfg(feature = "fs_fault_inject")]`, as are its
//! two call sites (one per architecture main). Nothing here is reachable from,
//! or referenced by, a production build; `scripts/check-fs-fault-production-clean.sh`
//! measures that claim against a real production ELF rather than trusting the
//! `#[cfg]`, and `tests/fs_fault_production_clean.rs` runs it in the host suite.
//!
//! # Anti-vacuity
//!
//! A fault leg that stopped injecting would print three PASS lines forever. Each
//! shape therefore has a disarm feature — `fs_fault_disarm_short_read`,
//! `fs_fault_disarm_eio`, `fs_fault_disarm_corrupt_inode` — which turns that
//! shape's `arm_*` into a no-op while leaving the assertions in place. The
//! operation then succeeds, the leg prints `verdict=FAIL detail=fault-not-observed`
//! for that shape only, and `docker/qemu/run-fs-fault-gate.sh --disarm <shape>`
//! requires exactly that. If the injector ever silently stops working, the armed
//! run reports the same FAIL, because "the fault did not happen" and "the fault
//! was not detected" are the same observation here.

use crate::block::{BlockDevice, BlockError};
use alloc::boxed::Box;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// The ext2 inode record fields this module rewrites, by byte offset within the
/// on-disk inode. Sourced from the ext2 on-disk format, matching
/// `super::ext2::Ext2Inode`'s field order.
const INODE_OFF_MODE: usize = 0;
const INODE_OFF_SIZE: usize = 4;
const INODE_OFF_LINKS: usize = 26;
const INODE_OFF_BLOCK_PTRS: usize = 40;
const INODE_BLOCK_PTR_COUNT: usize = 15;
/// Smallest on-disk inode record; the corruption never writes past it.
const INODE_MIN_RECORD: usize = 128;

const SHAPE_NONE: u32 = 0;
const SHAPE_SHORT_READ: u32 = 1;
const SHAPE_IO_ERROR: u32 = 2;
const SHAPE_CORRUPT_INODE: u32 = 3;

/// The armed shape, the device-sector window it applies to, and its parameter.
///
/// The arming state is module-level rather than a field of the wrapper because
/// `Ext2Fs::new` takes the device by value: for the `short_read` shape the
/// wrapper is consumed by a constructor that then fails, and the leg still has
/// to be able to read back how many times the fault actually fired.
static SHAPE: AtomicU32 = AtomicU32::new(SHAPE_NONE);
static WINDOW_START: AtomicU64 = AtomicU64::new(0);
static WINDOW_COUNT: AtomicU64 = AtomicU64::new(0);
/// `short_read`: bytes the device delivers. `corrupt_inode`: byte offset of the
/// inode record within the target sector.
static PARAM: AtomicU64 = AtomicU64::new(0);
/// `corrupt_inode`: which corruption to write (see `CORRUPT_*`).
static PARAM2: AtomicU64 = AtomicU64::new(0);
/// How many times the armed fault has fired since the last `disarm()`.
static HITS: AtomicU32 = AtomicU32::new(0);

/// Corrupt the inode's size to a value larger than the whole filesystem.
const CORRUPT_SIZE: u64 = 0;
/// Corrupt the inode's block pointers to blocks that do not exist.
const CORRUPT_BLOCKS: u64 = 1;

fn disarm() {
    SHAPE.store(SHAPE_NONE, Ordering::SeqCst);
    WINDOW_COUNT.store(0, Ordering::SeqCst);
    HITS.store(0, Ordering::SeqCst);
}

fn hits() -> u32 {
    HITS.load(Ordering::SeqCst)
}

fn arm(shape: u32, start: u64, count: u64, param: u64, param2: u64) {
    HITS.store(0, Ordering::SeqCst);
    WINDOW_START.store(start, Ordering::SeqCst);
    WINDOW_COUNT.store(count, Ordering::SeqCst);
    PARAM.store(param, Ordering::SeqCst);
    PARAM2.store(param2, Ordering::SeqCst);
    SHAPE.store(shape, Ordering::SeqCst);
}

/// Arm the short-read shape: the device answers `Ok` for `sector` having
/// written only `delivered` bytes into the caller's buffer.
///
/// Disarmed by `fs_fault_disarm_short_read`, which leaves the window unarmed so
/// the read succeeds in full and the leg reports the missing fault.
fn arm_short_read(sector: u64, delivered: u64) {
    #[cfg(not(feature = "fs_fault_disarm_short_read"))]
    arm(SHAPE_SHORT_READ, sector, 1, delivered, 0);
    #[cfg(feature = "fs_fault_disarm_short_read")]
    {
        let _ = (sector, delivered);
        disarm();
    }
}

/// Arm the EIO shape: the device fails `count` sectors from `start` with
/// `BlockError::IoError`, touching no buffer bytes.
///
/// Disarmed by `fs_fault_disarm_eio`.
fn arm_io_error(start: u64, count: u64) {
    #[cfg(not(feature = "fs_fault_disarm_eio"))]
    arm(SHAPE_IO_ERROR, start, count, 0, 0);
    #[cfg(feature = "fs_fault_disarm_eio")]
    {
        let _ = (start, count);
        disarm();
    }
}

/// Arm the corrupt-inode shape: the device answers `Ok` for `sector` with the
/// inode record at `offset` overwritten by corruption `which`.
///
/// Disarmed by `fs_fault_disarm_corrupt_inode`.
fn arm_corrupt_inode(sector: u64, offset: u64, which: u64) {
    #[cfg(not(feature = "fs_fault_disarm_corrupt_inode"))]
    arm(SHAPE_CORRUPT_INODE, sector, 1, offset, which);
    #[cfg(feature = "fs_fault_disarm_corrupt_inode")]
    {
        let _ = (sector, offset, which);
        disarm();
    }
}

/// A block device that answers faithfully except inside an armed fault window.
///
/// It deliberately does NOT override `read_blocks`, so the trait's default
/// per-block loop runs and every sector passes through `read_block` where the
/// injection decision is made. A multi-sector fast path would let a fault
/// window be stepped over.
struct FaultBlockDevice {
    inner: Box<dyn BlockDevice>,
}

impl FaultBlockDevice {
    fn new(inner: Box<dyn BlockDevice>) -> Self {
        Self { inner }
    }

    fn armed_for(&self, block_num: u64) -> u32 {
        let shape = SHAPE.load(Ordering::SeqCst);
        if shape == SHAPE_NONE {
            return SHAPE_NONE;
        }
        let start = WINDOW_START.load(Ordering::SeqCst);
        let count = WINDOW_COUNT.load(Ordering::SeqCst);
        if count == 0 || block_num < start || block_num >= start.saturating_add(count) {
            return SHAPE_NONE;
        }
        shape
    }
}

impl BlockDevice for FaultBlockDevice {
    fn read_block(&self, block_num: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        match self.armed_for(block_num) {
            SHAPE_NONE => self.inner.read_block(block_num, buf),
            SHAPE_IO_ERROR => {
                HITS.fetch_add(1, Ordering::SeqCst);
                Err(BlockError::IoError)
            }
            SHAPE_SHORT_READ => {
                // A real short read leaves the tail of the caller's buffer
                // untouched, so read into scratch and copy only the delivered
                // prefix. Overwriting the tail here would be a corruption
                // shape, not a truncation shape.
                let block_size = core::cmp::min(self.inner.block_size(), 4096);
                let mut scratch = [0u8; 4096];
                self.inner
                    .read_block(block_num, &mut scratch[..block_size])?;
                HITS.fetch_add(1, Ordering::SeqCst);
                let delivered = core::cmp::min(
                    PARAM.load(Ordering::SeqCst) as usize,
                    core::cmp::min(block_size, buf.len()),
                );
                buf[..delivered].copy_from_slice(&scratch[..delivered]);
                Ok(())
            }
            SHAPE_CORRUPT_INODE => {
                self.inner.read_block(block_num, buf)?;
                let offset = PARAM.load(Ordering::SeqCst) as usize;
                if offset + INODE_MIN_RECORD > buf.len() {
                    // Cannot corrupt what does not fit; leave the buffer honest
                    // and let the leg report a missing fault rather than a
                    // half-applied one.
                    return Ok(());
                }
                let record = &mut buf[offset..offset + INODE_MIN_RECORD];
                match PARAM2.load(Ordering::SeqCst) {
                    CORRUPT_SIZE => {
                        // A regular file, one link, and a size larger than any
                        // filesystem this kernel can mount.
                        record[INODE_OFF_MODE..INODE_OFF_MODE + 2]
                            .copy_from_slice(&0x81FFu16.to_le_bytes());
                        record[INODE_OFF_LINKS..INODE_OFF_LINKS + 2]
                            .copy_from_slice(&1u16.to_le_bytes());
                        record[INODE_OFF_SIZE..INODE_OFF_SIZE + 4]
                            .copy_from_slice(&0xFFFF_FFF0u32.to_le_bytes());
                    }
                    _ => {
                        // A plausible small regular file whose every block
                        // pointer names a block the device does not have.
                        record[INODE_OFF_MODE..INODE_OFF_MODE + 2]
                            .copy_from_slice(&0x81FFu16.to_le_bytes());
                        record[INODE_OFF_LINKS..INODE_OFF_LINKS + 2]
                            .copy_from_slice(&1u16.to_le_bytes());
                        record[INODE_OFF_SIZE..INODE_OFF_SIZE + 4]
                            .copy_from_slice(&1024u32.to_le_bytes());
                        for i in 0..INODE_BLOCK_PTR_COUNT {
                            let at = INODE_OFF_BLOCK_PTRS + i * 4;
                            record[at..at + 4].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
                        }
                    }
                }
                HITS.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
            _ => self.inner.read_block(block_num, buf),
        }
    }

    fn write_block(&self, block_num: u64, buf: &[u8]) -> Result<(), BlockError> {
        self.inner.write_block(block_num, buf)
    }

    fn block_size(&self) -> usize {
        self.inner.block_size()
    }

    fn num_blocks(&self) -> u64 {
        self.inner.num_blocks()
    }

    fn flush(&self) -> Result<(), BlockError> {
        self.inner.flush()
    }
}

#[cfg(target_arch = "aarch64")]
const ARCH: &str = "aarch64";
#[cfg(target_arch = "x86_64")]
const ARCH: &str = "x86";

/// Open a second handle to the ext2 block device.
///
/// The leg never touches the mounted root filesystem: it builds its own `Ext2Fs`
/// over its own device handle, so an injected fault cannot reach the mount the
/// rest of the boot depends on. Device selection mirrors `ext2::init_root_fs`'s
/// VirtIO arm (index 2 on x86_64, index 0 on AArch64 QEMU).
fn open_ext2_device() -> Option<Box<dyn BlockDevice>> {
    use crate::block::virtio::VirtioBlockWrapper;
    let dev = VirtioBlockWrapper::new(2).or_else(|| VirtioBlockWrapper::new(0))?;
    Some(Box::new(dev))
}

/// Sector arithmetic for one inode record, mirroring `Ext2Inode::read_from`.
///
/// Returns `(device_sector, byte_offset_within_that_sector)`.
fn inode_record_location(fs: &super::ext2::Ext2Fs, inode_num: u32, device_block_size: usize) -> (u64, u64) {
    let sb = &fs.superblock;
    let inodes_per_group =
        unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(sb.s_inodes_per_group)) };
    let s_rev_level = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(sb.s_rev_level)) };
    let s_inode_size = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(sb.s_inode_size)) };
    let s_log_block_size =
        unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(sb.s_log_block_size)) };

    let inode_index = inode_num - 1;
    let group = (inode_index / inodes_per_group) as usize;
    let local_index = inode_index % inodes_per_group;
    let inode_table_block = unsafe {
        core::ptr::read_unaligned(core::ptr::addr_of!(fs.block_groups[group].bg_inode_table))
    };
    let inode_size: u32 = if s_rev_level == 0 {
        128
    } else {
        s_inode_size as u32
    };
    let ext2_block_size = 1024u32 << s_log_block_size;
    let byte_offset = local_index * inode_size;
    let target_ext2_block = inode_table_block + byte_offset / ext2_block_size;
    let offset_in_ext2_block = byte_offset % ext2_block_size;

    let sectors_per_ext2_block = (ext2_block_size as usize / device_block_size) as u64;
    let sector = target_ext2_block as u64 * sectors_per_ext2_block
        + (offset_in_ext2_block as usize / device_block_size) as u64;
    let offset_in_sector = (offset_in_ext2_block as usize % device_block_size) as u64;
    (sector, offset_in_sector)
}

struct Tally {
    pass: u32,
    fail: u32,
}

impl Tally {
    fn record(&mut self, ok: bool) {
        if ok {
            self.pass += 1;
        } else {
            self.fail += 1;
        }
    }
}

/// Run the ext2/VFS fault-injection leg.
///
/// Called from each architecture's main immediately after the root filesystem
/// mounts, so that everything the boot does afterwards is evidence that the
/// kernel survived the faults.
pub fn run_fs_fault_leg() {
    use super::ext2::{read_file, Ext2Fs, EXT2_ROOT_INO};

    crate::serial_println!("[FSFAULT:{}:BEGIN]", ARCH);
    let mut tally = Tally { pass: 0, fail: 0 };

    let Some(device) = open_ext2_device() else {
        crate::serial_println!(
            "[FSFAULT:{}:setup:verdict=FAIL:detail=no-ext2-block-device]",
            ARCH
        );
        crate::serial_println!("[FSFAULT:{}:COMPLETE:pass=0:fail=1]", ARCH);
        return;
    };
    let device_block_size = device.block_size();

    // ---------------------------------------------------------------- baseline
    // Everything below asserts that a fault produces an error. That claim is
    // vacuous unless the same operations succeed with no fault armed, so the
    // clean arm runs first, through the same injector object.
    disarm();
    let baseline = Ext2Fs::new(Box::new(FaultBlockDevice::new(device)), usize::MAX);
    let fs = match baseline {
        Ok(fs) => {
            crate::serial_println!("[FSFAULT:{}:baseline_mount:verdict=PASS]", ARCH);
            tally.record(true);
            fs
        }
        Err(e) => {
            crate::serial_println!(
                "[FSFAULT:{}:baseline_mount:verdict=FAIL:detail=clean-mount-failed:{}]",
                ARCH,
                e
            );
            crate::serial_println!("[FSFAULT:{}:COMPLETE:pass=0:fail=1]", ARCH);
            return;
        }
    };

    let root_inode = match fs.read_inode(EXT2_ROOT_INO) {
        Ok(inode) => inode,
        Err(e) => {
            crate::serial_println!(
                "[FSFAULT:{}:baseline_inode:verdict=FAIL:detail=clean-root-inode-failed:{}]",
                ARCH,
                e
            );
            crate::serial_println!("[FSFAULT:{}:COMPLETE:pass={}:fail=1]", ARCH, tally.pass);
            return;
        }
    };
    let baseline_dir = match fs.read_directory(&root_inode) {
        Ok(bytes) => bytes,
        Err(e) => {
            crate::serial_println!(
                "[FSFAULT:{}:baseline_read:verdict=FAIL:detail=clean-root-read-failed:{}]",
                ARCH,
                e
            );
            crate::serial_println!("[FSFAULT:{}:COMPLETE:pass={}:fail=1]", ARCH, tally.pass);
            return;
        }
    };
    crate::serial_println!(
        "[FSFAULT:{}:baseline_read:verdict=PASS:bytes={}]",
        ARCH,
        baseline_dir.len()
    );
    tally.record(true);

    // ------------------------------------------------------------- short read
    // The superblock lives at byte 1024. Deliver only the first 16 bytes of the
    // sector that carries it: the ext2 magic at offset 56 is then never written,
    // and a filesystem must refuse to mount rather than mount a phantom.
    {
        let sb_sector = (1024 / device_block_size) as u64;
        arm_short_read(sb_sector, 16);
        let Some(device) = open_ext2_device() else {
            crate::serial_println!(
                "[FSFAULT:{}:short_read:verdict=FAIL:detail=device-handle-lost]",
                ARCH
            );
            tally.record(false);
            disarm();
            return;
        };
        let result = Ext2Fs::new(Box::new(FaultBlockDevice::new(device)), usize::MAX);
        let fired = hits();
        match result {
            Err(reason) => {
                crate::serial_println!(
                    "[FSFAULT:{}:short_read:verdict=PASS:hits={}:observed=Err:detail={}]",
                    ARCH,
                    fired,
                    reason
                );
                tally.record(true);
            }
            Ok(_) => {
                crate::serial_println!(
                    "[FSFAULT:{}:short_read:verdict=FAIL:hits={}:detail=fault-not-observed-mount-succeeded]",
                    ARCH,
                    fired
                );
                tally.record(false);
            }
        }
        disarm();
    }

    // -------------------------------------------------------- EIO, data block
    // The root directory's first data block, named by the inode itself, so the
    // failing sector is a real file data block rather than a guessed offset.
    {
        let i_block =
            unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(root_inode.i_block)) };
        let ext2_block_size = fs.superblock.block_size();
        let sectors_per_ext2_block = (ext2_block_size / device_block_size) as u64;
        let data_sector = i_block[0] as u64 * sectors_per_ext2_block;

        arm_io_error(data_sector, sectors_per_ext2_block.max(1));
        let faulted = fs.read_directory(&root_inode);
        let fired = hits();
        match faulted {
            Err(reason) => {
                crate::serial_println!(
                    "[FSFAULT:{}:eio_data_block:verdict=PASS:hits={}:sector={}:observed=Err:detail={}]",
                    ARCH,
                    fired,
                    data_sector,
                    reason
                );
                tally.record(true);
            }
            Ok(bytes) => {
                crate::serial_println!(
                    "[FSFAULT:{}:eio_data_block:verdict=FAIL:hits={}:sector={}:detail=fault-not-observed-read-returned-{}-bytes]",
                    ARCH,
                    fired,
                    data_sector,
                    bytes.len()
                );
                tally.record(false);
            }
        }

        // Recovery: the device stops failing and the identical read must work
        // again. This is what separates "the error propagated" from "the error
        // wedged the filesystem".
        disarm();
        match fs.read_directory(&root_inode) {
            Ok(bytes) if bytes.len() == baseline_dir.len() => {
                crate::serial_println!(
                    "[FSFAULT:{}:eio_recovery:verdict=PASS:bytes={}]",
                    ARCH,
                    bytes.len()
                );
                tally.record(true);
            }
            Ok(bytes) => {
                crate::serial_println!(
                    "[FSFAULT:{}:eio_recovery:verdict=FAIL:detail=short-after-recovery:bytes={}:expected={}]",
                    ARCH,
                    bytes.len(),
                    baseline_dir.len()
                );
                tally.record(false);
            }
            Err(reason) => {
                crate::serial_println!(
                    "[FSFAULT:{}:eio_recovery:verdict=FAIL:detail=still-failing:{}]",
                    ARCH,
                    reason
                );
                tally.record(false);
            }
        }
    }

    // ----------------------------------------------------------- corrupt inode
    // Two arms, because a corrupt inode has two distinct ways to hurt: a size
    // field that turns a read into a multi-gigabyte allocation, and block
    // pointers that name blocks the device does not have.
    {
        let (inode_sector, offset_in_sector) =
            inode_record_location(&fs, EXT2_ROOT_INO, device_block_size);

        for (which, arm_name) in [(CORRUPT_SIZE, "size"), (CORRUPT_BLOCKS, "blocks")] {
            arm_corrupt_inode(inode_sector, offset_in_sector, which);
            let corrupt = match fs.read_inode(EXT2_ROOT_INO) {
                Ok(inode) => inode,
                Err(reason) => {
                    // Rejecting the corrupt inode at read time is also a correct
                    // outcome: the error reached the caller and nothing was
                    // built on top of the corruption.
                    crate::serial_println!(
                        "[FSFAULT:{}:corrupt_inode:arm={}:verdict=PASS:hits={}:observed=Err:detail=rejected-at-read-inode:{}]",
                        ARCH,
                        arm_name,
                        hits(),
                        reason
                    );
                    tally.record(true);
                    disarm();
                    continue;
                }
            };
            let read = read_file(fs.device.as_ref(), &corrupt, &fs.superblock);
            let fired = hits();
            let size = corrupt.size();
            match read {
                Err(reason) => {
                    crate::serial_println!(
                        "[FSFAULT:{}:corrupt_inode:arm={}:verdict=PASS:hits={}:i_size={}:observed=Err:detail={}]",
                        ARCH,
                        arm_name,
                        fired,
                        size,
                        reason
                    );
                    tally.record(true);
                }
                Ok(bytes) => {
                    crate::serial_println!(
                        "[FSFAULT:{}:corrupt_inode:arm={}:verdict=FAIL:hits={}:i_size={}:detail=fault-not-observed-read-returned-{}-bytes]",
                        ARCH,
                        arm_name,
                        fired,
                        size,
                        bytes.len()
                    );
                    tally.record(false);
                }
            }
            disarm();
        }
    }

    // ---------------------------------------------------------------- liveness
    // The injector is disarmed; the shadow filesystem must still read, and the
    // real mounted root filesystem — which the leg never touched — must still
    // resolve a path. Everything the boot does after this line is the rest of
    // the liveness evidence.
    disarm();
    let shadow_ok = fs
        .read_directory(&root_inode)
        .map(|b| b.len() == baseline_dir.len())
        .unwrap_or(false);
    let root_ok = match super::ext2::root_fs_read().as_ref() {
        Some(root) => root.read_inode(EXT2_ROOT_INO).is_ok(),
        None => false,
    };
    if shadow_ok && root_ok {
        crate::serial_println!("[FSFAULT:{}:liveness:verdict=PASS:shadow=1:mounted=1]", ARCH);
        tally.record(true);
    } else {
        crate::serial_println!(
            "[FSFAULT:{}:liveness:verdict=FAIL:shadow={}:mounted={}]",
            ARCH,
            shadow_ok as u32,
            root_ok as u32
        );
        tally.record(false);
    }

    crate::serial_println!(
        "[FSFAULT:{}:COMPLETE:pass={}:fail={}]",
        ARCH,
        tally.pass,
        tally.fail
    );
}
