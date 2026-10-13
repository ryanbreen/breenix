//! Guest regression for disk-backed FIFOs and real directory descriptors.
use libbreenix::{
    fs::Stat,
    syscall::{nr, raw},
};
use std::ffi::CString;
const CWD: u64 = (-100i64) as u64;
const RDWR: u64 = 2;
const NONBLOCK: u64 = 0x800;
const DIRECTORY: u64 = libbreenix::fs::O_DIRECTORY as u64;
extern "C" {
    fn mkfifoat(fd: i32, path: *const u8, mode: u32) -> i32;
    fn __errno_location() -> *mut i32;
}
fn path(name: &str) -> CString {
    CString::new(name).unwrap()
}
fn open(fd: u64, name: &str, flags: u64) -> i64 {
    let p = path(name);
    unsafe { raw::syscall4(nr::OPENAT, fd, p.as_ptr() as u64, flags, 0) as i64 }
}
fn unlink(fd: u64, name: &str, flags: u64) -> i64 {
    let p = path(name);
    unsafe { raw::syscall3(nr::UNLINKAT, fd, p.as_ptr() as u64, flags) as i64 }
}
fn node(fd: u64, name: &str, mode: u64) -> i64 {
    node_dev(fd, name, mode, 0)
}
fn node_dev(fd: u64, name: &str, mode: u64, dev: u64) -> i64 {
    let p = path(name);
    unsafe { raw::syscall4(nr::MKNODAT, fd, p.as_ptr() as u64, mode, dev) as i64 }
}
fn stat(name: &str) -> Stat {
    let p = path(name);
    let mut st = Stat::default();
    assert_eq!(
        unsafe {
            raw::syscall4(
                nr::NEWFSTATAT,
                CWD,
                p.as_ptr() as u64,
                &mut st as *mut _ as u64,
                0,
            ) as i64
        },
        0
    );
    st
}
fn fstat(fd: u64) -> Stat {
    let mut st = Stat::default();
    assert_eq!(
        unsafe { raw::syscall2(nr::FSTAT, fd, &mut st as *mut _ as u64) as i64 },
        0
    );
    st
}
fn close(fd: u64) {
    assert_eq!(unsafe { raw::syscall1(nr::CLOSE, fd) as i64 }, 0)
}
fn main() {
    let pid = unsafe { raw::syscall0(nr::GETPID) };
    let base = format!("/tmp/fifo-fs-{pid}");
    libbreenix::fs::mkdir(&base, 0o777).unwrap();
    let dir = open(CWD, &base, DIRECTORY);
    assert!(dir >= 0);
    let dir = dir as u64;
    let fifo = path("fifo");
    assert_eq!(
        unsafe { mkfifoat(-1, fifo.as_ptr() as *const u8, 0o600) },
        -1
    );
    assert_eq!(unsafe { *__errno_location() }, 9);
    assert_eq!(
        unsafe { mkfifoat(dir as i32, fifo.as_ptr() as *const u8, 0o100640) },
        0
    );
    let absolute = format!("{base}/absolute");
    let absolute_c = path(&absolute);
    assert_eq!(
        unsafe { mkfifoat(-1, absolute_c.as_ptr() as *const u8, 0o600) },
        0
    );
    assert_eq!(unlink(CWD, &absolute, 0), 0);
    assert_eq!(node_dev(dir, "null-alias", 0o020600, 0x103), 0);
    let null = open(dir, "null-alias", RDWR);
    assert!(null >= 0);
    let by_path = stat(&format!("{base}/null-alias"));
    let by_fd = fstat(null as u64);
    assert_eq!(
        (by_fd.st_dev, by_fd.st_ino),
        (by_path.st_dev, by_path.st_ino)
    );
    assert_eq!(by_fd.st_rdev, 0x103);
    assert_eq!(unlink(dir, "null-alias", 0), 0);
    assert_eq!(fstat(null as u64).st_nlink, 0);
    let mut byte = [0u8; 1];
    assert_eq!(
        unsafe { raw::syscall3(nr::READ, null as u64, byte.as_mut_ptr() as u64, 1) as i64 },
        0
    );
    close(null as u64);
    assert_eq!(node_dev(dir, "block-alias", 0o060600, 0x103), 0);
    assert_eq!(open(dir, "block-alias", 0), -6);
    assert_eq!(unlink(dir, "block-alias", 0), 0);
    assert_eq!(node(dir, "socket-node", 0o140600), 0);
    assert_eq!(open(dir, "socket-node", 0), -6);
    assert_eq!(unlink(dir, "socket-node", 0), 0);
    assert_eq!(node(dir, "regular", 0o100600), 0);
    let regular = open(dir, "regular", RDWR);
    assert!(regular >= 0);
    assert_eq!(
        unsafe { mkfifoat(regular as i32, fifo.as_ptr() as *const u8, 0o600) },
        -1
    );
    assert_eq!(unsafe { *__errno_location() }, 20);
    assert_eq!(node(dir, "directory", 0o040700), -1);
    assert_eq!(node(dir, "invalid", 0o170600), -22);
    let st = stat(&format!("{base}/fifo"));
    assert_eq!(st.st_mode & 0o170000, 0o010000);
    assert_eq!(st.st_mode & 0o777, 0o640);
    assert_eq!(open(dir, "fifo/", NONBLOCK), -20);
    assert_eq!(unlink(dir, "fifo/", 0), -20);
    let slash = path(&format!("{base}/fifo/"));
    let mut slash_stat = Stat::default();
    assert_eq!(
        unsafe {
            raw::syscall4(
                nr::NEWFSTATAT,
                CWD,
                slash.as_ptr() as u64,
                &mut slash_stat as *mut _ as u64,
                0,
            ) as i64
        },
        -20
    );
    assert_eq!(
        unsafe { raw::syscall4(nr::FACCESSAT, CWD, slash.as_ptr() as u64, 0, 0) as i64 },
        -20
    );
    let mut records = [0u8; 4096];
    let count = unsafe {
        raw::syscall3(
            nr::GETDENTS64,
            dir,
            records.as_mut_ptr() as u64,
            records.len() as u64,
        )
    } as i64;
    assert!(count > 0);
    let mut cursor = 0;
    let mut found = false;
    while cursor < count as usize {
        let len = u16::from_ne_bytes([records[cursor + 16], records[cursor + 17]]) as usize;
        assert!(len >= 20);
        let end = records[cursor + 19..cursor + len]
            .iter()
            .position(|b| *b == 0)
            .unwrap()
            + cursor
            + 19;
        if &records[cursor + 19..end] == b"fifo" {
            assert_eq!(records[cursor + 18], 1);
            assert_eq!(
                u64::from_ne_bytes(records[cursor..cursor + 8].try_into().unwrap()),
                st.st_ino
            );
            found = true;
        }
        cursor += len;
    }
    assert!(found);
    let blocking = open(dir, "fifo", RDWR);
    assert!(blocking >= 0);
    close(blocking as u64);
    let endpoint = open(dir, "fifo", RDWR | NONBLOCK);
    assert!(endpoint >= 0);
    let mut capped = vec![0u8; 3];
    assert_eq!(
        unsafe { raw::syscall3(nr::WRITE, endpoint as u64, b"cap".as_ptr() as u64, 3) as i64 },
        3
    );
    assert_eq!(
        unsafe {
            raw::syscall3(
                nr::READ,
                endpoint as u64,
                capped.as_mut_ptr() as u64,
                1 << 30,
            ) as i64
        },
        3
    );
    assert_eq!(&capped, b"cap");
    let endpoint = endpoint as u64;
    assert_eq!(fstat(endpoint).st_ino, st.st_ino);
    assert_eq!(fstat(endpoint).st_dev, st.st_dev);
    let moved = format!("{base}-moved");
    libbreenix::fs::rename(&base, &moved).unwrap();
    let alias = format!("{moved}/alias");
    let full = format!("{moved}/fifo");
    libbreenix::fs::link(&full, &alias).unwrap();
    assert_eq!(stat(&alias).st_ino, st.st_ino);
    assert_eq!(fstat(endpoint).st_nlink, 2);
    let renamed = format!("{moved}/renamed");
    libbreenix::fs::rename(&full, &renamed).unwrap();
    assert_eq!(open(dir, "fifo", NONBLOCK), -2);
    let dup = unsafe { raw::syscall1(nr::DUP, endpoint) } as i64;
    assert!(dup >= 0);
    assert_eq!(
        unsafe { raw::syscall3(nr::WRITE, endpoint, b"old".as_ptr() as u64, 3) },
        3
    );
    close(endpoint);
    let mut data = [0; 3];
    assert_eq!(
        unsafe { raw::syscall3(nr::READ, dup as u64, data.as_mut_ptr() as u64, 3) },
        3
    );
    assert_eq!(&data, b"old");
    assert_eq!(unlink(dir, "renamed", 0), 0);
    assert_eq!(fstat(dup as u64).st_nlink, 1);
    assert_eq!(unlink(dir, "alias", 0), 0);
    assert_eq!(fstat(dup as u64).st_nlink, 0);
    assert_eq!(node(dir, "renamed", 0o010600), 0);
    let replacement = open(dir, "renamed", RDWR | NONBLOCK);
    assert!(replacement >= 0);
    assert_ne!(fstat(replacement as u64).st_ino, st.st_ino);
    assert_eq!(
        unsafe { raw::syscall3(nr::READ, replacement as u64, data.as_mut_ptr() as u64, 3) as i64 },
        -11
    );
    close(dup as u64);
    close(replacement as u64);
    let reopened = open(dir, "renamed", RDWR | NONBLOCK);
    assert!(reopened >= 0);
    assert_eq!(
        unsafe { raw::syscall3(nr::READ, reopened as u64, data.as_mut_ptr() as u64, 3) as i64 },
        -11
    );
    close(reopened as u64);
    close(regular as u64);
    assert_eq!(unlink(dir, "regular", 0), 0);
    libbreenix::fs::mkdir(&format!("{moved}/subdir"), 0o700).unwrap();
    assert_eq!(unlink(dir, "subdir", 0x200), 0);
    close(dir);
    assert_eq!(unlink(CWD, &moved, 0x200), -39);
    let dir = open(CWD, &moved, DIRECTORY);
    assert!(dir >= 0);
    assert_eq!(unlink(dir as u64, "renamed", 0), 0);
    close(dir as u64);
    assert_eq!(unlink(CWD, &moved, 0x200), 0);
    println!("FIFO_FS PASS");
}
