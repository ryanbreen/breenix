//! Directory streams (POSIX.1-2024 XBD <dirent.h>; XSH opendir, fdopendir, readdir,
//! readdir_r, closedir, rewinddir, telldir, seekdir, dirfd, scandir, alphasort).
//!
//! A DIR holds the descriptor and a buffer of records from getdents64 (the Breenix kernel syscall ABI). readdir copies one record at a time into the stream's own
//! `struct dirent` and refills the buffer when it runs out. A position (telldir, seekdir) is
//! the kernel's directory offset: the `d_off` of the last entry returned, which names the
//! entry after it, or 0 at the start. seekdir hands it back to the kernel with lseek.

use core::cell::UnsafeCell;
use core::ptr::null_mut;
use core::sync::atomic::{AtomicBool, Ordering};

extern "C" {
    fn malloc(size: usize) -> *mut u8;
    fn realloc(ptr: *mut u8, size: usize) -> *mut u8;
    fn free(ptr: *mut u8);
    fn open(path: *const u8, flags: i32, mode: u32) -> i32;
    fn close(fd: i32) -> i32;
    fn fstat(fd: i32, buf: *mut u8) -> i32;
    fn lseek(fd: i32, offset: i64, whence: i32) -> i64;
    fn __errno_location() -> *mut i32;
}

// Errnos, open/stat encodings and offsets below come from Breenix kernel syscall ABI.
const EIO: i32 = 5;
const EBADF: i32 = 9;
const ENOMEM: i32 = 12;
const ENOTDIR: i32 = 20;
const EINVAL: i32 = 22;

#[cfg(target_arch = "x86_64")]
const O_DIRECTORY: i32 = 0o200000;
#[cfg(target_arch = "aarch64")]
const O_DIRECTORY: i32 = 0o40000;
const O_CLOEXEC: i32 = 0o2000000;
const SEEK_SET: i32 = 0;
const SEEK_CUR: i32 = 1;

/// Offset of st_mode in the kernel's struct stat.
#[cfg(target_arch = "x86_64")]
const ST_MODE: usize = 24;
#[cfg(target_arch = "aarch64")]
const ST_MODE: usize = 16;
const S_IFMT: u32 = 0o170000;
const S_IFDIR: u32 = 0o040000;

/// The getdents64 record header: d_ino (8), d_off (8), d_reclen (2), d_type (1).
const RECORD_HEADER: usize = 19;
const BUF_SIZE: usize = 8192;

/// `struct dirent`, matching the Breenix getdents64 producer.
#[repr(C)]
pub struct Dirent {
    pub d_ino: u64,
    pub d_off: i64,
    pub d_reclen: u16,
    pub d_type: u8,
    pub d_name: [u8; 256],
}

/// The stream behind C's opaque `DIR`.
#[repr(C)]
pub struct Dir {
    lock: AtomicBool,
    data: UnsafeCell<DirData>,
}

struct DirData {
    fd: i32,
    eof: bool,
    /// Valid bytes in `buf`, and the offset of the next record.
    len: usize,
    at: usize,
    /// What telldir reports.
    pos: i64,
    entry: Dirent,
    buf: [u64; BUF_SIZE / 8],
}

unsafe fn set_errno(e: i32) {
    *__errno_location() = e;
}

unsafe fn errno() -> i32 {
    *__errno_location()
}

/// Wrap the directory descriptor `fd` in a new stream starting at position `pos`.
unsafe fn new_stream(fd: i32, pos: i64) -> *mut Dir {
    let d = malloc(core::mem::size_of::<Dir>()) as *mut Dir;
    if d.is_null() {
        set_errno(ENOMEM);
        return null_mut();
    }
    d.write(Dir {
        lock: AtomicBool::new(false),
        data: UnsafeCell::new(DirData {
            fd,
            eof: false,
            len: 0,
            at: 0,
            pos,
            entry: Dirent {
                d_ino: 0,
                d_off: 0,
                d_reclen: 0,
                d_type: 0,
                d_name: [0; 256],
            },
            buf: [0; BUF_SIZE / 8],
        }),
    });
    d
}

// The lock is acquired before any mutable reference to stream data is formed.
struct StreamGuard<'a>(&'a Dir);
impl Drop for StreamGuard<'_> {
    fn drop(&mut self) {
        self.0.lock.store(false, Ordering::Release);
    }
}
impl StreamGuard<'_> {
    fn data(&mut self) -> &mut DirData {
        unsafe { &mut *self.0.data.get() }
    }
}
unsafe fn stream<'a>(dirp: *mut Dir) -> StreamGuard<'a> {
    let dir = &*dirp;
    while dir
        .lock
        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
    StreamGuard(dir)
}

/// The next record in the stream's buffer, refilling it as needed. Ok(None) at the end;
/// Err(errno) when getdents64 fails.
unsafe fn next_record(d: &mut DirData) -> Result<Option<*const u8>, i32> {
    if d.at >= d.len {
        if d.eof {
            return Ok(None);
        }
        let n = libbreenix::raw::syscall3(
            libbreenix::syscall::nr::GETDENTS64,
            d.fd as u64,
            d.buf.as_mut_ptr() as u64,
            BUF_SIZE as u64,
        ) as i64;
        if n < 0 {
            return Err((-n) as i32);
        }
        if n == 0 {
            d.eof = true;
            return Ok(None);
        }
        d.len = n as usize;
        d.at = 0;
    }
    let record = (d.buf.as_ptr() as *const u8).add(d.at);
    let reclen = core::ptr::read_unaligned(record.add(16) as *const u16) as usize;
    if reclen < RECORD_HEADER || d.at + reclen > d.len {
        return Err(EIO);
    }
    d.at += reclen;
    d.pos = core::ptr::read_unaligned(record.add(8) as *const i64);
    Ok(Some(record))
}

/// Copy a getdents64 record into a `struct dirent`.
unsafe fn fill(entry: *mut Dirent, record: *const u8) {
    let reclen = core::ptr::read_unaligned(record.add(16) as *const u16) as usize;
    (*entry).d_ino = core::ptr::read_unaligned(record as *const u64);
    (*entry).d_off = core::ptr::read_unaligned(record.add(8) as *const i64);
    (*entry).d_type = *record.add(18);
    let name = record.add(RECORD_HEADER);
    let max = (reclen - RECORD_HEADER).min(255);
    let mut n = 0;
    while n < max && *name.add(n) != 0 {
        (*entry).d_name[n] = *name.add(n);
        n += 1;
    }
    (*entry).d_name[n] = 0;
    (*entry).d_reclen = (core::mem::offset_of!(Dirent, d_name) + n + 1) as u16;
}

/// opendir - open a directory stream.
#[no_mangle]
pub unsafe extern "C" fn opendir(name: *const u8) -> *mut Dir {
    let fd = open(name, O_DIRECTORY | O_CLOEXEC, 0);
    if fd < 0 {
        return null_mut();
    }
    let d = new_stream(fd, 0);
    if d.is_null() {
        close(fd);
    }
    d
}

/// fdopendir - a stream over an open directory descriptor, from its current position.
#[no_mangle]
pub unsafe extern "C" fn fdopendir(fd: i32) -> *mut Dir {
    let mut st = [0u64; 32];
    if fstat(fd, st.as_mut_ptr() as *mut u8) != 0 {
        return null_mut();
    }
    let mode = core::ptr::read((st.as_ptr() as *const u8).add(ST_MODE) as *const u32);
    if mode & S_IFMT != S_IFDIR {
        set_errno(ENOTDIR);
        return null_mut();
    }
    let saved = errno();
    let mut pos = lseek(fd, 0, SEEK_CUR);
    if pos < 0 {
        pos = 0;
        set_errno(saved);
    }
    new_stream(fd, pos)
}

/// readdir - the next entry, in storage the stream owns, or null at the end (errno
/// unchanged) or on error (errno set).
#[no_mangle]
pub unsafe extern "C" fn readdir(dirp: *mut Dir) -> *mut Dirent {
    if dirp.is_null() {
        set_errno(EBADF);
        return null_mut();
    }
    let mut guard = stream(dirp);
    let d = guard.data();
    match next_record(d) {
        Ok(Some(record)) => {
            fill(&mut d.entry, record);
            &mut d.entry
        }
        Ok(None) => null_mut(),
        Err(e) => {
            set_errno(e);
            null_mut()
        }
    }
}

/// readdir_r - readdir into the caller's `entry`; returns 0 or an errno value.
#[no_mangle]
pub unsafe extern "C" fn readdir_r(
    dirp: *mut Dir,
    entry: *mut Dirent,
    result: *mut *mut Dirent,
) -> i32 {
    if dirp.is_null() || entry.is_null() || result.is_null() {
        return EINVAL;
    }
    let mut guard = stream(dirp);
    match next_record(guard.data()) {
        Ok(Some(record)) => {
            fill(entry, record);
            *result = entry;
            0
        }
        Ok(None) => {
            *result = null_mut();
            0
        }
        Err(e) => {
            *result = null_mut();
            e
        }
    }
}

/// closedir - close the stream and its descriptor.
#[no_mangle]
pub unsafe extern "C" fn closedir(dirp: *mut Dir) -> i32 {
    if dirp.is_null() {
        set_errno(EBADF);
        return -1;
    }
    let r = close(stream(dirp).data().fd);
    free(dirp as *mut u8);
    r
}

/// dirfd - the stream's descriptor.
#[no_mangle]
pub unsafe extern "C" fn dirfd(dirp: *mut Dir) -> i32 {
    if dirp.is_null() {
        set_errno(EINVAL);
        return -1;
    }
    stream(dirp).data().fd
}

/// seekdir - continue from a position telldir returned.
#[no_mangle]
pub unsafe extern "C" fn seekdir(dirp: *mut Dir, pos: i64) {
    if dirp.is_null() {
        return;
    }
    let mut guard = stream(dirp);
    let d = guard.data();
    let saved = errno();
    if lseek(d.fd, pos, SEEK_SET) < 0 {
        set_errno(saved);
    }
    d.len = 0;
    d.at = 0;
    d.eof = false;
    d.pos = pos;
}

/// telldir - the stream's current position.
#[no_mangle]
pub unsafe extern "C" fn telldir(dirp: *mut Dir) -> i64 {
    if dirp.is_null() {
        set_errno(EBADF);
        return -1;
    }
    stream(dirp).data().pos
}

/// rewinddir - restart at the first entry, seeing the directory as it is now.
#[no_mangle]
pub unsafe extern "C" fn rewinddir(dirp: *mut Dir) {
    seekdir(dirp, 0);
}

unsafe fn name_cmp(a: *const u8, b: *const u8) -> i32 {
    let mut i = 0;
    loop {
        let (x, y) = (*a.add(i), *b.add(i));
        if x != y || x == 0 {
            return x as i32 - y as i32;
        }
        i += 1;
    }
}

/// alphasort - order entries by name. In the C and POSIX locales, and in C.UTF-8, strcoll
/// is byte order.
#[no_mangle]
pub unsafe extern "C" fn alphasort(a: *const *const Dirent, b: *const *const Dirent) -> i32 {
    name_cmp((**a).d_name.as_ptr(), (**b).d_name.as_ptr())
}

/// Heapsort `n` pointers in place, ascending by `cmp`.
pub(crate) unsafe fn sort_ptrs<T>(
    v: *mut *mut T,
    n: usize,
    mut cmp: impl FnMut(*mut *mut T, *mut *mut T) -> i32,
) {
    if n < 2 {
        return;
    }
    let s = core::slice::from_raw_parts_mut(v, n);
    let mut sift = |s: &mut [*mut T], mut root: usize, end: usize| loop {
        let mut child = 2 * root + 1;
        if child >= end {
            break;
        }
        if child + 1 < end && cmp(&mut s[child], &mut s[child + 1]) < 0 {
            child += 1;
        }
        if cmp(&mut s[root], &mut s[child]) >= 0 {
            break;
        }
        s.swap(root, child);
        root = child;
    };
    for start in (0..n / 2).rev() {
        sift(s, start, n);
    }
    for end in (1..n).rev() {
        s.swap(0, end);
        sift(s, 0, end);
    }
}

type Filter = Option<unsafe extern "C" fn(*const Dirent) -> i32>;
type Compar = Option<unsafe extern "C" fn(*const *const Dirent, *const *const Dirent) -> i32>;

/// scandir - every entry of `dir` that `filter` accepts, each in its own malloc'd
/// `struct dirent`, sorted by `compar`. Returns the count, or -1 with errno set.
#[no_mangle]
pub unsafe extern "C" fn scandir(
    dir: *const u8,
    namelist: *mut *mut *mut Dirent,
    filter: Filter,
    compar: Compar,
) -> i32 {
    let d = opendir(dir);
    if d.is_null() {
        return -1;
    }
    let mut list: *mut *mut Dirent = null_mut();
    let mut n = 0usize;
    let mut cap = 0usize;
    let saved = errno();
    let failed = loop {
        set_errno(0);
        let e = readdir(d);
        if e.is_null() {
            let err = errno();
            break if err != 0 { err } else { 0 };
        }
        if let Some(f) = filter {
            if f(e) == 0 {
                continue;
            }
        }
        if n == cap {
            let new_cap = if cap == 0 { 16 } else { cap * 2 };
            let grown = realloc(
                list as *mut u8,
                new_cap * core::mem::size_of::<*mut Dirent>(),
            ) as *mut *mut Dirent;
            if grown.is_null() {
                break ENOMEM;
            }
            list = grown;
            cap = new_cap;
        }
        let copy = malloc(core::mem::size_of::<Dirent>()) as *mut Dirent;
        if copy.is_null() {
            break ENOMEM;
        }
        core::ptr::copy_nonoverlapping(e, copy, 1);
        *list.add(n) = copy;
        n += 1;
    };
    closedir(d);
    if failed != 0 {
        for i in 0..n {
            free(*list.add(i) as *mut u8);
        }
        free(list as *mut u8);
        set_errno(failed);
        return -1;
    }
    set_errno(saved);
    if list.is_null() {
        list = malloc(core::mem::size_of::<*mut Dirent>()) as *mut *mut Dirent;
        if list.is_null() {
            set_errno(ENOMEM);
            return -1;
        }
    }
    if let Some(c) = compar {
        sort_ptrs(list, n, |a, b| {
            c(a as *const *const Dirent, b as *const *const Dirent)
        });
    }
    *namelist = list;
    n as i32
}
