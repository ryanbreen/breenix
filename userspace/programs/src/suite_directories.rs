//! Directories & links: POSIX assertions and explicitly named implementation checks.
//! The shared runner forks each case and enforces its default 10-second deadline.
//! Unsupported operations fail their cases; ext2 and Linux ABI checks are named.
use libbreenix::error::Error;
use libbreenix::suite::{case, category, check, fail, suite, CaseResult, Suite};
use libbreenix::syscall::{nr, raw};
use libbreenix::types::Fd;
use libbreenix::{
    fs::{self, *},
    io, process, time,
};
use std::cell::RefCell;

#[cfg(target_arch = "x86_64")]
const O_NOFOLLOW: u32 = 0x20000;
#[cfg(target_arch = "aarch64")]
const O_NOFOLLOW: u32 = 0x8000;
const O_NONBLOCK: u32 = 0x800;
const AT_FDCWD: u64 = (-100i64) as u64;
const UTIME_NOW: i64 = 1073741823;
const UTIME_OMIT: i64 = 1073741822;
#[cfg(target_arch = "x86_64")]
const ABI: [u64; 6] = [268, 260, 95, 105, 106, 280];
#[cfg(target_arch = "aarch64")]
const ABI: [u64; 6] = [53, 54, 166, 146, 144, 88];

fn request(n: u64, args: [u64; 4]) -> Result<u64, Error> {
    // SAFETY: callers retain NUL-terminated paths and ABI buffers through the call.
    let r = unsafe { raw::syscall4(n, args[0], args[1], args[2], args[3]) };
    Error::from_syscall(r as i64).map(|v| v as u64)
}
fn errno(result: i64, expected: i32) -> CaseResult {
    errno_one_of(result, &[expected])
}
fn errno_one_of(result: i64, expected: &[i32]) -> CaseResult {
    check(
        result < 0 && expected.iter().any(|n| result == -(*n as i64)),
        &format!("expected errno {expected:?}, got raw return {result}"),
    )
}
// Error assertions retain the raw return: Error::from_syscall loses unknown errno.
mod observed {
    use super::*;
    fn call(n: u64, args: [u64; 4]) -> i64 {
        // SAFETY: adapters retain their path/buffer storage through the syscall.
        unsafe { raw::syscall4(n, args[0], args[1], args[2], args[3]) as i64 }
    }
    pub fn mkdir(path: &str, mode: u32) -> i64 {
        let cpath = cpath(path);
        // SAFETY: NUL-terminated paths and mutable buffers remain alive through the call.
        let ret = unsafe {
            #[cfg(target_arch = "x86_64")]
            {
                raw::syscall2(nr::MKDIR, cpath.as_ptr() as u64, mode as u64) as i64
            }
            #[cfg(target_arch = "aarch64")]
            {
                raw::syscall3(nr::MKDIRAT, AT_FDCWD, cpath.as_ptr() as u64, mode as u64) as i64
            }
        };
        ret
    }
    pub fn rmdir(path: &str) -> i64 {
        let cpath = cpath(path);
        // SAFETY: NUL-terminated paths and mutable buffers remain alive through the call.
        let ret = unsafe {
            #[cfg(target_arch = "x86_64")]
            {
                raw::syscall1(nr::RMDIR, cpath.as_ptr() as u64) as i64
            }
            #[cfg(target_arch = "aarch64")]
            {
                raw::syscall3(nr::UNLINKAT, AT_FDCWD, cpath.as_ptr() as u64, 0x200) as i64
            }
        };
        ret
    }
    pub fn open(path: &str, flags: u32) -> i64 {
        // With O_CREAT a new file gets 0666 less the process umask, as creat()
        // and C's fopen() give; use open_with_mode for any other mode.
        const DEFAULT_CREATE_MODE: u64 = 0o666;
        let cpath = cpath(path);
        // SAFETY: NUL-terminated paths and mutable buffers remain alive through the call.
        let ret = unsafe {
            #[cfg(target_arch = "x86_64")]
            {
                raw::syscall3(
                    nr::OPEN,
                    cpath.as_ptr() as u64,
                    flags as u64,
                    DEFAULT_CREATE_MODE,
                ) as i64
            }
            #[cfg(target_arch = "aarch64")]
            {
                raw::syscall4(
                    nr::OPENAT,
                    AT_FDCWD,
                    cpath.as_ptr() as u64,
                    flags as u64,
                    DEFAULT_CREATE_MODE,
                ) as i64
            }
        };
        ret
    }
    pub fn link(oldpath: &str, newpath: &str) -> i64 {
        let cold = cpath(oldpath);
        let cnew = cpath(newpath);
        // SAFETY: NUL-terminated paths and mutable buffers remain alive through the call.
        let ret = unsafe {
            #[cfg(target_arch = "x86_64")]
            {
                raw::syscall2(nr::LINK, cold.as_ptr() as u64, cnew.as_ptr() as u64) as i64
            }
            #[cfg(target_arch = "aarch64")]
            {
                raw::syscall5(
                    nr::LINKAT,
                    AT_FDCWD,
                    cold.as_ptr() as u64,
                    AT_FDCWD,
                    cnew.as_ptr() as u64,
                    0,
                ) as i64
            }
        };
        ret
    }
    pub fn unlink(path: &str) -> i64 {
        let cpath = cpath(path);
        // SAFETY: NUL-terminated paths and mutable buffers remain alive through the call.
        let ret = unsafe {
            #[cfg(target_arch = "x86_64")]
            {
                raw::syscall1(nr::UNLINK, cpath.as_ptr() as u64) as i64
            }
            #[cfg(target_arch = "aarch64")]
            {
                raw::syscall3(nr::UNLINKAT, AT_FDCWD, cpath.as_ptr() as u64, 0) as i64
            }
        };
        ret
    }
    pub fn rename(oldpath: &str, newpath: &str) -> i64 {
        let cold = cpath(oldpath);
        let cnew = cpath(newpath);
        // SAFETY: NUL-terminated paths and mutable buffers remain alive through the call.
        let ret = unsafe {
            #[cfg(target_arch = "x86_64")]
            {
                raw::syscall2(nr::RENAME, cold.as_ptr() as u64, cnew.as_ptr() as u64) as i64
            }
            #[cfg(target_arch = "aarch64")]
            {
                raw::syscall4(
                    nr::RENAMEAT,
                    AT_FDCWD,
                    cold.as_ptr() as u64,
                    AT_FDCWD,
                    cnew.as_ptr() as u64,
                ) as i64
            }
        };
        ret
    }
    pub fn symlink(target: &str, linkpath: &str) -> i64 {
        let ctarget = cpath(target);
        let clink = cpath(linkpath);
        // SAFETY: NUL-terminated paths and mutable buffers remain alive through the call.
        let ret = unsafe {
            #[cfg(target_arch = "x86_64")]
            {
                raw::syscall2(nr::SYMLINK, ctarget.as_ptr() as u64, clink.as_ptr() as u64) as i64
            }
            #[cfg(target_arch = "aarch64")]
            {
                raw::syscall3(
                    nr::SYMLINKAT,
                    ctarget.as_ptr() as u64,
                    AT_FDCWD,
                    clink.as_ptr() as u64,
                ) as i64
            }
        };
        ret
    }
    pub fn readlink(pathname: &str, buf: &mut [u8]) -> i64 {
        let cpath = cpath(pathname);
        // SAFETY: NUL-terminated paths and mutable buffers remain alive through the call.
        let ret = unsafe {
            #[cfg(target_arch = "x86_64")]
            {
                raw::syscall3(
                    nr::READLINK,
                    cpath.as_ptr() as u64,
                    buf.as_mut_ptr() as u64,
                    buf.len() as u64,
                ) as i64
            }
            #[cfg(target_arch = "aarch64")]
            {
                raw::syscall4(
                    nr::READLINKAT,
                    AT_FDCWD,
                    cpath.as_ptr() as u64,
                    buf.as_mut_ptr() as u64,
                    buf.len() as u64,
                ) as i64
            }
        };
        ret
    }
    pub fn access(path: &str, mode: u32) -> i64 {
        let cpath = cpath(path);
        // SAFETY: NUL-terminated paths and mutable buffers remain alive through the call.
        let ret = unsafe {
            #[cfg(target_arch = "x86_64")]
            {
                raw::syscall2(nr::ACCESS, cpath.as_ptr() as u64, mode as u64) as i64
            }
            #[cfg(target_arch = "aarch64")]
            {
                raw::syscall4(
                    nr::FACCESSAT,
                    AT_FDCWD,
                    cpath.as_ptr() as u64,
                    mode as u64,
                    0,
                ) as i64
            }
        };
        ret
    }
    pub fn stat(p: &str, nofollow: bool) -> i64 {
        let p = cpath(p);
        let mut s = Stat::new();
        call(
            nr::NEWFSTATAT,
            [
                AT_FDCWD,
                p.as_ptr() as u64,
                &mut s as *mut Stat as u64,
                if nofollow { 256 } else { 0 },
            ],
        )
    }
    pub fn getcwd(b: &mut [u8]) -> i64 {
        call(nr::GETCWD, [b.as_mut_ptr() as u64, b.len() as u64, 0, 0])
    }
    pub fn chdir(p: &[u8]) -> i64 {
        call(nr::CHDIR, [p.as_ptr() as u64, 0, 0, 0])
    }
    pub fn chmod(p: &str, mode: u32) -> i64 {
        let p = cpath(p);
        call(ABI[0], [AT_FDCWD, p.as_ptr() as u64, mode as u64, 0])
    }
    pub fn set_times(p: &str, values: [(i64, i64); 2], flags: u64) -> i64 {
        let p = cpath(p);
        let times = values.map(|(sec, nsec)| Timespec { sec, nsec });
        call(
            ABI[5],
            [AT_FDCWD, p.as_ptr() as u64, times.as_ptr() as u64, flags],
        )
    }
}
fn cpath(p: &str) -> Vec<u8> {
    let mut b = p.as_bytes().to_vec();
    b.push(0);
    b
}
fn stat(p: &str, nofollow: bool) -> Result<Stat, Error> {
    let p = cpath(p);
    let mut s = Stat::new();
    request(
        nr::NEWFSTATAT,
        [
            AT_FDCWD,
            p.as_ptr() as u64,
            &mut s as *mut Stat as u64,
            if nofollow { 256 } else { 0 },
        ],
    )?;
    Ok(s)
}
fn chmod(p: &str, mode: u32) -> Result<(), Error> {
    let p = cpath(p);
    request(ABI[0], [AT_FDCWD, p.as_ptr() as u64, mode as u64, 0]).map(|_| ())
}
fn chown(p: &str, uid: u32, gid: u32) -> Result<(), Error> {
    let p = cpath(p);
    // fchownat uses five arguments in the syscall ABI.
    // SAFETY: p is NUL-terminated and remains alive through the syscall.
    let r = unsafe {
        raw::syscall5(
            ABI[1],
            AT_FDCWD,
            p.as_ptr() as u64,
            uid as u64,
            gid as u64,
            0,
        )
    };
    Error::from_syscall(r as i64).map(|_| ())
}
fn umask(mode: u32) -> Result<u64, Error> {
    request(ABI[2], [mode as u64, 0, 0, 0])
}
fn unprivileged() -> CaseResult {
    // Clear inherited supplementary groups before measuring primary classes.
    #[cfg(target_arch = "x86_64")]
    const SETGROUPS: u64 = 116;
    #[cfg(target_arch = "aarch64")]
    const SETGROUPS: u64 = 159;
    request(SETGROUPS, [0, 0, 0, 0])?;
    request(ABI[4], [1001, 0, 0, 0])?;
    request(ABI[3], [1001, 0, 0, 0])?;
    Ok(())
}
struct Tree {
    root: String,
    paths: RefCell<Vec<String>>,
}
impl Tree {
    fn new() -> Result<Self, libbreenix::suite::CaseError> {
        umask(0)?;
        let root = format!("/tmp/directories-{}", process::getpid()?.raw());
        fs::mkdir(&root, 0o777)?;
        Ok(Self {
            root,
            paths: RefCell::new(Vec::new()),
        })
    }
    fn path(&self, name: &str) -> String {
        let p = format!("{}/{name}", self.root);
        self.paths.borrow_mut().push(p.clone());
        p
    }
    fn dir(&self, name: &str) -> Result<String, libbreenix::suite::CaseError> {
        let p = self.path(name);
        fs::mkdir(&p, 0o777)?;
        Ok(p)
    }
    fn file(&self, name: &str, data: &[u8]) -> Result<String, libbreenix::suite::CaseError> {
        let p = self.path(name);
        let fd = fs::open_with_mode(&p, O_CREAT | O_EXCL | O_RDWR, 0o666)?;
        let written = write_all(fd, data);
        let closed = io::close(fd);
        written?;
        closed?;
        Ok(p)
    }
    fn sym(&self, name: &str, target: &str) -> Result<String, libbreenix::suite::CaseError> {
        let p = self.path(name);
        fs::symlink(target, &p)?;
        Ok(p)
    }
}
impl Drop for Tree {
    fn drop(&mut self) {
        let _ = process::chdir(b"/\0");
        // Cleanup supplies no assertion evidence; choose the operation by lstat
        // type so a broken unlink-on-directory cannot corrupt directory metadata.
        for p in self.paths.get_mut().iter().rev() {
            if let Ok(meta) = stat(p, true) {
                if meta.is_dir() {
                    let _ = fs::rmdir(p);
                } else {
                    let _ = fs::unlink(p);
                }
            }
        }
        let _ = fs::rmdir(&self.root);
    }
}
fn write_all(fd: Fd, mut bytes: &[u8]) -> CaseResult {
    while !bytes.is_empty() {
        let n = io::write(fd, bytes)?;
        check(n > 0 && n <= bytes.len(), "invalid write count")?;
        bytes = &bytes[n..];
    }
    Ok(())
}
fn contents(fd: Fd, expected: &[u8]) -> CaseResult {
    fs::lseek(fd, 0, SEEK_SET)?;
    let mut data = vec![0; expected.len() + 1];
    let mut used = 0;
    while used < data.len() {
        let n = io::read(fd, &mut data[used..])?;
        check(n <= data.len() - used, "invalid read count")?;
        if n == 0 {
            break;
        }
        used += n;
    }
    check(&data[..used] == expected, "filesystem data differs")
}
fn bytes(p: &str, expected: &[u8]) -> CaseResult {
    let fd = File::open(p, O_RDONLY)?;
    contents(fd.fd(), expected)
}
fn cwd() -> Result<String, libbreenix::suite::CaseError> {
    let mut b = [0xa5; 256];
    process::getcwd(&mut b)?;
    let end = b.iter().position(|v| *v == 0).ok_or("getcwd lacks NUL")?;
    check(end > 0 && b[0] == b'/', "getcwd is not absolute")?;
    String::from_utf8(b[..end].to_vec()).map_err(|e| e.to_string().into())
}
#[derive(Debug, PartialEq, Eq)]
struct Entry {
    name: String,
    ino: u64,
}
/// Thin, unbuffered directory stream over getdents64 and lseek. This suite
/// measures kernel iteration/cookie semantics, not the separate libc interfaces
/// milestone. No entry is replayed from a userspace cache. Fixtures use short
/// names so a 32-byte syscall buffer admits exactly one Linux dirent record.
struct Directory {
    file: File,
}
impl Directory {
    fn open(p: &str) -> Result<Self, Error> {
        Ok(Self {
            file: File::open(p, O_RDONLY | O_DIRECTORY)?,
        })
    }
    fn readdir(&mut self) -> Result<Option<Entry>, libbreenix::suite::CaseError> {
        let mut b = [0u8; 32];
        let n = fs::getdents64(self.file.fd(), &mut b)?;
        if n == 0 {
            return Ok(None);
        }
        check(n >= 24 && n <= b.len(), "invalid dirent length")?;
        let len = u16::from_ne_bytes([b[16], b[17]]) as usize;
        check(len == n, "expected exactly one complete dirent")?;
        let end = b[19..n]
            .iter()
            .position(|v| *v == 0)
            .ok_or("dirent lacks NUL")?
            + 19;
        let name = String::from_utf8(b[19..end].to_vec()).map_err(|e| e.to_string())?;
        Ok(Some(Entry {
            name,
            ino: u64::from_ne_bytes(b[..8].try_into().unwrap()),
        }))
    }
    fn all(&mut self) -> Result<Vec<Entry>, libbreenix::suite::CaseError> {
        let mut entries = Vec::new();
        while let Some(e) = self.readdir()? {
            check(entries.len() < 32, "directory stream did not terminate")?;
            entries.push(e);
        }
        Ok(entries)
    }
    fn telldir(&self) -> Result<u64, Error> {
        fs::lseek(self.file.fd(), 0, SEEK_CUR)
    }
    fn seekdir(&mut self, cookie: u64) -> CaseResult {
        check(
            fs::lseek(self.file.fd(), cookie as i64, SEEK_SET)? == cookie,
            "directory seek returned a different cookie",
        )
    }
    fn rewinddir(&mut self) -> CaseResult {
        self.seekdir(0)
    }
}
/// Every record one getdents64 call returns, each with its d_off: the cookie
/// for the position after it. The buffer admits names of any length.
fn dirents(fd: Fd) -> Result<Vec<(Entry, u64)>, libbreenix::suite::CaseError> {
    let mut b = [0u8; 4096];
    let n = fs::getdents64(fd, &mut b)?;
    check(n <= b.len(), "invalid getdents64 length")?;
    let mut records = Vec::new();
    let mut at = 0;
    while at < n {
        check(at + 24 <= n, "truncated dirent")?;
        let len = u16::from_ne_bytes([b[at + 16], b[at + 17]]) as usize;
        check(len >= 24 && at + len <= n, "invalid dirent length")?;
        let end = b[at + 19..at + len]
            .iter()
            .position(|v| *v == 0)
            .ok_or("dirent lacks NUL")?
            + at
            + 19;
        let name = String::from_utf8(b[at + 19..end].to_vec()).map_err(|e| e.to_string())?;
        let ino = u64::from_ne_bytes(b[at..at + 8].try_into().unwrap());
        let off = i64::from_ne_bytes(b[at + 8..at + 16].try_into().unwrap());
        check(off >= 0, "negative d_off")?;
        records.push((Entry { name, ino }, off as u64));
        at += len;
    }
    Ok(records)
}
/// The first entry at a cookie, read by seeking a descriptor to it.
fn entry_at(fd: Fd, cookie: u64) -> Result<Option<Entry>, libbreenix::suite::CaseError> {
    check(
        fs::lseek(fd, cookie as i64, SEEK_SET)? == cookie,
        "directory seek returned a different cookie",
    )?;
    Ok(dirents(fd)?.into_iter().next().map(|(e, _)| e))
}
/// The filesystem's free inode and block counts, from fstatfs.
fn free_counts(fd: Fd) -> Result<(u64, u64), Error> {
    let s = fs::fstatfs(fd)?;
    Ok((s.f_ffree, s.f_bfree))
}
/// Free counts once nothing is releasing inodes or blocks: the same pair on
/// five reads 20 ms apart, so an earlier case's deferred reclamation is not
/// mistaken for this case's.
fn settled_counts(fd: Fd) -> Result<(u64, u64), libbreenix::suite::CaseError> {
    let mut last = free_counts(fd)?;
    let mut same = 0;
    for _ in 0..150 {
        time::sleep_ms(20)?;
        let now = free_counts(fd)?;
        if now == last {
            same += 1;
            if same == 4 {
                return Ok(now);
            }
        } else {
            same = 0;
            last = now;
        }
    }
    Err("free counts never settled".into())
}
fn listing() -> Result<Tree, libbreenix::suite::CaseError> {
    let f = Tree::new()?;
    f.file("beta", b"b")?;
    f.file("alpha", b"a")?;
    f.dir("dir")?;
    Ok(f)
}
fn atomic_replace() -> CaseResult {
    let f = Tree::new()?;
    let src = f.file("src", b"NEW-DATA")?;
    let dst = f.file("dst", b"OLD-DATA")?;
    // The handshake establishes a live reader, not instruction-level overlap.
    // A finite scheduled run can detect missing/partial replacement but cannot
    // prove atomicity against every interleaving.
    let (ready_r, ready_w) = io::pipe()?;
    let (done_r, done_w) = io::pipe()?;
    match process::fork()? {
        process::ForkResult::Child => {
            let result = (|| -> CaseResult {
                io::close(ready_r)?;
                io::close(done_w)?;
                bytes(&dst, b"OLD-DATA")?;
                write_all(ready_w, b"r")?;
                io::fcntl(done_r, 4, O_NONBLOCK as i64)?;
                loop {
                    let fd = File::open(&dst, O_RDONLY)?;
                    let mut b = [0; 9];
                    let n = io::read(fd.fd(), &mut b)?;
                    check(
                        n == 8 && (&b[..8] == b"OLD-DATA" || &b[..8] == b"NEW-DATA"),
                        "reader saw missing or partial replacement",
                    )?;
                    match io::read(done_r, &mut [0; 1]) {
                        Ok(1) => break,
                        Err(Error::Os(libbreenix::Errno::EAGAIN)) => {}
                        other => return fail(format!("completion pipe: {other:?}")),
                    }
                }
                bytes(&dst, b"NEW-DATA")
            })();
            process::exit(if result.is_ok() { 0 } else { 1 });
        }
        process::ForkResult::Parent(pid) => {
            io::close(ready_w)?;
            io::close(done_r)?;
            check(io::read(ready_r, &mut [0; 1])? == 1, "reader did not start")?;
            let renamed = fs::rename(&src, &dst);
            write_all(done_w, b"d")?;
            io::close(done_w)?;
            io::close(ready_r)?;
            let mut status = 0;
            process::waitpid(pid.raw() as i32, &mut status, 0)?;
            renamed?;
            check(
                process::wifexited(status) && process::wexitstatus(status) == 0,
                "concurrent reader observed a missing or partial replacement",
            )?;
            bytes(&dst, b"NEW-DATA")
        }
    }
}
#[repr(C)]
struct Timespec {
    sec: i64,
    nsec: i64,
}
fn set_times(p: &str, values: [(i64, i64); 2], flags: u64) -> Result<(), Error> {
    let p = cpath(p);
    let times = values.map(|(sec, nsec)| Timespec { sec, nsec });
    request(
        ABI[5],
        [AT_FDCWD, p.as_ptr() as u64, times.as_ptr() as u64, flags],
    )
    .map(|_| ())
}
fn old_times(p: &str) -> CaseResult {
    set_times(p, [(1000000000, 0), (1000000001, 0)], 0)?;
    let s = stat(p, false)?;
    check(
        atime(&s) == (1000000000, 0) && mtime(&s) == (1000000001, 0),
        "timestamp setup was not stored",
    )
}
fn next_second(old: &Stat) -> Result<(i64, i64), Error> {
    loop {
        let now = realtime()?;
        if now.0 > old.st_ctime {
            return Ok(now);
        }
        time::sleep_ms(10)?;
    }
}
fn atime(s: &Stat) -> (i64, i64) {
    (s.st_atime, s.st_atime_nsec)
}
fn mtime(s: &Stat) -> (i64, i64) {
    (s.st_mtime, s.st_mtime_nsec)
}
fn ctime(s: &Stat) -> (i64, i64) {
    (s.st_ctime, s.st_ctime_nsec)
}
fn realtime() -> Result<(i64, i64), Error> {
    let mut t = libbreenix::Timespec::new();
    time::clock_gettime(time::CLOCK_REALTIME, &mut t)?;
    Ok((t.tv_sec, t.tv_nsec))
}
fn in_window(t: (i64, i64), before: (i64, i64), after: (i64, i64)) -> bool {
    // POSIX permits rounding down to the filesystem's timestamp resolution.
    // ext2 stores whole seconds; allow that floor, never a stale epoch value.
    t >= (before.0, 0) && t <= after && (0..1000000000).contains(&t.1)
}

static SUITE: Suite = suite("directories", "Directories & links", &[
    category("mkdir-rmdir", "mkdir & rmdir", &[
        case("create", "mkdir creates a directory with the requested mode", mkdir_rmdir_create),
        case("existing-dir", "mkdir on an existing directory fails with EEXIST", mkdir_rmdir_existing_dir),
        case("existing-file", "mkdir on an existing file fails with EEXIST", mkdir_rmdir_existing_file),
        case("missing-parent", "mkdir with a missing parent fails with ENOENT", mkdir_rmdir_missing_parent),
        case("file-parent", "mkdir beneath a regular file fails with ENOTDIR", mkdir_rmdir_file_parent),
        case("empty", "rmdir removes an empty directory", mkdir_rmdir_empty),
        case("nonempty-file", "rmdir with a file child fails with EEXIST or ENOTEMPTY and preserves the tree", mkdir_rmdir_nonempty_file),
        case("nonempty-dir", "rmdir with a directory child fails with EEXIST or ENOTEMPTY", mkdir_rmdir_nonempty_dir),
        case("regular-file", "rmdir on a regular file fails with ENOTDIR", mkdir_rmdir_regular_file),
        case("missing", "rmdir on a missing path fails with ENOENT", mkdir_rmdir_missing),
        case("symlink", "rmdir does not follow a final symlink to a directory", mkdir_rmdir_symlink),
        case("parent-nlink", "ext2 mkdir and rmdir update the parent directory link count", mkdir_rmdir_parent_nlink),
        case("reclaim", "rmdir returns the directory's inode and block to the filesystem's free counts", mkdir_rmdir_reclaim),
    ]),
    category("readdir", "readdir & seekdir", &[
        case("entries", "readdir returns every child exactly once without imposing lexical order", readdir_entries),
        case("dot", "readdir includes dot with the directory inode", readdir_dot),
        case("dotdot", "readdir includes dot-dot with the parent inode", readdir_dotdot),
        case("inodes", "readdir child inode numbers agree with fresh pathname stat", readdir_inodes),
        case("eof", "readdir at end stays at end", readdir_eof),
        case("rewind", "rewinddir replays an unchanged directory in its original order", readdir_rewind),
        case("tell-seek", "seekdir to a telldir cookie replays the remaining entries in order", readdir_tell_seek),
        case("tell-after-rewind", "telldir and seekdir work with a fresh cookie after rewinddir", readdir_tell_after_rewind),
        case("two-streams", "independent directory streams have independent positions", readdir_two_streams),
        case("not-directory", "opening a regular file as a directory fails with ENOTDIR", readdir_not_directory),
        case("cookie-churn", "seekdir resumes at the surviving entries after names are created and removed around its cookie", readdir_cookie_churn),
        case("cookie-blocks", "cookies address every record of a multi-block directory and only the directory they seek", readdir_cookie_blocks),
    ]),
    category("links", "hard links & unlink", &[
        case("identity", "hard links share an inode and report two links", links_identity),
        case("shared-write", "writes through a hard link are visible by reopening the original", links_shared_write),
        case("unlink-one", "unlink of one hard link preserves data and decrements nlink", links_unlink_one),
        case("open-unlink", "an open descriptor sees link counts two, one and zero and retains data", links_open_unlink),
        case("destination-exists", "link to an existing destination fails with EEXIST", links_destination_exists),
        case("source-missing", "link of a missing source fails with ENOENT", links_source_missing),
        case("directory", "implementation prohibits hard linking a directory with EPERM", links_directory),
        case("cross-directory", "hard links across directories share data and link counts", links_cross_directory),
        case("unlink-missing", "unlink of a missing name fails with ENOENT", links_unlink_missing),
        case("unlink-directory", "implementation prohibits unlink of a directory with EPERM or EISDIR", links_unlink_directory),
    ]),
    category("symlinks", "symlinks & readlink", &[
        case("absolute", "an absolute symlink resolves to its target", symlinks_absolute),
        case("relative", "a relative symlink resolves from its containing directory", symlinks_relative),
        case("dangling", "a dangling symlink exists in lstat but opening it fails with ENOENT", symlinks_dangling),
        case("existing", "symlink at an existing name fails with EEXIST", symlinks_existing),
        case("readlink", "readlink returns target bytes without a terminating NUL", symlinks_readlink),
        case("truncate", "readlink silently truncates to the buffer size without a NUL", symlinks_truncate),
        case("readlink-regular", "readlink on a regular file fails with EINVAL", symlinks_readlink_regular),
        case("self-loop", "opening a self-referential symlink fails with ELOOP", symlinks_self_loop),
        case("mutual-loop", "opening a two-link cycle fails with ELOOP", symlinks_mutual_loop),
        case("nofollow", "O_NOFOLLOW rejects a final symlink with ELOOP", symlinks_nofollow),
        case("nofollow-prefix", "O_NOFOLLOW still follows symlinks in prefix components", symlinks_nofollow_prefix),
        case("unlink-link", "unlink removes the symlink rather than its target", symlinks_unlink_link),
    ]),
    category("rename", "rename & atomic replace", &[
        case("file", "rename moves a file while preserving inode and contents", rename_file),
        case("replace-open", "rename replaces a file while old open descriptors retain the replaced inode", rename_replace_open),
        case("atomic-replace", "a concurrent reader sees the destination continuously as whole old or new data", rename_atomic_replace),
        case("empty-dir", "rename of a directory onto an empty directory replaces it", rename_empty_dir),
        case("nonempty-dir", "rename onto a non-empty directory fails with EEXIST or ENOTEMPTY", rename_nonempty_dir),
        case("cross-directory", "rename moves a file across directories", rename_cross_directory),
        case("directory-parent", "moving a directory updates dot-dot and ext2 parent link counts", rename_directory_parent),
        case("same-name", "rename of a name onto itself succeeds without altering data", rename_same_name),
        case("same-inode", "rename between hard links to the same inode preserves both names", rename_same_inode),
        case("file-to-dir", "rename of a file onto a directory fails with EISDIR", rename_file_to_dir),
        case("dir-to-file", "rename of a directory onto a file fails with ENOTDIR", rename_dir_to_file),
        case("descendant", "rename of a directory into its descendant fails with EINVAL", rename_descendant),
        case("symlink", "rename moves a symlink itself and preserves its target text", rename_symlink),
        case("missing-source", "rename of a missing source fails with ENOENT and preserves the destination", rename_missing_source),
    ]),
    category("cwd", "getcwd, chdir & relative paths", &[
        case("absolute", "chdir followed by getcwd reports the absolute directory path", cwd_absolute),
        case("relative", "relative opens after chdir resolve from the working directory", cwd_relative),
        case("parent", "chdir dot-dot moves to the parent directory", cwd_parent),
        case("relative-create", "mkdir and rename with relative paths operate in the working directory", cwd_relative_create),
        case("renamed", "getcwd follows a renamed working directory to its new pathname", cwd_renamed),
        case("ancestor-renamed", "getcwd follows a renamed ancestor of the working directory", cwd_ancestor_renamed),
        case("removed", "rmdir of cwd returns EBUSY or Linux getcwd of removed cwd returns ENOENT", cwd_removed),
        case("small-buffer", "getcwd with an insufficient buffer fails with ERANGE", cwd_small_buffer),
        case("not-dir", "chdir to a regular file fails with ENOTDIR and keeps cwd", cwd_not_dir),
        case("missing", "chdir to a missing path fails with ENOENT and keeps cwd", cwd_missing),
        case("count-abi", "Linux getcwd syscall byte count includes the NUL terminator", cwd_count_abi),
        case("symlink-parent-open", "open resolves symlink before dot-dot", cwd_symlink_parent_open),
        case("symlink-parent-chdir", "chdir resolves symlink before dot-dot", cwd_symlink_parent_chdir),
        case("symlink-physical", "getcwd after chdir of a symlink names the physical directory", cwd_symlink_physical),
    ]),
    category("permissions", "chmod, chown, access & umask", &[
        case("chmod-file", "pathname and descriptor chmod preserve file type and update modes across symlinks and unlink", permissions_chmod_file),
        case("chmod-dir", "directory descriptor metadata retains inode identity and SGID", permissions_chmod_dir),
        case("chmod-missing", "chmod of a missing path fails with ENOENT", permissions_chmod_missing),
        case("chown", "pathname and descriptor chown preserve full-width IDs and symlink behavior", permissions_chown),
        case("chown-unchanged", "chown preserves minus-one IDs and enforces owner, FIFO and privilege-bit rules", permissions_chown_unchanged),
        case("umask-return", "umask returns masked state and credentials survive fork, exec, spawn and clone", permissions_umask_return),
        case("umask-file", "creation modes and FIFO descriptor identity survive unlink and replacement", permissions_umask_file),
        case("umask-dir", "umask applies at directory creation", permissions_umask_dir),
        case("chmod-umask", "umask does not mask chmod permission bits", permissions_chmod_umask),
        case("access-exists", "access F_OK reports existence even without permission bits", permissions_access_exists),
        case("access-owner", "access checks owner permission bits using real credentials", permissions_access_owner),
        case("access-group", "access checks group permission bits using real credentials", permissions_access_group),
        case("access-other", "access checks other permission bits using real credentials", permissions_access_other),
        case("access-execute", "access checks execute permission bits using real credentials", permissions_access_execute),
        case("search-denied", "path traversal without directory search permission fails with EACCES", permissions_search_denied),
        case("root-execute", "Linux policy denies root X_OK on non-directory inodes without execute bits", permissions_root_execute),
        case("access-owner-denied", "access denies owner bits even when another class grants them", permissions_access_owner_denied),
        case("access-group-denied", "access denies group bits even when another class grants them", permissions_access_group_denied),
        case("access-other-denied", "access denies other bits even when another class grants them", permissions_access_other_denied),
    ]),
    category("timestamps", "utimensat & time updates", &[
        case("explicit", "utimensat sets explicit access and modification times", timestamps_explicit),
        case("now", "UTIME_NOW sets both times to the current realtime clock", timestamps_now),
        case("omit-atime", "UTIME_OMIT preserves atime while setting mtime", timestamps_omit_atime),
        case("omit-mtime", "UTIME_OMIT preserves mtime while setting atime", timestamps_omit_mtime),
        case("omit-both", "Linux policy preserves atime mtime and ctime for two UTIME_OMIT values", timestamps_omit_both),
        case("now-omit", "UTIME_NOW and UTIME_OMIT can be used in the same request", timestamps_now_omit),
        case("invalid-nsec", "utimensat rejects an ordinary nanosecond value of one billion with EINVAL", timestamps_invalid_nsec),
        case("missing", "utimensat of a missing file fails with ENOENT", timestamps_missing),
        case("symlink-follow", "utimensat follows symlinks by default", timestamps_symlink_follow),
        case("symlink-nofollow", "utimensat AT_SYMLINK_NOFOLLOW updates only the symlink", timestamps_symlink_nofollow),
        case("write-update", "writing file data updates mtime and ctime", timestamps_write_update),
        case("parent-create", "creating a child updates its parent directory mtime and ctime", timestamps_parent_create),
        case("parent-unlink", "unlink updates the parent directory mtime and ctime", timestamps_parent_unlink),
        case("parent-rename", "cross-directory rename updates both parent directories timestamps", timestamps_parent_rename),
        case("directory", "utimensat sets and reads back directory timestamps", timestamps_directory),
    ]),
]);
fn main() {
    SUITE.run()
}

fn mkdir_rmdir_create() -> CaseResult {
    let f = Tree::new()?;
    let p = f.path("dir");
    fs::mkdir(&p, 0o751)?;
    let s = stat(&p, false)?;
    check(
        s.is_dir() && s.st_mode & 0o777 == 0o751,
        "directory type or mode differs",
    )
}

fn mkdir_rmdir_existing_dir() -> CaseResult {
    let f = Tree::new()?;
    errno(observed::mkdir(&f.root, 0o777), 17)
}

fn mkdir_rmdir_existing_file() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"old")?;
    errno(observed::mkdir(&p, 0o777), 17)?;
    bytes(&p, b"old")
}

fn mkdir_rmdir_missing_parent() -> CaseResult {
    let f = Tree::new()?;
    errno(observed::mkdir(&f.path("absent/dir"), 0o777), 2)
}

fn mkdir_rmdir_file_parent() -> CaseResult {
    let f = Tree::new()?;
    f.file("file", b"x")?;
    errno(observed::mkdir(&f.path("file/dir"), 0o777), 20)
}

fn mkdir_rmdir_empty() -> CaseResult {
    let f = Tree::new()?;
    let p = f.dir("dir")?;
    fs::rmdir(&p)?;
    errno(observed::stat(&p, false), 2)
}

fn mkdir_rmdir_nonempty_file() -> CaseResult {
    let f = Tree::new()?;
    let p = f.dir("dir")?;
    let child = f.file("dir/file", b"stay")?;
    errno_one_of(observed::rmdir(&p), &[17, 39])?;
    bytes(&child, b"stay")
}

fn mkdir_rmdir_nonempty_dir() -> CaseResult {
    let f = Tree::new()?;
    let p = f.dir("dir")?;
    let c = f.dir("dir/child")?;
    errno_one_of(observed::rmdir(&p), &[17, 39])?;
    check(stat(&c, false)?.is_dir(), "child disappeared")
}

fn mkdir_rmdir_regular_file() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"stay")?;
    errno(observed::rmdir(&p), 20)?;
    bytes(&p, b"stay")
}

fn mkdir_rmdir_missing() -> CaseResult {
    let f = Tree::new()?;
    errno(observed::rmdir(&f.path("absent")), 2)
}

fn mkdir_rmdir_symlink() -> CaseResult {
    let f = Tree::new()?;
    let d = f.dir("dir")?;
    let p = f.sym("sym", &d)?;
    errno(observed::rmdir(&p), 20)?;
    check(
        stat(&d, false)?.is_dir() && stat(&p, true)?.is_symlink(),
        "rmdir damaged link or target",
    )
}

fn mkdir_rmdir_parent_nlink() -> CaseResult {
    let f = Tree::new()?;
    let before = stat(&f.root, false)?.st_nlink;
    let p = f.dir("dir")?;
    check(
        stat(&f.root, false)?.st_nlink == before + 1,
        "mkdir did not increment parent nlink",
    )?;
    fs::rmdir(&p)?;
    check(
        stat(&f.root, false)?.st_nlink == before,
        "rmdir did not restore parent nlink",
    )
}

fn mkdir_rmdir_reclaim() -> CaseResult {
    // Each mkdir allocates one inode and one block; rmdir must return both.
    // The counts are the filesystem's own (fstatfs), so a removed directory
    // whose inode or block stays allocated is a deficit here.
    const N: u64 = 8;
    let f = Tree::new()?;
    let root = Directory::open(&f.root)?;
    let fd = root.file.fd();
    let base = settled_counts(fd)?;
    let dirs: Vec<String> = (0..N).map(|i| f.path(&format!("d{i}"))).collect();
    for d in &dirs {
        fs::mkdir(d, 0o777)?;
    }
    let made = free_counts(fd)?;
    check(
        made == (base.0 - N, base.1 - N),
        &format!("{N} mkdirs moved free inodes and blocks {base:?} -> {made:?}"),
    )?;
    for d in &dirs {
        fs::rmdir(d)?;
    }
    // The ext2 finalizer releases a removed directory after rmdir returns.
    let mut now = free_counts(fd)?;
    for _ in 0..250 {
        if now == base {
            return Ok(());
        }
        time::sleep_ms(20)?;
        now = free_counts(fd)?;
    }
    fail(format!(
        "after {N} rmdirs free inodes and blocks are {now:?}, not {base:?}"
    ))
}

fn readdir_entries() -> CaseResult {
    let f = listing()?;
    let mut d = Directory::open(&f.root)?;
    let entries = d.all()?;
    let mut names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
    names.sort();
    check(
        names == [".", "..", "alpha", "beta", "dir"],
        "entries missing or duplicated",
    )
}

fn readdir_dot() -> CaseResult {
    let f = listing()?;
    let mut d = Directory::open(&f.root)?;
    let entries = d.all()?;
    let ino = stat(&f.root, false)?.st_ino;
    check(
        entries
            .iter()
            .filter(|e| e.name == "." && e.ino == ino)
            .count()
            == 1,
        "dot missing or wrong inode",
    )
}

fn readdir_dotdot() -> CaseResult {
    let f = listing()?;
    let mut d = Directory::open(&f.root)?;
    let entries = d.all()?;
    let ino = stat("/tmp", false)?.st_ino;
    check(
        entries
            .iter()
            .filter(|e| e.name == ".." && e.ino == ino)
            .count()
            == 1,
        "dot-dot missing or wrong inode",
    )
}

fn readdir_inodes() -> CaseResult {
    let f = listing()?;
    let mut d = Directory::open(&f.root)?;
    for e in d.all()? {
        check(
            e.ino == stat(&format!("{}/{}", f.root, e.name), false)?.st_ino,
            "dirent inode differs from stat",
        )?;
    }
    Ok(())
}

fn readdir_eof() -> CaseResult {
    let f = listing()?;
    let mut d = Directory::open(&f.root)?;
    d.all()?;
    check(
        d.readdir()?.is_none() && d.readdir()?.is_none(),
        "EOF produced another entry",
    )
}

fn readdir_rewind() -> CaseResult {
    let f = listing()?;
    let mut d = Directory::open(&f.root)?;
    let before = d.all()?;
    d.rewinddir()?;
    check(d.all()? == before, "rewind changed entry sequence")
}

fn readdir_tell_seek() -> CaseResult {
    let f = listing()?;
    let mut d = Directory::open(&f.root)?;
    check(d.readdir()?.is_some(), "empty stream")?;
    let cookie = d.telldir()?;
    let tail = d.all()?;
    check(!tail.is_empty(), "missing tail")?;
    d.seekdir(cookie)?;
    check(d.all()? == tail, "seek did not restore the saved position")
}

fn readdir_tell_after_rewind() -> CaseResult {
    let f = listing()?;
    let mut d = Directory::open(&f.root)?;
    d.all()?;
    d.rewinddir()?;
    check(d.readdir()?.is_some(), "rewind empty")?;
    let cookie = d.telldir()?;
    let tail = d.all()?;
    d.seekdir(cookie)?;
    check(d.all()? == tail, "fresh post-rewind cookie failed")
}

fn readdir_two_streams() -> CaseResult {
    let f = listing()?;
    let mut a = Directory::open(&f.root)?;
    let mut b = Directory::open(&f.root)?;
    let first = a.readdir()?;
    a.all()?;
    check(b.readdir()? == first, "one stream moved the other")
}

fn readdir_not_directory() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    errno(observed::open(&p, O_RDONLY | O_DIRECTORY), 20)
}

fn readdir_cookie_churn() -> CaseResult {
    let f = Tree::new()?;
    for n in ["a0", "a1", "a2", "a3", "a4", "a5"] {
        f.file(n, b"")?;
    }
    let mut d = Directory::open(&f.root)?;
    let mut head = Vec::new();
    for _ in 0..4 {
        head.push(d.readdir()?.ok_or("directory ended early")?);
    }
    let cookie = d.telldir()?;
    let tail = d.all()?;
    check(tail.len() >= 3, "too few entries after the cookie")?;
    // Remove a name read before the cookie and the one the cookie names,
    // then create two.
    let before = head
        .iter()
        .find(|e| e.name.starts_with('a'))
        .ok_or("no removable entry before the cookie")?;
    fs::unlink(&f.path(&before.name))?;
    fs::unlink(&f.path(&tail[0].name))?;
    f.file("b0", b"")?;
    f.file("b1", b"")?;
    d.seekdir(cookie)?;
    let after = d.all()?;
    // Entries present throughout follow in their original order exactly once;
    // a name created since may appear, but only once.
    let old: Vec<&Entry> = after.iter().filter(|e| !e.name.starts_with('b')).collect();
    let survivors: Vec<&Entry> = tail[1..].iter().collect();
    check(old == survivors, "seekdir lost, repeated or replayed an entry")?;
    for n in ["b0", "b1"] {
        check(
            after.iter().filter(|e| e.name == n).count() <= 1,
            "a created name appeared twice",
        )?;
    }
    // rewinddir restarts at the beginning and sees the directory as it is.
    d.rewinddir()?;
    let all = d.all()?;
    check(
        all.len() >= 2 && all[0].name == "." && all[1].name == "..",
        "rewinddir did not restart at dot and dot-dot",
    )?;
    let mut names: Vec<&str> = all.iter().map(|e| e.name.as_str()).collect();
    names.sort();
    let mut expected = vec![".", "..", "b0", "b1"];
    for n in ["a0", "a1", "a2", "a3", "a4", "a5"] {
        if n != before.name && n != tail[0].name {
            expected.push(n);
        }
    }
    expected.sort();
    check(names == expected, "rewinddir listing differs from the directory")
}

fn readdir_cookie_blocks() -> CaseResult {
    // Breenix directories do not grow past their first block, so the one
    // measured is a fixture the disk image builder fills past three blocks
    // (scripts/create_ext2_disk.sh, /test/dir-blocks).
    let dir = Directory::open("/test/dir-blocks")?;
    let fd = dir.file.fd();
    let block = fs::fstatfs(fd)?.f_bsize as u64;
    check(
        fs::fstat(fd)?.st_size as u64 >= 2 * block,
        "/test/dir-blocks does not span two blocks",
    )?;
    let mut records = Vec::new();
    loop {
        let batch = dirents(fd)?;
        if batch.is_empty() {
            break;
        }
        records.extend(batch);
        check(records.len() < 4096, "directory stream did not terminate")?;
    }
    // The builder makes names 0..n, zero-padded to 200 digits, while 208 * i
    // stays within three blocks, all linked to one file. Each of those
    // names, dot and dot-dot appears exactly once, with the inode stat gives it.
    let n = (3 * block / 208 + 1) as usize;
    let file_ino = stat(&format!("/test/dir-blocks/{:0200}", 0), false)?.st_ino;
    let mut expected: Vec<(String, u64)> = (0..n).map(|i| (format!("{i:0200}"), file_ino)).collect();
    expected.push((".".into(), fs::fstat(fd)?.st_ino));
    expected.push(("..".into(), stat("/test", false)?.st_ino));
    expected.sort();
    let mut listed: Vec<(String, u64)> = records.iter().map(|(e, _)| (e.name.clone(), e.ino)).collect();
    listed.sort();
    check(
        listed == expected,
        &format!("listed {} entries, not the builder's {}", listed.len(), expected.len()),
    )?;
    check(records[0].0.name == ".", "the first record is not dot")?;
    check(
        records.windows(2).all(|w| w[0].1 < w[1].1),
        "cookies do not increase",
    )?;
    check(
        records.iter().any(|(_, c)| *c > block && *c < records.last().unwrap().1),
        "no cookie lies past the first block",
    )?;
    // Every cookie, including those at a block boundary, resumes at the next
    // entry; 0 is the beginning.
    check(entry_at(fd, 0)? == Some(Entry { name: ".".into(), ino: records[0].0.ino }), "cookie 0 is not dot")?;
    for (i, (_, cookie)) in records.iter().enumerate() {
        let want = records.get(i + 1).map(|(e, _)| e);
        check(
            entry_at(fd, *cookie)?.as_ref() == want,
            &format!("cookie {cookie} did not resume at entry {}", i + 1),
        )?;
    }
    // A position inside a record resumes at the first whole record after
    // it, and reads nothing only inside the last record: inside the first
    // record of each block, every eighth record and the last two.
    for (i, (_, cookie)) in records.iter().enumerate() {
        if i % 8 != 0 && cookie % block != 0 && i + 2 < records.len() {
            continue;
        }
        let want = records.get(i + 2).map(|(e, _)| e);
        check(
            entry_at(fd, cookie + 1)?.as_ref() == want,
            &format!("position {} did not resume at entry {}", cookie + 1, i + 2),
        )?;
    }
    // A cookie from that directory, given to another, positions within the
    // other's own records: at or before one of them it reads that record,
    // and past the last record's start it reads nothing.
    let f = Tree::new()?;
    let own_path = f.file("own", b"")?;
    let small = Directory::open(&f.root)?;
    let sfd = small.file.fd();
    let mut identities = vec![
        (".".to_string(), stat(&f.root, false)?.st_ino),
        ("..".to_string(), stat("/tmp", false)?.st_ino),
        ("own".to_string(), stat(&own_path, false)?.st_ino),
    ];
    identities.sort();
    check(fs::lseek(sfd, 0, SEEK_SET)? == 0, "directory seek to 0 failed")?;
    let own = dirents(sfd)?;
    let mut own_listed: Vec<(String, u64)> = own.iter().map(|(e, _)| (e.name.clone(), e.ino)).collect();
    own_listed.sort();
    check(own_listed == identities, "the small directory's entries are not its own")?;
    let starts: Vec<u64> = core::iter::once(0).chain(own.iter().map(|(_, c)| *c)).take(own.len()).collect();
    let last = records.last().unwrap().1;
    check(entry_at(sfd, last)?.is_none(), "a foreign cookie read past the directory")?;
    for (_, cookie) in records.iter().filter(|(_, c)| *c < block).step_by(2) {
        let want = starts.iter().position(|s| s >= cookie).map(|k| &own[k].0);
        check(
            entry_at(sfd, *cookie)?.as_ref() == want,
            &format!("foreign cookie {cookie} did not read the small directory's next record"),
        )?;
    }
    Ok(())
}

fn links_identity() -> CaseResult {
    let f = Tree::new()?;
    let a = f.file("a", b"one")?;
    let b = f.path("b");
    fs::link(&a, &b)?;
    let x = stat(&a, false)?;
    let y = stat(&b, false)?;
    check(
        x.st_dev == y.st_dev && x.st_ino == y.st_ino && x.st_nlink == 2 && y.st_nlink == 2,
        "hard-link identity or nlink differs",
    )
}

fn links_shared_write() -> CaseResult {
    let f = Tree::new()?;
    let a = f.file("a", b"one")?;
    let b = f.path("b");
    fs::link(&a, &b)?;
    let fd = File::open(&b, O_WRONLY)?;
    write_all(fd.fd(), b"two")?;
    drop(fd);
    bytes(&a, b"two")
}

fn links_unlink_one() -> CaseResult {
    let f = Tree::new()?;
    let a = f.file("a", b"stay")?;
    let b = f.path("b");
    fs::link(&a, &b)?;
    fs::unlink(&a)?;
    errno(observed::stat(&a, false), 2)?;
    check(stat(&b, false)?.st_nlink == 1, "survivor nlink is not one")?;
    bytes(&b, b"stay")
}

fn links_open_unlink() -> CaseResult {
    let f = Tree::new()?;
    let a = f.file("a", b"stay")?;
    let fd = File::open(&a, O_RDWR)?;
    let b = f.path("b");
    fs::link(&a, &b)?;
    check(fs::fstat(fd.fd())?.st_nlink == 2, "open nlink not two")?;
    fs::unlink(&a)?;
    check(fs::fstat(fd.fd())?.st_nlink == 1, "open nlink not one")?;
    fs::unlink(&b)?;
    check(fs::fstat(fd.fd())?.st_nlink == 0, "open nlink not zero")?;
    errno(observed::stat(&b, false), 2)?;
    contents(fd.fd(), b"stay")?;
    fs::lseek(fd.fd(), 0, SEEK_SET)?;
    write_all(fd.fd(), b"live")?;
    contents(fd.fd(), b"live")
}

fn links_destination_exists() -> CaseResult {
    let f = Tree::new()?;
    let a = f.file("a", b"a")?;
    let b = f.file("b", b"b")?;
    errno(observed::link(&a, &b), 17)?;
    bytes(&b, b"b")
}

fn links_source_missing() -> CaseResult {
    let f = Tree::new()?;
    errno(observed::link(&f.path("absent"), &f.path("b")), 2)
}

fn links_directory() -> CaseResult {
    let f = Tree::new()?;
    errno(observed::link(&f.root, &f.path("b")), 1)
}

fn links_cross_directory() -> CaseResult {
    let f = Tree::new()?;
    f.dir("dir")?;
    let a = f.file("a", b"data")?;
    let b = f.path("dir/b");
    fs::link(&a, &b)?;
    check(
        stat(&a, false)?.st_ino == stat(&b, false)?.st_ino && stat(&b, false)?.st_nlink == 2,
        "cross-directory link differs",
    )?;
    bytes(&b, b"data")
}

fn links_unlink_missing() -> CaseResult {
    let f = Tree::new()?;
    errno(observed::unlink(&f.path("absent")), 2)
}

fn symlinks_absolute() -> CaseResult {
    let f = Tree::new()?;
    let a = f.file("a", b"data")?;
    let s = f.sym("sym", &a)?;
    bytes(&s, b"data")
}

fn symlinks_relative() -> CaseResult {
    let f = Tree::new()?;
    f.dir("dir")?;
    f.file("a", b"data")?;
    let s = f.sym("dir/sym", "../a")?;
    process::chdir(b"/\0")?;
    bytes(&s, b"data")
}

fn symlinks_dangling() -> CaseResult {
    let f = Tree::new()?;
    let s = f.sym("sym", "absent")?;
    check(
        stat(&s, true)?.is_symlink(),
        "lstat did not report a symlink",
    )?;
    errno(observed::open(&s, O_RDONLY), 2)
}

fn symlinks_existing() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("a", b"stay")?;
    errno(observed::symlink("absent", &p), 17)?;
    bytes(&p, b"stay")
}

fn symlinks_readlink() -> CaseResult {
    let f = Tree::new()?;
    let p = f.sym("sym", "target")?;
    let mut b = [0xa5; 16];
    let n = fs::readlink(&p, &mut b)?;
    check(
        n == 6 && &b[..6] == b"target" && b[6..].iter().all(|v| *v == 0xa5),
        "readlink bytes, count or terminator differ",
    )
}

fn symlinks_truncate() -> CaseResult {
    let f = Tree::new()?;
    let p = f.sym("sym", "long-target")?;
    let mut b = [0xa5; 5];
    let n = fs::readlink(&p, &mut b[..4])?;
    check(
        n == 4 && &b[..4] == b"long" && b[4] == 0xa5,
        "readlink truncation differs",
    )
}

fn symlinks_readlink_regular() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("a", b"x")?;
    errno(observed::readlink(&p, &mut [0; 16]), 22)
}

fn symlinks_self_loop() -> CaseResult {
    let f = Tree::new()?;
    let p = f.sym("sym", "sym")?;
    errno(observed::open(&p, O_RDONLY), 40)
}

fn symlinks_mutual_loop() -> CaseResult {
    let f = Tree::new()?;
    let p = f.sym("a", "b")?;
    f.sym("b", "a")?;
    errno(observed::open(&p, O_RDONLY), 40)
}

fn symlinks_nofollow() -> CaseResult {
    let f = Tree::new()?;
    let a = f.file("a", b"x")?;
    let s = f.sym("sym", &a)?;
    errno(observed::open(&s, O_RDONLY | O_NOFOLLOW), 40)
}

fn symlinks_nofollow_prefix() -> CaseResult {
    let f = Tree::new()?;
    let d = f.dir("dir")?;
    f.file("dir/a", b"data")?;
    let sym = f.sym("sym", &d)?;
    errno(observed::open(&sym, O_RDONLY | O_NOFOLLOW), 40)?;
    let fd = File::open(&f.path("sym/a"), O_RDONLY | O_NOFOLLOW)?;
    contents(fd.fd(), b"data")
}

fn symlinks_unlink_link() -> CaseResult {
    let f = Tree::new()?;
    let a = f.file("a", b"stay")?;
    let p = f.sym("sym", &a)?;
    fs::unlink(&p)?;
    errno(observed::stat(&p, true), 2)?;
    bytes(&a, b"stay")
}

fn rename_file() -> CaseResult {
    let f = Tree::new()?;
    let a = f.file("a", b"data")?;
    let ino = stat(&a, false)?.st_ino;
    let b = f.path("b");
    fs::rename(&a, &b)?;
    errno(observed::stat(&a, false), 2)?;
    check(stat(&b, false)?.st_ino == ino, "rename changed inode")?;
    bytes(&b, b"data")
}

fn rename_replace_open() -> CaseResult {
    let f = Tree::new()?;
    let a = f.file("a", b"new")?;
    let b = f.file("b", b"old")?;
    let old = File::open(&b, O_RDONLY)?;
    let new = File::open(&a, O_RDONLY)?;
    fs::rename(&a, &b)?;
    errno(observed::stat(&a, false), 2)?;
    check(
        stat(&b, false)?.st_ino == fs::fstat(new.fd())?.st_ino
            && fs::fstat(old.fd())?.st_nlink == 0,
        "replacement inode or old nlink differs",
    )?;
    bytes(&b, b"new")?;
    contents(old.fd(), b"old")
}

fn rename_atomic_replace() -> CaseResult {
    atomic_replace()
}

fn rename_empty_dir() -> CaseResult {
    let f = Tree::new()?;
    let a = f.dir("a")?;
    let ino = stat(&a, false)?.st_ino;
    f.file("a/file", b"stay")?;
    let b = f.dir("b")?;
    let out = f.path("b/file");
    fs::rename(&a, &b)?;
    errno(observed::stat(&a, false), 2)?;
    check(
        stat(&b, false)?.st_ino == ino,
        "directory replacement changed inode",
    )?;
    bytes(&out, b"stay")
}

fn rename_nonempty_dir() -> CaseResult {
    let f = Tree::new()?;
    let a = f.dir("a")?;
    let b = f.dir("b")?;
    let c = f.file("b/file", b"stay")?;
    errno_one_of(observed::rename(&a, &b), &[17, 39])?;
    check(stat(&a, false)?.is_dir(), "source disappeared")?;
    bytes(&c, b"stay")
}

fn rename_cross_directory() -> CaseResult {
    let f = Tree::new()?;
    f.dir("a")?;
    f.dir("b")?;
    let src = f.file("a/file", b"data")?;
    let dst = f.path("b/file");
    fs::rename(&src, &dst)?;
    errno(observed::stat(&src, false), 2)?;
    bytes(&dst, b"data")
}

fn rename_directory_parent() -> CaseResult {
    let f = Tree::new()?;
    let a = f.dir("a")?;
    let b = f.dir("b")?;
    let src = f.dir("a/dir")?;
    let dst = f.path("b/dir");
    let an = stat(&a, false)?.st_nlink;
    let bn = stat(&b, false)?.st_nlink;
    fs::rename(&src, &dst)?;
    let parent_ino = stat(&b, false)?.st_ino;
    check(
        Directory::open(&dst)?
            .all()?
            .iter()
            .any(|e| e.name == ".." && e.ino == parent_ino),
        "dot-dot still names old parent",
    )?;
    check(
        stat(&a, false)?.st_nlink == an - 1 && stat(&b, false)?.st_nlink == bn + 1,
        "parent nlink not transferred",
    )
}

fn rename_same_name() -> CaseResult {
    let f = Tree::new()?;
    let a = f.file("a", b"stay")?;
    fs::rename(&a, &a)?;
    bytes(&a, b"stay")
}

fn rename_same_inode() -> CaseResult {
    let f = Tree::new()?;
    let a = f.file("a", b"stay")?;
    let b = f.path("b");
    fs::link(&a, &b)?;
    fs::rename(&a, &b)?;
    check(
        stat(&a, false)?.st_nlink == 2 && stat(&a, false)?.st_ino == stat(&b, false)?.st_ino,
        "same-inode rename removed a name",
    )
}

fn rename_file_to_dir() -> CaseResult {
    let f = Tree::new()?;
    let a = f.file("a", b"stay")?;
    let b = f.dir("b")?;
    errno(observed::rename(&a, &b), 21)?;
    bytes(&a, b"stay")
}

fn rename_dir_to_file() -> CaseResult {
    let f = Tree::new()?;
    let a = f.dir("a")?;
    let b = f.file("b", b"stay")?;
    errno(observed::rename(&a, &b), 20)?;
    bytes(&b, b"stay")
}

fn rename_descendant() -> CaseResult {
    let f = Tree::new()?;
    let a = f.dir("a")?;
    f.dir("a/dir")?;
    errno(observed::rename(&a, &f.path("a/dir/moved")), 22)
}

fn rename_symlink() -> CaseResult {
    let f = Tree::new()?;
    let a = f.sym("a", "absent")?;
    let b = f.path("b");
    fs::rename(&a, &b)?;
    errno(observed::stat(&a, true), 2)?;
    let mut buf = [0; 16];
    let n = fs::readlink(&b, &mut buf)?;
    check(
        stat(&b, true)?.is_symlink() && &buf[..n] == b"absent",
        "rename followed or changed symlink",
    )
}

fn rename_missing_source() -> CaseResult {
    let f = Tree::new()?;
    let b = f.file("b", b"stay")?;
    errno(observed::rename(&f.path("absent"), &b), 2)?;
    bytes(&b, b"stay")
}

fn cwd_absolute() -> CaseResult {
    let f = Tree::new()?;
    process::chdir(&cpath(&f.root))?;
    check(cwd()? == f.root, "getcwd differs from chdir target")
}

fn cwd_relative() -> CaseResult {
    let f = Tree::new()?;
    f.dir("dir")?;
    f.file("dir/file", b"data")?;
    process::chdir(&cpath(&f.root))?;
    process::chdir(b"dir\0")?;
    bytes("file", b"data")
}

fn cwd_parent() -> CaseResult {
    let f = Tree::new()?;
    let p = f.dir("dir")?;
    process::chdir(&cpath(&p))?;
    process::chdir(b"..\0")?;
    check(
        stat(".", false)?.st_ino == stat(&f.root, false)?.st_ino,
        "dot-dot inode differs",
    )?;
    check(cwd()? == f.root, "dot-dot cwd differs")
}

fn cwd_relative_create() -> CaseResult {
    let f = Tree::new()?;
    let a = f.path("a");
    let b = f.path("b");
    process::chdir(&cpath(&f.root))?;
    fs::mkdir("a", 0o755)?;
    fs::rename("a", "b")?;
    errno(observed::stat(&a, false), 2)?;
    check(
        stat(&b, false)?.is_dir(),
        "relative rename destination missing",
    )
}

fn cwd_renamed() -> CaseResult {
    let f = Tree::new()?;
    let a = f.dir("a")?;
    let b = f.path("b");
    process::chdir(&cpath(&a))?;
    fs::rename(&a, &b)?;
    check(cwd()? == b, "getcwd retained old name")
}

fn cwd_ancestor_renamed() -> CaseResult {
    let f = Tree::new()?;
    let a = f.dir("a")?;
    let d = f.dir("a/dir")?;
    let b = f.path("b");
    f.path("b/dir");
    process::chdir(&cpath(&d))?;
    fs::rename(&a, &b)?;
    check(cwd()? == format!("{b}/dir"), "getcwd retained old ancestor")
}

fn cwd_removed() -> CaseResult {
    let f = Tree::new()?;
    let d = f.dir("dir")?;
    process::chdir(&cpath(&d))?;
    // POSIX rmdir permits EBUSY for a cwd; successful removal makes getcwd
    // ENOENT a Linux ABI policy check, rather than a universal POSIX rule.
    match fs::rmdir(&d) {
        Err(Error::Os(libbreenix::Errno::EBUSY)) => {
            check(
                stat(".", false)?.st_ino == stat(&d, false)?.st_ino,
                "EBUSY changed cwd",
            )?;
            check(cwd()? == d, "EBUSY changed pathname")
        }
        other => {
            other?;
            errno(observed::getcwd(&mut [0; 256]), 2)
        }
    }
}

fn cwd_small_buffer() -> CaseResult {
    let f = Tree::new()?;
    process::chdir(&cpath(&f.root))?;
    // The pathname fits but its NUL does not; nothing may be written.
    let mut buf = [0xa5; 256];
    errno(observed::getcwd(&mut buf[..f.root.len()]), 34)?;
    check(buf.iter().all(|v| *v == 0xa5), "getcwd wrote into a buffer it refused")
}

fn cwd_not_dir() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    process::chdir(&cpath(&f.root))?;
    errno(observed::chdir(&cpath(&p)), 20)?;
    check(
        stat(".", false)?.st_ino == stat(&f.root, false)?.st_ino,
        "failed chdir changed cwd inode",
    )?;
    check(cwd()? == f.root, "failed chdir changed cwd")
}

fn cwd_missing() -> CaseResult {
    let f = Tree::new()?;
    process::chdir(&cpath(&f.root))?;
    errno(observed::chdir(&cpath(&f.path("absent"))), 2)?;
    check(
        stat(".", false)?.st_ino == stat(&f.root, false)?.st_ino,
        "failed chdir changed cwd inode",
    )?;
    check(cwd()? == f.root, "failed chdir changed cwd")
}

fn permissions_chmod_file() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    #[cfg(target_arch = "x86_64")]
    {
        let c = cpath(&p);
        request(90, [c.as_ptr() as u64, 0o600, 0, 0])?;
        check(stat(&p, false)?.st_mode & 0o7777 == 0o600, "legacy chmod failed")?;
    }
    chmod(&p, 0o654)?;
    check(
        stat(&p, false)?.st_mode & 0o7777 == 0o654,
        "chmod mode differs",
    )?;
    let fd = File::open(&p, O_RDONLY)?;
    request(nr::FCHMOD, [fd.fd().raw(), 0xffff, 0, 0])?;
    let mode = fs::fstat(fd.fd())?.st_mode;
    check(mode & 0o170000 == 0o100000 && mode & 0o7777 == 0o7777,
        "fchmod changed type or failed to mask mode")?;
    let link = f.sym("link", &p)?;
    chmod(&link, 0o640)?;
    check(stat(&p, false)?.st_mode & 0o7777 == 0o640 && stat(&link, true)?.is_symlink(),
        "chmod did not follow final symlink")?;
    fs::unlink(&p)?;
    request(nr::FCHMOD, [fd.fd().raw(), 0o600, 0, 0])?;
    check(fs::fstat(fd.fd())?.st_mode & 0o7777 == 0o600, "fchmod lost unlinked inode")
}

fn permissions_chmod_dir() -> CaseResult {
    let f = Tree::new()?;
    let p = f.dir("dir")?;
    chmod(&p, 0o711)?;
    check(
        stat(&p, false)?.st_mode & 0o777 == 0o711,
        "directory chmod mode differs",
    )?;
    let fd = File::open(&p, O_RDONLY | O_DIRECTORY)?;
    request(nr::FCHMOD, [fd.fd().raw(), 0o2750, 0, 0])?;
    check(fs::fstat(fd.fd())?.st_mode & 0o177777 == 0o42750, "fchmod directory mode or type differs")?;
    // A counted directory handle follows the existing cwd EBUSY policy.
    errno(observed::rmdir(&p), 16)?;
    request(nr::FCHOWN, [fd.fd().raw(), 1001, 1001, 0])?;
    unprivileged()?;
    request(nr::FCHOWN, [fd.fd().raw(), u32::MAX as u64, 1001, 0])?;
    check(fs::fstat(fd.fd())?.st_mode & 0o2000 != 0, "directory chown cleared SGID")
}

fn permissions_chmod_missing() -> CaseResult {
    let f = Tree::new()?;
    errno(observed::chmod(&f.path("absent"), 0o600), 2)
}

fn permissions_chown() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    #[cfg(target_arch = "x86_64")]
    {
        let c = cpath(&p);
        request(92, [c.as_ptr() as u64, 1111, 2222, 0])?;
        let st = stat(&p, false)?;
        check(st.st_uid == 1111 && st.st_gid == 2222, "legacy chown failed")?;
    }
    chown(&p, 1234, 2345)?;
    let s = stat(&p, false)?;
    check(
        s.st_uid == 1234 && s.st_gid == 2345,
        "chown ownership differs",
    )?;
    let fd = File::open(&p, O_RDONLY)?;
    request(nr::FCHOWN, [fd.fd().raw(), 0x12345678, 0x23456789, 0])?;
    let owned = fs::fstat(fd.fd())?;
    check(owned.st_uid == 0x12345678 && owned.st_gid == 0x23456789, "fchown truncated ownership IDs")?;
    let link = f.sym("link", &p)?;
    let c = cpath(&link);
    #[cfg(target_arch = "x86_64")]
    {
        request(94, [c.as_ptr() as u64, 2222, 3333, 0])?;
        let st = stat(&link, true)?;
        check(st.st_uid == 2222 && st.st_gid == 3333 && stat(&p, false)?.st_uid == 0x12345678,
            "legacy lchown followed target or failed ownership")?;
    }
    // SAFETY: c stays NUL-terminated through fchownat.
    let ret = unsafe { raw::syscall5(nr::FCHOWNAT, AT_FDCWD, c.as_ptr() as u64, 3456, 4567, 0x100) };
    Error::from_syscall(ret as i64)?;
    let l = stat(&link, true)?;
    check(l.st_uid == 3456 && l.st_gid == 4567 && l.is_symlink(), "lchown followed final symlink")?;
    check(stat(&p, false)?.st_uid == 0x12345678, "lchown changed target ownership")?;
    chmod(&p, 0o6755)?;
    chown(&link, 1234, 2345)?;
    let target = stat(&p, false)?;
    check(target.st_uid == 1234 && target.st_gid == 2345 && target.st_mode & 0o6000 == 0,
        "chown failed to follow symlink or clear privilege bits")
}

fn permissions_chown_unchanged() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    chown(&p, 1234, 2345)?;
    chown(&p, u32::MAX, 3456)?;
    let s = stat(&p, false)?;
    check(
        s.st_uid == 1234 && s.st_gid == 3456,
        "minus-one UID was not preserved",
    )?;
    chown(&p, 4567, u32::MAX)?;
    let s = stat(&p, false)?;
    check(
        s.st_uid == 4567 && s.st_gid == 3456,
        "minus-one GID was not preserved",
    )?;
    chown(&p, 1001, 2001)?;
    chmod(&p, 0o6755)?;
    let fd = File::open(&p, O_RDWR)?;
    let fifo_path = f.path("fifo");
    fs::mkfifo(&fifo_path, 0o7777)?;
    check(stat(&fifo_path, false)?.st_mode & 0o7777 == 0o7777, "mkfifo discarded special mode bits")?;
    chown(&fifo_path, 1001, 1001)?;
    chmod(&fifo_path, 0)?;
    unprivileged()?;
    let symlink = f.sym("owned-link", &p)?;
    check(stat(&symlink, true)?.st_uid == 1001 && stat(&symlink, true)?.st_gid == 1001,
        "symlink did not inherit creator ownership")?;
    let link_c = cpath(&symlink);
    // SAFETY: NUL-terminated link path remains alive through fchownat.
    Error::from_syscall(unsafe { raw::syscall5(nr::FCHOWNAT, AT_FDCWD,
        link_c.as_ptr() as u64, u32::MAX as u64, 1001, 0x100) } as i64)?;
    errno(observed::open(&fifo_path, O_RDONLY | O_NONBLOCK), 13)?;
    chmod(&p, 0o6755)?;
    check(stat(&p, false)?.st_mode & 0o6000 == 0o4000, "unprivileged chmod retained nonmember SGID")?;
    request(nr::FCHOWN, [fd.fd().raw(), u32::MAX as u64, 1001, 0])?;
    check(stat(&p, false)?.st_mode & 0o6000 == 0, "unprivileged chown retained privilege bits")?;
    let c = cpath(&p);
    // SAFETY: c stays alive through the syscall.
    let denied = unsafe { raw::syscall5(nr::FCHOWNAT, AT_FDCWD, c.as_ptr() as u64, 2001, u32::MAX as u64, 0) as i64 };
    errno(denied, 1)?;
    check(stat(&p, false)?.st_uid == 1001, "denied chown changed owner")?;
    chmod(&p, 0o6610)?;
    write_all(fd.fd(), b"y")?;
    check(stat(&p, false)?.st_mode & 0o6000 == 0, "unprivileged write retained privilege bits")?;
    chmod(&p, 0o6600)?;
    write_all(fd.fd(), b"z")?;
    check(stat(&p, false)?.st_mode & 0o6000 == 0o2000, "write cleared non-executable SGID")?;
    chmod(&p, 0o6610)?;
    request(nr::FTRUNCATE, [fd.fd().raw(), 0, 0, 0])?;
    check(stat(&p, false)?.st_mode & 0o6000 == 0, "unprivileged truncate retained privilege bits")?;
    chmod(&p, 0o6600)?;
    request(nr::FTRUNCATE, [fd.fd().raw(), 0, 0, 0])?;
    check(stat(&p, false)?.st_mode & 0o6000 == 0o2000, "truncate cleared non-executable SGID")
}

fn permissions_umask_return() -> CaseResult {
    umask(0o027)?;
    check(
        umask(0o7022)? == 0o027 && umask(0)? == 0o022,
        "umask old value or mask bits differ",
    )?;
    umask(0o027)?;
    let groups = [1001u32, 2345];
    request(nr::SETGROUPS, [2, groups.as_ptr() as u64, 0, 0])?;
    match process::fork()? {
        process::ForkResult::Child => {
            if umask(0o007).ok() != Some(0o027) { process::exit(1); }
            if umask(0o027).ok() != Some(0o007) { process::exit(2); }
            let path = b"/usr/local/test/bin/umask_exec_test\0";
            let argv = [path.as_ptr(), core::ptr::null()];
            let _ = process::execv(path, argv.as_ptr());
            process::exit(3);
        }
        process::ForkResult::Parent(pid) => {
            let mut status = 0;
            let waited = process::waitpid(pid.raw() as i32, &mut status, 0)?;
            check(waited == pid && process::wifexited(status) && process::wexitstatus(status) == 0,
                "fork/exec lost umask or supplementary groups")?;
        }
    }
    check(umask(0)? == 0o027, "child changed parent's umask")?;
    let mut actual = [0u32; 2];
    check(request(nr::GETGROUPS, [2, actual.as_mut_ptr() as u64, 0, 0])? == 2 && actual == groups,
        "child changed parent's supplementary groups")?;
    // Empty-path AT_FDCWD must select cwd instead of looking up fd -100.
    let f = Tree::new()?;
    process::chdir(&cpath(&f.root))?;
    let empty = b"\0";
    // SAFETY: empty is NUL-terminated and alive through fchownat.
    Error::from_syscall(unsafe { raw::syscall5(nr::FCHOWNAT, AT_FDCWD,
        empty.as_ptr() as u64, 1001, 1001, 0x1000) } as i64)?;
    check(stat(".", false)?.st_uid == 1001, "empty-path chown did not select cwd")?;
    let cwd_fd = File::open(&f.root, O_RDONLY | O_DIRECTORY)?;
    request(ABI[4], [1001, 0, 0, 0])?;
    request(ABI[3], [1001, 0, 0, 0])?;
    request(nr::FCHMOD, [cwd_fd.fd().raw(), 0, 0, 0])?;
    // SAFETY: empty is NUL-terminated and alive through the syscall. No
    // pathname search is permitted here; ownership still authorizes the change.
    Error::from_syscall(unsafe { raw::syscall5(nr::FCHOWNAT, AT_FDCWD,
        empty.as_ptr() as u64, u32::MAX as u64, 1001, 0x1000) } as i64)?;
    request(nr::FCHMOD, [cwd_fd.fd().raw(), 0o777, 0, 0])?;
    umask(0o027)?;
    let path = b"/usr/local/test/bin/umask_exec_test\0";
    let arg = b"unprivileged\0";
    let argv = [path.as_ptr(), arg.as_ptr(), core::ptr::null()];
    let pid = process::spawnv(path, argv.as_ptr())?;
    let mut status = 0;
    let waited = process::waitpid(pid.raw() as i32, &mut status, 0)?;
    check(waited == pid && process::wifexited(status) && process::wexitstatus(status) == 0,
        "spawn/clone lost credentials, umask or groups")
}

fn permissions_umask_file() -> CaseResult {
    let f = Tree::new()?;
    umask(0o027)?;
    let p = f.path("file");
    let fd = fs::open_with_mode(&p, O_CREAT | O_EXCL | O_RDWR, 0o666)?;
    io::close(fd)?;
    check(
        stat(&p, false)?.st_mode & 0o777 == 0o640,
        "file creation ignored umask",
    )?;
    for (name, mode, expected) in [("requested", 0o623, 0o600), ("zero", 0, 0)] {
        let p = f.path(name);
        let fd = fs::open_with_mode(&p, O_CREAT | O_EXCL | O_RDWR, mode)?;
        check(fs::fstat(fd)?.st_mode & 0o7777 == expected, "open ignored requested mode")?;
        io::close(fd)?;
    }
    let fifo = f.path("fifo");
    let c = cpath(&fifo);
    request(nr::MKNODAT, [AT_FDCWD, c.as_ptr() as u64, 0o10777, 0])?;
    check(stat(&fifo, false)?.st_mode & 0o777 == 0o750, "mkfifo ignored umask")?;
    let fd = File::open(&fifo, O_RDONLY | O_NONBLOCK)?;
    check(fs::fstat(fd.fd())?.st_mode & 0o777 == 0o750, "FIFO fstat ignored creation mode")?;
    fs::unlink(&fifo)?;
    check(fs::fstat(fd.fd())?.st_mode & 0o777 == 0o750, "FIFO lost mode after unlink")?;
    let duplicate = io::dup(fd.fd())?;
    fs::mkfifo(&fifo, 0o666)?;
    let reader = File::open(&fifo, O_RDONLY | O_NONBLOCK)?;
    drop(fd);
    io::close(duplicate)?;
    let writer = File::open(&fifo, O_WRONLY | O_NONBLOCK)?;
    write_all(writer.fd(), b"r")?;
    let mut byte = [0u8];
    check(io::read(reader.fd(), &mut byte)? == 1 && byte == *b"r",
        "closing old FIFO changed replacement readers or buffer")
}

fn permissions_umask_dir() -> CaseResult {
    let f = Tree::new()?;
    umask(0o027)?;
    let p = f.path("dir");
    fs::mkdir(&p, 0o777)?;
    check(
        stat(&p, false)?.st_mode & 0o777 == 0o750,
        "mkdir ignored umask",
    )?;
    for (name, mode, expected) in [("requested", 0o721, 0o700), ("zero", 0, 0)] {
        let p = f.path(name);
        fs::mkdir(&p, mode)?;
        check(stat(&p, false)?.st_mode & 0o777 == expected, "mkdir ignored requested mode")?;
    }
    Ok(())
}

fn permissions_chmod_umask() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    umask(0o777)?;
    chmod(&p, 0o765)?;
    check(
        stat(&p, false)?.st_mode & 0o777 == 0o765,
        "chmod applied umask",
    )
}

fn permissions_access_exists() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    chmod(&p, 0)?;
    unprivileged()?;
    fs::access(&p, F_OK)?;
    errno(observed::access(&f.path("absent"), F_OK), 2)
}

fn permissions_access_owner() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    chown(&p, 1001, 1001)?;
    chmod(&p, 0o400)?;
    unprivileged()?;
    fs::access(&p, R_OK)?;
    errno(observed::access(&p, W_OK), 13)?;
    errno(observed::access(&p, X_OK), 13)?;
    errno(observed::access(&p, W_OK | X_OK), 13)
}

fn permissions_access_group() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    chown(&p, 2001, 1001)?;
    chmod(&p, 0o40)?;
    unprivileged()?;
    fs::access(&p, R_OK)?;
    errno(observed::access(&p, W_OK), 13)?;
    errno(observed::access(&p, X_OK), 13)?;
    errno(observed::access(&p, W_OK | X_OK), 13)
}

fn permissions_access_other() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    chown(&p, 2001, 2001)?;
    chmod(&p, 0o4)?;
    unprivileged()?;
    fs::access(&p, R_OK)?;
    errno(observed::access(&p, W_OK), 13)?;
    errno(observed::access(&p, X_OK), 13)?;
    errno(observed::access(&p, W_OK | X_OK), 13)
}

fn permissions_access_execute() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    chown(&p, 1001, 1001)?;
    chmod(&p, 0o100)?;
    unprivileged()?;
    fs::access(&p, X_OK)?;
    errno(observed::access(&p, R_OK), 13)?;
    errno(observed::access(&p, W_OK), 13)?;
    errno(observed::access(&p, R_OK | W_OK), 13)
}

fn permissions_search_denied() -> CaseResult {
    let f = Tree::new()?;
    let d = f.dir("dir")?;
    let p = f.file("dir/file", b"x")?;
    chmod(&d, 0o666)?;
    unprivileged()?;
    errno(observed::access(&p, F_OK), 13)
}

fn permissions_root_execute() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    check(
        stat(&p, false)?.st_mode & 0o111 == 0,
        "fixture unexpectedly executable",
    )?;
    errno(observed::access(&p, X_OK), 13)?;
    let fifo = f.path("fifo");
    fs::mkfifo(&fifo, 0o600)?;
    errno(observed::access(&fifo, X_OK), 13)
}

fn timestamps_explicit() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    set_times(&p, [(1000000000, 0), (1000000001, 0)], 0)?;
    let s = stat(&p, false)?;
    check(
        atime(&s) == (1000000000, 0) && mtime(&s) == (1000000001, 0),
        "explicit times differ",
    )
}

fn timestamps_now() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    old_times(&p)?;
    let before = realtime()?;
    set_times(&p, [(0, UTIME_NOW), (0, UTIME_NOW)], 0)?;
    let after = realtime()?;
    let s = stat(&p, false)?;
    check(
        in_window(atime(&s), before, after) && in_window(mtime(&s), before, after),
        "UTIME_NOW outside clock window",
    )
}

fn timestamps_omit_atime() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    old_times(&p)?;
    let old = stat(&p, false)?;
    set_times(&p, [(0, UTIME_OMIT), (1000000002, 0)], 0)?;
    let s = stat(&p, false)?;
    check(
        atime(&s) == atime(&old) && mtime(&s) == (1000000002, 0),
        "OMIT atime or explicit mtime differs",
    )
}

fn timestamps_omit_mtime() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    old_times(&p)?;
    let old = stat(&p, false)?;
    set_times(&p, [(1000000002, 0), (0, UTIME_OMIT)], 0)?;
    let s = stat(&p, false)?;
    check(
        mtime(&s) == mtime(&old) && atime(&s) == (1000000002, 0),
        "OMIT mtime or explicit atime differs",
    )
}

fn timestamps_omit_both() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    old_times(&p)?;
    let old = stat(&p, false)?;
    next_second(&old)?;
    // POSIX.1-2024 says ctime need not be marked for update with two OMITs.
    // This case measures Linux policy (unchanged ctime), not a POSIX mandate.
    // https://pubs.opengroup.org/onlinepubs/9799919799/functions/utimensat.html
    set_times(&p, [(0, UTIME_OMIT), (0, UTIME_OMIT)], 0)?;
    let s = stat(&p, false)?;
    check(
        atime(&s) == atime(&old) && mtime(&s) == mtime(&old) && ctime(&s) == ctime(&old),
        "two OMIT values changed timestamps",
    )
}

fn timestamps_now_omit() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    old_times(&p)?;
    let old = stat(&p, false)?;
    let before = realtime()?;
    set_times(&p, [(0, UTIME_NOW), (0, UTIME_OMIT)], 0)?;
    let after = realtime()?;
    let s = stat(&p, false)?;
    check(
        in_window(atime(&s), before, after) && mtime(&s) == mtime(&old),
        "mixed NOW and OMIT differ",
    )
}

fn timestamps_invalid_nsec() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    errno(observed::set_times(&p, [(0, 1000000000), (0, 0)], 0), 22)
}

fn timestamps_missing() -> CaseResult {
    let f = Tree::new()?;
    errno(
        observed::set_times(&f.path("absent"), [(1, 0), (2, 0)], 0),
        2,
    )
}

fn timestamps_symlink_follow() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    let s = f.sym("sym", &p)?;
    set_times(&s, [(1000000000, 0), (1000000001, 0)], 0)?;
    check(
        mtime(&stat(&p, false)?) == (1000000001, 0),
        "utimensat did not update target",
    )
}

fn timestamps_symlink_nofollow() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    old_times(&p)?;
    let old = stat(&p, false)?;
    let s = f.sym("sym", &p)?;
    set_times(&s, [(1000000003, 0), (1000000004, 0)], 256)?;
    check(
        mtime(&stat(&s, true)?) == (1000000004, 0) && mtime(&stat(&p, false)?) == mtime(&old),
        "nofollow changed target or missed link",
    )
}

fn timestamps_write_update() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    old_times(&p)?;
    let fd = File::open(&p, O_WRONLY)?;
    let old = stat(&p, false)?;
    let before = next_second(&old)?;
    write_all(fd.fd(), b"y")?;
    drop(fd);
    let after = realtime()?;
    let s = stat(&p, false)?;
    check(
        mtime(&s) > mtime(&old)
            && ctime(&s) > ctime(&old)
            && in_window(mtime(&s), before, after)
            && in_window(ctime(&s), before, after),
        "write timestamps not current",
    )
}

fn timestamps_parent_create() -> CaseResult {
    let f = Tree::new()?;
    old_times(&f.root)?;
    let old = stat(&f.root, false)?;
    let before = next_second(&old)?;
    f.file("file", b"x")?;
    let after = realtime()?;
    let s = stat(&f.root, false)?;
    check(
        mtime(&s) > mtime(&old)
            && ctime(&s) > ctime(&old)
            && in_window(mtime(&s), before, after)
            && in_window(ctime(&s), before, after),
        "create parent times not current",
    )
}

fn timestamps_parent_unlink() -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    old_times(&f.root)?;
    let old = stat(&f.root, false)?;
    let before = next_second(&old)?;
    fs::unlink(&p)?;
    let after = realtime()?;
    let s = stat(&f.root, false)?;
    check(
        mtime(&s) > mtime(&old)
            && ctime(&s) > ctime(&old)
            && in_window(mtime(&s), before, after)
            && in_window(ctime(&s), before, after),
        "unlink parent times not current",
    )
}

fn timestamps_parent_rename() -> CaseResult {
    let f = Tree::new()?;
    let a = f.dir("a")?;
    let b = f.dir("b")?;
    let src = f.file("a/file", b"x")?;
    let dst = f.path("b/file");
    old_times(&a)?;
    old_times(&b)?;
    let olds = [stat(&a, false)?, stat(&b, false)?];
    next_second(&olds[0])?;
    let before = next_second(&olds[1])?;
    fs::rename(&src, &dst)?;
    let after = realtime()?;
    for (p, old) in [&a, &b].into_iter().zip(&olds) {
        let s = stat(p, false)?;
        check(
            mtime(&s) > mtime(old)
                && ctime(&s) > ctime(old)
                && in_window(mtime(&s), before, after)
                && in_window(ctime(&s), before, after),
            "rename parent times not current",
        )?;
    }
    Ok(())
}

fn links_unlink_directory() -> CaseResult {
    let f = Tree::new()?;
    let d = f.dir("dir")?;
    errno_one_of(observed::unlink(&d), &[1, 21])?;
    check(stat(&d, false)?.is_dir(), "unlink damaged directory")
}
fn cwd_count_abi() -> CaseResult {
    let f = Tree::new()?;
    process::chdir(&cpath(&f.root))?;
    // A buffer that exactly fits the pathname and its NUL; the bytes past it
    // must be untouched.
    let mut buf = [0xa5; 256];
    let size = f.root.len() + 1;
    let n = process::getcwd(&mut buf[..size])?;
    check(
        n == size && buf[f.root.len()] == 0,
        "Linux getcwd byte count excludes NUL",
    )?;
    check(&buf[..f.root.len()] == f.root.as_bytes(), "getcwd pathname differs")?;
    check(buf[size..].iter().all(|v| *v == 0xa5), "getcwd wrote past its buffer")
}
fn symlink_tree() -> Result<(Tree, String, String), libbreenix::suite::CaseError> {
    let f = Tree::new()?;
    let parent = f.dir("real")?;
    let target = f.dir("real/child")?;
    f.sym("sym", &target)?;
    f.file("real/file", b"physical")?;
    f.file("file", b"textual")?;
    Ok((f, parent, target))
}
fn cwd_symlink_parent_open() -> CaseResult {
    let (f, _, _) = symlink_tree()?;
    bytes(&f.path("sym/../file"), b"physical")
}
fn cwd_symlink_parent_chdir() -> CaseResult {
    let (f, parent, _) = symlink_tree()?;
    process::chdir(&cpath(&f.path("sym/..")))?;
    check(
        stat(".", false)?.st_ino == stat(&parent, false)?.st_ino,
        "chdir resolved dot-dot textually",
    )?;
    check(cwd()? == parent, "symlink parent pathname differs")
}
fn cwd_symlink_physical() -> CaseResult {
    let (f, _, target) = symlink_tree()?;
    process::chdir(&cpath(&f.path("sym")))?;
    check(
        stat(".", false)?.st_ino == stat(&target, false)?.st_ino,
        "chdir missed symlink target",
    )?;
    check(cwd()? == target, "getcwd retained symlink component")
}
fn access_class_denied(uid: u32, gid: u32, mode: u32) -> CaseResult {
    let f = Tree::new()?;
    let p = f.file("file", b"x")?;
    chown(&p, uid, gid)?;
    chmod(&p, mode)?;
    unprivileged()?;
    for bit in [R_OK, W_OK, X_OK] {
        errno(observed::access(&p, bit), 13)?;
    }
    Ok(())
}
fn permissions_access_owner_denied() -> CaseResult {
    access_class_denied(1001, 1001, 0o077)
}
fn permissions_access_group_denied() -> CaseResult {
    access_class_denied(2001, 1001, 0o707)
}
fn permissions_access_other_denied() -> CaseResult {
    access_class_denied(2001, 2001, 0o770)
}
fn timestamps_directory() -> CaseResult {
    let f = Tree::new()?;
    let d = f.dir("dir")?;
    old_times(&d)
}
