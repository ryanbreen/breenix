//! Run the production shared-object and VMA logic with a host frame allocator.
#![cfg(test)]
#![feature(c_variadic)]
#![feature(allocator_api)]
extern crate alloc;

pub mod irq_safe_mutex {
    #[derive(Debug)]
    pub struct IrqSafeMutex<T>(std::sync::Mutex<T>);
    impl<T> IrqSafeMutex<T> {
        pub const fn new(value: T) -> Self {
            Self(std::sync::Mutex::new(value))
        }
        pub fn lock(&self) -> std::sync::MutexGuard<'_, T> {
            self.0.lock().unwrap()
        }
    }
}
pub mod fs {
    pub mod vfs {
        pub mod mount {
            pub fn find_mount(_p: &str) -> Option<u32> {
                Some(1)
            }
        }
    }
    pub mod permissions {
        #[derive(Clone)]
        pub struct Credentials {
            pub euid: u32,
            pub egid: u32,
            pub groups: Vec<u32>,
            pub umask: u32,
        }
        thread_local! { pub static CRED: std::cell::RefCell<Credentials> = std::cell::RefCell::new(Credentials{euid:0,egid:0,groups:vec![],umask:0}); }
        impl Credentials {
            pub fn current(_real: bool) -> Self {
                CRED.with(|c| c.borrow().clone())
            }
            pub fn permits_mode(&self, uid: u32, gid: u32, mode: u32, wanted: u32) -> bool {
                if self.euid == 0 {
                    return wanted & 1 == 0 || mode & 0o111 != 0;
                }
                let shift = if self.euid == uid {
                    6
                } else if self.in_group(gid) {
                    3
                } else {
                    0
                };
                (mode >> shift) & wanted == wanted
            }
            pub fn in_group(&self, gid: u32) -> bool {
                self.egid == gid || self.groups.contains(&gid)
            }
        }
    }
}
pub fn arch_without_interrupts<R>(f: impl FnOnce() -> R) -> R {
    f()
}
pub mod time {
    pub fn current_unix_time_coarse() -> i64 {
        crate::ACTIVITY_TIME.with(|t| t.get())
    }
}
pub mod per_cpu_aarch64 {
    pub fn current_thread_ptr() -> *mut u8 {
        crate::ACTIVITY_THREAD.with(|t| t.as_ptr() as *mut u8)
    }
}
#[cfg(target_arch = "x86_64")]
pub mod arch_impl {
    pub trait PerCpuOps {
        fn current_thread_ptr() -> *mut u8;
    }
    pub mod x86_64 {
        pub mod percpu {
            pub struct X86PerCpu;
            impl crate::arch_impl::PerCpuOps for X86PerCpu {
                fn current_thread_ptr() -> *mut u8 {
                    crate::per_cpu_aarch64::current_thread_ptr()
                }
            }
        }
    }
}
thread_local! {
    static ACTIVITY_TIME:std::cell::Cell<i64>=const{std::cell::Cell::new(100)};
    static ACTIVITY_THREAD:std::cell::RefCell<task::thread::Thread>=const{std::cell::RefCell::new(task::thread::Thread{cpu_account:Some(task::thread::CpuAccount{process_id:77})})};
}
pub mod task {
    pub mod thread {
        pub struct CpuAccount {
            pub process_id: u64,
        }
        pub struct Thread {
            pub cpu_account: Option<CpuAccount>,
        }
    }
    pub mod scheduler {
        pub const MAX_CPUS: usize = 4;
        pub fn set_need_resched() {}
        pub fn current_cpu() -> usize {
            0
        }
        pub fn yield_current() {
            std::thread::yield_now();
        }
    }
}
pub mod syscall {
    pub mod nr {
        pub const FUTEX: u64 = 98;
        pub const GETPID: u64 = 172;
        pub const YIELD: u64 = 124;
    }
    pub mod errno {
        pub const EAGAIN: i32 = 11;
        pub const EIO: i32 = 5;
        pub const EOVERFLOW: i32 = 75;
        pub const EMLINK: i32 = 31;
        pub const EXDEV: i32 = 18;
        pub const EBUSY: i32 = 16;
        pub const EBADF: i32 = 9;
        pub const EPERM: i32 = 1;
        pub const EINVAL: i32 = 22;
        pub const ENOMEM: i32 = 12;
        pub const EFBIG: i32 = 27;
        pub const ESRCH: i32 = 3;
        pub const EMFILE: i32 = 24;
        pub const EEXIST: i32 = 17;
        pub const EACCES: i32 = 13;
        pub const ENOENT: i32 = 2;
        pub const EISDIR: i32 = 21;
        pub const ENOTDIR: i32 = 20;
        pub const EFAULT: i32 = 14;
        pub const ENAMETOOLONG: i32 = 36;
        pub const EOPNOTSUPP: i32 = 95;
        pub const ENODEV: i32 = 19;
    }
    #[derive(Debug, PartialEq, Eq)]
    pub enum SyscallResult {
        Ok(u64),
        Err(u64),
    }
    pub mod resource {
        pub fn current_fsize() -> u64 {
            crate::FSIZE_LIMIT.with(|f| f.get())
        }
        pub fn signal_fsize() {
            crate::FSIZE_SIGNAL.with(|f| f.set(true));
        }
    }
    pub mod userptr {
        pub fn copy_to_user<T: Copy>(ptr: *mut T, value: &T) -> Result<(), u64> {
            if ptr.is_null() {
                return Err(14);
            }
            unsafe {
                ptr.write(*value);
            }
            Ok(())
        }
        pub fn write_user_bytes(ptr: u64, bytes: *const u8, len: usize) -> Result<(), u64> {
            if ptr == 0 {
                return Err(14);
            }
            unsafe {
                std::ptr::copy_nonoverlapping(bytes, ptr as *mut u8, len);
            }
            Ok(())
        }
    }
    pub mod fs {
        #[cfg(target_arch = "x86_64")]
        pub const O_DIRECTORY: u32 = 0x10000;
        #[cfg(target_arch = "aarch64")]
        pub const O_DIRECTORY: u32 = 0x4000;
        #[derive(Clone, Copy, Default)]
        pub struct Stat {
            pub st_dev: u64,
            pub st_ino: u64,
            pub st_mode: u32,
            pub st_nlink: u64,
            pub st_uid: u32,
            pub st_gid: u32,
            pub st_size: i64,
            pub st_blksize: i64,
            pub st_blocks: i64,
        }
        impl Stat {
            pub fn zeroed() -> Self {
                Self::default()
            }
        }
    }
    pub mod mmap {
        pub fn sys_munmap(_a: u64, _l: u64) -> super::SyscallResult {
            super::SyscallResult::Ok(0)
        }
        pub fn clear_for_mapping(
            p: &mut crate::process::Process,
            start: u64,
            end: u64,
            _prot: crate::memory::vma::Protection,
            flags: crate::memory::vma::MmapFlags,
        ) -> Result<(), u64> {
            use crate::memory::vma::MmapFlags;
            let overlaps = p
                .vmas
                .iter()
                .any(|v| v.start.as_u64() < end && start < v.end.as_u64());
            if overlaps && flags.contains(MmapFlags::FIXED_NOREPLACE) {
                return Err(17);
            }
            if overlaps && !flags.contains(MmapFlags::FIXED) {
                return Err(12);
            }
            if overlaps {
                p.vmas
                    .retain(|v| v.end.as_u64() <= start || v.start.as_u64() >= end);
            }
            p.vmas.try_reserve(3).map_err(|_| 12u64)?;
            Ok(())
        }
    }
    pub mod memory_advice {
        pub fn sys_mlock(addr: u64, len: u64) -> super::SyscallResult {
            match populate(crate::CURRENT.with(|c| c.get()), addr, addr + len) {
                Ok(()) => super::SyscallResult::Ok(0),
                Err(e) => super::SyscallResult::Err(e),
            }
        }
        pub fn record_future(_p: &mut crate::process::Process, _start: u64, _end: u64) {}
        pub fn populate(thread: u64, start: u64, end: u64) -> Result<(), u64> {
            for a in (start..end).step_by(4096) {
                crate::object::prepare_population(thread, a)?;
                let mut guard = crate::process::manager();
                let p = guard.as_mut().unwrap().rows.get_mut(&thread).unwrap();
                match crate::object::populate_page(p, a) {
                    Some(Ok(true)) => {}
                    _ => return Err(12),
                }
            }
            Ok(())
        }
    }
    pub mod memory_common {
        pub const PAGE_SIZE: u64 = 4096;
        pub fn get_current_thread_id() -> Option<u64> {
            Some(crate::CURRENT.with(|c| c.get()))
        }
        pub fn is_page_aligned(a: u64) -> bool {
            a & 4095 == 0
        }
        pub fn round_down_to_page(a: u64) -> u64 {
            a & !4095
        }
        pub fn flush_new_mapping(_a: crate::memory::Addr) {}
        pub fn flush_tlb(_a: crate::memory::Addr) {}
        pub fn place_below_hint(p: &mut crate::process::Process, len: u64) -> Option<u64> {
            let mut top = p.mmap_hint;
            loop {
                let start = top.checked_sub(len)? & !4095;
                if start < crate::memory::layout::MMAP_REGION_START {
                    return None;
                }
                if let Some(low) = p
                    .vmas
                    .iter()
                    .filter(|v| v.start.as_u64() < start + len && start < v.end.as_u64())
                    .map(|v| v.start.as_u64())
                    .min()
                {
                    top = low;
                } else {
                    p.mmap_hint = start;
                    return Some(start);
                }
            }
        }
        pub fn allocate_zeroed_frames(count: usize) -> Option<Vec<crate::memory::Frame>> {
            if crate::FAIL_PAGE.with(|f| f.replace(false)) {
                return None;
            }
            Some(
                (0..count)
                    .map(|_| crate::memory::frame_allocator::allocate_frame())
                    .collect(),
            )
        }
    }
}
pub mod signal {
    pub mod types {
        pub struct SigInfo;
        impl SigInfo {
            pub fn kernel() -> Self {
                Self
            }
        }
    }
    pub mod constants {
        pub const SIGBUS: u32 = 7;
        pub const SIGSEGV: u32 = 11;
        pub const SIGKILL: u32 = 9;
    }
}
#[path = "../../../kernel/src/memory/fork_copy.rs"]
pub mod host_fork_copy;
#[path = "../../../kernel/src/memory/vma.rs"]
pub mod host_vma;
pub use memory::file_map;
pub mod memory {
    pub use crate::host_fork_copy as fork_copy;
    #[cfg(target_arch = "aarch64")]
    pub use arch_stub::{Page, PageTableFlags as Flags, Size4KiB, VirtAddr as Addr};
    #[cfg(target_arch = "x86_64")]
    pub use x86_64::{
        structures::paging::{Page, PageTableFlags as Flags, Size4KiB},
        VirtAddr as Addr,
    };
    pub type HostPage = Page<Size4KiB>;
    pub mod process_memory {
        pub use crate::kernel_support::ProcessPageTable;
        pub fn is_cow_page(f: super::Flags) -> bool {
            f.contains(super::Flags::BIT_9)
        }
        pub fn make_cow_flags(mut f: super::Flags) -> super::Flags {
            f.remove(super::Flags::WRITABLE);
            f.insert(super::Flags::BIT_9);
            f
        }
    }
    pub mod anon_map {
        pub fn page_flags(prot: super::vma::Protection) -> super::Flags {
            let mut f = super::Flags::PRESENT | super::Flags::USER_ACCESSIBLE;
            if prot.contains(super::vma::Protection::WRITE) {
                f.insert(super::Flags::WRITABLE);
            }
            f
        }
    }
    pub mod arch_stub {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
        pub struct VirtAddr(u64);
        impl VirtAddr {
            pub fn new(v: u64) -> Self {
                Self(v)
            }
            pub fn as_u64(self) -> u64 {
                self.0
            }
        }
        #[derive(Clone, Copy, Debug)]
        pub struct Page<T> {
            addr: VirtAddr,
            marker: core::marker::PhantomData<T>,
        }
        impl<T: Copy> Page<T> {
            pub fn containing_address(addr: VirtAddr) -> Self {
                Self {
                    addr: VirtAddr::new(addr.as_u64() & !4095),
                    marker: core::marker::PhantomData,
                }
            }
            pub fn start_address(self) -> VirtAddr {
                self.addr
            }
        }
        #[derive(Clone, Copy, Debug)]
        pub struct PageTableFlags(u64);
        impl PageTableFlags {
            pub const PRESENT: Self = Self(1);
            pub const USER_ACCESSIBLE: Self = Self(2);
            pub const WRITABLE: Self = Self(4);
            pub const BIT_9: Self = Self(512);
            pub fn contains(self, o: Self) -> bool {
                self.0 & o.0 == o.0
            }
            pub fn insert(&mut self, o: Self) {
                self.0 |= o.0;
            }
            pub fn remove(&mut self, o: Self) {
                self.0 &= !o.0;
            }
        }
        impl core::ops::BitOr for PageTableFlags {
            type Output = Self;
            fn bitor(self, o: Self) -> Self {
                Self(self.0 | o.0)
            }
        }
        #[derive(Debug, Clone, Copy)]
        pub struct Size4KiB;
        #[derive(Debug, Clone, Copy)]
        pub struct PhysFrame<T = Size4KiB>(pub u64, pub core::marker::PhantomData<T>);
        impl<T> PhysFrame<T> {
            pub fn start_address(self) -> VirtAddr {
                VirtAddr::new(self.0)
            }
        }
    }
    #[cfg(target_arch = "aarch64")]
    pub type Frame = arch_stub::PhysFrame;
    #[cfg(target_arch = "x86_64")]
    pub type Frame = x86_64::structures::paging::PhysFrame;
    pub mod layout {
        pub const MMAP_REGION_START: u64 = 0x100000;
        pub const MMAP_REGION_END: u64 = 0x10000000;
    }
    pub mod file_map {
        #[derive(Clone, Copy)]
        pub enum Access {
            Read,
            Write,
            Execute,
        }
        #[derive(Debug, PartialEq, Eq)]
        pub enum FaultOutcome {
            NotFile,
            Resolved,
            Signal(u32),
        }
        #[derive(Debug)]
        pub struct Binding;
    }
    pub use crate::host_vma as vma;
    pub fn physical_memory_offset() -> arch_stub::VirtAddr {
        arch_stub::VirtAddr::new(0)
    }
    pub mod frame_allocator {
        use super::Frame;
        use std::collections::BTreeMap;
        use std::sync::{LazyLock, Mutex};
        #[repr(align(4096))]
        struct FrameBytes([u8; 4096]);
        static FRAMES: LazyLock<Mutex<BTreeMap<u64, (Box<FrameBytes>, u32)>>> =
            LazyLock::new(|| Mutex::new(BTreeMap::new()));
        pub fn allocate_frame() -> Frame {
            let mut bytes = Box::new(FrameBytes([0; 4096]));
            let key = bytes.0.as_mut_ptr() as u64;
            FRAMES.lock().unwrap().insert(key, (bytes, 0));
            #[cfg(target_arch = "aarch64")]
            {
                super::arch_stub::PhysFrame(key, core::marker::PhantomData)
            }
            #[cfg(target_arch = "x86_64")]
            {
                Frame::containing_address(x86_64::PhysAddr::new(key))
            }
        }
        pub fn register(frame: Frame) {
            FRAMES
                .lock()
                .unwrap()
                .get_mut(&frame.start_address().as_u64())
                .unwrap()
                .1 += 1;
        }
        pub fn decref(frame: Frame) -> bool {
            let mut frames = FRAMES.lock().unwrap();
            let refs = &mut frames.get_mut(&frame.start_address().as_u64()).unwrap().1;
            assert!(*refs > 0);
            *refs -= 1;
            *refs == 0
        }
        pub fn refs(frame: Frame) -> u32 {
            FRAMES
                .lock()
                .unwrap()
                .get(&frame.start_address().as_u64())
                .map_or(0, |f| f.1)
        }
        pub fn deallocate_frame(frame: Frame) {
            let entry = FRAMES
                .lock()
                .unwrap()
                .remove(&frame.start_address().as_u64())
                .unwrap();
            assert_eq!(entry.1, 0);
        }
    }
    pub mod frame_metadata {
        use super::{frame_allocator, Frame};
        pub fn frame_register(f: Frame) {
            frame_allocator::register(f);
        }
        pub fn frame_incref(f: Frame) {
            frame_allocator::register(f);
        }
        pub fn frame_is_shared(f: Frame) -> bool {
            frame_allocator::refs(f) > 1
        }
        pub fn frame_decref(f: Frame) -> bool {
            frame_allocator::decref(f)
        }
    }
}
mod kernel_support;
pub use kernel_support::process;
#[path = "../../../kernel/src/ipc/object.rs"]
pub mod ipc_object;
#[path = "../../../kernel/src/ipc/shmem.rs"]
pub mod shm_kernel;
pub mod ipc {
    pub use crate::ipc_object as object;
    pub use crate::kernel_support::fd;
    pub use crate::shm_kernel as shmem;
}
pub use ipc::shmem as object;
thread_local! { static CURRENT: std::cell::Cell<u64> = const { std::cell::Cell::new(1) }; }
static KERNEL_TEST: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn sizing_zeroes_growth_preserves_prefix_and_clears_shrunken_tail() {
    let obj = object::ShmObject::new(5, 0o600, 1, 2).unwrap();
    obj.ensure_page(0).unwrap();
    let f = obj.frame_at(0).unwrap();
    let p = f.start_address().as_u64() as *mut u8;
    unsafe {
        assert_eq!(*p, 0);
        *p = 17;
        *p.add(4) = 42;
    }
    obj.resize(3).unwrap();
    obj.resize(8192).unwrap();
    unsafe {
        assert_eq!(*p, 17);
        assert_eq!(*p.add(4), 0);
    }
    obj.ensure_page(4096).unwrap();
    let second = obj.frame_at(4096).unwrap();
    unsafe {
        assert_eq!(*(second.start_address().as_u64() as *const u64), 0);
    }
    obj.resize(0).unwrap();
    assert!(obj.frame_at(0).is_none());
    assert_eq!(memory::frame_allocator::refs(f), 1);
    object::release_frame(f);
    object::release_frame(second);
}
#[test]
fn pte_references_survive_the_last_object_reference() {
    let obj = object::ShmObject::new(4096, 0o600, 0, 0).unwrap();
    obj.ensure_page(0).unwrap();
    let f = obj.frame_at(0).unwrap();
    assert_eq!(memory::frame_allocator::refs(f), 2);
    drop(obj);
    assert_eq!(memory::frame_allocator::refs(f), 1);
    object::release_frame(f);
    assert_eq!(memory::frame_allocator::refs(f), 0);
}
#[test]
fn vma_split_preserves_object_offset_and_counts_live_tokens() {
    #[cfg(target_arch = "aarch64")]
    use memory::arch_stub::VirtAddr;
    use memory::vma::{MmapFlags, Protection, Vma};
    #[cfg(target_arch = "x86_64")]
    use x86_64::VirtAddr;
    let obj = object::ShmObject::new(8192, 0o600, 0, 0).unwrap();
    let mut v = Vma::new(
        VirtAddr::new(0x100000),
        VirtAddr::new(0x102000),
        Protection::READ,
        MmapFlags::SHARED,
    );
    v.shm = Some(object::ShmMapping::new(&obj));
    v.shm_offset = 4096;
    let piece = v.piece(VirtAddr::new(0x101000), v.end);
    assert_eq!(piece.shm_offset, 8192);
    assert_eq!(obj.attachments(), 2);
    let fork = piece.shm.as_ref().unwrap().fork(77);
    assert_eq!(obj.attachments(), 3);
    drop(v);
    drop(piece);
    assert_eq!(obj.attachments(), 1);
    drop(fork);
    assert_eq!(obj.attachments(), 0);
}
#[test]
fn eof_rounds_to_pages_and_invalid_resize_leaves_size_unchanged() {
    assert_eq!(object::page_count(0), Ok(0));
    assert_eq!(object::page_count(4097), Ok(2));
    assert_eq!(object::page_count(u64::MAX), Err(22));
    let obj = object::ShmObject::new(1, 0o600, 0, 0).unwrap();
    assert!(!obj.resizing());
    obj.ensure_page(4095).unwrap();
    let f = obj.frame_at(4095).unwrap();
    object::release_frame(f);
    assert!(obj.frame_at(4096).is_none());
    assert_eq!(obj.resize(u64::MAX), Err(22));
    assert_eq!(obj.size(), 1);
}

// Futex operations are emulated on the host; the semaphore algorithm below is
// the production source, including its error and waiter bookkeeping paths.
extern crate self as libbreenix;
pub mod raw {
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Condvar, LazyLock, Mutex};
    type Queue = Arc<(Mutex<()>, Condvar)>;
    static QUEUES: LazyLock<Mutex<BTreeMap<u64, Queue>>> =
        LazyLock::new(|| Mutex::new(BTreeMap::new()));
    thread_local! { static INTERRUPT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
    thread_local! { static WAKE_CALLS: std::cell::RefCell<BTreeMap<u64, usize>> = const { std::cell::RefCell::new(BTreeMap::new()) }; }
    pub fn wake_calls(addr: u64) -> usize {
        WAKE_CALLS.with(|c| *c.borrow().get(&addr).unwrap_or(&0))
    }
    pub fn interrupt_next() {
        INTERRUPT.with(|i| i.set(true));
    }
    pub unsafe fn syscall0(nr: u64) -> u64 {
        match nr {
            crate::syscall::nr::GETPID => std::process::id() as u64,
            crate::syscall::nr::YIELD => {
                std::thread::yield_now();
                0
            }
            _ => panic!("unexpected syscall"),
        }
    }
    pub unsafe fn syscall6(
        _nr: u64,
        addr: u64,
        op: u64,
        value: u64,
        timeout: u64,
        _other: u64,
        mask: u64,
    ) -> u64 {
        assert_eq!(mask, u32::MAX as u64);
        let key = if op & 128 == 0 {
            crate::host_fs::shared_key(addr).unwrap_or(addr)
        } else {
            addr
        };
        let queue = QUEUES
            .lock()
            .unwrap()
            .entry(key)
            .or_insert_with(|| Arc::new((Mutex::new(()), Condvar::new())))
            .clone();
        let guard = queue.0.lock().unwrap();
        match op & 127 {
            1 => {
                WAKE_CALLS.with(|c| *c.borrow_mut().entry(addr).or_default() += 1);
                if value == i32::MAX as u64 {
                    queue.1.notify_all();
                } else {
                    queue.1.notify_one();
                }
                0
            }
            0 | 9 => {
                if INTERRUPT.with(|i| i.replace(false)) {
                    return (-4i64) as u64;
                }
                if (&*(addr as *const AtomicU32)).load(Ordering::SeqCst) != value as u32 {
                    return (-11i64) as u64;
                }
                if timeout != 0 {
                    let ts = timeout as *const i64;
                    let mut at = i128::from(*ts) * 1_000_000_000 + i128::from(*ts.add(1));
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos() as i128;
                    if op & 127 == 0 {
                        at += now;
                    }
                    if at <= now {
                        return (-110i64) as u64;
                    }
                    let (guard, result) = queue
                        .1
                        .wait_timeout(guard, std::time::Duration::from_nanos((at - now) as u64))
                        .unwrap();
                    drop(guard);
                    if result.timed_out() {
                        (-110i64) as u64
                    } else {
                        0
                    }
                } else {
                    drop(queue.1.wait(guard).unwrap());
                    0
                }
            }
            _ => panic!("unexpected futex operation"),
        }
    }
}
thread_local! { static ERRNO: std::cell::Cell<i32> = const { std::cell::Cell::new(0) }; }
fn syscall_result_to_c_int(result: i64) -> i32 {
    if result < 0 {
        ERRNO.with(|e| e.set(-result as i32));
        -1
    } else {
        result as i32
    }
}
// The semaphore waits are cancellation points. With no thread runtime, as
// here, the library's cancel_syscall makes the system call directly.
mod pthread {
    pub unsafe fn cancel_syscall(nr: u64, a: [u64; 6]) -> i64 {
        crate::raw::syscall6(nr, a[0], a[1], a[2], a[3], a[4], a[5]) as i64
    }
}
#[path = "../../../libs/libbreenix-libc/src/semaphore.rs"]
mod semaphore;

#[test]
fn semaphore_limits_trywait_and_destroy() {
    use semaphore::*;
    let mut s = core::mem::MaybeUninit::<Sem>::uninit();
    unsafe {
        assert_eq!(sem_init(s.as_mut_ptr(), 0, SEM_VALUE_MAX + 1), -1);
        assert_eq!(ERRNO.with(|e| e.get()), 22);
        assert_eq!(sem_init(s.as_mut_ptr(), 0, SEM_VALUE_MAX), 0);
        assert_eq!(core::mem::size_of::<Sem>(), 32);
        assert_eq!(sem_post(s.as_mut_ptr()), -1);
        assert_eq!(ERRNO.with(|e| e.get()), 75);
        assert_eq!(sem_trywait(s.as_mut_ptr()), 0);
        let mut v = 0;
        assert_eq!(sem_getvalue(s.as_mut_ptr(), &mut v), 0);
        assert_eq!(v, SEM_VALUE_MAX as i32 - 1);
        assert_eq!(sem_destroy(s.as_mut_ptr()), 0);
        assert_eq!(sem_init(s.as_mut_ptr(), 0, 0), 0);
        assert_eq!(sem_trywait(s.as_mut_ptr()), -1);
        assert_eq!(ERRNO.with(|e| e.get()), 11);
    }
}
#[test]
fn semaphore_ready_ignores_invalid_deadline_and_failure_preserves_value() {
    use semaphore::*;
    let mut s = core::mem::MaybeUninit::<Sem>::uninit();
    unsafe {
        assert_eq!(sem_init(s.as_mut_ptr(), 0, 1), 0);
        let bad = [-1, 1_000_000_000];
        assert_eq!(sem_timedwait(s.as_mut_ptr(), bad.as_ptr()), 0);
        assert_eq!(sem_timedwait(s.as_mut_ptr(), bad.as_ptr()), -1);
        assert_eq!(ERRNO.with(|e| e.get()), 22);
        let past = [-1, 0];
        assert_eq!(sem_timedwait(s.as_mut_ptr(), past.as_ptr()), -1);
        assert_eq!(ERRNO.with(|e| e.get()), 110);
        let mut v = -1;
        sem_getvalue(s.as_mut_ptr(), &mut v);
        assert_eq!(v, 0);
        raw::interrupt_next();
        assert_eq!(sem_wait(s.as_mut_ptr()), -1);
        assert_eq!(ERRNO.with(|e| e.get()), 4);
        assert_eq!(sem_post(s.as_mut_ptr()), 0);
        assert_eq!(sem_trywait(s.as_mut_ptr()), 0);
    }
}
#[test]
fn semaphore_contended_posts_are_never_lost() {
    use semaphore::*;
    for shared in [0, 1] {
        let mut s = Box::new(core::mem::MaybeUninit::<Sem>::uninit());
        let addr = s.as_mut_ptr() as usize;
        unsafe {
            assert_eq!(sem_init(addr as *mut Sem, shared, 0), 0);
        }
        std::thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(move || {
                    for _ in 0..1000 {
                        unsafe {
                            assert_eq!(sem_wait(addr as *mut Sem), 0);
                        }
                    }
                });
            }
            for _ in 0..4000 {
                unsafe {
                    assert_eq!(sem_post(addr as *mut Sem), 0);
                }
            }
        });
        unsafe {
            let mut v = -1;
            sem_getvalue(addr as *mut Sem, &mut v);
            assert_eq!(v, 0);
        }
    }
}

#[path = "../../../libs/libbreenix-libc/src/shm.rs"]
mod shm;
fn __errno_location() -> *mut i32 {
    ERRNO.with(|e| e.as_ptr())
}
mod host_fs {
    use std::{
        collections::BTreeMap,
        fs::File,
        os::fd::AsRawFd,
        sync::{Arc, LazyLock, Mutex},
    };
    struct Object {
        ino: u64,
        file: File,
    }
    struct Mapping {
        len: usize,
        ino: u64,
        offset: u64,
    }
    struct Fs {
        names: BTreeMap<String, Arc<Object>>,
        fds: BTreeMap<i32, Arc<Object>>,
        maps: BTreeMap<usize, Mapping>,
        next_fd: i32,
        next_ino: u64,
    }
    static FS: LazyLock<Mutex<Fs>> = LazyLock::new(|| {
        Mutex::new(Fs {
            names: BTreeMap::new(),
            fds: BTreeMap::new(),
            maps: BTreeMap::new(),
            next_fd: 10,
            next_ino: 1,
        })
    });
    unsafe extern "C" {
        #[link_name = "mmap"]
        fn native_mmap(
            addr: *mut u8,
            len: usize,
            prot: i32,
            flags: i32,
            fd: i32,
            offset: i64,
        ) -> *mut u8;
        #[link_name = "munmap"]
        fn native_munmap(addr: *mut u8, len: usize) -> i32;
    }
    pub fn shared_key(addr: u64) -> Option<u64> {
        FS.lock().unwrap().maps.iter().find_map(|(start, map)| {
            let offset = addr.checked_sub(*start as u64)?;
            (offset < map.len as u64).then_some((map.ino << 32) + map.offset + offset)
        })
    }
    unsafe fn text(p: *const u8) -> String {
        let mut len = 0;
        while *p.add(len) != 0 {
            len += 1;
        }
        String::from_utf8(std::slice::from_raw_parts(p, len).to_vec()).unwrap()
    }
    pub unsafe fn open(path: *const u8, flags: i32, _mode: u32) -> i32 {
        let name = text(path);
        let temporary = flags & 0x400000 != 0;
        let mut fs = FS.lock().unwrap();
        if flags & 0x40 == 0 && crate::UNLINK_OPEN_RACE.with(|c| c.replace(false)) {
            fs.names.remove(&name);
            return super::syscall_result_to_c_int(-2);
        }
        let obj = if let Some(obj) = fs.names.get(&name) {
            if flags & 0xc0 == 0xc0 {
                return super::syscall_result_to_c_int(-17);
            }
            obj.clone()
        } else {
            if flags & 0x40 == 0 && !temporary {
                return super::syscall_result_to_c_int(-2);
            }
            let ino = fs.next_ino;
            fs.next_ino += 1;
            let path =
                std::env::temp_dir().join(format!("breenix-ipc-host-{}-{ino}", std::process::id()));
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&path)
                .unwrap();
            std::fs::remove_file(path).unwrap();
            let obj = Arc::new(Object { ino, file });
            if !temporary {
                fs.names.insert(name, obj.clone());
            }
            obj
        };
        let fd = fs.next_fd;
        fs.next_fd += 1;
        fs.fds.insert(fd, obj);
        fd
    }
    pub unsafe fn linkat(
        fd: i32,
        old: *const u8,
        _newdir: i32,
        path: *const u8,
        flags: i32,
    ) -> i32 {
        assert_eq!(flags, 0x1000);
        assert_eq!(*old, 0);
        let mut fs = FS.lock().unwrap();
        let name = text(path);
        if fs.names.contains_key(&name) {
            return super::syscall_result_to_c_int(-17);
        }
        let obj = fs.fds.get(&fd).unwrap().clone();
        fs.names.insert(name, obj);
        0
    }
    pub unsafe fn unlink(path: *const u8) -> i32 {
        if FS.lock().unwrap().names.remove(&text(path)).is_some() {
            0
        } else {
            super::syscall_result_to_c_int(-2)
        }
    }
    pub unsafe fn ftruncate(fd: i32, len: i64) -> i32 {
        if crate::SLOW_CREATOR.with(|f| f.get()) {
            crate::SLOW_CREATOR_STARTED.store(true, std::sync::atomic::Ordering::Release);
            std::thread::sleep(std::time::Duration::from_millis(1200));
        }
        let fs = FS.lock().unwrap();
        let obj = fs.fds.get(&fd).unwrap();
        obj.file.set_len(len as u64).unwrap();
        0
    }
    pub unsafe fn fstat(fd: i32, out: *mut u8) -> i32 {
        let fs = FS.lock().unwrap();
        let obj = fs.fds.get(&fd).unwrap();
        let mut stat = [0u64; 18];
        stat[1] = obj.ino;
        stat[6] = obj.file.metadata().unwrap().len();
        std::ptr::copy_nonoverlapping(stat.as_ptr() as *const u8, out, 144);
        0
    }
    pub unsafe fn mmap(
        addr: *mut u8,
        len: usize,
        prot: i32,
        flags: i32,
        fd: i32,
        offset: i64,
    ) -> *mut u8 {
        let mut fs = FS.lock().unwrap();
        let obj = fs.fds.get(&fd).unwrap().clone();
        let p = native_mmap(addr, len, prot, flags, obj.file.as_raw_fd(), offset);
        assert_ne!(p as usize, usize::MAX);
        fs.maps.insert(
            p as usize,
            Mapping {
                len,
                ino: obj.ino,
                offset: offset as u64,
            },
        );
        p
    }
    pub unsafe fn munmap(addr: *mut u8, len: usize) -> i32 {
        let mut fs = FS.lock().unwrap();
        assert!(fs.maps.remove(&(addr as usize)).is_some());
        native_munmap(addr, len)
    }
    pub fn close(fd: i32) -> i32 {
        FS.lock().unwrap().fds.remove(&fd).unwrap();
        0
    }
}
use host_fs::{close, fstat, ftruncate, linkat, mmap, munmap, open, unlink};
unsafe fn realloc(p: *mut u8, size: usize) -> *mut u8 {
    use std::alloc::{alloc, realloc, Layout};
    use std::{
        collections::BTreeMap,
        sync::{LazyLock, Mutex},
    };
    static ALLOCS: LazyLock<Mutex<BTreeMap<usize, usize>>> =
        LazyLock::new(|| Mutex::new(BTreeMap::new()));
    let mut allocations = ALLOCS.lock().unwrap();
    let new = if p.is_null() {
        alloc(Layout::from_size_align(size, 8).unwrap())
    } else {
        let old = allocations.remove(&(p as usize)).unwrap();
        realloc(p, Layout::from_size_align(old, 8).unwrap(), size)
    };
    if !new.is_null() {
        allocations.insert(new as usize, size);
    }
    new
}

#[test]
fn names_enforce_flat_namespace_and_component_limits() {
    let mut out = [0; 265];
    for bad in [b"bad\0".as_slice(), b"/\0", b"/a/b\0"] {
        assert_eq!(
            unsafe { shm::name_with_prefix(bad.as_ptr(), b"/dev/shm/", &mut out) },
            Err(22)
        );
    }
    let mut name = vec![b'a'; 257];
    name[0] = b'/';
    name[256] = 0;
    unsafe {
        assert_eq!(
            shm::name_with_prefix(name.as_ptr(), b"/dev/shm/", &mut out),
            Ok(())
        );
    }
    name.insert(256, b'a');
    unsafe {
        assert_eq!(
            shm::name_with_prefix(name.as_ptr(), b"/dev/shm/", &mut out),
            Err(36)
        );
    }
}
#[test]
fn named_handles_share_storage_retain_counts_and_survive_unlink() {
    use semaphore::*;
    let name = b"/host-named\0";
    unsafe {
        let s = sem_open(name.as_ptr(), 0xc0, 0o600u32, 0u32);
        assert!(!s.is_null());
        assert!(sem_open(name.as_ptr(), 0xc0, 0o600u32, 0u32).is_null());
        assert_eq!(ERRNO.with(|e| e.get()), 17);
        let again = sem_open(name.as_ptr(), 0);
        assert_eq!(again, s);
        assert_eq!(sem_close(s), 0); // one handle still keeps the mapping live
        let address = again as usize;
        std::thread::scope(|scope| {
            let waiter = scope.spawn(move || sem_wait(address as *mut Sem));
            assert_eq!(sem_post(again), 0);
            assert_eq!(waiter.join().unwrap(), 0);
        });
        assert_eq!(sem_unlink(name.as_ptr()), 0);
        assert!(sem_open(name.as_ptr(), 0).is_null());
        assert_eq!(ERRNO.with(|e| e.get()), 2);
        assert_eq!(sem_post(again), 0);
        let fresh = sem_open(name.as_ptr(), 0xc0, 0o600u32, 3u32);
        assert!(!fresh.is_null());
        assert_ne!(fresh, again);
        let mut v = 0;
        sem_getvalue(again, &mut v);
        assert_eq!(v, 1);
        sem_getvalue(fresh, &mut v);
        assert_eq!(v, 3);
        assert_eq!(sem_close(again), 0);
        assert_eq!(sem_close(fresh), 0);
        assert_eq!(sem_unlink(name.as_ptr()), 0);
        assert!(sem_open(name.as_ptr(), 0x40, 0o600u32, SEM_VALUE_MAX + 1).is_null());
        assert_eq!(ERRNO.with(|e| e.get()), 22);
    }
}

#[test]
fn shared_semaphore_waits_through_a_distinct_mapping() {
    use semaphore::*;
    unsafe {
        let name = b"/host-alias\0";
        let fd = shm::shm_open(name.as_ptr(), 0xc2, 0o600);
        assert!(fd >= 0);
        ftruncate(fd, 4096);
        let a = mmap(core::ptr::null_mut(), 4096, 3, 1, fd, 0) as *mut Sem;
        let other = shm::shm_open(name.as_ptr(), 2, 0);
        assert!(other >= 0);
        let b = mmap(core::ptr::null_mut(), 4096, 3, 1, other, 0) as *mut Sem;
        assert_ne!(a, b);
        assert_eq!(sem_init(a, 1, 0), 0);
        let address = b as usize;
        std::thread::scope(|scope| {
            let waiter = scope.spawn(move || {
                let at = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    + std::time::Duration::from_secs(2);
                let deadline = [at.as_secs() as i64, at.subsec_nanos() as i64];
                sem_timedwait(address as *mut Sem, deadline.as_ptr())
            });
            let start = std::time::Instant::now();
            let waiters = &*((a as *const u8).add(4) as *const std::sync::atomic::AtomicU32);
            while waiters.load(std::sync::atomic::Ordering::SeqCst) == 0
                && start.elapsed().as_secs() < 1
            {
                std::thread::yield_now();
            }
            assert_ne!(waiters.load(std::sync::atomic::Ordering::SeqCst), 0);
            assert_eq!(sem_post(a), 0);
            assert_eq!(waiter.join().unwrap(), 0);
        });
        let mut value = -1;
        sem_getvalue(a, &mut value);
        assert_eq!(value, 0);
        munmap(a as *mut u8, 4096);
        munmap(b as *mut u8, 4096);
        close(fd);
        close(other);
        assert_eq!(shm::shm_unlink(name.as_ptr()), 0);
    }
}

thread_local! { static FAIL_PAGE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
#[test]
fn large_sparse_resize_and_failed_first_touch_are_fallible() {
    let object = object::ShmObject::new(0, 0o600, 1, 2).unwrap();
    object.resize(1 << 30).unwrap();
    assert_eq!(object.size(), 1 << 30);
    assert!(object.frame_at(0).is_none());
    assert!(object.frame_at((1 << 30) - 1).is_none());
    FAIL_PAGE.with(|f| f.set(true));
    assert_eq!(object.ensure_page(0), Err(12));
    assert_eq!(object.size(), 1 << 30);
    assert!(object.frame_at(0).is_none());
    object.ensure_page(0).unwrap();
    let frame = object.frame_at(0).unwrap();
    object::release_frame(frame);
}
#[test]
fn growing_zeroes_bytes_written_through_a_mapping_beyond_eof() {
    let obj = object::ShmObject::new(5, 0o600, 1, 2).unwrap();
    obj.ensure_page(0).unwrap();
    let frame = obj.frame_at(0).unwrap();
    let p = frame.start_address().as_u64() as *mut u8;
    unsafe {
        *p.add(3) = 12;
        *p.add(5) = 99;
        *p.add(4000) = 42;
    }
    obj.resize(4096).unwrap();
    unsafe {
        assert_eq!(*p.add(3), 12);
        assert_eq!(*p.add(5), 0);
        assert_eq!(*p.add(4000), 0);
    }
    object::release_frame(frame);
}
#[test]
fn mapping_origin_offset_and_maximum_permission_survive_splits() {
    use object::{MappingOrigin, ShmMapping};
    let obj = object::ShmObject::new(8192, 0o644, 7, 8).unwrap();
    let ro = ShmMapping::with_origin(&obj, MappingOrigin::Mmap, false);
    assert!(!ro.may_write());
    assert!(!ro.clone().may_write());
    assert!(!ro.matches_attachment(0x10000, 0, 0x10000));
    let sysv = ShmMapping::with_origin(&obj, MappingOrigin::SysV, false);
    assert!(sysv.matches_attachment(0x11000, 4096, 0x10000));
    assert!(!sysv.matches_attachment(0x11000, 4096, 0x11000));
    assert!(!sysv.clone().may_write());
}
#[test]
fn mutable_metadata_checks_owner_groups_and_unlinked_link_count() {
    use fs::permissions::Credentials;
    let obj = object::ShmObject::new(0, 0o7644, 7, 8).unwrap();
    let stranger = Credentials {
        euid: 9,
        egid: 8,
        groups: vec![],
        umask: 0,
    };
    let owner = Credentials {
        euid: 7,
        egid: 8,
        groups: vec![10],
        umask: 0,
    };
    let root = Credentials {
        euid: 0,
        egid: 0,
        groups: vec![],
        umask: 0,
    };
    assert_eq!(obj.chmod(0o600, &stranger), Err(1));
    assert_eq!(obj.chown(9, u32::MAX, &owner), Err(1));
    obj.chmod(0o664, &owner).unwrap();
    obj.chown(u32::MAX, 10, &owner).unwrap();
    assert_eq!(obj.metadata(), (0o664, 7, 10, true));
    obj.chmod(0o7777, &root).unwrap();
    obj.chown(11, 12, &root).unwrap();
    assert_eq!(obj.metadata(), (0o1777, 11, 12, true));
    obj.unlink();
    assert!(!obj.metadata().3);
}
#[test]
fn abandoned_named_semaphore_initialization_returns_without_spinning() {
    let name = b"/sem.abandoned-empty\0";
    let fd = unsafe { shm::shm_open(name.as_ptr(), 0xc2, 0o600) };
    assert!(fd >= 0);
    let start = std::time::Instant::now();
    assert!(unsafe { semaphore::sem_open(b"/abandoned-empty\0".as_ptr(), 0) }.is_null());
    assert_eq!(ERRNO.with(|e| e.get()), 22);
    assert!(start.elapsed() < std::time::Duration::from_secs(3));
    unsafe {
        shm::shm_unlink(name.as_ptr());
    }
    close(fd);
    let name = b"/sem.abandoned-ready\0";
    let fd = unsafe { shm::shm_open(name.as_ptr(), 0xc2, 0o600) };
    unsafe {
        ftruncate(fd, 32);
    }
    let start = std::time::Instant::now();
    assert!(unsafe { semaphore::sem_open(b"/abandoned-ready\0".as_ptr(), 0) }.is_null());
    assert_eq!(ERRNO.with(|e| e.get()), 22);
    assert!(start.elapsed() < std::time::Duration::from_secs(3));
    unsafe {
        shm::shm_unlink(name.as_ptr());
    }
    close(fd);
}

fn kernel_fixture() -> std::sync::MutexGuard<'static, ()> {
    let lock = KERNEL_TEST.lock().unwrap();
    let mut manager = process::ProcessManager::default();
    for id in [1, 2] {
        manager.rows.insert(id, process::Process::new(id));
    }
    *process::manager() = Some(manager);
    CURRENT.with(|c| c.set(1));
    fs::permissions::CRED.with(|c| {
        *c.borrow_mut() = fs::permissions::Credentials {
            euid: 0,
            egid: 0,
            groups: vec![],
            umask: 0,
        }
    });
    lock
}
fn fd_number(r: Option<syscall::SyscallResult>) -> i32 {
    match r.unwrap() {
        syscall::SyscallResult::Ok(fd) => fd as i32,
        other => panic!("{other:?}"),
    }
}
fn ipc_file(fd: i32) -> std::sync::Arc<dyn ipc::object::IpcFile> {
    let g = process::manager();
    let p = g
        .as_ref()
        .unwrap()
        .rows
        .get(&CURRENT.with(|c| c.get()))
        .unwrap();
    let ipc::fd::FdKind::Ipc(file) = &p.fd_table.get(fd).unwrap().kind else {
        panic!("ipc fd expected")
    };
    file.clone()
}
#[test]
fn production_shrink_invalidates_both_processes_and_regrowth_uses_new_pages() {
    use memory::{
        file_map::{Access, FaultOutcome},
        vma::Protection,
        Addr,
    };
    let _lock = kernel_fixture();
    let obj = object::ShmObject::new(8192, 0o600, 0, 0).unwrap();
    let a = object::map_object(&obj, 0, 8192, Protection::from_bits(3).unwrap(), false).unwrap();
    CURRENT.with(|c| c.set(2));
    let b = object::map_object(&obj, 0, 8192, Protection::from_bits(3).unwrap(), false).unwrap();
    for (tid, base) in [(1, a), (2, b)] {
        object::prepare_population(tid, base + 4096).unwrap();
        let mut g = process::manager();
        let p = g.as_mut().unwrap().rows.get_mut(&tid).unwrap();
        assert_eq!(object::populate_page(p, base + 4096), Some(Ok(true)));
    }
    let old = obj.frame_at(4096).unwrap();
    unsafe {
        *(old.start_address().as_u64() as *mut u8) = 91;
    }
    obj.resize(4096).unwrap();
    for (tid, base) in [(1, a), (2, b)] {
        let mut g = process::manager();
        let p = g.as_mut().unwrap().rows.get_mut(&tid).unwrap();
        let table = p.page_table.as_deref_mut().unwrap();
        assert!(table.translate(Addr::new(base + 4096)).is_none());
        assert_eq!(
            object::resolve_page(table, &p.vmas, base + 4096, Access::Read),
            FaultOutcome::Signal(7)
        );
    }
    obj.resize(8192).unwrap();
    for (tid, base) in [(1, a), (2, b)] {
        object::prepare_population(tid, base + 4096).unwrap();
        let mut g = process::manager();
        let p = g.as_mut().unwrap().rows.get_mut(&tid).unwrap();
        assert_eq!(object::populate_page(p, base + 4096), Some(Ok(true)));
    }
    let new = obj.frame_at(4096).unwrap();
    assert_ne!(old.start_address(), new.start_address());
    unsafe {
        assert_eq!(*(new.start_address().as_u64() as *const u8), 0);
    }
    object::release_frame(old);
    object::release_frame(new);
}
#[test]
fn production_private_mapping_copies_and_preserves_the_canonical_page() {
    use memory::{
        file_map::{Access, FaultOutcome},
        vma::{MmapFlags, Protection},
        Addr,
    };
    let _lock = kernel_fixture();
    let fd = fd_number(object::open_path(
        "/dev/shm/private-regression",
        0xc2,
        0o644,
    ));
    ipc_file(fd).truncate(8192).unwrap();
    let readonly = fd_number(object::open_path("/dev/shm/private-regression", 0, 0));
    let private = match object::mmap_fd(
        readonly,
        0,
        8192,
        Protection::from_bits(3).unwrap(),
        MmapFlags::PRIVATE,
        0,
    )
    .unwrap()
    {
        syscall::SyscallResult::Ok(a) => a,
        other => panic!("{other:?}"),
    };
    object::prepare_population(1, private).unwrap();
    let canonical = {
        let g = process::manager();
        let v = &g.as_ref().unwrap().rows[&1].vmas[0];
        v.shm.as_ref().unwrap().object().frame_at(0).unwrap()
    };
    unsafe {
        *(canonical.start_address().as_u64() as *mut u8) = 7;
    }
    {
        let mut g = process::manager();
        let p = g.as_mut().unwrap().rows.get_mut(&1).unwrap();
        let table = p.page_table.as_deref_mut().unwrap();
        assert_eq!(
            object::resolve_page(table, &p.vmas, private, Access::Read),
            FaultOutcome::Resolved
        );
        let (frame, flags) = table
            .get_page_info(memory::HostPage::containing_address(Addr::new(private)))
            .unwrap();
        assert_eq!(frame.start_address(), canonical.start_address());
        assert!(memory::process_memory::is_cow_page(flags));
        assert_eq!(
            object::resolve_page(table, &p.vmas, private, Access::Write),
            FaultOutcome::Resolved
        );
        let frame = table.translate(Addr::new(private)).unwrap();
        assert_ne!(frame.start_address(), canonical.start_address());
        unsafe {
            *(frame.start_address().as_u64() as *mut u8) = 99;
            assert_eq!(*(canonical.start_address().as_u64() as *const u8), 7);
        }
    }
    assert!(object::object_at(private).is_none());
    object::release_frame(canonical);
    object::unlink_path("/dev/shm/private-regression");
}
#[test]
fn production_namespace_reports_permissions_modes_links_and_directory_cookies() {
    use syscall::SyscallResult::{Err as Failure, Ok as Success};
    let _lock = kernel_fixture();
    let path = "/dev/shm/namespace-regression";
    let fd = fd_number(object::open_path(path, 0xc2, 0o7644));
    let file = ipc_file(fd);
    let mut st = syscall::fs::Stat::zeroed();
    file.fill_stat(&mut st);
    assert_eq!(st.st_mode, 0o107644);
    assert_eq!(
        object::open_path(path, syscall::fs::O_DIRECTORY, 0),
        Some(Failure(20))
    );
    let ro = fd_number(object::open_path(path, 0, 0));
    let addr = match object::mmap_fd(
        ro,
        0,
        8192,
        memory::vma::Protection::READ,
        memory::vma::MmapFlags::SHARED,
        0,
    )
    .unwrap()
    {
        Success(a) => a,
        r => panic!("{r:?}"),
    };
    {
        let g = process::manager();
        let v = g.as_ref().unwrap().rows[&1]
            .vmas
            .iter()
            .find(|v| v.start.as_u64() == addr)
            .unwrap();
        assert!(!object::mprotect_allowed(v, memory::vma::Protection::WRITE));
    }
    fs::permissions::CRED.with(|c| c.borrow_mut().euid = 100);
    assert_eq!(object::unlink_path(path), Some(Failure(1)));
    fs::permissions::CRED.with(|c| c.borrow_mut().euid = 0);
    let directory = fd_number(object::open_path("/dev/shm", syscall::fs::O_DIRECTORY, 0));
    let dir = ipc_file(directory);
    let mut small = [0u8; 24];
    assert_eq!(dir.getdents(0, small.len()), Err(14)); // copy failure retains cookie
    assert_eq!(dir.getdents(small.as_mut_ptr() as u64, small.len()), Ok(24));
    assert_eq!(u64::from_ne_bytes(small[..8].try_into().unwrap()), 0x1000);
    assert_eq!(&small[19..21], b".\0");
    assert_eq!(dir.getdents(small.as_mut_ptr() as u64, small.len()), Ok(24));
    assert_eq!(&small[19..22], b"..\0");
    let mut bytes = [0u8; 4096];
    let n = dir
        .getdents(bytes.as_mut_ptr() as u64, bytes.len())
        .unwrap() as usize;
    assert!(bytes[..n]
        .windows(b"namespace-regression".len())
        .any(|s| s == b"namespace-regression"));
    assert_eq!(dir.getdents(bytes.as_mut_ptr() as u64, bytes.len()), Ok(0));
    let cred = fs::permissions::Credentials::current(false);
    assert_eq!(object::access_path(path, 4, &cred), Some(Success(0)));
    file.chmod(0o600, &cred).unwrap();
    file.chown(7, 8, &cred).unwrap();
    file.fill_stat(&mut st);
    assert_eq!((st.st_mode, st.st_uid, st.st_gid), (0o100600, 7, 8));
    assert_eq!(object::unlink_path(path), Some(Success(0)));
    file.fill_stat(&mut st);
    assert_eq!(st.st_nlink, 0);
}
#[test]
fn production_split_sysv_attachment_uses_original_address_and_offset() {
    use memory::{vma::Protection, Addr};
    let _lock = kernel_fixture();
    let obj = object::ShmObject::new(8192, 0o600, 0, 0).unwrap();
    let base = object::map_object(&obj, 0, 8192, Protection::READ, false).unwrap();
    {
        let mut g = process::manager();
        let p = g.as_mut().unwrap().rows.get_mut(&1).unwrap();
        let suffix = p.vmas[0].piece(Addr::new(base + 4096), Addr::new(base + 8192));
        p.vmas[0] = suffix;
    }
    assert!(std::sync::Arc::ptr_eq(
        &object::object_at(base).unwrap(),
        &obj
    ));
    assert!(object::object_at(base + 4096).is_none());
}

thread_local! { static UNLINK_OPEN_RACE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
#[test]
fn named_create_retries_when_the_existing_name_is_unlinked_between_opens() {
    let name = b"/create-unlink-race\0";
    unsafe {
        let old = semaphore::sem_open(name.as_ptr(), 0xc0, 0o600u32, 1u32);
        assert!(!old.is_null());
        UNLINK_OPEN_RACE.with(|c| c.set(true));
        let new = semaphore::sem_open(name.as_ptr(), 0x40, 0o600u32, 2u32);
        assert!(!new.is_null());
        assert_ne!(new, old);
        let mut value = 0;
        assert_eq!(semaphore::sem_getvalue(new, &mut value), 0);
        assert_eq!(value, 2);
        assert_eq!(semaphore::sem_getvalue(old, &mut value), 0);
        assert_eq!(value, 1);
        assert_eq!(semaphore::sem_close(old), 0);
        assert_eq!(semaphore::sem_close(new), 0);
        assert_eq!(semaphore::sem_unlink(name.as_ptr()), 0);
    }
}

#[test]
fn unpublished_fork_stays_visible_until_child_publication() {
    let _lock = kernel_fixture();
    let mut claim = memory::fork_copy::CopyClaim::take(0x1000, 0x2000);
    assert!(memory::fork_copy::copying(0x1000));
    assert!(memory::fork_copy::forming(0x2000));
    assert!(memory::fork_copy::any_forming());
    claim.end_copy();
    memory::fork_copy::wait(0x1000);
    assert!(!memory::fork_copy::copying(0x1000));
    assert!(memory::fork_copy::any_forming());
    drop(claim);
    assert!(!memory::fork_copy::any_forming());
}
#[test]
fn shrink_crosses_the_invalidation_chunk_boundary() {
    use memory::{vma::Protection, Addr};
    let _lock = kernel_fixture();
    let obj = object::ShmObject::new(70 * 4096, 0o600, 0, 0).unwrap();
    let base =
        object::map_object(&obj, 0, 70 * 4096, Protection::from_bits(3).unwrap(), false).unwrap();
    for page in 0..70 {
        object::prepare_population(1, base + page * 4096).unwrap();
        let mut g = process::manager();
        let p = g.as_mut().unwrap().rows.get_mut(&1).unwrap();
        assert_eq!(object::populate_page(p, base + page * 4096), Some(Ok(true)));
    }
    assert_eq!(obj.allocated_pages(), 70);
    obj.resize(0).unwrap();
    assert_eq!(obj.allocated_pages(), 0);
    let g = process::manager();
    let table = g.as_ref().unwrap().rows[&1].page_table.as_deref().unwrap();
    for page in 0..70 {
        assert!(table.translate(Addr::new(base + page * 4096)).is_none());
    }
}

#[test]
fn shared_api_records_attach_detach_and_fixed_noreplace_preserves_existing_mapping() {
    use memory::vma::Protection;
    let _lock = kernel_fixture();
    let obj = object::ShmObject::new(8192, 0o600, 0, 0).unwrap();
    ACTIVITY_TIME.with(|t| t.set(100));
    let mapping = object::ShmMapping::new(&obj);
    assert_eq!(obj.activity(), (100, 0, 77));
    ACTIVITY_TIME.with(|t| t.set(101));
    let cloned = mapping.fork(77);
    assert_eq!(obj.activity(), (101, 0, 77));
    ACTIVITY_TIME.with(|t| t.set(102));
    drop(cloned);
    assert_eq!(obj.activity(), (101, 102, 77));
    drop(mapping);
    let base = object::map_object_noreplace(&obj, 0x100000, 8192, Protection::READ).unwrap();
    let other = object::ShmObject::new(8192, 0o600, 0, 0).unwrap();
    assert_eq!(
        object::map_object_noreplace(&other, base, 8192, Protection::READ),
        Err(17)
    );
    assert!(std::sync::Arc::ptr_eq(
        &object::object_at(base).unwrap(),
        &obj
    ));
}

thread_local! {
    static FSIZE_LIMIT:std::cell::Cell<u64>=const{std::cell::Cell::new(u64::MAX)};
    static FSIZE_SIGNAL:std::cell::Cell<bool>=const{std::cell::Cell::new(false)};
}
#[test]
fn shm_ftruncate_obeys_file_size_limit_without_changing_the_object() {
    let _lock = kernel_fixture();
    let fd = fd_number(object::open_path("/dev/shm/fsize-regression", 0xc2, 0o600));
    let file = ipc_file(fd);
    file.truncate(8).unwrap();
    FSIZE_LIMIT.with(|f| f.set(16));
    FSIZE_SIGNAL.with(|f| f.set(false));
    assert_eq!(file.truncate(32), Err(27));
    assert!(FSIZE_SIGNAL.with(|f| f.get()));
    let mut stat = syscall::fs::Stat::zeroed();
    file.fill_stat(&mut stat);
    assert_eq!(stat.st_size, 8);
    FSIZE_LIMIT.with(|f| f.set(u64::MAX));
    object::unlink_path("/dev/shm/fsize-regression");
}

#[test]
fn protecting_the_second_page_of_a_none_mapping_uses_object_page_one() {
    use memory::{vma::Protection, Addr};
    let _lock = kernel_fixture();
    let obj = object::ShmObject::new(8192, 0o600, 0, 0).unwrap();
    obj.ensure_page(0).unwrap();
    obj.ensure_page(4096).unwrap();
    let first = obj.frame_at(0).unwrap();
    let second = obj.frame_at(4096).unwrap();
    unsafe {
        *(first.start_address().as_u64() as *mut u8) = 11;
        *(second.start_address().as_u64() as *mut u8) = 22;
    }
    let base = object::map_object(&obj, 0, 8192, Protection::NONE, false).unwrap();
    {
        let mut g = process::manager();
        let p = g.as_mut().unwrap().rows.get_mut(&1).unwrap();
        let prefix = p.vmas[0].piece(Addr::new(base), Addr::new(base + 4096));
        let mut middle = p.vmas[0].piece(Addr::new(base + 4096), Addr::new(base + 8192));
        middle.prot = Protection::READ;
        p.vmas[0] = middle;
        p.vmas.push(prefix);
    }
    object::prepare_population(1, base + 4096).unwrap();
    {
        let mut g = process::manager();
        let p = g.as_mut().unwrap().rows.get_mut(&1).unwrap();
        assert_eq!(object::populate_page(p, base + 4096), Some(Ok(true)));
        let frame = p
            .page_table
            .as_deref()
            .unwrap()
            .translate(Addr::new(base + 4096))
            .unwrap();
        assert_eq!(frame.start_address(), second.start_address());
        unsafe {
            assert_eq!(*(frame.start_address().as_u64() as *const u8), 22);
        }
    }
    object::release_frame(first);
    object::release_frame(second);
}

#[test]
fn private_readonly_pages_require_an_upgrade_before_lazy_cow() {
    use memory::{
        file_map::{Access, FaultOutcome},
        vma::{MmapFlags, Protection},
        Addr, HostPage,
    };
    let _lock = kernel_fixture();
    let fd = fd_number(object::open_path(
        "/dev/shm/private-ro-upgrade",
        0xc2,
        0o644,
    ));
    ipc_file(fd).truncate(4096).unwrap();
    let ro = fd_number(object::open_path("/dev/shm/private-ro-upgrade", 0, 0));
    let base = match object::mmap_fd(ro, 0, 4096, Protection::READ, MmapFlags::PRIVATE, 0).unwrap()
    {
        syscall::SyscallResult::Ok(a) => a,
        r => panic!("{r:?}"),
    };
    object::prepare_population(1, base).unwrap();
    let mut g = process::manager();
    let p = g.as_mut().unwrap().rows.get_mut(&1).unwrap();
    let table = p.page_table.as_deref_mut().unwrap();
    assert_eq!(
        object::resolve_page(table, &p.vmas, base, Access::Read),
        FaultOutcome::Resolved
    );
    let (canonical, flags) = table
        .get_page_info(HostPage::containing_address(Addr::new(base)))
        .unwrap();
    assert!(!memory::process_memory::is_cow_page(flags));
    assert_eq!(
        object::resolve_page(table, &p.vmas, base, Access::Write),
        FaultOutcome::Signal(11)
    );
    assert_eq!(
        object::protect_page(table, &p.vmas[0], base, Protection::from_bits(3).unwrap()),
        Ok(true)
    );
    assert_eq!(
        table.translate(Addr::new(base)).unwrap().start_address(),
        canonical.start_address()
    );
    assert!(memory::process_memory::is_cow_page(
        table
            .get_page_info(HostPage::containing_address(Addr::new(base)))
            .unwrap()
            .1
    ));
    p.vmas[0].prot = Protection::from_bits(3).unwrap();
    FAIL_COW_ALLOC.with(|f| f.set(true));
    assert_eq!(
        object::resolve_page(table, &p.vmas, base, Access::Write),
        FaultOutcome::Signal(9)
    );
    assert_eq!(
        object::resolve_page(table, &p.vmas, base, Access::Write),
        FaultOutcome::Resolved
    );
    let private = table.translate(Addr::new(base)).unwrap();
    assert_ne!(private.start_address(), canonical.start_address());
    unsafe {
        *(private.start_address().as_u64() as *mut u8) = 99;
        assert_eq!(*(canonical.start_address().as_u64() as *const u8), 0);
    }
    object::protect_page(table, &p.vmas[0], base, Protection::READ).unwrap();
    p.vmas[0].prot = Protection::READ;
    assert!(!memory::process_memory::is_cow_page(
        table
            .get_page_info(HostPage::containing_address(Addr::new(base)))
            .unwrap()
            .1
    ));
    assert_eq!(
        object::resolve_page(table, &p.vmas, base, Access::Write),
        FaultOutcome::Signal(11)
    );
    drop(g);
    object::unlink_path("/dev/shm/private-ro-upgrade");
}

thread_local! {static FAIL_COW_ALLOC:std::cell::Cell<bool>=const{std::cell::Cell::new(false)};}

#[test]
fn deferred_signal_backing_and_repeated_user_copy_resolve() {
    use memory::{
        file_map::{Access, FaultOutcome},
        vma::Protection,
    };
    let _lock = kernel_fixture();
    let obj = object::ShmObject::new(8192, 0o600, 0, 0).unwrap();
    let base = object::map_object(&obj, 0, 8192, Protection::from_bits(3).unwrap(), false).unwrap();
    {
        let g = process::manager();
        let p = &g.as_ref().unwrap().rows[&1];
        object::request_signal_page(p, base).unwrap();
        object::request_signal_page(p, base + 4096).unwrap();
    }
    assert_eq!(obj.allocated_pages(), 0);
    object::drain_signal_pages();
    assert_eq!(obj.allocated_pages(), 2);
    let mut g = process::manager();
    let p = g.as_mut().unwrap().rows.get_mut(&1).unwrap();
    let table = p.page_table.as_deref_mut().unwrap();
    for addr in [base, base + 4096] {
        for _ in 0..2 {
            assert_eq!(
                object::resolve_page(table, &p.vmas, addr, Access::Write),
                FaultOutcome::Resolved
            );
        }
    }
}

#[test]
fn adversarial_sparse_indices_cannot_exhaust_tree_depth() {
    // Increasing index/priority subsequence of the old public SplitMix treap.
    fn old_priority(mut x: u64) -> u64 {
        x = x.wrapping_add(0x9e3779b97f4a7c15);
        x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
        x ^ (x >> 31)
    }
    let obj = object::ShmObject::new(16384 * 4096, 0o600, 0, 0).unwrap();
    let mut tails: Vec<(u64, usize)> = Vec::new();
    let mut previous = vec![None; 16384];
    for index in 0..16384 {
        let p = old_priority(index as u64);
        let at = tails.partition_point(|(v, _)| *v < p);
        previous[index] = at.checked_sub(1).map(|k| tails[k].1);
        if at == tails.len() {
            tails.push((p, index));
        } else {
            tails[at] = (p, index);
        }
    }
    let mut indices = Vec::new();
    let mut at = Some(tails.last().unwrap().1);
    while let Some(i) = at {
        indices.push(i);
        at = previous[i];
    }
    indices.reverse();
    assert!(indices.len() > 63);
    for &i in &indices {
        obj.ensure_page(i as u64 * 4096).unwrap();
    }
    assert_eq!(obj.allocated_pages(), indices.len() as u64);
    obj.resize(8192 * 4096).unwrap();
    assert_eq!(
        obj.allocated_pages(),
        indices.iter().filter(|i| **i < 8192).count() as u64
    );
}

#[test]
fn tmpfile_publication_links_seek_and_io_use_the_same_object() {
    use syscall::SyscallResult::{Err as Failure, Ok as Success};
    let _lock = kernel_fixture();
    let tmp = fd_number(object::open_path(
        "/dev/shm",
        2 | 0x400000 | syscall::fs::O_DIRECTORY,
        0o600,
    ));
    let file = ipc_file(tmp);
    let mut stat = syscall::fs::Stat::zeroed();
    file.fill_stat(&mut stat);
    assert_eq!(stat.st_nlink, 0);
    assert_eq!(file.write(b"initialized", false), Ok(11));
    assert_eq!(
        object::open_path("/dev/shm/published", 2, 0),
        Some(Failure(2))
    );
    assert_eq!(object::link_fd(tmp, "/dev/shm/published"), Some(Success(0)));
    assert_eq!(
        object::link_fd(tmp, "/dev/shm/published"),
        Some(Failure(17))
    );
    file.fill_stat(&mut stat);
    assert_eq!(stat.st_nlink, 1);
    assert_eq!(
        object::link_path("/dev/shm/published", "/dev/shm/alias"),
        Some(Success(0))
    );
    file.fill_stat(&mut stat);
    assert_eq!(stat.st_nlink, 2);
    assert_eq!(
        object::rename_path("/dev/shm/alias", "/dev/shm/renamed"),
        Some(Success(0))
    );
    assert_eq!(file.seek(0, 0), Ok(0));
    let mut bytes = [0; 11];
    assert_eq!(file.read(bytes.as_mut_ptr() as u64, 11), Ok(11));
    assert_eq!(&bytes, b"initialized");
    assert_eq!(file.seek(4096, 0), Ok(4096));
    assert_eq!(file.write(b"x", false), Ok(1));
    assert_eq!(file.seek(11, 0), Ok(11));
    let mut hole = [1u8; 16];
    file.read(hole.as_mut_ptr() as u64, 16).unwrap();
    assert_eq!(hole, [0; 16]);
    let dir = ipc_file(fd_number(object::open_path(
        "/dev/shm",
        syscall::fs::O_DIRECTORY,
        0,
    )));
    assert_eq!(dir.seek(0, 0), Ok(0));
    assert_eq!(dir.seek(0, 2), Err(22));
    object::unlink_path("/dev/shm/published");
    object::unlink_path("/dev/shm/renamed");
    file.fill_stat(&mut stat);
    assert_eq!(stat.st_nlink, 0);
}

#[test]
fn shrinking_above_lowered_fsize_limit_and_offset_overflow() {
    use memory::vma::{MmapFlags, Protection};
    let _lock = kernel_fixture();
    let fd = fd_number(object::open_path("/dev/shm/limit-shrink", 0xc2, 0o600));
    let file = ipc_file(fd);
    file.truncate(8192).unwrap();
    FSIZE_LIMIT.with(|f| f.set(1));
    FSIZE_SIGNAL.with(|f| f.set(false));
    assert_eq!(file.truncate(4096), Ok(()));
    assert!(!FSIZE_SIGNAL.with(|f| f.get()));
    FSIZE_LIMIT.with(|f| f.set(u64::MAX));
    assert_eq!(
        object::mmap_fd(
            fd,
            0,
            4096,
            Protection::READ,
            MmapFlags::SHARED,
            u64::MAX & !4095
        ),
        Some(syscall::SyscallResult::Err(75))
    );
    object::unlink_path("/dev/shm/limit-shrink");
}

thread_local! { static SLOW_CREATOR:std::cell::Cell<bool>=const{std::cell::Cell::new(false)}; }
static SLOW_CREATOR_STARTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
#[test]
fn slow_named_creator_cannot_poison_or_remove_a_replacement() {
    use semaphore::*;
    use std::sync::atomic::Ordering;
    let name = b"/slow-private-creator\0";
    SLOW_CREATOR_STARTED.store(false, Ordering::Release);
    let creator = std::thread::spawn(move || unsafe {
        SLOW_CREATOR.with(|f| f.set(true));
        sem_open(name.as_ptr(), 0x40, 0o600u32, 4u32) as usize
    });
    while !SLOW_CREATOR_STARTED.load(Ordering::Acquire) {
        std::thread::yield_now();
    }
    unsafe {
        // The slow creator has no public name. Another creator may publish.
        let replacement = sem_open(name.as_ptr(), 0xc0, 0o600u32, 7u32);
        assert!(!replacement.is_null());
        let first = creator.join().unwrap() as *mut Sem;
        assert_eq!(first, replacement);
        let mut value = 0;
        assert_eq!(sem_getvalue(first, &mut value), 0);
        assert_eq!(value, 7);
        assert_eq!(sem_close(first), 0);
        assert_eq!(sem_close(replacement), 0);
        assert_eq!(sem_unlink(name.as_ptr()), 0);
    }
}
#[test]
fn closing_an_unpublished_creator_leaves_the_public_name_available() {
    unsafe {
        let fd = open(b"/dev/shm\0".as_ptr(), 2 | 0x400000, 0o600);
        assert!(fd >= 0);
        ftruncate(fd, 32);
        close(fd);
        let name = b"/creator-died-private\0";
        let sem = semaphore::sem_open(name.as_ptr(), 0xc0, 0o600u32, 2u32);
        assert!(!sem.is_null());
        assert_eq!(semaphore::sem_close(sem), 0);
        assert_eq!(semaphore::sem_unlink(name.as_ptr()), 0);
    }
}

#[test]
fn shm_open_accepts_linux_writeonly_access_and_rejects_invalid_access_mode() {
    let name = b"/linux-writeonly-shm\0";
    unsafe {
        let fd = shm::shm_open(name.as_ptr(), 0xc1, 0o600);
        assert!(fd >= 0);
        assert_eq!(ftruncate(fd, 4096), 0);
        close(fd);
        assert_eq!(shm::shm_unlink(name.as_ptr()), 0);
        assert_eq!(shm::shm_open(name.as_ptr(), 3, 0), -1);
        assert_eq!(ERRNO.with(|e| e.get()), 22);
    }
}

#[test]
fn opening_an_existing_named_semaphore_ignores_the_creation_value() {
    use semaphore::*;
    let name = b"/existing-ignores-value\0";
    unsafe {
        let first = sem_open(name.as_ptr(), 0xc0, 0o600u32, 3u32);
        assert!(!first.is_null());
        let again = sem_open(name.as_ptr(), 0x40, 0o600u32, SEM_VALUE_MAX + 1);
        assert_eq!(first, again);
        let mut value = 0;
        assert_eq!(sem_getvalue(again, &mut value), 0);
        assert_eq!(value, 3);
        assert_eq!(sem_close(first), 0);
        assert_eq!(sem_close(again), 0);
        assert_eq!(sem_unlink(name.as_ptr()), 0);
    }
}

#[test]
fn protection_replacement_preserves_activity_and_split_counts_vm_opens() {
    use memory::{
        vma::{MmapFlags, Protection, Vma},
        Addr,
    };
    let _lock = kernel_fixture();
    let obj = object::ShmObject::new(3 * 4096, 0o600, 0, 0).unwrap();
    ACTIVITY_TIME.with(|t| t.set(100));
    let mut original = Vma::new(
        Addr::new(0x100000),
        Addr::new(0x103000),
        Protection::READ,
        MmapFlags::SHARED,
    );
    original.shm = Some(object::ShmMapping::new(&obj));
    ACTIVITY_TIME.with(|t| t.set(101));
    let whole = original.replacement_piece(original.start, original.end);
    drop(original);
    assert_eq!(obj.attachments(), 1);
    assert_eq!(obj.activity(), (100, 0, 77));
    let mut original = whole;
    let prefix = original.piece(Addr::new(0x100000), Addr::new(0x101000));
    let suffix = original.piece(Addr::new(0x102000), Addr::new(0x103000));
    let middle = original.replacement_piece(Addr::new(0x101000), Addr::new(0x102000));
    assert_eq!(obj.attachments(), 3);
    drop(original);
    assert_eq!(obj.attachments(), 3);
    assert_eq!(obj.activity(), (101, 0, 77));
    ACTIVITY_TIME.with(|t| t.set(102));
    drop(prefix);
    assert_eq!(obj.attachments(), 2);
    assert_eq!(obj.activity(), (101, 102, 77));
    drop(middle);
    drop(suffix);
    assert_eq!(obj.attachments(), 0);
}

#[test]
fn an_abandoned_registration_does_not_make_every_post_wake_an_empty_queue() {
    use semaphore::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    unsafe {
        let mut storage = std::mem::MaybeUninit::<Sem>::uninit();
        let s = storage.as_mut_ptr();
        assert_eq!(sem_init(s, 1, 0), 0);
        // A killed waiter's user-space registration need not run its cleanup.
        (&*((s as *const u8).add(4) as *const AtomicU32)).store(1, Ordering::SeqCst);
        (&*(s as *const AtomicU32)).store(0x80000000, Ordering::SeqCst);
        let before = raw::wake_calls(s as u64);
        for _ in 0..1000 {
            assert_eq!(sem_post(s), 0);
            assert_eq!(sem_trywait(s), 0);
        }
        assert_eq!(raw::wake_calls(s as u64) - before, 1);
        let mut value = -1;
        assert_eq!(sem_getvalue(s, &mut value), 0);
        assert_eq!(value, 0);
    }
}
#[test]
fn a_woken_cohort_still_requires_one_post_for_each_successful_wait() {
    use semaphore::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    unsafe {
        let mut storage = std::mem::MaybeUninit::<Sem>::uninit();
        let s = storage.as_mut_ptr();
        assert_eq!(sem_init(s, 1, 0), 0);
        let address = s as usize;
        std::thread::scope(|scope| {
            let threads: Vec<_> = (0..16)
                .map(|_| scope.spawn(move || assert_eq!(sem_wait(address as *mut Sem), 0)))
                .collect();
            let waiters = &*((s as *const u8).add(4) as *const AtomicU32);
            while waiters.load(Ordering::SeqCst) != 16 {
                std::thread::yield_now();
            }
            assert_eq!(sem_post(s), 0);
            for _ in 1..16 {
                assert_eq!(sem_post(s), 0);
            }
            for thread in threads {
                thread.join().unwrap();
            }
        });
        let mut value = -1;
        assert_eq!(sem_getvalue(s, &mut value), 0);
        assert_eq!(value, 0);
        assert_eq!(sem_trywait(s), -1);
    }
}

#[test]
fn waiter_metadata_does_not_change_public_value_or_overflow_capacity() {
    use semaphore::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    unsafe {
        let mut storage = std::mem::MaybeUninit::<Sem>::uninit();
        let s = storage.as_mut_ptr();
        assert_eq!(sem_init(s, 1, 0), 0);
        (&*(s as *const AtomicU32)).store(u32::MAX, Ordering::SeqCst);
        let mut value = -1;
        assert_eq!(sem_getvalue(s, &mut value), 0);
        assert_eq!(value, SEM_VALUE_MAX as i32);
        assert_eq!(sem_post(s), -1);
        assert_eq!(ERRNO.with(|e| e.get()), 75);
        assert_eq!(sem_trywait(s), 0);
        assert_eq!(sem_getvalue(s, &mut value), 0);
        assert_eq!(value, (SEM_VALUE_MAX - 1) as i32);
    }
}
