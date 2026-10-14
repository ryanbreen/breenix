//! POSIX.1-2024 process, time and file-time functions that sit directly on the
//! kernel: the exec family over execve, wait/wait3/wait4, sleep and usleep,
//! gettimeofday, gethostname, ttyname, getpagesize, and utime/utimes/futimens/
//! utimensat over the kernel's utimensat.

use core::ffi::VaList;
use core::ptr::{null, null_mut};

const EINTR: i32 = 4;
const ENOEXEC: i32 = 8;
const ENOMEM: i32 = 12;
const EACCES: i32 = 13;
const EINVAL: i32 = 22;
const ENOTTY: i32 = 25;
const ERANGE: i32 = 34;
const ENAMETOOLONG: i32 = 36;
const ENOENT: i32 = 2;
const ENOTDIR: i32 = 20;

const AT_FDCWD: i32 = -100;
const AT_SYMLINK_NOFOLLOW: i32 = 0x100;
const CLOCK_REALTIME: i32 = 0;
const S_IFMT: u32 = 0o170000;
const S_IFCHR: u32 = 0o020000;

/// The shell execvp runs a file the kernel will not execute (POSIX: ENOEXEC).
const SHELL: &[u8] = b"/bin/sh\0";
/// PATH when the environment has none.
const DEFAULT_PATH: &[u8] = b"/usr/local/bin:/bin:/sbin";

mod nr {
    #[cfg(target_arch = "aarch64")]
    pub const UTIMENSAT: u64 = 88;
    #[cfg(target_arch = "x86_64")]
    pub const UTIMENSAT: u64 = 280;
    #[cfg(target_arch = "aarch64")]
    pub const WAIT4: u64 = 260;
    #[cfg(target_arch = "x86_64")]
    pub const WAIT4: u64 = 61;
}

extern "C" {
    static mut environ: *const *const u8;
    fn getenv(name: *const u8) -> *const u8;
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Timespec {
    tv_sec: i64,
    tv_nsec: i64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Timeval {
    tv_sec: i64,
    tv_usec: i64,
}

fn set_errno(e: i32) {
    crate::pthread::set_errno(e)
}

unsafe fn c_len(s: *const u8) -> usize {
    let mut n = 0;
    while *s.add(n) != 0 {
        n += 1;
    }
    n
}

// ---------------------------------------------------------------------------------
// The exec family
// ---------------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn execv(path: *const u8, argv: *const *const u8) -> i32 {
    crate::execve(path, argv, environ)
}

/// Run `path` as a shell script: `sh path argv[1..]`, as execvp does for ENOEXEC.
unsafe fn exec_script(path: *const u8, argv: *const *const u8, envp: *const *const u8) -> i32 {
    let mut argc = 0usize;
    while !(*argv.add(argc)).is_null() {
        argc += 1;
    }
    let count = argc.max(1) + 2;
    let args = crate::malloc(count * core::mem::size_of::<*const u8>()) as *mut *const u8;
    if args.is_null() {
        set_errno(ENOMEM);
        return -1;
    }
    *args = SHELL.as_ptr();
    *args.add(1) = path;
    let mut out = 2;
    for i in 1..argc {
        *args.add(out) = *argv.add(i);
        out += 1;
    }
    *args.add(out) = null();
    crate::execve(SHELL.as_ptr(), args, envp);
    crate::free(args as *mut u8);
    -1
}

/// execvp/execvpe: a file name without a slash is looked up in PATH (POSIX 2.9.1.1).
/// A file the kernel rejects as not executable (ENOEXEC) is run by the shell. EACCES on
/// one candidate does not end the search, but is reported if nothing else is found.
unsafe fn exec_search(file: *const u8, argv: *const *const u8, envp: *const *const u8) -> i32 {
    let file_len = c_len(file);
    if file_len == 0 {
        set_errno(ENOENT);
        return -1;
    }
    let name = core::slice::from_raw_parts(file, file_len);
    if name.contains(&b'/') {
        crate::execve(file, argv, envp);
        if crate::pthread::errno() == ENOEXEC {
            return exec_script(file, argv, envp);
        }
        return -1;
    }
    let path_var = getenv(b"PATH\0".as_ptr());
    let path = if path_var.is_null() { DEFAULT_PATH } else { core::slice::from_raw_parts(path_var, c_len(path_var)) };
    let mut buf = [0u8; 4096];
    let mut denied = false;
    for dir in path.split(|&b| b == b':') {
        // An empty element is the current directory.
        let dir: &[u8] = if dir.is_empty() { b"." } else { dir };
        if dir.len() + 1 + file_len + 1 > buf.len() {
            set_errno(ENAMETOOLONG);
            return -1;
        }
        buf[..dir.len()].copy_from_slice(dir);
        buf[dir.len()] = b'/';
        buf[dir.len() + 1..dir.len() + 1 + file_len].copy_from_slice(name);
        buf[dir.len() + 1 + file_len] = 0;
        crate::execve(buf.as_ptr(), argv, envp);
        match crate::pthread::errno() {
            ENOEXEC => return exec_script(buf.as_ptr(), argv, envp),
            EACCES => denied = true,
            ENOENT | ENOTDIR => {},
            _ => return -1,
        }
    }
    set_errno(if denied { EACCES } else { ENOENT });
    -1
}

#[no_mangle]
pub unsafe extern "C" fn execvp(file: *const u8, argv: *const *const u8) -> i32 {
    exec_search(file, argv, environ)
}

#[no_mangle]
pub unsafe extern "C" fn execvpe(file: *const u8, argv: *const *const u8, envp: *const *const u8) -> i32 {
    exec_search(file, argv, envp)
}

/// Collect `first` and the variadic arguments up to the null pointer into an argv array,
/// then call `run` with it (and, for execle, the environment that follows the null).
unsafe fn with_arg_list(
    first: *const u8,
    mut ap: VaList,
    with_env: bool,
    run: &dyn Fn(*const *const u8, *const *const u8) -> i32,
) -> i32 {
    let mut count = 1usize;
    let mut probe = ap.clone();
    if !first.is_null() {
        while !probe.arg::<*const u8>().is_null() {
            count += 1;
        }
    }
    let args = crate::malloc((count + 1) * core::mem::size_of::<*const u8>()) as *mut *const u8;
    if args.is_null() {
        set_errno(ENOMEM);
        return -1;
    }
    *args = first;
    if !first.is_null() {
        for i in 1..count {
            *args.add(i) = ap.arg::<*const u8>();
        }
        ap.arg::<*const u8>();
    }
    *args.add(count) = null();
    let envp = if with_env { ap.arg::<*const *const u8>() } else { environ };
    let result = run(args, envp);
    crate::free(args as *mut u8);
    result
}

#[no_mangle]
pub unsafe extern "C" fn execl(path: *const u8, arg0: *const u8, mut args: ...) -> i32 {
    with_arg_list(arg0, args.as_va_list(), false, &|argv, envp| crate::execve(path, argv, envp))
}

#[no_mangle]
pub unsafe extern "C" fn execle(path: *const u8, arg0: *const u8, mut args: ...) -> i32 {
    with_arg_list(arg0, args.as_va_list(), true, &|argv, envp| crate::execve(path, argv, envp))
}

#[no_mangle]
pub unsafe extern "C" fn execlp(file: *const u8, arg0: *const u8, mut args: ...) -> i32 {
    with_arg_list(arg0, args.as_va_list(), false, &|argv, envp| exec_search(file, argv, envp))
}

// ---------------------------------------------------------------------------------
// Waiting
// ---------------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn wait(status: *mut i32) -> i32 {
    crate::waitpid(-1, status, 0)
}

#[no_mangle]
pub unsafe extern "C" fn wait4(pid: i32, status: *mut i32, options: i32, usage: *mut u8) -> i32 {
    let r = libbreenix::raw::syscall4(nr::WAIT4, pid as i64 as u64, status as u64, options as u64, usage as u64);
    crate::syscall_result_to_c_int(r as i64)
}

#[no_mangle]
pub unsafe extern "C" fn wait3(status: *mut i32, options: i32, usage: *mut u8) -> i32 {
    wait4(-1, status, options, usage)
}

// ---------------------------------------------------------------------------------
// Sleeping and the time of day
// ---------------------------------------------------------------------------------

/// sleep: the unslept seconds when a signal ends the sleep early, rounded up.
#[no_mangle]
pub unsafe extern "C" fn sleep(seconds: u32) -> u32 {
    let req = Timespec { tv_sec: seconds as i64, tv_nsec: 0 };
    let mut rem = Timespec { tv_sec: 0, tv_nsec: 0 };
    if crate::nanosleep(&req as *const _ as *const u8, &mut rem as *mut _ as *mut u8) == 0 {
        return 0;
    }
    if crate::pthread::errno() != EINTR {
        return seconds;
    }
    rem.tv_sec as u32 + (rem.tv_nsec > 0) as u32
}

#[no_mangle]
pub unsafe extern "C" fn usleep(usec: u32) -> i32 {
    let req = Timespec { tv_sec: (usec / 1_000_000) as i64, tv_nsec: (usec % 1_000_000) as i64 * 1000 };
    crate::nanosleep(&req as *const _ as *const u8, null_mut())
}

/// gettimeofday: CLOCK_REALTIME in microseconds. A timezone argument, which POSIX
/// leaves unspecified, is filled with UTC.
#[no_mangle]
pub unsafe extern "C" fn gettimeofday(tv: *mut u8, tz: *mut u8) -> i32 {
    if !tv.is_null() {
        let mut now = Timespec { tv_sec: 0, tv_nsec: 0 };
        if crate::clock_gettime(CLOCK_REALTIME, &mut now as *mut _ as *mut u8) != 0 {
            return -1;
        }
        (tv as *mut Timeval).write(Timeval { tv_sec: now.tv_sec, tv_usec: now.tv_nsec / 1000 });
    }
    if !tz.is_null() {
        (tz as *mut [i32; 2]).write([0, 0]);
    }
    0
}

// ---------------------------------------------------------------------------------
// Names
// ---------------------------------------------------------------------------------

/// gethostname: the node name uname reports; ENAMETOOLONG when it and its terminator
/// do not fit (Linux's choice where POSIX leaves truncation unspecified).
#[no_mangle]
pub unsafe extern "C" fn gethostname(name: *mut u8, len: usize) -> i32 {
    let mut uts: crate::utsname = core::mem::zeroed();
    if crate::uname(&mut uts) != 0 {
        return -1;
    }
    let node = &uts.nodename;
    let n = node.iter().position(|&b| b == 0).unwrap_or(node.len());
    if n + 1 > len {
        set_errno(ENAMETOOLONG);
        return -1;
    }
    core::ptr::copy_nonoverlapping(node.as_ptr(), name, n);
    *name.add(n) = 0;
    0
}

#[no_mangle]
pub extern "C" fn getpagesize() -> i32 {
    4096
}

#[no_mangle]
pub extern "C" fn getdtablesize() -> i32 {
    let mut limit = [0u64; 2];
    // RLIMIT_NOFILE is 7.
    if unsafe { crate::getrlimit(7, limit.as_mut_ptr() as *mut _) } == 0 && limit[0] < i32::MAX as u64 {
        limit[0] as i32
    } else {
        1024
    }
}

/// The st_mode and st_rdev of `fd`'s or `path`'s file.
unsafe fn mode_and_rdev(fd: i32, path: *const u8) -> Option<(u32, u64)> {
    let mut st = [0u64; 18];
    let r = if path.is_null() {
        crate::fstat(fd, st.as_mut_ptr() as *mut u8)
    } else {
        crate::stat(path, st.as_mut_ptr() as *mut u8)
    };
    if r != 0 {
        return None;
    }
    let bytes = st.as_ptr() as *const u8;
    #[cfg(target_arch = "aarch64")]
    let (mode, rdev) = (*(bytes.add(16) as *const u32), *(bytes.add(32) as *const u64));
    #[cfg(target_arch = "x86_64")]
    let (mode, rdev) = (*(bytes.add(24) as *const u32), *(bytes.add(40) as *const u64));
    Some((mode, rdev))
}

/// Look in `dir` (a directory path ending in '/') for a character device numbered
/// `rdev`, leaving its full path in `out`.
unsafe fn find_device(dir: &[u8], rdev: u64, out: &mut [u8; 256]) -> bool {
    let mut dirpath = [0u8; 64];
    dirpath[..dir.len()].copy_from_slice(dir);
    // O_RDONLY | O_DIRECTORY | O_CLOEXEC
    #[cfg(target_arch = "aarch64")]
    let flags = 0o40000 | 0o2000000;
    #[cfg(target_arch = "x86_64")]
    let flags = 0o200000 | 0o2000000;
    let fd = crate::open(dirpath.as_ptr(), flags, 0);
    if fd < 0 {
        return false;
    }
    let mut buf = [0u8; 2048];
    let mut found = false;
    'scan: loop {
        let n = crate::getdents64(fd, buf.as_mut_ptr(), buf.len());
        if n <= 0 {
            break;
        }
        let mut off = 0usize;
        while off < n as usize {
            let rec = buf.as_ptr().add(off);
            let reclen = *(rec.add(16) as *const u16) as usize;
            let name = rec.add(19);
            let name_len = c_len(name);
            if reclen == 0 {
                break 'scan;
            }
            off += reclen;
            if dir.len() + name_len + 1 > out.len() || *name == b'.' {
                continue;
            }
            out[..dir.len()].copy_from_slice(dir);
            core::ptr::copy_nonoverlapping(name, out.as_mut_ptr().add(dir.len()), name_len);
            out[dir.len() + name_len] = 0;
            if let Some((mode, dev)) = mode_and_rdev(-1, out.as_ptr()) {
                if mode & S_IFMT == S_IFCHR && dev == rdev {
                    found = true;
                    break 'scan;
                }
            }
        }
    }
    crate::close(fd);
    found
}

/// ttyname_r: the path in /dev/pts or /dev of the terminal open on `fd`.
#[no_mangle]
pub unsafe extern "C" fn ttyname_r(fd: i32, buf: *mut u8, len: usize) -> i32 {
    if crate::isatty(fd) == 0 {
        return crate::pthread::errno();
    }
    let rdev = match mode_and_rdev(fd, null()) {
        Some((mode, rdev)) if mode & S_IFMT == S_IFCHR => rdev,
        Some(_) => return ENOTTY,
        None => return crate::pthread::errno(),
    };
    let mut path = [0u8; 256];
    if !find_device(b"/dev/pts/", rdev, &mut path) && !find_device(b"/dev/", rdev, &mut path) {
        return ENOTTY;
    }
    let n = c_len(path.as_ptr());
    if n + 1 > len {
        return ERANGE;
    }
    core::ptr::copy_nonoverlapping(path.as_ptr(), buf, n + 1);
    0
}

static mut TTYNAME: [u8; 256] = [0; 256];

#[no_mangle]
pub unsafe extern "C" fn ttyname(fd: i32) -> *mut u8 {
    let buf = core::ptr::addr_of_mut!(TTYNAME) as *mut u8;
    match ttyname_r(fd, buf, 256) {
        0 => buf,
        e => {
            set_errno(e);
            null_mut()
        }
    }
}

// ---------------------------------------------------------------------------------
// File times
// ---------------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn utimensat(dirfd: i32, path: *const u8, times: *const u8, flags: i32) -> i32 {
    let r = libbreenix::raw::syscall4(nr::UTIMENSAT, dirfd as i64 as u64, path as u64, times as u64, flags as u64);
    crate::syscall_result_to_c_int(r as i64)
}

#[no_mangle]
pub unsafe extern "C" fn futimens(fd: i32, times: *const u8) -> i32 {
    utimensat(fd, null(), times, 0)
}

/// Microsecond times as nanosecond ones; EINVAL for a microsecond count out of range.
unsafe fn from_timevals(tv: *const Timeval, out: &mut [Timespec; 2]) -> Result<*const u8, ()> {
    if tv.is_null() {
        return Ok(null());
    }
    for (i, slot) in out.iter_mut().enumerate() {
        let t = *tv.add(i);
        if !(0..1_000_000).contains(&t.tv_usec) {
            set_errno(EINVAL);
            return Err(());
        }
        *slot = Timespec { tv_sec: t.tv_sec, tv_nsec: t.tv_usec * 1000 };
    }
    Ok(out.as_ptr() as *const u8)
}

#[no_mangle]
pub unsafe extern "C" fn utimes(path: *const u8, tv: *const u8) -> i32 {
    let mut ts = [Timespec { tv_sec: 0, tv_nsec: 0 }; 2];
    match from_timevals(tv as *const Timeval, &mut ts) {
        Ok(times) => utimensat(AT_FDCWD, path, times, 0),
        Err(()) => -1,
    }
}

#[no_mangle]
pub unsafe extern "C" fn lutimes(path: *const u8, tv: *const u8) -> i32 {
    let mut ts = [Timespec { tv_sec: 0, tv_nsec: 0 }; 2];
    match from_timevals(tv as *const Timeval, &mut ts) {
        Ok(times) => utimensat(AT_FDCWD, path, times, AT_SYMLINK_NOFOLLOW),
        Err(()) => -1,
    }
}

#[no_mangle]
pub unsafe extern "C" fn futimes(fd: i32, tv: *const u8) -> i32 {
    let mut ts = [Timespec { tv_sec: 0, tv_nsec: 0 }; 2];
    match from_timevals(tv as *const Timeval, &mut ts) {
        Ok(times) => utimensat(fd, null(), times, 0),
        Err(()) => -1,
    }
}

/// utime: whole-second times from a struct utimbuf, or the current time for null.
#[no_mangle]
pub unsafe extern "C" fn utime(path: *const u8, times: *const [i64; 2]) -> i32 {
    if times.is_null() {
        return utimensat(AT_FDCWD, path, null(), 0);
    }
    let t = *times;
    let ts = [Timespec { tv_sec: t[0], tv_nsec: 0 }, Timespec { tv_sec: t[1], tv_nsec: 0 }];
    utimensat(AT_FDCWD, path, ts.as_ptr() as *const u8, 0)
}
