//! File system statistics: statfs and fstatfs over the Breenix kernel's statfs
//! system calls, and POSIX.1-2024 statvfs and fstatvfs (<sys/statvfs.h>) built on them.

mod nr {
    #[cfg(target_arch = "aarch64")]
    pub const STATFS: u64 = 43;
    #[cfg(target_arch = "aarch64")]
    pub const FSTATFS: u64 = 44;
    #[cfg(target_arch = "x86_64")]
    pub const STATFS: u64 = 137;
    #[cfg(target_arch = "x86_64")]
    pub const FSTATFS: u64 = 138;
}

/// The kernel's struct statfs: fifteen 64-bit words on both architectures.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Statfs {
    kind: i64,
    bsize: i64,
    blocks: u64,
    bfree: u64,
    bavail: u64,
    files: u64,
    ffree: u64,
    fsid: [i32; 2],
    namelen: i64,
    frsize: i64,
    flags: i64,
    spare: [i64; 4],
}

/// POSIX struct statvfs (include/sys/statvfs.h).
#[repr(C)]
struct Statvfs {
    f_bsize: u64,
    f_frsize: u64,
    f_blocks: u64,
    f_bfree: u64,
    f_bavail: u64,
    f_files: u64,
    f_ffree: u64,
    f_favail: u64,
    f_fsid: u64,
    f_flag: u64,
    f_namemax: u64,
}

/// kernel/src/syscall/fs.rs sets bit 5 when filling statfs flags.
/// It is kernel metadata, so the POSIX statvfs flags omit it.
const ST_VALID: i64 = 0x20;

#[no_mangle]
pub unsafe extern "C" fn statfs(path: *const u8, buf: *mut u8) -> i32 {
    let r = libbreenix::raw::syscall2(nr::STATFS, path as u64, buf as u64);
    crate::syscall_result_to_c_int(r as i64)
}

#[no_mangle]
pub unsafe extern "C" fn fstatfs(fd: i32, buf: *mut u8) -> i32 {
    let r = libbreenix::raw::syscall2(nr::FSTATFS, fd as i64 as u64, buf as u64);
    crate::syscall_result_to_c_int(r as i64)
}

fn to_statvfs(s: &Statfs) -> Statvfs {
    Statvfs {
        f_bsize: s.bsize as u64,
        f_frsize: if s.frsize != 0 { s.frsize } else { s.bsize } as u64,
        f_blocks: s.blocks,
        f_bfree: s.bfree,
        f_bavail: s.bavail,
        f_files: s.files,
        f_ffree: s.ffree,
        f_favail: s.ffree,
        f_fsid: (s.fsid[0] as u32 as u64) | ((s.fsid[1] as u32 as u64) << 32),
        f_flag: (s.flags & !ST_VALID) as u64,
        f_namemax: s.namelen as u64,
    }
}

#[no_mangle]
pub unsafe extern "C" fn statvfs(path: *const u8, buf: *mut u8) -> i32 {
    let mut s = Statfs::default();
    if statfs(path, &mut s as *mut _ as *mut u8) != 0 {
        return -1;
    }
    (buf as *mut Statvfs).write(to_statvfs(&s));
    0
}

#[no_mangle]
pub unsafe extern "C" fn fstatvfs(fd: i32, buf: *mut u8) -> i32 {
    let mut s = Statfs::default();
    if fstatfs(fd, &mut s as *mut _ as *mut u8) != 0 {
        return -1;
    }
    (buf as *mut Statvfs).write(to_statvfs(&s));
    0
}
