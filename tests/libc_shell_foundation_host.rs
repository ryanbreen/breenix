//! Focused host harness: compile the foundation implementations unchanged with
//! scripted syscall responses and an in-memory procfs/resolver configuration.
extern crate self as libbreenix;
use std::{
    cell::{Cell, RefCell, UnsafeCell},
    collections::HashMap,
    ffi::CStr,
};
#[path = "../libs/libbreenix-libc/src/conf.rs"]
mod conf;
#[path = "../libs/libbreenix-libc/src/inet.rs"]
mod inet;
#[path = "../libs/libbreenix-libc/src/netdb.rs"]
mod netdb;
#[path = "../libs/libbreenix-libc/src/process.rs"]
mod process;
#[path = "../libs/libbreenix-libc/src/sysinfo.rs"]
mod sysinfo;
thread_local! {
    static ERRNO: UnsafeCell<i32> = const { UnsafeCell::new(0) };
    static H_ERRNO: UnsafeCell<i32> = const { UnsafeCell::new(0) };
    static FILES: RefCell<HashMap<Vec<u8>, Vec<u8>>> = RefCell::new(HashMap::new());
    static OPEN: RefCell<HashMap<i32, (Vec<u8>, usize)>> = RefCell::new(HashMap::new());
    static REPLIES: RefCell<std::collections::VecDeque<i64>> = const { RefCell::new(std::collections::VecDeque::new()) };
    static CALLS: RefCell<Vec<(u64, Vec<u64>)>> = const { RefCell::new(Vec::new()) };
    static STAT_DEV: Cell<u64> = const { Cell::new(1) };
    static STAT_ERROR: Cell<i32> = const { Cell::new(0) };
    static READ_ERROR: Cell<i32> = const { Cell::new(0) };
}
fn __errno_location() -> *mut i32 {
    ERRNO.with(|e| e.get())
}
mod pthread {
    pub fn set_errno(e: i32) {
        unsafe {
            *crate::__errno_location() = e;
        }
    }
    pub fn errno() -> i32 {
        unsafe { *crate::__errno_location() }
    }
    pub unsafe fn pthread_key_create(
        key: *mut u32,
        _: Option<unsafe extern "C" fn(*mut u8)>,
    ) -> i32 {
        *key = 1;
        0
    }
    pub unsafe fn pthread_once(once: *mut i32, init: unsafe extern "C" fn()) -> i32 {
        if *once == 0 {
            init();
            *once = 1;
        }
        0
    }
    pub unsafe fn pthread_getspecific(_: u32) -> *mut u8 {
        std::ptr::null_mut()
    }
    pub unsafe fn pthread_setspecific(_: u32, _: *const u8) -> i32 {
        0
    }
    pub fn __h_errno_location() -> *mut i32 {
        crate::H_ERRNO.with(|e| e.get())
    }
    pub fn processor_count() -> i64 {
        4
    }
}
#[repr(C)]
struct rlimit {
    rlim_cur: u64,
    rlim_max: u64,
}
unsafe fn getrlimit(resource: i32, out: *mut rlimit) -> i32 {
    // NOFILE 256; NPROC unlimited.
    (*out).rlim_cur = if resource == 7 { 256 } else { u64::MAX };
    (*out).rlim_max = (*out).rlim_cur;
    0
}
pub mod errno {
    #[repr(i32)]
    pub enum Errno {
        ENOENT = 2,
        EINTR = 4,
        ENOMEM = 12,
        EACCES = 13,
        EFAULT = 14,
        ENOSPC = 28,
        EAFNOSUPPORT = 97,
    }
}
pub mod syscall {
    pub mod nr {
        pub const GETPGID: u64 = 155;
        pub const SETSID: u64 = 157;
    }
}
pub mod raw {
    unsafe fn run(n: u64, args: &[u64]) -> u64 {
        crate::CALLS.with(|c| c.borrow_mut().push((n, args.to_vec())));
        crate::REPLIES.with(|r| r.borrow_mut().pop_front().expect("unscripted syscall")) as u64
    }
    pub unsafe fn syscall0(n: u64) -> u64 {
        run(n, &[])
    }
    pub unsafe fn syscall1(n: u64, a: u64) -> u64 {
        run(n, &[a])
    }
    pub unsafe fn syscall2(n: u64, a: u64, b: u64) -> u64 {
        run(n, &[a, b])
    }
    pub unsafe fn syscall3(n: u64, a: u64, b: u64, c: u64) -> u64 {
        run(n, &[a, b, c])
    }
}
pub mod dns {
    pub enum DnsError {
        SocketError,
        BindError,
        SendError,
        RecvError,
        Timeout,
        ParseError,
        ServerError(u8),
        NoAddress,
        HostnameTooLong,
        InvalidHostname,
    }
    pub struct Answer {
        pub addr: [u8; 4],
    }
    pub struct Reply {
        pub answers: [Answer; 1],
        pub len: usize,
        pub canonical: [u8; 256],
        pub canonical_len: usize,
    }
    pub fn resolve_all(name: &str, _: [u8; 4]) -> Result<Reply, DnsError> {
        if name == "www.example" {
            let mut canonical = [0; 256];
            canonical[..12].copy_from_slice(b"real.example");
            return Ok(Reply { answers: [Answer { addr: [198, 51, 100, 7] }], len: 1, canonical, canonical_len: 12 });
        }
        Err(match name {
            "temporary.example" => DnsError::Timeout,
            "noaddress.example" => DnsError::NoAddress,
            _ => DnsError::ServerError(3),
        })
    }
}
#[repr(C)]
struct Utsname {
    nodename: [u8; 65],
}
// netdb uses the production spelling for this ABI type.
use Utsname as utsname;
unsafe fn uname(out: *mut utsname) -> i32 {
    (*out).nodename = [0; 65];
    0
}
fn syscall_result_to_c_int(r: i64) -> i32 {
    if r < 0 {
        pthread::set_errno(-r as i32);
        -1
    } else {
        r as i32
    }
}
fn kill(pid: i32, sig: i32) -> i32 {
    unsafe { syscall_result_to_c_int(raw::syscall2(129, pid as i64 as u64, sig as u64) as i64) }
}
unsafe fn stat(_: *const u8, out: *mut u8) -> i32 {
    out.cast::<u64>().write(STAT_DEV.with(Cell::get));
    status()
}
unsafe fn fstat(_: i32, out: *mut u8) -> i32 {
    out.cast::<u64>().write(STAT_DEV.with(Cell::get));
    status()
}
mod statfs {
    /// struct statfs with a 1 KiB block size.
    pub unsafe fn statfs(_: *const u8, buf: *mut u8) -> i32 {
        *buf.cast::<[i64; 2]>() = [0xef53, 1024];
        0
    }
    pub unsafe fn fstatfs(_: i32, buf: *mut u8) -> i32 {
        statfs(std::ptr::null(), buf)
    }
}
fn status() -> i32 {
    STAT_ERROR.with(|e| {
        if e.get() == 0 {
            0
        } else {
            pthread::set_errno(e.get());
            -1
        }
    })
}
unsafe fn open(path: *const u8, _: i32, _: u32) -> i32 {
    let key = CStr::from_ptr(path.cast()).to_bytes().to_vec();
    if !FILES.with(|f| f.borrow().contains_key(&key)) {
        pthread::set_errno(2);
        return -1;
    }
    OPEN.with(|o| {
        let mut o = o.borrow_mut();
        let fd = o.len() as i32 + 3;
        o.insert(fd, (key, 0));
        fd
    })
}
unsafe fn read(fd: i32, dest: *mut u8, cap: usize) -> isize {
    let error = READ_ERROR.with(|e| e.replace(0));
    if error != 0 {
        pthread::set_errno(error);
        return -1;
    }
    OPEN.with(|o| {
        let mut o = o.borrow_mut();
        let (key, offset) = o.get_mut(&fd).unwrap();
        FILES.with(|f| {
            let f = f.borrow();
            let bytes = &f[key];
            let n = cap.min(13).min(bytes.len() - *offset);
            std::ptr::copy_nonoverlapping(bytes[*offset..].as_ptr(), dest, n);
            *offset += n;
            n as isize
        })
    })
}
unsafe fn close(fd: i32) -> i32 {
    OPEN.with(|o| o.borrow_mut().remove(&fd));
    0
}
unsafe fn malloc(n: usize) -> *mut u8 {
    std::alloc::alloc(std::alloc::Layout::from_size_align(n, 16).unwrap())
}
unsafe fn calloc(n: usize, size: usize) -> *mut u8 {
    let p = malloc(n * size);
    p.write_bytes(0, n * size);
    p
}
unsafe fn free(p: *mut u8) {
    // getaddrinfo is not called by these legacy-resolver tests; no allocated result escapes.
    assert!(p.is_null());
}
fn reset() {
    REPLIES.with(|r| r.borrow_mut().clear());
    CALLS.with(|c| c.borrow_mut().clear());
    FILES.with(|f| f.borrow_mut().clear());
    OPEN.with(|o| o.borrow_mut().clear());
    pthread::set_errno(77);
    STAT_ERROR.with(|e| e.set(0));
}
fn replies(r: &[i64]) {
    REPLIES.with(|q| q.borrow_mut().extend(r));
}
fn file(name: &str, bytes: &[u8]) {
    FILES.with(|f| {
        f.borrow_mut()
            .insert(name.as_bytes().to_vec(), bytes.to_vec())
    });
}
#[test]
fn groups_and_errno() {
    reset();
    replies(&[7, 8, 9, -3, 0, 0]);
    assert_eq!(process::getpgid(42), 7);
    assert_eq!(process::getpgrp(), 8);
    assert_eq!(process::setsid(), 9);
    assert_eq!(process::getpgid(-1), -1);
    assert_eq!(pthread::errno(), 3);
    assert_eq!(process::killpg(7, 15), 0);
    assert_eq!(process::killpg(0, 0), 0);
    assert_eq!(process::killpg(1, 15), -1);
    assert_eq!(process::killpg(i32::MIN, 15), -1);
    assert_eq!(pthread::errno(), 22);
    CALLS.with(|c| {
        let c = c.borrow();
        assert_eq!(c[0], (155, vec![42]));
        assert_eq!(c[1], (155, vec![0]));
        assert_eq!(c[3].1, vec![u64::MAX]);
        assert_eq!(c[4].1, vec![(-7i64) as u64, 15]);
        assert_eq!(c[5].1, vec![0, 0]);
    });
}
#[test]
fn priority_bias_nice_minus_one_and_permission() {
    reset();
    replies(&[40, 1, 21, -3]);
    assert_eq!(process::getpriority(0, 0), -20);
    assert_eq!(process::getpriority(1, 7), 19);
    assert_eq!(process::getpriority(2, 8), -1);
    assert_eq!(pthread::errno(), 77);
    assert_eq!(process::getpriority(0, 999), -1);
    assert_eq!(pthread::errno(), 3);
    pthread::set_errno(77);
    replies(&[21, 0, 21]);
    assert_eq!(process::nice(0), -1);
    assert_eq!(pthread::errno(), 77);
    replies(&[20, 0, 1]);
    assert_eq!(process::nice(i32::MAX), 19);
    CALLS.with(|c| assert_eq!(c.borrow()[8].1, vec![0, 0, 19]));
    replies(&[20, -13]);
    assert_eq!(process::nice(-1), -1);
    assert_eq!(pthread::errno(), 1);
    replies(&[-22]);
    assert_eq!(process::setpriority(3, 0, 1), -1);
    assert_eq!(pthread::errno(), 22);
}
#[test]
fn configuration_truncation_invalid_queries_and_bad_objects() {
    unsafe {
        reset();
        assert_eq!(conf::confstr(0, std::ptr::null_mut(), 0), 14);
        assert_eq!(conf::confstr(0, std::ptr::null_mut(), 1), 14);
        assert_eq!(pthread::errno(), 77);
        let mut bytes = [0xaa; 20];
        assert_eq!(conf::confstr(0, bytes.as_mut_ptr(), 5), 14);
        assert_eq!(&bytes[..5], b"/bin\0");
        assert_eq!(bytes[5], 0xaa);
        assert_eq!(conf::confstr(0, bytes.as_mut_ptr(), 20), 14);
        assert_eq!(
            CStr::from_ptr(bytes.as_ptr().cast()).to_bytes(),
            b"/bin:/usr/bin"
        );
        assert_eq!(conf::confstr(99, bytes.as_mut_ptr(), 20), 0);
        assert_eq!(pthread::errno(), 22);
        // The LP64 environment's flags are empty; ILP32's have no value.
        pthread::set_errno(77);
        assert_eq!(conf::confstr(1168, bytes.as_mut_ptr(), 20), 1);
        assert_eq!(bytes[0], 0);
        assert_eq!(conf::confstr(1160, bytes.as_mut_ptr(), 20), 0);
        assert_eq!(pthread::errno(), 77);
        assert_eq!(conf::confstr(1151, bytes.as_mut_ptr(), 20), 20);
        assert_eq!(&bytes[..20], b"POSIX_V8_LP64_OFF64\0");
        assert_eq!(conf::system_value(4), 256);
        assert_eq!(conf::system_value(1), -1);
        assert_eq!(conf::system_value(29), 202405);
        assert_eq!(conf::system_value(7), 1);
        assert_eq!(conf::system_value(84), 4);
        assert_eq!(conf::system_value(252), 1);
        assert_eq!(conf::system_value(250), -1);
        assert_eq!(pthread::errno(), 77);
        assert_eq!(conf::system_value(9999), -1);
        assert_eq!(pthread::errno(), 22);
        assert_eq!(conf::pathconf(b"/\0".as_ptr(), 0), 32000);
        assert_eq!(conf::fpathconf(3, 3), 255);
        assert_eq!(conf::fpathconf(3, 5), 4096);
        assert_eq!(conf::fpathconf(3, 19), 1023);
        assert_eq!(conf::pathconf(b"/\0".as_ptr(), 13), 64);
        assert_eq!(conf::fpathconf(3, 99), -1);
        assert_eq!(pthread::errno(), 22);
        STAT_ERROR.with(|e| e.set(9));
        assert_eq!(conf::fpathconf(-1, 3), -1);
        assert_eq!(pthread::errno(), 9);
        STAT_ERROR.with(|e| e.set(2));
        assert_eq!(conf::pathconf(b"/missing\0".as_ptr(), 3), -1);
        assert_eq!(pthread::errno(), 2);
    }
}
#[test]
fn configuration_distinguishes_pseudo_file_systems_and_terminal_lines() {
    unsafe {
        reset();
        // MAX_CANON and MAX_INPUT are the line discipline's buffer.
        assert_eq!(conf::fpathconf(3, 1), 4096);
        assert_eq!(conf::fpathconf(3, 2), 4095);
        for dev in [0xffff0002, 0xffff0004] {
            STAT_DEV.with(|d| d.set(dev));
            // ext2's link, size, block, symlink and timestamp values do not
            // apply to procfs or devfs: valid names, indeterminate values.
            for name in [0, 13, 18, 19, 20, 21] {
                assert_eq!(conf::pathconf(b"/\0".as_ptr(), name), -1);
                assert_eq!(pthread::errno(), 77);
            }
            assert_eq!(conf::fpathconf(3, 3), 255);
        }
        STAT_DEV.with(|d| d.set(1));
        assert_eq!(conf::fpathconf(3, 0), 32000);
        assert_eq!(conf::fpathconf(3, 18), 1024);
        assert_eq!(conf::fpathconf(3, 20), 1);
        assert_eq!(conf::fpathconf(3, 21), 1_000_000_000);
        // POSIX.1-2024's TEXTDOMAIN_MAX (no limit) and _POSIX_FALLOC (no
        // posix_fallocate): valid names, -1 with errno unchanged.
        for name in [22, 23] {
            assert_eq!(conf::fpathconf(3, name), -1);
            assert_eq!(pthread::errno(), 77);
        }
    }
}

#[test]
fn sysinfo_is_the_kernel_call_with_linux_layout() {
    unsafe {
        reset();
        let mut info: sysinfo::SysInfo = std::mem::zeroed();
        assert_eq!(std::mem::size_of_val(&info), 112);
        assert_eq!(std::mem::offset_of!(sysinfo::SysInfo, procs), 80);
        assert_eq!(std::mem::offset_of!(sysinfo::SysInfo, mem_unit), 104);
        // No procfs file or descriptor is involved: only the syscall.
        replies(&[0, -14]);
        assert_eq!(sysinfo::sysinfo(&mut info), 0);
        assert_eq!(pthread::errno(), 77);
        assert_eq!(sysinfo::sysinfo(8 as *mut sysinfo::SysInfo), -1);
        assert_eq!(pthread::errno(), 14);
        let number = if cfg!(target_arch = "aarch64") { 179 } else { 99 };
        CALLS.with(|c| {
            assert_eq!(*c.borrow(), vec![(number, vec![&mut info as *mut _ as u64]), (number, vec![8])])
        });
        OPEN.with(|o| assert!(o.borrow().is_empty()));
    }
}
#[test]
fn legacy_hostname_numeric_hosts_and_errors() {
    unsafe {
        reset();
        file(
            "/etc/hosts",
            b"192.0.2.1 server alias other\n192.0.2.2 server alias\n192.0.2.1 server alias\n",
        );
        let host = netdb::gethostbyname(b"alias\0".as_ptr());
        assert!(!host.is_null());
        assert_eq!(CStr::from_ptr((*host).name.cast()).to_bytes(), b"server");
        assert_eq!(CStr::from_ptr((*(*host).aliases).cast()).to_bytes(), b"alias");
        assert_eq!(CStr::from_ptr((*(*host).aliases.add(1)).cast()).to_bytes(), b"other");
        assert!((*(*host).aliases.add(2)).is_null());
        assert_eq!((*host).addrtype, 2);
        assert_eq!((*host).length, 4);
        assert_eq!(*(*(*host).addr_list).cast::<[u8; 4]>(), [192, 0, 2, 1]);
        assert_eq!(
            *(*(*host).addr_list.add(1)).cast::<[u8; 4]>(),
            [192, 0, 2, 2]
        );
        assert!((*(*host).addr_list.add(2)).is_null());
        let alias = |host: *mut netdb::HostEnt, i: usize| {
            let p = *(*host).aliases.add(i);
            (!p.is_null()).then(|| CStr::from_ptr(p.cast()).to_bytes().to_vec())
        };
        // A retried read still parses every line; the canonical name is never its own alias.
        file("/etc/hosts", b"# names\n192.0.2.9 box box.local loghost box\n");
        READ_ERROR.with(|e| e.set(4));
        let host = netdb::gethostbyname(b"box\0".as_ptr());
        assert_eq!(CStr::from_ptr((*host).name.cast()).to_bytes(), b"box");
        assert_eq!(alias(host, 0).as_deref(), Some(b"box.local".as_slice()));
        assert_eq!(alias(host, 1).as_deref(), Some(b"loghost".as_slice()));
        assert_eq!(alias(host, 2), None);
        let host = netdb::gethostbyname(b"www.example\0".as_ptr());
        assert_eq!(CStr::from_ptr((*host).name.cast()).to_bytes(), b"real.example");
        assert_eq!(alias(host, 0).as_deref(), Some(b"www.example".as_slice()));
        assert_eq!(alias(host, 1), None);
        let host = netdb::gethostbyname(b"127.0.0.1\0".as_ptr());
        assert_eq!(*(*(*host).addr_list).cast::<[u8; 4]>(), [127, 0, 0, 1]);
        assert!((*(*host).aliases).is_null());
        assert!(netdb::gethostbyname(b"missing.invalid\0".as_ptr()).is_null());
        assert_eq!(*pthread::__h_errno_location(), 1);
        assert!(netdb::gethostbyname(b"temporary.example\0".as_ptr()).is_null());
        assert_eq!(*pthread::__h_errno_location(), 2);
        assert!(netdb::gethostbyname(b"noaddress.example\0".as_ptr()).is_null());
        assert_eq!(*pthread::__h_errno_location(), 4);
        let mut crowded = b"192.0.2.5 crowded".to_vec();
        for i in 0..70 {
            crowded.extend_from_slice(format!(" n{i}").as_bytes());
        }
        crowded.push(b'\n');
        file("/etc/hosts", &crowded);
        assert!(netdb::gethostbyname(b"crowded\0".as_ptr()).is_null());
        assert_eq!(*pthread::__h_errno_location(), 3);
        assert_eq!(pthread::errno(), 12);
        pthread::set_errno(77);
        assert!(!CStr::from_ptr(netdb::hstrerror(2).cast())
            .to_bytes()
            .is_empty());
        assert_eq!(pthread::errno(), 77);
    }
}
#[test]
fn hostname_setter_returns_enosys_without_syscall() {
    unsafe {
        reset();
        assert_eq!(process::sethostname(b"new\0".as_ptr(), 3), -1);
        assert_eq!(pthread::errno(), 38);
        CALLS.with(|c| assert!(c.borrow().is_empty()));
    }
}

#[test]
fn v8_configuration_environments() {
    unsafe {
        reset();
        let mut buffer = [0xff; 64];
        // _CS_POSIX_V8_WIDTH_RESTRICTED_ENVS and the V7 name (Linux's 5).
        assert_eq!(conf::confstr(1151, buffer.as_mut_ptr(), buffer.len()), 20);
        assert_eq!(CStr::from_ptr(buffer.as_ptr().cast()).to_bytes(), b"POSIX_V8_LP64_OFF64");
        assert_eq!(conf::confstr(5, buffer.as_mut_ptr(), buffer.len()), 20);
        assert_eq!(CStr::from_ptr(buffer.as_ptr().cast()).to_bytes(), b"POSIX_V7_LP64_OFF64");
        // LP64_OFF64 CFLAGS/LDFLAGS/LIBS of both versions and the thread flags are empty.
        for name in [1168, 1169, 1170, 1140, 1141, 1142, 1152, 1153, 1154, 1155, 1149, 1150] {
            assert_eq!(conf::confstr(name, buffer.as_mut_ptr(), 1), 1);
            assert_eq!(buffer[0], 0);
        }
        // ILP32 and LPBIG environments are valid names with no value.
        for name in [1160, 1161, 1162, 1164, 1172, 1132, 1136, 1144] {
            assert_eq!(conf::confstr(name, buffer.as_mut_ptr(), 64), 0);
            assert_eq!(pthread::errno(), 77);
        }
    }
}
