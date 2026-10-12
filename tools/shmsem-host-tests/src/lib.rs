//! Run the production shared-object and VMA logic with a host frame allocator.
#![cfg(test)]
#![feature(c_variadic)]
extern crate alloc;

pub mod irq_safe_mutex {
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
pub mod syscall {
    pub mod nr {
        pub const FUTEX: u64 = 98;
        pub const GETPID: u64 = 172;
        pub const YIELD: u64 = 124;
    }
    pub mod errno {
        pub const EINVAL: i32 = 22;
        pub const ENOMEM: i32 = 12;
    }
    pub mod memory_common {
        pub const PAGE_SIZE: u64 = 4096;
        pub fn allocate_zeroed_frames(count: usize) -> Option<Vec<crate::memory::Frame>> {
            Some(
                (0..count)
                    .map(|_| crate::memory::frame_allocator::allocate_frame())
                    .collect(),
            )
        }
    }
}
pub mod memory {
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
        #[derive(Debug)]
        pub struct Binding;
    }
    #[path = "../../../../kernel/src/memory/vma.rs"]
    pub mod vma;
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
        pub fn frame_decref(f: Frame) -> bool {
            frame_allocator::decref(f)
        }
    }
}
pub mod ipc {
    pub use crate::object as shmem;
}
#[path = "../../../kernel/src/ipc/shmem/object.rs"]
pub mod object;

#[test]
fn sizing_zeroes_growth_preserves_prefix_and_clears_shrunken_tail() {
    let obj = object::ShmObject::new(5, 0o600, 1, 2).unwrap();
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
    let mut mapping = obj.frames_for_mapping(0, 4096).unwrap();
    let f = mapping.pop().unwrap();
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
    let fork = piece.shm.clone();
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
                queue.1.notify_one();
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
                    assert_eq!(op & 256, 256);
                    let ts = timeout as *const i64;
                    let at = i128::from(*ts) * 1_000_000_000 + i128::from(*ts.add(1));
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos() as i128;
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
        let mut fs = FS.lock().unwrap();
        let obj = if let Some(obj) = fs.names.get(&name) {
            if flags & 0xc0 == 0xc0 {
                return super::syscall_result_to_c_int(-17);
            }
            obj.clone()
        } else {
            if flags & 0x40 == 0 {
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
            fs.names.insert(name, obj.clone());
            obj
        };
        let fd = fs.next_fd;
        fs.next_fd += 1;
        fs.fds.insert(fd, obj);
        fd
    }
    pub unsafe fn unlink(path: *const u8) -> i32 {
        if FS.lock().unwrap().names.remove(&text(path)).is_some() {
            0
        } else {
            super::syscall_result_to_c_int(-2)
        }
    }
    pub unsafe fn ftruncate(fd: i32, len: i64) -> i32 {
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
use host_fs::{close, fstat, ftruncate, mmap, munmap, open, unlink};
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
