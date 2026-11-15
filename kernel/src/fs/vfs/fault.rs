//! ext2/VFS fault-injection leg — TEST PROFILE ONLY.
//!
//! Drives the filesystem read path through block-layer short reads, EIO and
//! corrupted inode records. The format registration supplies private geometry.
//!
//! # The three shapes and what each one must do
//!
//! | shape             | injected at the block layer                              | required behaviour |
//! |-------------------|----------------------------------------------------------|--------------------|
//! | `short_read`      | device returns `Ok` having filled only the first N bytes  | the superblock parse must REJECT the truncated image (mount → `Err`), not build a filesystem out of a half-read block |
//! | `eio_data_block`  | device returns `Err(IoError)` for one file data block     | the error must propagate to the caller of the read, and the same read must succeed again once the device recovers |
//! | `corrupt_inode`   | device returns `Ok` with the inode record overwritten     | the corrupt inode must produce `Err`, whether the corruption is an implausible size or wild block pointers |
//!
//! Common to these three shapes, and the reason this is a leg rather than three unit
//! tests: **no panic, no hang, and the kernel is still live afterwards** — the
//! leg runs during boot and checks liveness after disarming.
//!
//! # Production cleanliness
//!
//! The whole module is behind `#[cfg(feature = "fs_fault_inject")]`, as are its
//! two call sites (one per architecture main). Production exclusion is checked by
//! `scripts/check-fs-fault-production-clean.sh` against a production ELF rather than trusting the
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

use super::*;
use crate::block::{BlockDevice, BlockError};
use crate::fs::errno::Errno;
use alloc::{boxed::Box, sync::Arc, vec::Vec};
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

const SHAPE_NONE: u32 = 0;
const SHAPE_SHORT_READ: u32 = 1;
const SHAPE_IO_ERROR: u32 = 2;
const SHAPE_CORRUPT_INODE: u32 = 3;

/// The armed shape, the device-sector window it applies to, and its parameter.
///
/// The arming state is module-level rather than a field of the wrapper because
/// the mount constructor takes the device by value: for the `short_read` shape the
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
/// per-block loop runs and each requested sector passes through `read_block` where the
/// injection decision is made. A multi-sector fast path would let a fault
/// window be stepped over.
pub struct FaultBlockDevice {
    corrupt: fn(&mut [u8], usize, u64) -> bool,
    inner: Box<dyn BlockDevice>,
}

impl FaultBlockDevice {
    pub fn new(inner: Box<dyn BlockDevice>, corrupt: fn(&mut [u8], usize, u64) -> bool) -> Self {
        Self { inner, corrupt }
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
    fn is_read_only(&self) -> bool {
        self.inner.is_read_only()
    }

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
                if !(self.corrupt)(
                    buf,
                    PARAM.load(Ordering::SeqCst) as usize,
                    PARAM2.load(Ordering::SeqCst),
                ) {
                    return Ok(());
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
/// The injector reads a private filesystem: it builds its own filesystem
/// over its own device handle, so an injected fault cannot reach the mount the
/// rest of the boot depends on. Device selection mirrors the root device selection
/// (disk 2, else disk 0, on x86_64; VirtIO index 0 on AArch64 QEMU).
fn open_fault_device() -> Option<Box<dyn BlockDevice>> {
    #[cfg(target_arch = "x86_64")]
    {
        crate::block::disk(2).or_else(|| crate::block::disk(0))
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        use crate::block::virtio::VirtioBlockWrapper;
        let dev = VirtioBlockWrapper::new(2).or_else(|| VirtioBlockWrapper::new(0))?;
        Some(Box::new(dev))
    }
}

#[derive(Clone, Copy)]
pub struct FaultLayout {
    pub header_sector: u64,
    pub data_sector: u64,
    pub data_sectors: u64,
    pub inode_sector: u64,
    pub inode_offset: u64,
}
pub struct FaultImage {
    pub filesystem: Arc<dyn FileSystem>,
    pub layout: FaultLayout,
}

/// Decorate a private filesystem whose block device carries the fault window.
/// Filesystem operations still run the backend's ordinary validation and I/O.
pub struct FaultFs<F: FileSystem + ?Sized> {
    inner: Arc<F>,
    layout: FaultLayout,
}
impl FaultFs<dyn FileSystem> {
    fn new(image: FaultImage) -> Self {
        Self {
            inner: image.filesystem,
            layout: image.layout,
        }
    }
}
impl<F: FileSystem + ?Sized> FileSystem for FaultFs<F> {
    fn kind(&self) -> Result<&'static str, Errno> {
        self.inner.kind()
    }
    fn root(&self) -> Result<u64, Errno> {
        self.inner.root()
    }
    fn statfs(&self) -> Result<StatFs, Errno> {
        self.inner.statfs()
    }
    fn sync(&self) -> Result<(), Errno> {
        self.inner.sync()
    }
    fn check_live(&self) -> Result<(), Errno> {
        self.inner.check_live()
    }
    #[cfg(feature = "watchdog_fault_inject")]
    fn hold_writer(&self, held: &dyn Fn()) -> Result<(), Errno> {
        self.inner.hold_writer(held)
    }
    fn init_service(&self) -> Result<(), Errno> {
        self.inner.init_service()
    }
    fn shutdown(&self, name: &str, deadline: u64) -> Result<(), Errno> {
        self.inner.shutdown(name, deadline)
    }
    fn report(
        &self,
        report: Report,
        name: &str,
        text: &mut alloc::string::String,
    ) -> Result<(), Errno> {
        self.inner.report(report, name, text)
    }
    fn read_only(&self) -> Result<bool, Errno> {
        self.inner.read_only()
    }
    fn max_file_size(&self) -> Result<u64, Errno> {
        self.inner.max_file_size()
    }
    fn getattr(&self, ino: u64) -> Result<NodeAttrs, Errno> {
        self.inner.getattr(ino)
    }
    fn setattr(&self, ino: u64, attrs: SetAttr) -> Result<(), Errno> {
        self.inner.setattr(ino, attrs)
    }
    fn open(&self, ino: u64) -> Result<Arc<VNode>, Errno> {
        self.inner.open(ino)
    }
    fn lookup(&self, dir: u64, name: &[u8]) -> Result<u64, Errno> {
        self.inner.lookup(dir, name)
    }
    fn lookup_open(&self, dir: &VNode, name: &[u8]) -> Result<Arc<VNode>, Errno> {
        self.inner.lookup_open(dir, name)
    }
    fn touch(&self, node: &VNode, read: bool) {
        self.inner.touch(node, read)
    }
    fn parent(&self, dir: u64) -> Result<u64, Errno> {
        self.inner.parent(dir)
    }
    fn readdir(
        &self,
        dir: u64,
        cookie: u64,
        sink: &mut dyn FnMut(DirEnt, u64) -> Result<bool, Errno>,
    ) -> Result<u64, Errno> {
        self.inner.readdir(dir, cookie, sink)
    }
    fn create(&self, dir: u64, name: &[u8], node: NewNode<'_>) -> Result<u64, Errno> {
        self.inner.create(dir, name, node)
    }
    fn create_open(
        &self,
        dir: u64,
        name: &[u8],
        kind: NodeKind,
        check: &dyn Fn(NodeAttrs) -> Result<InodeAttrs, Errno>,
        prepare: &mut dyn FnMut(Arc<VNode>) -> Result<(), Errno>,
    ) -> Result<(), Errno> {
        self.inner.create_open(dir, name, kind, check, prepare)
    }
    fn link(&self, ino: u64, dir: u64, name: &[u8], check: &LinkCheck<'_>) -> Result<(), Errno> {
        self.inner.link(ino, dir, name, check)
    }
    fn remove(&self, dir: u64, name: &[u8], which: Remove) -> Result<(), Errno> {
        self.inner.remove(dir, name, which)
    }
    fn rename(
        &self,
        from_dir: u64,
        from: &[u8],
        to_dir: u64,
        to: &[u8],
        mode: RenameMode,
        check: &RenameCheck<'_>,
    ) -> Result<(), Errno> {
        self.inner.rename(from_dir, from, to_dir, to, mode, check)
    }
    fn readlink(&self, ino: u64) -> Result<Vec<u8>, Errno> {
        self.inner.readlink(ino)
    }
    fn read(&self, node: &VNode, off: u64, buf: &mut [u8]) -> Result<usize, Errno> {
        self.inner.read(node, off, buf)
    }
    fn write(&self, node: &VNode, req: WriteReq, buf: &[u8]) -> Result<WriteResult, WriteError> {
        self.inner.write(node, req, buf)
    }
    fn resize(
        &self,
        node: &VNode,
        size: u64,
        clear_setid: bool,
        check: &dyn Fn(NodeAttrs) -> Result<(), Errno>,
    ) -> Result<(), Errno> {
        self.inner.resize(node, size, clear_setid, check)
    }
    fn fsync(&self, node: &VNode, data_only: bool) -> Result<(), Errno> {
        self.inner.fsync(node, data_only)
    }
    fn writeback_pages(
        &self,
        node: &VNode,
        page: u64,
        bytes: &[u8],
        io: &dyn MappingIo,
    ) -> Result<(), Errno> {
        self.inner.writeback_pages(node, page, bytes, io)
    }
    fn mapping_io(&self, node: &VNode) -> Result<Box<dyn MappingIo>, Errno> {
        self.inner.mapping_io(node)
    }
    fn clear_mapping_privilege(&self, node: &VNode) -> Result<(), Errno> {
        self.inner.clear_mapping_privilege(node)
    }
    fn request_eviction(&self) {
        self.inner.request_eviction()
    }
    fn xattr(&self, ino: u64, op: Xattr<'_>) -> Result<usize, Errno> {
        self.inner.xattr(ino, op)
    }
}

fn mount_image(device: Box<dyn BlockDevice>) -> Result<FaultImage, Errno> {
    // The private constructor must see an armed short read itself, rather than
    // consuming it in a separate probe first. The selected root supplies its kind.
    let kind = super::root_filesystem()?.kind()?;
    let format = crate::fs::FORMATS
        .iter()
        .find(|f| f.kind == kind)
        .ok_or(Errno(crate::syscall::errno::ENODEV))?;
    (format.fault_mount)(device)
}

fn read_all(fs: &dyn FileSystem, node: &VNode) -> Result<Vec<u8>, Errno> {
    let size = fs.getattr(node.ino)?.size as usize;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(size)
        .map_err(|_| Errno(crate::syscall::errno::ENOMEM))?;
    bytes.resize(size, 0);
    let mut at = 0;
    while at < size {
        let n = fs.read(node, at as u64, &mut bytes[at..])?;
        if n == 0 {
            return Err(Errno(crate::syscall::errno::EIO));
        }
        at += n;
    }
    Ok(bytes)
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
    crate::serial_println!("[FSFAULT:{}:BEGIN]", ARCH);
    let mut tally = Tally { pass: 0, fail: 0 };

    let Some(device) = open_fault_device() else {
        crate::serial_println!(
            "[FSFAULT:{}:setup:verdict=FAIL:detail=no-ext2-block-device]",
            ARCH
        );
        crate::serial_println!("[FSFAULT:{}:COMPLETE:pass=0:fail=1]", ARCH);
        return;
    };

    // ---------------------------------------------------------------- baseline
    // Everything below asserts that a fault produces an error. That claim is
    // vacuous unless the same operations succeed with no fault armed, so the
    // clean arm runs first, through the same injector object.
    disarm();
    let baseline = mount_image(device);
    let fs = match baseline {
        Ok(image) => {
            crate::serial_println!("[FSFAULT:{}:baseline_mount:verdict=PASS]", ARCH);
            tally.record(true);
            FaultFs::new(image)
        }
        Err(e) => {
            crate::serial_println!(
                "[FSFAULT:{}:baseline_mount:verdict=FAIL:detail=clean-mount-failed:{}]",
                ARCH,
                e.0
            );
            crate::serial_println!("[FSFAULT:{}:COMPLETE:pass=0:fail=1]", ARCH);
            return;
        }
    };

    let root_inode = match fs.open(fs.root().unwrap_or(0)) {
        Ok(inode) => inode,
        Err(e) => {
            crate::serial_println!(
                "[FSFAULT:{}:baseline_inode:verdict=FAIL:detail=clean-root-inode-failed:{}]",
                ARCH,
                e.0
            );
            crate::serial_println!("[FSFAULT:{}:COMPLETE:pass={}:fail=1]", ARCH, tally.pass);
            return;
        }
    };
    let baseline_dir = match read_all(&fs, &root_inode) {
        Ok(bytes) => bytes,
        Err(e) => {
            crate::serial_println!(
                "[FSFAULT:{}:baseline_read:verdict=FAIL:detail=clean-root-read-failed:{}]",
                ARCH,
                e.0
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
    // Deliver only the first 16 bytes of the registered header sector. The
    // constructor must refuse the truncated header.
    {
        arm_short_read(fs.layout.header_sector, 16);
        let Some(device) = open_fault_device() else {
            crate::serial_println!(
                "[FSFAULT:{}:short_read:verdict=FAIL:detail=device-handle-lost]",
                ARCH
            );
            tally.record(false);
            disarm();
            return;
        };
        let result = mount_image(device);
        let fired = hits();
        match result {
            Err(reason) => {
                crate::serial_println!(
                    "[FSFAULT:{}:short_read:verdict=PASS:hits={}:observed=Err:detail={}]",
                    ARCH,
                    fired,
                    reason.0
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
        let data_sector = fs.layout.data_sector;
        arm_io_error(data_sector, fs.layout.data_sectors);
        let faulted = read_all(&fs, &root_inode);
        let fired = hits();
        match faulted {
            Err(reason) => {
                crate::serial_println!(
                    "[FSFAULT:{}:eio_data_block:verdict=PASS:hits={}:sector={}:observed=Err:detail={}]",
                    ARCH,
                    fired,
                    data_sector,
                    reason.0
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
        match read_all(&fs, &root_inode) {
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
                    reason.0
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
        let (inode_sector, offset_in_sector) = (fs.layout.inode_sector, fs.layout.inode_offset);

        for (which, arm_name) in [(CORRUPT_SIZE, "size"), (CORRUPT_BLOCKS, "blocks")] {
            arm_corrupt_inode(inode_sector, offset_in_sector, which);
            let corrupt = match fs.open(fs.root().unwrap_or(0)) {
                Ok(inode) => inode,
                Err(reason) => {
                    // Rejecting the corrupt inode at read time is also a correct
                    // outcome: the error reached the caller before a read.
                    crate::serial_println!(
                        "[FSFAULT:{}:corrupt_inode:arm={}:verdict=PASS:hits={}:observed=Err:detail=rejected-at-read-inode:{}]",
                        ARCH,
                        arm_name,
                        hits(),
                        reason.0
                    );
                    tally.record(true);
                    disarm();
                    continue;
                }
            };
            let mut bytes = [0; 4096];
            let read = fs.read(&corrupt, 0, &mut bytes);
            let fired = hits();
            let size = corrupt.size.load(Ordering::Acquire);
            match read {
                Err(reason) => {
                    crate::serial_println!(
                        "[FSFAULT:{}:corrupt_inode:arm={}:verdict=PASS:hits={}:i_size={}:observed=Err:detail={}]",
                        ARCH,
                        arm_name,
                        fired,
                        size,
                        reason.0
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
                        bytes
                    );
                    tally.record(false);
                }
            }
            disarm();
        }
    }

    // ---------------------------------------------------------------- liveness
    // The injector is disarmed; the shadow filesystem must still read, and the
    // mounted root must still pass its ordinary backend health check.
    disarm();
    let shadow_ok = read_all(&fs, &root_inode)
        .map(|b| b.len() == baseline_dir.len())
        .unwrap_or(false);
    let root_ok = super::root_filesystem()
        .and_then(|fs| fs.check_live())
        .is_ok();
    if shadow_ok && root_ok {
        crate::serial_println!(
            "[FSFAULT:{}:liveness:verdict=PASS:shadow=1:mounted=1]",
            ARCH
        );
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
