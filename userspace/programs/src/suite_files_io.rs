//! POSIX Files & I/O effort suite. Each case runs in the shared runner's child.
//! Fixtures live on the writable root filesystem, not an in-memory mock. No
//! unsupported operation is skipped: missing syscalls fail.
use libbreenix::error::Error;
use libbreenix::suite::{case, category, check, fail, suite, CaseError, CaseResult, Suite};
use libbreenix::syscall::{nr, raw};
use libbreenix::types::Fd;
use libbreenix::{
    fs::{self, *},
    io, memory, process, signal, time,
};
use std::sync::atomic::{AtomicUsize, Ordering};

const BAD: Fd = Fd::from_raw(10000);
const CLOEXEC: u32 = 0x80000;
// Linux ABI numbers not yet exposed by libbreenix. Sending the real request
// lets ENOSYS be scored as a failure without adding wrappers or kernel code.
#[cfg(target_arch = "x86_64")]
const PREAD: u64 = 17;
#[cfg(target_arch = "aarch64")]
const PREAD: u64 = 67;
#[cfg(target_arch = "x86_64")]
const PWRITE: u64 = 18;
#[cfg(target_arch = "aarch64")]
const PWRITE: u64 = 68;
#[cfg(target_arch = "x86_64")]
const FTRUNCATE: u64 = 77;
#[cfg(target_arch = "aarch64")]
const FTRUNCATE: u64 = 46;
#[cfg(target_arch = "x86_64")]
const TRUNCATE: u64 = 76;
#[cfg(target_arch = "aarch64")]
const TRUNCATE: u64 = 45;
#[cfg(target_arch = "x86_64")]
const MSYNC: u64 = 26;
#[cfg(target_arch = "aarch64")]
const MSYNC: u64 = 227;
#[cfg(target_arch = "x86_64")]
const SETUID: u64 = 105;
#[cfg(target_arch = "aarch64")]
const SETUID: u64 = 146;

#[cfg(target_arch = "x86_64")]
const FSYNC: u64 = 74;
#[cfg(target_arch = "aarch64")]
const FSYNC: u64 = 82;
#[cfg(target_arch = "x86_64")]
const FDATASYNC: u64 = 75;
#[cfg(target_arch = "aarch64")]
const FDATASYNC: u64 = 83;

fn sc(n: u64, a: u64, b: u64, c: u64, d: u64, operation: &str) -> Result<u64, String> {
    // SAFETY: callers supply the ABI arguments and live buffers for this request.
    let ret = unsafe { raw::syscall4(n, a, b, c, d) } as i64;
    if ret < 0 {
        Err(format!("{operation} returned errno {}", -ret))
    } else {
        Ok(ret as u64)
    }
}
fn expect_errno<T, E: std::fmt::Display>(r: Result<T, E>, errno: i64, op: &str) -> CaseResult {
    match r {
        Ok(_) => fail(format!("{op} succeeded; expected errno {errno}")),
        Err(e) => {
            let text = e.to_string();
            let expected = format!("errno {errno}");
            let name = format!("{:?}", libbreenix::Errno::from_raw(errno));
            check(
                text == name || text.ends_with(&expected),
                &format!("{op}: expected {name} ({errno}), got {text}"),
            )
        }
    }
}
fn cpath(p: &str) -> Vec<u8> {
    let mut b = p.as_bytes().to_vec();
    b.push(0);
    b
}
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture {
    path: String,
    file: Option<Fd>,
}
impl Fixture {
    fn empty() -> Result<Self, String> {
        let pid = process::getpid().map_err(|e| format!("fixture getpid: {e}"))?;
        Ok(Self {
            path: format!(
                "/tmp/files-io-{}-{}",
                pid.raw(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ),
            file: None,
        })
    }
    fn new(bytes: &[u8]) -> Result<Self, String> {
        let mut f = Self::empty()?;
        let d = f
            .open(O_CREAT | O_EXCL | O_RDWR)
            .map_err(|e| format!("fixture create: {e}"))?;
        f.file = Some(d);
        write_all(d, bytes)?;
        fs::lseek(d, 0, SEEK_SET).map_err(|e| format!("fixture rewind: {e}"))?;
        Ok(f)
    }
    fn fd(&self) -> Fd {
        self.file.expect("fixture has an open file")
    }
    fn open(&self, flags: u32) -> Result<Fd, Error> {
        fs::open_with_mode(&self.path, flags, 0o600)
    }
    fn extra(&self, suffix: &str) -> String {
        format!("{}.{}", self.path, suffix)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(fd) = self.file {
            let _ = io::close(fd);
        }
        let _ = fs::unlink(&self.extra("link"));
        let _ = fs::unlink(&self.path);
    }
}
fn write_all(fd: Fd, mut b: &[u8]) -> Result<(), String> {
    while !b.is_empty() {
        let n = io::write(fd, b).map_err(|e| format!("write: {e}"))?;
        if n == 0 || n > b.len() {
            return Err(format!("write returned invalid count {n}"));
        }
        b = &b[n..];
    }
    Ok(())
}
fn contents(fd: Fd, expected: &[u8]) -> CaseResult {
    fs::lseek(fd, 0, SEEK_SET)?;
    let mut b = vec![0; expected.len() + 1];
    let mut used = 0;
    while used < b.len() {
        let n = io::read(fd, &mut b[used..])?;
        if n == 0 {
            break;
        }
        check(
            n <= b.len() - used,
            "read returned more bytes than the buffer",
        )?;
        used += n;
    }
    check(
        &b[..used] == expected,
        &format!(
            "file bytes differ: expected {} bytes, read {used}",
            expected.len()
        ),
    )
}
#[repr(C)]
struct Iovec {
    base: *mut u8,
    len: usize,
}
impl Iovec {
    fn new(b: &mut [u8]) -> Self {
        Self {
            base: b.as_mut_ptr(),
            len: b.len(),
        }
    }
}
fn vector(n: u64, d: Fd, v: &[Iovec]) -> Result<u64, String> {
    sc(
        n,
        d.raw(),
        v.as_ptr() as u64,
        v.len() as u64,
        0,
        "vector I/O",
    )
}
fn positioned(n: u64, d: Fd, b: &mut [u8], off: i64) -> Result<u64, String> {
    sc(
        n,
        d.raw(),
        b.as_mut_ptr() as u64,
        b.len() as u64,
        off as u64,
        "positioned I/O",
    )
}
// Decode the kernel ABI rather than libbreenix::fs::Stat, whose current
// definition uses the x86 layout on both architectures. The kernel exposes
// Linux's 144-byte x86 and 128-byte ARM layouts; offsets from size onward agree.
struct FileStat {
    st_dev: u64,
    st_ino: u64,
    st_nlink: u64,
    st_mode: u32,
    st_size: i64,
    st_blocks: u64,
    st_atime: i64,
    st_atime_nsec: i64,
    st_mtime: i64,
    st_mtime_nsec: i64,
    st_ctime: i64,
    st_ctime_nsec: i64,
}
impl FileStat {
    fn is_file(&self) -> bool {
        self.st_mode & S_IFMT == S_IFREG
    }
    fn is_dir(&self) -> bool {
        self.st_mode & S_IFMT == S_IFDIR
    }
    fn is_symlink(&self) -> bool {
        self.st_mode & S_IFMT == S_IFLNK
    }
    fn decode(b: &[u64; 18]) -> Self {
        #[cfg(target_arch = "x86_64")]
        let (mode, nlink) = (b[3] as u32, b[2]);
        #[cfg(target_arch = "aarch64")]
        let (mode, nlink) = (b[2] as u32, b[2] >> 32);
        Self {
            st_dev: b[0],
            st_ino: b[1],
            st_nlink: nlink,
            st_mode: mode,
            st_size: b[6] as i64,
            st_blocks: b[8],
            st_atime: b[9] as i64,
            st_atime_nsec: b[10] as i64,
            st_mtime: b[11] as i64,
            st_mtime_nsec: b[12] as i64,
            st_ctime: b[13] as i64,
            st_ctime_nsec: b[14] as i64,
        }
    }
}
fn fstat(fd: Fd) -> Result<FileStat, Error> {
    let mut b = [0u64; 18];
    // SAFETY: b is aligned and large enough for either kernel stat ABI.
    let r = unsafe { raw::syscall2(nr::FSTAT, fd.raw(), b.as_mut_ptr() as u64) };
    Error::from_syscall(r as i64)?;
    Ok(FileStat::decode(&b))
}
fn stat(path: &str, nofollow: bool) -> Result<FileStat, String> {
    let p = cpath(path);
    let mut b = [0u64; 18];
    sc(
        nr::NEWFSTATAT,
        (-100i64) as u64,
        p.as_ptr() as u64,
        b.as_mut_ptr() as u64,
        if nofollow { 256 } else { 0 },
        "lstat/stat",
    )?;
    Ok(FileStat::decode(&b))
}

fn truncate_fd(fd: Fd, len: i64) -> Result<u64, String> {
    sc(FTRUNCATE, fd.raw(), len as u64, 0, 0, "ftruncate")
}
#[repr(C)]
struct Flock {
    kind: i16,
    whence: i16,
    start: i64,
    len: i64,
    pid: i32,
}
impl Flock {
    fn new(kind: i16) -> Self {
        Self {
            kind,
            whence: 0,
            start: 0,
            len: 0,
            pid: 0,
        }
    }
}
fn lock(fd: Fd, cmd: i32, l: &mut Flock) -> Result<i64, Error> {
    io::fcntl(fd, cmd, l as *mut Flock as i64)
}
fn wait_child(pid: i32) -> Result<i32, String> {
    // The shared runner bounds the entire case, including its helpers. Do not
    // impose a second, shorter deadline on fork/exec or filesystem work.
    let mut status = 0;
    let done = process::waitpid(pid, &mut status, 0).map_err(|e| format!("waitpid: {e}"))?;
    if done.raw() as i32 != pid {
        return Err("waitpid returned a different child".into());
    }
    Ok(status)
}
fn child_ok(status: i32, why: &str) -> CaseResult {
    check(
        process::wifexited(status) && process::wexitstatus(status) == 0,
        why,
    )
}
fn exec_descriptor(fd: Fd, closed: bool) -> CaseResult {
    match process::fork()? {
        process::ForkResult::Child => {
            let descriptor = cpath(&fd.raw().to_string());
            let state = if closed {
                b"closed\0".as_slice()
            } else {
                b"open\0".as_slice()
            };
            let args = [
                b"files-io-exec_test\0".as_ptr(),
                descriptor.as_ptr(),
                state.as_ptr(),
                std::ptr::null(),
            ];
            let _ = process::execv(b"/usr/local/test/bin/files-io-exec_test\0", args.as_ptr());
            process::exit(99);
        }
        process::ForkResult::Parent(pid) => child_ok(
            wait_child(pid.raw() as i32)?,
            "exec helper did not observe the required descriptor lifetime (exit 99 means exec failed)",
        ),
    }
}
fn lock_conflict(f: &Fixture, owner: bool) -> CaseResult {
    lock(f.fd(), 6, &mut Flock::new(1)).map_err(|e| format!("F_SETLK write lock failed: {e}"))?;
    lock_peer(f, owner, false)
}
fn lock_peer(f: &Fixture, owner: bool, available: bool) -> CaseResult {
    let parent = process::getpid()?.raw() as i32;
    match process::fork()? {
        process::ForkResult::Child => {
            let result = (|| -> CaseResult {
                let d = f.open(O_RDWR)?;
                let mut l = Flock::new(1);
                if owner {
                    lock(d, 5, &mut l)?;
                    check(
                        l.kind == 1 && l.pid == parent,
                        "F_GETLK returned wrong owner",
                    )
                } else if available {
                    lock(d, 6, &mut l)
                        .map_err(|e| format!("child could not acquire the unlocked file: {e}"))?;
                    Ok(())
                } else {
                    match lock(d, 6, &mut l) {
                        Err(Error::Os(libbreenix::Errno::EACCES | libbreenix::Errno::EAGAIN)) => {
                            Ok(())
                        }
                        other => fail(format!(
                            "conflicting F_SETLK did not return EACCES/EAGAIN: {other:?}"
                        )),
                    }
                }
            })();
            process::exit(if result.is_ok() { 0 } else { 1 });
        }
        process::ForkResult::Parent(pid) => child_ok(
            wait_child(pid.raw() as i32)?,
            if owner {
                "F_GETLK did not report the other process's lock and PID"
            } else if available {
                "child could not acquire the lock after F_UNLCK"
            } else {
                "conflicting F_SETLK did not return EACCES or EAGAIN"
            },
        ),
    }
}
const F_GETLK: i32 = 5;
const F_SETLK: i32 = 6;
const F_SETLKW: i32 = 7;
const F_WRLCK: i16 = 1;
const F_UNLCK: i16 = 2;
fn setlk(fd: Fd, cmd: i32, kind: i16, start: i64, len: i64) -> Result<i64, Error> {
    let mut l = Flock {
        kind,
        whence: 0,
        start,
        len,
        pid: 0,
    };
    lock(fd, cmd, &mut l)
}
/// Fork a child that opens `f`, takes a write lock on byte `held` if given,
/// writes one byte to a pipe and then runs `wait`, exiting 0 if it returns
/// `Ok`. The parent gets the child's PID once the child is blocked: it has
/// sent its byte and is still running 200 ms later.
fn fork_waiter(
    f: &Fixture,
    held: Option<i64>,
    wait: impl FnOnce(Fd) -> CaseResult,
) -> Result<i32, CaseError> {
    let (r, w) = io::pipe()?;
    match process::fork()? {
        process::ForkResult::Child => {
            let result = (|| -> CaseResult {
                let d = f.open(O_RDWR)?;
                if let Some(byte) = held {
                    setlk(d, F_SETLK, F_WRLCK, byte, 1)?;
                }
                io::write(w, b"w")?;
                wait(d)
            })();
            process::exit(if result.is_ok() { 0 } else { 1 });
        }
        process::ForkResult::Parent(pid) => {
            let pid = pid.raw() as i32;
            io::close(w)?;
            let mut byte = [0u8; 1];
            let n = io::read(r, &mut byte)?;
            io::close(r)?;
            if n != 1 {
                return Err("the waiting child failed before it started to wait".into());
            }
            time::sleep_ms(200)?;
            let mut status = 0;
            let done = process::waitpid(pid, &mut status, process::WNOHANG)
                .map_err(|e| format!("waitpid: {e}"))?;
            if done.raw() != 0 {
                return Err("F_SETLKW returned while a conflicting lock was held".into());
            }
            Ok(pid)
        }
    }
}
extern "C" fn ignore_signal(_: i32) {}
fn pipe_sigpipe() -> CaseResult {
    let (r, w) = io::pipe()?;
    io::close(r)?;
    match process::fork()? {
        process::ForkResult::Child => {
            if signal::sigaction(
                signal::SIGPIPE,
                Some(&signal::Sigaction::default_action()),
                None,
            )
            .is_err()
            {
                process::exit(99);
            }
            let _ = io::write(w, b"X");
            process::exit(1);
        }
        process::ForkResult::Parent(pid) => {
            let s = wait_child(pid.raw() as i32)?;
            check(
                process::wifsignaled(s) && (s & 127) == signal::SIGPIPE,
                "write without readers did not terminate the child with SIGPIPE",
            )
        }
    }
}
fn fill_pipe(w: Fd) -> CaseResult {
    let b = [42; 4096];
    for _ in 0..256 {
        match io::write(w, &b) {
            Err(Error::Os(libbreenix::Errno::EAGAIN)) => return Ok(()),
            Ok(n) if n > 0 && n <= b.len() => {}
            other => {
                return fail(format!(
                    "nonblocking pipe fill returned {other:?}, expected bytes or EAGAIN"
                ))
            }
        }
    }
    fail("pipe did not reach EAGAIN within 1 MiB")
}
fn pipe_atomic_nonblock() -> CaseResult {
    let (r, w) = io::pipe2(O_NONBLOCK as i32)?;
    fill_pipe(w)?;
    // Fill the remaining capacity one byte at a time, then free just one byte.
    let mut full = false;
    for _ in 0..4097 {
        match io::write(w, &[42]) {
            Err(Error::Os(libbreenix::Errno::EAGAIN)) => {
                full = true;
                break;
            }
            Ok(1) => {}
            other => return fail(format!("one-byte pipe fill returned {other:?}")),
        }
    }
    check(full, "pipe never became completely full")?;
    check(
        io::read(r, &mut [0; 1])? == 1,
        "could not free one byte in full pipe",
    )?;
    expect_errno(
        io::write(w, &[88; 4096]),
        11,
        "PIPE_BUF write with only one byte free",
    )?;
    io::close(w)?;
    loop {
        let mut b = [0; 4096];
        let n = io::read(r, &mut b)?;
        if n == 0 {
            break;
        }
        check(
            b[..n].iter().all(|&x| x == 42),
            "failed atomic write left partial bytes in the pipe",
        )?;
    }
    Ok(())
}
fn pipe_atomic(len: usize) -> CaseResult {
    let (r, w) = io::pipe()?;
    let (go, ready) = io::pipe()?;
    let mut children = Vec::new();
    for byte in [65, 66] {
        match process::fork()? {
            process::ForkResult::Child => {
                let _ = io::close(r);
                let _ = io::close(ready);
                if io::read(go, &mut [0; 1]).ok() != Some(1) {
                    process::exit(2);
                }
                let b = vec![byte; len];
                for _ in 0..4 {
                    if io::write(w, &b).ok() != Some(len) {
                        process::exit(3);
                    }
                }
                process::exit(0);
            }
            process::ForkResult::Parent(pid) => children.push(pid.raw() as i32),
        }
    }
    io::close(go)?;
    write_all(ready, b"XX")?;
    io::close(ready)?;
    io::close(w)?;
    let mut bytes = Vec::new();
    loop {
        let mut b = [0; 4096];
        let n = io::read(r, &mut b)?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&b[..n]);
    }
    for pid in children {
        child_ok(
            wait_child(pid)?,
            "atomic pipe writer failed or returned a short write",
        )?;
    }
    check(
        bytes.len() == 8 * len,
        "pipe lost bytes from concurrent writers",
    )?;
    check(
        bytes.chunks_exact(len).filter(|b| b[0] == 65).count() == 4,
        "pipe did not retain four records from each writer",
    )?;
    check(
        bytes
            .chunks_exact(len)
            .all(|b| (b[0] == 65 || b[0] == 66) && b.iter().all(|&x| x == b[0])),
        "concurrent writes at or below PIPE_BUF interleaved",
    )
}
fn map(fd: Fd, flags: i32, off: usize) -> Result<*mut u8, Error> {
    memory::mmap(
        std::ptr::null_mut(),
        4096,
        memory::PROT_READ | memory::PROT_WRITE,
        flags,
        fd.raw() as i32,
        off as i64,
    )
}
fn sync_mapping(p: *mut u8) -> CaseResult {
    sc(MSYNC, p as u64, 4096, 4, 0, "msync MS_SYNC")?;
    Ok(())
}
fn sync_fd(fd: Fd, data: bool) -> Result<(), String> {
    let (number, operation) = if data {
        (FDATASYNC, "fdatasync")
    } else {
        (FSYNC, "fsync")
    };
    sc(number, fd.raw(), 0, 0, 0, operation)?;
    Ok(())
}

static SUITE: Suite = suite(
    "files-io",
    "Files & I/O",
    &[
        category(
            "open",
            "open / creat flags",
            &[
                case(
                    "create",
                    "O_CREAT creates a missing regular file",
                    open_create,
                ),
                case(
                    "exclusive-new",
                    "O_CREAT|O_EXCL succeeds for a missing path",
                    open_exclusive_new,
                ),
                case(
                    "exclusive-existing",
                    "O_CREAT|O_EXCL returns EEXIST for an existing file",
                    open_exclusive_existing,
                ),
                case(
                    "create-preserves",
                    "O_CREAT without O_TRUNC preserves existing data",
                    open_create_preserves,
                ),
                case(
                    "truncate",
                    "O_TRUNC reduces an existing file to zero bytes",
                    open_truncate,
                ),
                case(
                    "truncate-offset",
                    "An O_TRUNC open starts at offset zero",
                    open_truncate_offset,
                ),
                case(
                    "append",
                    "O_APPEND writes at EOF after seeking to the start",
                    open_append,
                ),
                case(
                    "append-each-write",
                    "Every O_APPEND write uses the current EOF",
                    open_append_each_write,
                ),
                case(
                    "readonly-write",
                    "O_RDONLY rejects write with EBADF",
                    open_readonly_write,
                ),
                case(
                    "writeonly-read",
                    "O_WRONLY rejects read with EBADF",
                    open_writeonly_read,
                ),
                case(
                    "readwrite-read",
                    "O_RDWR permits reading",
                    open_readwrite_read,
                ),
                case(
                    "readwrite-write",
                    "O_RDWR permits writing",
                    open_readwrite_write,
                ),
                case(
                    "cloexec-flag",
                    "O_CLOEXEC sets FD_CLOEXEC",
                    open_cloexec_flag,
                ),
                case(
                    "cloexec-exec",
                    "O_CLOEXEC closes the descriptor across exec",
                    open_cloexec_exec,
                ),
                case(
                    "ordinary-exec",
                    "An ordinary open descriptor survives exec",
                    open_ordinary_exec,
                ),
                case("directory", "O_DIRECTORY opens a directory", open_directory),
                case(
                    "directory-regular",
                    "O_DIRECTORY rejects a regular file with ENOTDIR",
                    open_directory_regular,
                ),
                case(
                    "missing",
                    "Opening a missing file without O_CREAT returns ENOENT",
                    open_missing,
                ),
                case(
                    "missing-parent",
                    "O_CREAT cannot create missing parent directories",
                    open_missing_parent,
                ),
                case(
                    "directory-write",
                    "Opening a directory for write returns EISDIR",
                    open_directory_write,
                ),
                case(
                    "nondirectory-parent",
                    "A regular file in a path prefix produces ENOTDIR",
                    open_nondirectory_parent,
                ),
                case(
                    "permission-denied",
                    "A non-root process cannot open a mode-000 file",
                    open_permission_denied,
                ),
            ],
        ),
        category(
            "read-write",
            "read / write and vectors",
            &[
                case(
                    "short-read",
                    "A read larger than the file returns only available bytes",
                    read_write_short_read,
                ),
                case(
                    "zero-read",
                    "A zero-length read returns zero",
                    read_write_zero_read,
                ),
                case(
                    "zero-write",
                    "A zero-length write returns zero",
                    read_write_zero_write,
                ),
                case(
                    "zero-offset",
                    "Zero-length I/O does not advance the file offset",
                    read_write_zero_offset,
                ),
                case("eof", "Reading at EOF returns zero", read_write_eof),
                case(
                    "eof-repeat",
                    "Repeated reads at EOF keep returning zero",
                    read_write_eof_repeat,
                ),
                case(
                    "empty",
                    "Reading an empty file returns zero",
                    read_write_empty,
                ),
                case(
                    "binary",
                    "read and write preserve embedded NUL and high bytes",
                    read_write_binary,
                ),
                case(
                    "large",
                    "A 64 KiB buffer survives a write/read round trip",
                    read_write_large,
                ),
                case(
                    "overwrite",
                    "A write within the file preserves the trailing bytes",
                    read_write_overwrite,
                ),
                case(
                    "extend",
                    "Writing at EOF extends the file",
                    read_write_extend,
                ),
                case(
                    "readv",
                    "readv scatters data in vector order",
                    read_write_readv,
                ),
                case(
                    "readv-short",
                    "readv stops at EOF in the middle of a vector",
                    read_write_readv_short,
                ),
                case(
                    "writev",
                    "writev gathers data in vector order",
                    read_write_writev,
                ),
                case(
                    "readv-empty",
                    "readv with zero vectors returns zero",
                    read_write_readv_empty,
                ),
                case(
                    "readv-bad-fd",
                    "readv rejects an invalid descriptor with EBADF",
                    read_write_readv_bad_fd,
                ),
                case(
                    "writev-empty",
                    "writev with zero vectors returns zero",
                    read_write_writev_empty,
                ),
                case(
                    "writev-bad-fd",
                    "writev rejects an invalid descriptor with EBADF",
                    read_write_writev_bad_fd,
                ),
            ],
        ),
        category(
            "offsets",
            "Offsets and positioned I/O",
            &[
                case("set", "SEEK_SET sets an absolute offset", offsets_set),
                case("cur", "SEEK_CUR adds to the current offset", offsets_cur),
                case("end", "SEEK_END measures from file size", offsets_end),
                case(
                    "past-end",
                    "Seeking past EOF succeeds without growing the file",
                    offsets_past_end,
                ),
                case(
                    "read-position",
                    "read advances the file offset by its byte count",
                    offsets_read_position,
                ),
                case(
                    "write-position",
                    "write advances the file offset by its byte count",
                    offsets_write_position,
                ),
                case(
                    "negative",
                    "Seeking before offset zero returns EINVAL",
                    offsets_negative,
                ),
                case(
                    "invalid-whence",
                    "An invalid seek origin returns EINVAL",
                    offsets_invalid_whence,
                ),
                case("pipe", "Seeking on a pipe returns ESPIPE", offsets_pipe),
                case(
                    "hole",
                    "Writing past EOF makes the gap read as zeros",
                    offsets_hole,
                ),
                case(
                    "past-end-read",
                    "Reading past EOF returns zero",
                    offsets_past_end_read,
                ),
                case(
                    "pread-data",
                    "pread reads from the requested absolute offset",
                    offsets_pread_data,
                ),
                case(
                    "pread-offset",
                    "pread does not change the shared file offset",
                    offsets_pread_offset,
                ),
                case("pread-eof", "pread at EOF returns zero", offsets_pread_eof),
                case(
                    "pwrite-data",
                    "pwrite writes at the requested absolute offset",
                    offsets_pwrite_data,
                ),
                case(
                    "pwrite-offset",
                    "pwrite does not change the shared file offset",
                    offsets_pwrite_offset,
                ),
                case(
                    "pread-negative",
                    "pread rejects a negative offset with EINVAL",
                    offsets_pread_negative,
                ),
                case(
                    "pwrite-negative",
                    "pwrite rejects a negative offset with EINVAL",
                    offsets_pwrite_negative,
                ),
            ],
        ),
        category(
            "descriptors",
            "Descriptor lifetime and duplication",
            &[
                case(
                    "dup",
                    "dup returns a distinct valid descriptor",
                    descriptors_dup,
                ),
                case(
                    "dup-offset",
                    "dup shares the open file offset",
                    descriptors_dup_offset,
                ),
                case(
                    "separate-offset",
                    "Independent opens have independent offsets",
                    descriptors_separate_offset,
                ),
                case(
                    "dup-clear-cloexec",
                    "dup clears FD_CLOEXEC on the new descriptor",
                    descriptors_dup_clear_cloexec,
                ),
                case(
                    "dup-close",
                    "Closing one duplicate leaves the other usable",
                    descriptors_dup_close,
                ),
                case(
                    "dup-lowest",
                    "dup chooses the lowest unused descriptor",
                    descriptors_dup_lowest,
                ),
                case(
                    "open-lowest",
                    "open reuses the lowest free descriptor",
                    descriptors_open_lowest,
                ),
                case(
                    "dup2-target",
                    "dup2 returns the requested target descriptor",
                    descriptors_dup2_target,
                ),
                case(
                    "dup2-replace",
                    "dup2 closes and replaces an occupied target",
                    descriptors_dup2_replace,
                ),
                case(
                    "dup2-self",
                    "dup2 of a valid descriptor onto itself succeeds",
                    descriptors_dup2_self,
                ),
                case(
                    "dup2-clear-cloexec",
                    "dup2 clears FD_CLOEXEC on its target",
                    descriptors_dup2_clear_cloexec,
                ),
                case(
                    "dup2-invalid-preserves",
                    "dup2 with an invalid source preserves the target",
                    descriptors_dup2_invalid_preserves,
                ),
                case(
                    "dup3-cloexec",
                    "dup3 with O_CLOEXEC sets FD_CLOEXEC",
                    descriptors_dup3_cloexec,
                ),
                case(
                    "dup3-self",
                    "dup3 onto itself returns EINVAL",
                    descriptors_dup3_self,
                ),
                case(
                    "dup3-flags",
                    "dup3 rejects unsupported flags with EINVAL",
                    descriptors_dup3_flags,
                ),
                case(
                    "close-twice",
                    "Closing a descriptor twice returns EBADF",
                    descriptors_close_twice,
                ),
                case(
                    "read-bad-fd",
                    "read rejects an invalid descriptor with EBADF",
                    descriptors_read_bad_fd,
                ),
                case(
                    "write-bad-fd",
                    "write rejects an invalid descriptor with EBADF",
                    descriptors_write_bad_fd,
                ),
                case(
                    "close-bad-fd",
                    "close rejects an invalid descriptor with EBADF",
                    descriptors_close_bad_fd,
                ),
                case(
                    "dup-bad-fd",
                    "dup rejects an invalid descriptor with EBADF",
                    descriptors_dup_bad_fd,
                ),
                case(
                    "seek-bad-fd",
                    "seek rejects an invalid descriptor with EBADF",
                    descriptors_seek_bad_fd,
                ),
            ],
        ),
        category(
            "fcntl",
            "fcntl flags and advisory locks",
            &[
                case(
                    "getfl-readonly",
                    "F_GETFL reports O_RDONLY",
                    fcntl_getfl_readonly,
                ),
                case(
                    "getfl-writeonly",
                    "F_GETFL reports O_WRONLY",
                    fcntl_getfl_writeonly,
                ),
                case(
                    "getfl-readwrite",
                    "F_GETFL reports O_RDWR",
                    fcntl_getfl_readwrite,
                ),
                case(
                    "setfl-append",
                    "F_SETFL enables append behavior",
                    fcntl_setfl_append,
                ),
                case(
                    "clear-append",
                    "F_SETFL can clear O_APPEND",
                    fcntl_clear_append,
                ),
                case(
                    "setfl-access",
                    "F_SETFL cannot change the access mode",
                    fcntl_setfl_access,
                ),
                case(
                    "shared-status",
                    "File status flags are shared by duplicates",
                    fcntl_shared_status,
                ),
                case(
                    "getfd-default",
                    "FD_CLOEXEC is clear on an ordinary open",
                    fcntl_getfd_default,
                ),
                case("setfd", "F_SETFD sets FD_CLOEXEC", fcntl_setfd),
                case("clearfd", "F_SETFD clears FD_CLOEXEC", fcntl_clearfd),
                case(
                    "private-fd-flags",
                    "Descriptor flags are not shared by duplicates",
                    fcntl_private_fd_flags,
                ),
                case(
                    "dupfd-minimum",
                    "F_DUPFD returns a descriptor at or above its minimum",
                    fcntl_dupfd_minimum,
                ),
                case(
                    "dupfd-offset",
                    "F_DUPFD shares the file offset",
                    fcntl_dupfd_offset,
                ),
                case(
                    "dupfd-negative",
                    "F_DUPFD rejects a negative minimum with EINVAL",
                    fcntl_dupfd_negative,
                ),
                case(
                    "bad-fd",
                    "F_GETFL rejects an invalid descriptor with EBADF",
                    fcntl_bad_fd,
                ),
                case(
                    "lock-set",
                    "F_SETLK establishes a write lock",
                    fcntl_lock_set,
                ),
                case(
                    "lock-unlock",
                    "F_SETLK can release an advisory lock",
                    fcntl_lock_unlock,
                ),
                case(
                    "lock-get",
                    "F_GETLK reports F_UNLCK when no other process holds a lock",
                    fcntl_lock_get,
                ),
                case(
                    "lock-conflict",
                    "A conflicting process cannot acquire a write lock",
                    fcntl_lock_conflict,
                ),
                case(
                    "lock-owner",
                    "F_GETLK reports the conflicting lock owner PID",
                    fcntl_lock_owner,
                ),
                case(
                    "lock-efault",
                    "F_GETLK and F_SETLK reject an unmapped or read-only struct flock with EFAULT",
                    fcntl_lock_efault,
                ),
                case(
                    "lockw-wait",
                    "F_SETLKW waits for a conflicting lock and acquires it once released",
                    fcntl_lockw_wait,
                ),
                case(
                    "lockw-eintr",
                    "A signal ends an F_SETLKW wait with EINTR",
                    fcntl_lockw_eintr,
                ),
                case(
                    "lockw-deadlock",
                    "F_SETLKW fails with EDEADLK instead of waiting into a deadlock",
                    fcntl_lockw_deadlock,
                ),
                case(
                    "lockw-woken",
                    "A woken F_SETLKW waiter no longer counts toward deadlock detection",
                    fcntl_lockw_woken,
                ),
                case(
                    "lockw-killed",
                    "Killing a process blocked in F_SETLKW releases its locks and its wait",
                    fcntl_lockw_killed,
                ),
            ],
        ),
        category(
            "metadata",
            "Metadata, truncation and timestamps",
            &[
                case(
                    "stat-type",
                    "stat identifies a regular file",
                    metadata_stat_type,
                ),
                case(
                    "fstat-type",
                    "fstat identifies a regular file",
                    metadata_fstat_type,
                ),
                case(
                    "stat-size",
                    "stat reports the exact byte size",
                    metadata_stat_size,
                ),
                case(
                    "fstat-size",
                    "fstat reports the exact byte size",
                    metadata_fstat_size,
                ),
                case(
                    "identity",
                    "stat and fstat agree on device and inode",
                    metadata_identity,
                ),
                case(
                    "links",
                    "A newly created file has one hard link",
                    metadata_links,
                ),
                case(
                    "mode",
                    "Creation mode restricts group and other access",
                    metadata_mode,
                ),
                case(
                    "directory",
                    "stat identifies a directory",
                    metadata_directory,
                ),
                case(
                    "lstat-link",
                    "lstat describes the symlink rather than its target",
                    metadata_lstat_link,
                ),
                case(
                    "stat-link",
                    "stat follows a symlink to its target",
                    metadata_stat_link,
                ),
                case(
                    "lstat-size",
                    "lstat reports the symlink target string length",
                    metadata_lstat_size,
                ),
                case(
                    "missing",
                    "stat on a missing path returns ENOENT",
                    metadata_missing,
                ),
                case(
                    "bad-fd",
                    "fstat on an invalid descriptor returns EBADF",
                    metadata_bad_fd,
                ),
                case(
                    "write-size",
                    "Metadata size grows after a write at EOF",
                    metadata_write_size,
                ),
                case(
                    "ftruncate-shrink",
                    "ftruncate shrinks the file and preserves its prefix",
                    metadata_ftruncate_shrink,
                ),
                case(
                    "ftruncate-grow",
                    "ftruncate grows the file with zero-filled bytes",
                    metadata_ftruncate_grow,
                ),
                case(
                    "ftruncate-size",
                    "fstat reflects the size after ftruncate",
                    metadata_ftruncate_size,
                ),
                case(
                    "ftruncate-offset",
                    "ftruncate does not move the file offset",
                    metadata_ftruncate_offset,
                ),
                case(
                    "ftruncate-negative",
                    "ftruncate with a negative size returns EINVAL",
                    metadata_ftruncate_negative,
                ),
                case(
                    "truncate",
                    "truncate by path changes the file size",
                    metadata_truncate,
                ),
                case(
                    "mtime-advances",
                    "mtime advances after write",
                    metadata_mtime_advances,
                ),
                case(
                    "ctime-advances",
                    "ctime advances after write",
                    metadata_ctime_advances,
                ),
                case(
                    "atime-advances",
                    "atime advances after read",
                    metadata_atime_advances,
                ),
                case(
                    "timestamp-nanos",
                    "Timestamp nanoseconds are in the POSIX range",
                    metadata_timestamp_nanos,
                ),
            ],
        ),
        category(
            "pipes",
            "Pipes, atomicity and nonblocking I/O",
            &[
                case(
                    "roundtrip",
                    "A pipe preserves written bytes",
                    pipes_roundtrip,
                ),
                case(
                    "short-read",
                    "Pipe read returns available bytes without filling the buffer",
                    pipes_short_read,
                ),
                case(
                    "eof",
                    "Pipe read returns EOF after the last writer closes",
                    pipes_eof,
                ),
                case(
                    "drain-before-eof",
                    "Buffered pipe bytes remain readable before EOF",
                    pipes_drain_before_eof,
                ),
                case(
                    "duplicate-writer",
                    "EOF waits for a duplicated writer to close",
                    pipes_duplicate_writer,
                ),
                case(
                    "epipe",
                    "Writing with no readers returns EPIPE when SIGPIPE is ignored",
                    pipes_epipe,
                ),
                case(
                    "sigpipe",
                    "Writing with no readers delivers SIGPIPE",
                    pipes_sigpipe,
                ),
                case(
                    "nonblock-empty",
                    "An empty nonblocking pipe returns EAGAIN",
                    pipes_nonblock_empty,
                ),
                case(
                    "nonblock-full",
                    "A full nonblocking pipe returns EAGAIN",
                    pipes_nonblock_full,
                ),
                case(
                    "nonblock-flags",
                    "pipe2 O_NONBLOCK applies to both ends",
                    pipes_nonblock_flags,
                ),
                case(
                    "cloexec",
                    "pipe2 O_CLOEXEC applies to both ends",
                    pipes_cloexec,
                ),
                case(
                    "atomic-small",
                    "Concurrent 512-byte pipe writes do not interleave",
                    pipes_atomic_small,
                ),
                case(
                    "atomic-pipe-buf",
                    "Concurrent PIPE_BUF-sized writes do not interleave",
                    pipes_atomic_pipe_buf,
                ),
                case(
                    "atomic-nonblock",
                    "A nonblocking PIPE_BUF write is all-or-nothing",
                    pipes_atomic_nonblock,
                ),
            ],
        ),
        category(
            "mmap",
            "File-backed mappings",
            &[
                case(
                    "read",
                    "A file-backed mapping exposes the file bytes",
                    mmap_read,
                ),
                case(
                    "shared-writeback",
                    "MAP_SHARED changes reach the file after msync",
                    mmap_shared_writeback,
                ),
                case(
                    "shared-peer",
                    "MAP_SHARED changes are visible in another shared mapping",
                    mmap_shared_peer,
                ),
                case(
                    "private-copy",
                    "MAP_PRIVATE writes do not change the file",
                    mmap_private_copy,
                ),
                case(
                    "private-readback",
                    "MAP_PRIVATE writes remain readable in that mapping",
                    mmap_private_readback,
                ),
                case(
                    "close-fd",
                    "A mapping stays valid after its file descriptor closes",
                    mmap_close_fd,
                ),
                case(
                    "unaligned-offset",
                    "mmap rejects an unaligned file offset with EINVAL",
                    mmap_unaligned_offset,
                ),
                case(
                    "bad-fd",
                    "File-backed mmap rejects an invalid descriptor with EBADF",
                    mmap_bad_fd,
                ),
            ],
        ),
        category(
            "sync",
            "File synchronization",
            &[
                case(
                    "fsync",
                    "fsync returns success for a writable regular file",
                    sync_fsync,
                ),
                case(
                    "fsync-bad-fd",
                    "fsync rejects an invalid descriptor with EBADF",
                    sync_fsync_bad_fd,
                ),
                case(
                    "fsync-pipe",
                    "fsync rejects a pipe with EINVAL",
                    sync_fsync_pipe,
                ),
                case(
                    "fdatasync",
                    "fdatasync returns success for a writable regular file",
                    sync_fdatasync,
                ),
                case(
                    "fdatasync-bad-fd",
                    "fdatasync rejects an invalid descriptor with EBADF",
                    sync_fdatasync_bad_fd,
                ),
                case(
                    "fdatasync-pipe",
                    "fdatasync rejects a pipe with EINVAL",
                    sync_fdatasync_pipe,
                ),
            ],
        ),
    ],
);

fn open_create() -> CaseResult {
    let f = Fixture::empty()?;
    let fd = f.open(O_CREAT | O_RDWR)?;
    check(
        fstat(fd)?.is_file(),
        "O_CREAT did not create a regular file",
    )
}

fn open_exclusive_new() -> CaseResult {
    let f = Fixture::empty()?;
    f.open(O_CREAT | O_EXCL | O_RDWR)?;
    Ok(())
}

fn open_exclusive_existing() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    expect_errno(f.open(O_CREAT | O_EXCL | O_RDWR), 17, "exclusive open")
}

fn open_create_preserves() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let fd = f.open(O_CREAT | O_RDWR)?;
    contents(fd, b"abcdef")
}

fn open_truncate() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let fd = f.open(O_WRONLY | O_TRUNC)?;
    check(fstat(fd)?.st_size == 0, "O_TRUNC left a nonzero size")
}

fn open_truncate_offset() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let fd = f.open(O_RDWR | O_TRUNC)?;
    check(
        fs::lseek(fd, 0, SEEK_CUR)? == 0,
        "O_TRUNC open started at a nonzero offset",
    )
}

fn open_append() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let fd = f.open(O_WRONLY | O_APPEND)?;
    fs::lseek(fd, 0, SEEK_SET)?;
    write_all(fd, b"XY")?;
    contents(f.fd(), b"abcdefXY")
}

fn open_append_each_write() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let fd = f.open(O_WRONLY | O_APPEND)?;
    write_all(fd, b"X")?;
    fs::lseek(fd, 1, SEEK_SET)?;
    write_all(fd, b"Y")?;
    contents(f.fd(), b"abcdefXY")
}

fn open_readonly_write() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let fd = f.open(O_RDONLY)?;
    expect_errno(io::write(fd, b"X"), 9, "write on O_RDONLY")
}

fn open_writeonly_read() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let fd = f.open(O_WRONLY)?;
    expect_errno(io::read(fd, &mut [0; 1]), 9, "read on O_WRONLY")
}

fn open_readwrite_read() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    contents(f.fd(), b"abcdef")
}

fn open_readwrite_write() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    write_all(f.fd(), b"XY")?;
    contents(f.fd(), b"XYcdef")
}

fn open_cloexec_flag() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let fd = f.open(O_RDONLY | CLOEXEC)?;
    check(
        io::fcntl_getfd(fd)? & 1 == 1,
        "O_CLOEXEC did not set FD_CLOEXEC",
    )
}

fn open_cloexec_exec() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let fd = f.open(O_RDONLY | CLOEXEC)?;
    exec_descriptor(fd, true)
}

fn open_ordinary_exec() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let fd = f.open(O_RDONLY)?;
    exec_descriptor(fd, false)
}

fn open_directory() -> CaseResult {
    let fd = fs::open("/tmp", O_RDONLY | O_DIRECTORY)?;
    check(fstat(fd)?.is_dir(), "O_DIRECTORY did not open a directory")
}

fn open_directory_regular() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    expect_errno(f.open(O_RDONLY | O_DIRECTORY), 20, "O_DIRECTORY on a file")
}

fn open_missing() -> CaseResult {
    let f = Fixture::empty()?;
    expect_errno(f.open(O_RDONLY), 2, "open missing file")
}

fn open_missing_parent() -> CaseResult {
    let f = Fixture::empty()?;
    expect_errno(
        fs::open_with_mode(&format!("{}/child", f.path), O_CREAT | O_RDWR, 0o600),
        2,
        "open with missing parent",
    )
}

fn open_directory_write() -> CaseResult {
    expect_errno(fs::open("/tmp", O_WRONLY), 21, "open directory for write")
}

fn open_nondirectory_parent() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    expect_errno(
        fs::open(&format!("{}/child", f.path), O_RDONLY),
        20,
        "open through regular-file parent",
    )
}

fn open_permission_denied() -> CaseResult {
    let f = Fixture::empty()?;
    let fd = fs::open_with_mode(&f.path, O_CREAT | O_RDWR | O_EXCL, 0)?;
    io::close(fd)?;
    match process::fork()? {
        process::ForkResult::Child => {
            let result = (|| -> CaseResult {
                sc(SETUID, 65534, 0, 0, 0, "setuid for permission check")?;
                expect_errno(f.open(O_RDONLY), 13, "open mode-000 file as non-root")
            })();
            process::exit(if result.is_ok() { 0 } else { 1 });
        }
        process::ForkResult::Parent(pid) => child_ok(
            wait_child(pid.raw() as i32)?,
            "unprivileged child did not get EACCES for a mode-000 file",
        ),
    }
}

fn read_write_short_read() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let mut b = [0; 32];
    check(
        io::read(f.fd(), &mut b)? == 6 && &b[..6] == b"abcdef",
        "read did not return six available bytes",
    )
}

fn read_write_zero_read() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    check(
        io::read(f.fd(), &mut [])? == 0,
        "zero-length read returned a nonzero count",
    )
}

fn read_write_zero_write() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    check(
        io::write(f.fd(), &[])? == 0,
        "zero-length write returned a nonzero count",
    )
}

fn read_write_zero_offset() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    io::read(f.fd(), &mut [])?;
    io::write(f.fd(), &[])?;
    check(
        fs::lseek(f.fd(), 0, SEEK_CUR)? == 0,
        "zero-length I/O changed the offset",
    )
}

fn read_write_eof() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    fs::lseek(f.fd(), 0, SEEK_END)?;
    check(
        io::read(f.fd(), &mut [0; 1])? == 0,
        "read at EOF returned data",
    )
}

fn read_write_eof_repeat() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    fs::lseek(f.fd(), 0, SEEK_END)?;
    io::read(f.fd(), &mut [0; 1])?;
    check(
        io::read(f.fd(), &mut [0; 1])? == 0,
        "second read at EOF returned data",
    )
}

fn read_write_empty() -> CaseResult {
    let f = Fixture::new(b"")?;
    check(
        io::read(f.fd(), &mut [0; 1])? == 0,
        "empty file returned data",
    )
}

fn read_write_binary() -> CaseResult {
    let f = Fixture::new(&[0, 255, 128, 10, 0])?;
    contents(f.fd(), &[0, 255, 128, 10, 0])
}

fn read_write_large() -> CaseResult {
    let bytes: Vec<u8> = (0..65536).map(|i| (i % 251) as u8).collect();
    let f = Fixture::new(&bytes)?;
    contents(f.fd(), &bytes)
}

fn read_write_overwrite() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    write_all(f.fd(), b"XY")?;
    contents(f.fd(), b"XYcdef")
}

fn read_write_extend() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    fs::lseek(f.fd(), 0, SEEK_END)?;
    write_all(f.fd(), b"XY")?;
    contents(f.fd(), b"abcdefXY")
}

fn read_write_readv() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let mut a = [0; 2];
    let mut b = [0; 4];
    let v = [Iovec::new(&mut a), Iovec::new(&mut b)];
    let n = vector(nr::READV, f.fd(), &v)?;
    check(
        n == 6 && &a == b"ab" && &b == b"cdef",
        "readv count or vector contents were wrong",
    )
}

fn read_write_readv_short() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let mut a = [0; 4];
    let mut b = [99; 4];
    let v = [Iovec::new(&mut a), Iovec::new(&mut b)];
    let n = vector(nr::READV, f.fd(), &v)?;
    check(
        n == 6 && b == [101, 102, 99, 99],
        "readv did not leave bytes beyond EOF untouched",
    )
}

fn read_write_writev() -> CaseResult {
    let f = Fixture::new(b"")?;
    let mut a = *b"ab";
    let mut b = *b"cdef";
    let v = [Iovec::new(&mut a), Iovec::new(&mut b)];
    check(
        vector(nr::WRITEV, f.fd(), &v)? == 6,
        "writev returned wrong count",
    )?;
    contents(f.fd(), b"abcdef")
}

fn read_write_readv_empty() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    check(
        vector(nr::READV, f.fd(), &[])? == 0,
        "readv with zero vectors returned nonzero",
    )
}

fn read_write_readv_bad_fd() -> CaseResult {
    let mut b = [0; 1];
    expect_errno(
        vector(nr::READV, BAD, &[Iovec::new(&mut b)]),
        9,
        "readv invalid fd",
    )
}

fn read_write_writev_empty() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    check(
        vector(nr::WRITEV, f.fd(), &[])? == 0,
        "writev with zero vectors returned nonzero",
    )
}

fn read_write_writev_bad_fd() -> CaseResult {
    let mut b = [0; 1];
    expect_errno(
        vector(nr::WRITEV, BAD, &[Iovec::new(&mut b)]),
        9,
        "writev invalid fd",
    )
}

fn offsets_set() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    check(
        fs::lseek(f.fd(), 2, SEEK_SET)? == 2,
        "SEEK_SET sets an absolute offset failed",
    )
}

fn offsets_cur() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    check(
        {
            fs::lseek(f.fd(), 2, SEEK_SET)?;
            fs::lseek(f.fd(), 2, SEEK_CUR)? == 4
        },
        "SEEK_CUR adds to the current offset failed",
    )
}

fn offsets_end() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    check(
        fs::lseek(f.fd(), -2, SEEK_END)? == 4,
        "SEEK_END measures from file size failed",
    )
}

fn offsets_past_end() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    check(
        fs::lseek(f.fd(), 100, SEEK_SET)? == 100 && fstat(f.fd())?.st_size == 6,
        "Seeking past EOF succeeds without growing the file failed",
    )
}

fn offsets_read_position() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    check(
        {
            io::read(f.fd(), &mut [0; 2])?;
            fs::lseek(f.fd(), 0, SEEK_CUR)? == 2
        },
        "read advances the file offset by its byte count failed",
    )
}

fn offsets_write_position() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    check(
        {
            write_all(f.fd(), b"XY")?;
            fs::lseek(f.fd(), 0, SEEK_CUR)? == 2
        },
        "write advances the file offset by its byte count failed",
    )
}

fn offsets_negative() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    expect_errno(fs::lseek(f.fd(), -1, SEEK_SET), 22, "negative SEEK_SET")
}

fn offsets_invalid_whence() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    expect_errno(fs::lseek(f.fd(), 0, 99), 22, "invalid seek origin")
}

fn offsets_pipe() -> CaseResult {
    let (r, _w) = io::pipe()?;
    expect_errno(fs::lseek(r, 0, SEEK_SET), 29, "lseek on pipe")
}

fn offsets_hole() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    fs::lseek(f.fd(), 10, SEEK_SET)?;
    write_all(f.fd(), b"X")?;
    contents(f.fd(), b"abcdef\0\0\0\0X")
}

fn offsets_past_end_read() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    fs::lseek(f.fd(), 100, SEEK_SET)?;
    check(
        io::read(f.fd(), &mut [0; 1])? == 0,
        "read past EOF returned data",
    )
}

fn offsets_pread_data() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let mut b = [0; 2];
    check(
        positioned(PREAD, f.fd(), &mut b, 2)? == 2 && &b == b"cd",
        "pread returned wrong count or bytes",
    )
}

fn offsets_pread_offset() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    fs::lseek(f.fd(), 1, SEEK_SET)?;
    positioned(PREAD, f.fd(), &mut [0; 2], 3)?;
    check(
        fs::lseek(f.fd(), 0, SEEK_CUR)? == 1,
        "pread moved the file offset",
    )
}

fn offsets_pread_eof() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    check(
        positioned(PREAD, f.fd(), &mut [0; 2], 6)? == 0,
        "pread at EOF returned data",
    )
}

fn offsets_pwrite_data() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let mut bytes = *b"XY";
    positioned(PWRITE, f.fd(), &mut bytes, 2)?;
    contents(f.fd(), b"abXYef")
}

fn offsets_pwrite_offset() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    fs::lseek(f.fd(), 1, SEEK_SET)?;
    let mut bytes = *b"XY";
    positioned(PWRITE, f.fd(), &mut bytes, 3)?;
    check(
        fs::lseek(f.fd(), 0, SEEK_CUR)? == 1,
        "pwrite moved the file offset",
    )
}

fn offsets_pread_negative() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    expect_errno(
        positioned(PREAD, f.fd(), &mut [0; 1], -1),
        22,
        "pread negative offset",
    )
}

fn offsets_pwrite_negative() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    expect_errno(
        positioned(PWRITE, f.fd(), &mut [0; 1], -1),
        22,
        "pwrite negative offset",
    )
}

fn descriptors_dup() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let d = io::dup(f.fd())?;
    check(
        d != f.fd() && fstat(d)?.st_ino == fstat(f.fd())?.st_ino,
        "dup did not return a distinct descriptor for the same file",
    )
}

fn descriptors_dup_offset() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let d = io::dup(f.fd())?;
    io::read(d, &mut [0; 2])?;
    check(
        fs::lseek(f.fd(), 0, SEEK_CUR)? == 2,
        "dup did not share the file offset",
    )
}

fn descriptors_separate_offset() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let d = f.open(O_RDONLY)?;
    io::read(d, &mut [0; 2])?;
    check(
        fs::lseek(f.fd(), 0, SEEK_CUR)? == 0,
        "independent opens shared an offset",
    )
}

fn descriptors_dup_clear_cloexec() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    io::fcntl_setfd(f.fd(), 1)?;
    let d = io::dup(f.fd())?;
    check(io::fcntl_getfd(d)? & 1 == 0, "dup inherited FD_CLOEXEC")
}

fn descriptors_dup_close() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let d = io::dup(f.fd())?;
    io::close(d)?;
    contents(f.fd(), b"abcdef")
}

fn descriptors_dup_lowest() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let d = io::dup(f.fd())?;
    let e = io::dup(f.fd())?;
    io::close(d)?;
    check(
        io::dup(f.fd())? == d && e.raw() > d.raw(),
        "dup did not reuse the lowest free descriptor",
    )
}

fn descriptors_open_lowest() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let d = f.open(O_RDONLY)?;
    let e = f.open(O_RDONLY)?;
    io::close(d)?;
    check(
        f.open(O_RDONLY)? == d && e.raw() > d.raw(),
        "open did not reuse the lowest free descriptor",
    )
}

fn descriptors_dup2_target() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let d = Fd::from_raw(40);
    check(
        io::dup2(f.fd(), d)? == d,
        "dup2 returned a different descriptor",
    )
}

fn descriptors_dup2_replace() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let other = Fixture::new(b"other")?;
    let d = other.fd();
    io::dup2(f.fd(), d)?;
    contents(d, b"abcdef")
}

fn descriptors_dup2_self() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    check(
        io::dup2(f.fd(), f.fd())? == f.fd(),
        "dup2 onto itself failed",
    )
}

fn descriptors_dup2_clear_cloexec() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let d = Fd::from_raw(40);
    io::dup2(f.fd(), d)?;
    io::fcntl_setfd(d, 1)?;
    io::dup2(f.fd(), d)?;
    check(io::fcntl_getfd(d)? & 1 == 0, "dup2 retained FD_CLOEXEC")
}

fn descriptors_dup2_invalid_preserves() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    expect_errno(io::dup2(BAD, f.fd()), 9, "dup2 invalid source")?;
    contents(f.fd(), b"abcdef")
}

fn descriptors_dup3_cloexec() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let d = Fd::from_raw(40);
    sc(nr::DUP3, f.fd().raw(), d.raw(), CLOEXEC as u64, 0, "dup3")?;
    check(io::fcntl_getfd(d)? & 1 == 1, "dup3 did not set FD_CLOEXEC")
}

fn descriptors_dup3_self() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    expect_errno(
        sc(nr::DUP3, f.fd().raw(), f.fd().raw(), 0, 0, "dup3"),
        22,
        "dup3 onto itself",
    )
}

fn descriptors_dup3_flags() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    expect_errno(
        sc(nr::DUP3, f.fd().raw(), 40, O_APPEND as u64, 0, "dup3"),
        22,
        "dup3 invalid flags",
    )
}

fn descriptors_close_twice() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let d = f.open(O_RDONLY)?;
    io::close(d)?;
    expect_errno(io::close(d), 9, "second close")
}

fn descriptors_read_bad_fd() -> CaseResult {
    expect_errno(io::read(BAD, &mut [0; 1]), 9, "read invalid fd")
}

fn descriptors_write_bad_fd() -> CaseResult {
    expect_errno(io::write(BAD, b"X"), 9, "write invalid fd")
}

fn descriptors_close_bad_fd() -> CaseResult {
    expect_errno(io::close(BAD), 9, "close invalid fd")
}

fn descriptors_dup_bad_fd() -> CaseResult {
    expect_errno(io::dup(BAD), 9, "dup invalid fd")
}

fn descriptors_seek_bad_fd() -> CaseResult {
    expect_errno(fs::lseek(BAD, 0, SEEK_SET), 9, "seek invalid fd")
}

fn fcntl_getfl_readonly() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let d = f.open(O_RDONLY)?;
    check(
        io::fcntl_getfl(d)? & 3 == O_RDONLY as i64,
        "F_GETFL reported the wrong access mode",
    )
}

fn fcntl_getfl_writeonly() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let d = f.open(O_WRONLY)?;
    check(
        io::fcntl_getfl(d)? & 3 == O_WRONLY as i64,
        "F_GETFL reported the wrong access mode",
    )
}

fn fcntl_getfl_readwrite() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let d = f.open(O_RDWR)?;
    check(
        io::fcntl_getfl(d)? & 3 == O_RDWR as i64,
        "F_GETFL reported the wrong access mode",
    )
}

fn fcntl_setfl_append() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    io::fcntl_setfl(f.fd(), O_APPEND as i32)?;
    write_all(f.fd(), b"X")?;
    contents(f.fd(), b"abcdefX")
}

fn fcntl_clear_append() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let d = f.open(O_RDWR | O_APPEND)?;
    io::fcntl_setfl(d, 0)?;
    write_all(d, b"X")?;
    contents(f.fd(), b"Xbcdef")
}

fn fcntl_setfl_access() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let d = f.open(O_RDONLY)?;
    io::fcntl_setfl(d, O_RDWR as i32)?;
    expect_errno(io::write(d, b"X"), 9, "write after F_SETFL access change")
}

fn fcntl_shared_status() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let d = io::dup(f.fd())?;
    io::fcntl_setfl(d, O_APPEND as i32)?;
    check(
        io::fcntl_getfl(f.fd())? & O_APPEND as i64 != 0,
        "F_SETFL flags were not shared across dup",
    )
}

fn fcntl_getfd_default() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    check(
        io::fcntl_getfd(f.fd())? & 1 == 0,
        "ordinary open set FD_CLOEXEC",
    )
}

fn fcntl_setfd() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    io::fcntl_setfd(f.fd(), 1)?;
    check(
        io::fcntl_getfd(f.fd())? & 1 == 1,
        "F_SETFD did not set FD_CLOEXEC",
    )
}

fn fcntl_clearfd() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    io::fcntl_setfd(f.fd(), 1)?;
    io::fcntl_setfd(f.fd(), 0)?;
    check(
        io::fcntl_getfd(f.fd())? & 1 == 0,
        "F_SETFD did not clear FD_CLOEXEC",
    )
}

fn fcntl_private_fd_flags() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let d = io::dup(f.fd())?;
    io::fcntl_setfd(d, 1)?;
    check(
        io::fcntl_getfd(f.fd())? & 1 == 0,
        "FD_CLOEXEC leaked to another descriptor",
    )
}

fn fcntl_dupfd_minimum() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let d = io::fcntl(f.fd(), 0, 40)?;
    check(
        d >= 40,
        "F_DUPFD returned a descriptor below its minimum of 40",
    )
}

fn fcntl_dupfd_offset() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let d = Fd::from_raw(io::fcntl(f.fd(), 0, 40)? as u64);
    io::read(d, &mut [0; 2])?;
    check(
        fs::lseek(f.fd(), 0, SEEK_CUR)? == 2,
        "F_DUPFD did not share the offset",
    )
}

fn fcntl_dupfd_negative() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    expect_errno(io::fcntl(f.fd(), 0, -1), 22, "F_DUPFD negative minimum")
}

fn fcntl_bad_fd() -> CaseResult {
    expect_errno(io::fcntl_getfl(BAD), 9, "F_GETFL invalid fd")
}

fn fcntl_lock_set() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    lock_conflict(&f, false)
}

fn fcntl_lock_unlock() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    // First prove that the lock excludes a child; then prove that unlocking
    // permits a child to acquire it. Neither a no-op lock nor unlock can pass.
    lock_conflict(&f, false)?;
    lock(f.fd(), 6, &mut Flock::new(2)).map_err(|e| format!("F_SETLK unlock failed: {e}"))?;
    lock_peer(&f, false, true)
}

fn fcntl_lock_get() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let mut l = Flock::new(1);
    lock(f.fd(), 5, &mut l).map_err(|e| format!("F_GETLK failed: {e}"))?;
    check(l.kind == 2, "F_GETLK did not report F_UNLCK")
}

fn fcntl_lock_conflict() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    lock_conflict(&f, false)
}

fn fcntl_lock_owner() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    lock_conflict(&f, true)
}

fn fcntl_lock_efault() -> CaseResult {
    use libbreenix::memory::{MAP_ANONYMOUS, MAP_PRIVATE, PROT_READ, PROT_WRITE};
    let f = Fixture::new(b"abcdef")?;
    let page = memory::mmap(
        std::ptr::null_mut(),
        4096,
        PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS,
        -1,
        0,
    )?;
    memory::munmap(page, 4096)?;
    expect_errno(
        io::fcntl(f.fd(), F_GETLK, page as i64),
        14,
        "F_GETLK with an unmapped struct flock",
    )?;
    expect_errno(
        io::fcntl(f.fd(), F_SETLK, page as i64),
        14,
        "F_SETLK with an unmapped struct flock",
    )?;
    // A zeroed page reads as an F_RDLCK request, so F_GETLK must write the
    // F_UNLCK answer back into a page it may not write.
    let read_only = memory::mmap(
        std::ptr::null_mut(),
        4096,
        PROT_READ,
        MAP_PRIVATE | MAP_ANONYMOUS,
        -1,
        0,
    )?;
    expect_errno(
        io::fcntl(f.fd(), F_GETLK, read_only as i64),
        14,
        "F_GETLK into a read-only struct flock",
    )
}

fn fcntl_lockw_wait() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    setlk(f.fd(), F_SETLK, F_WRLCK, 0, 0).map_err(|e| format!("F_SETLK write lock: {e}"))?;
    let pid = fork_waiter(&f, None, |d| {
        setlk(d, F_SETLKW, F_WRLCK, 0, 0).map_err(|e| format!("F_SETLKW: {e}"))?;
        Ok(())
    })?;
    setlk(f.fd(), F_SETLK, F_UNLCK, 0, 0).map_err(|e| format!("F_SETLK unlock: {e}"))?;
    child_ok(
        wait_child(pid)?,
        "the waiting process did not acquire the lock once it was released",
    )
}

fn fcntl_lockw_eintr() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    setlk(f.fd(), F_SETLK, F_WRLCK, 0, 0).map_err(|e| format!("F_SETLK write lock: {e}"))?;
    let pid = fork_waiter(&f, None, |d| {
        signal::sigaction(
            signal::SIGUSR1,
            Some(&signal::Sigaction::new(ignore_signal)),
            None,
        )?;
        match setlk(d, F_SETLKW, F_WRLCK, 0, 0) {
            Err(Error::Os(libbreenix::Errno::EINTR)) => Ok(()),
            other => fail(format!("F_SETLKW returned {other:?}, expected EINTR")),
        }
    })?;
    signal::kill(pid, signal::SIGUSR1)?;
    child_ok(wait_child(pid)?, "a signal did not end the wait with EINTR")
}

fn fcntl_lockw_deadlock() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    setlk(f.fd(), F_SETLK, F_WRLCK, 0, 1).map_err(|e| format!("F_SETLK byte 0: {e}"))?;
    // The child holds byte 1 and waits for byte 0, so waiting for byte 1
    // here would wait for a process that waits for this one.
    let pid = fork_waiter(&f, Some(1), |d| {
        setlk(d, F_SETLKW, F_WRLCK, 0, 1).map_err(|e| format!("F_SETLKW byte 0: {e}"))?;
        Ok(())
    })?;
    let deadlock = expect_errno(
        setlk(f.fd(), F_SETLKW, F_WRLCK, 1, 1),
        35,
        "F_SETLKW that would deadlock",
    );
    setlk(f.fd(), F_SETLK, F_UNLCK, 0, 1).map_err(|e| format!("F_SETLK unlock: {e}"))?;
    deadlock?;
    child_ok(
        wait_child(pid)?,
        "the child did not acquire byte 0 once it was released",
    )
}

fn fcntl_lockw_woken() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    setlk(f.fd(), F_SETLK, F_WRLCK, 1, 1).map_err(|e| format!("F_SETLK byte 1: {e}"))?;
    // The child holds byte 0 and waits for byte 1. Releasing byte 1 wakes it,
    // so it no longer waits for this process: waiting here for byte 0 is not
    // a deadlock, and the child releases byte 0 once it has byte 1.
    let pid = fork_waiter(&f, Some(0), |d| {
        setlk(d, F_SETLKW, F_WRLCK, 1, 1).map_err(|e| format!("F_SETLKW byte 1: {e}"))?;
        setlk(d, F_SETLK, F_UNLCK, 0, 1).map_err(|e| format!("F_SETLK unlock byte 0: {e}"))?;
        Ok(())
    })?;
    setlk(f.fd(), F_SETLK, F_UNLCK, 1, 1).map_err(|e| format!("F_SETLK unlock: {e}"))?;
    setlk(f.fd(), F_SETLKW, F_WRLCK, 0, 1)
        .map_err(|e| format!("F_SETLKW for the woken child's byte: {e}"))?;
    child_ok(wait_child(pid)?, "the woken child did not acquire byte 1")
}

fn fcntl_lockw_killed() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    setlk(f.fd(), F_SETLK, F_WRLCK, 0, 1).map_err(|e| format!("F_SETLK byte 0: {e}"))?;
    let pid = fork_waiter(&f, Some(1), |d| {
        setlk(d, F_SETLKW, F_WRLCK, 0, 1).map_err(|e| format!("F_SETLKW byte 0: {e}"))?;
        Ok(())
    })?;
    signal::kill(pid, signal::SIGKILL)?;
    let status = wait_child(pid)?;
    check(
        process::wifsignaled(status) && process::wtermsig(status) == signal::SIGKILL,
        "the waiting child was not killed by SIGKILL",
    )?;
    let mut l = Flock {
        kind: F_WRLCK,
        whence: 0,
        start: 1,
        len: 1,
        pid: 0,
    };
    lock(f.fd(), F_GETLK, &mut l).map_err(|e| format!("F_GETLK: {e}"))?;
    check(
        l.kind == F_UNLCK,
        "the killed waiter's lock on byte 1 was not released",
    )?;
    // The killed waiter's wait is gone too: it no longer blocks or deadlocks
    // anything, so this process can take its byte and give up its own.
    setlk(f.fd(), F_SETLKW, F_WRLCK, 1, 1).map_err(|e| format!("F_SETLKW byte 1: {e}"))?;
    setlk(f.fd(), F_SETLK, F_UNLCK, 0, 0).map_err(|e| format!("F_SETLK unlock: {e}"))?;
    Ok(())
}

fn metadata_stat_type() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    check(
        stat(&f.path, false)?.is_file(),
        "stat reported the wrong file type",
    )
}

fn metadata_fstat_type() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    check(
        fstat(f.fd())?.is_file(),
        "fstat reported the wrong file type",
    )
}

fn metadata_stat_size() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    check(
        stat(&f.path, false)?.st_size == 6,
        "stat reported the wrong size",
    )
}

fn metadata_fstat_size() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    check(fstat(f.fd())?.st_size == 6, "fstat reported the wrong size")
}

fn metadata_identity() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let a = stat(&f.path, false)?;
    let b = fstat(f.fd())?;
    check(
        a.st_dev == b.st_dev && a.st_ino == b.st_ino && a.st_ino != 0,
        "stat and fstat identities differ or inode is zero",
    )
}

fn metadata_links() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    check(
        fstat(f.fd())?.st_nlink == 1,
        "new file link count was not one",
    )
}

fn metadata_mode() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    check(
        fstat(f.fd())?.st_mode & 0o777 == 0o600,
        "creation mode 0600 was not preserved",
    )
}

fn metadata_directory() -> CaseResult {
    check(
        stat("/tmp", false)?.is_dir(),
        "stat reported the wrong directory type",
    )
}

fn metadata_lstat_link() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let p = f.extra("link");
    fs::symlink(&f.path, &p)?;
    check(stat(&p, true)?.is_symlink(), "lstat followed the symlink")
}

fn metadata_stat_link() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let p = f.extra("link");
    fs::symlink(&f.path, &p)?;
    check(
        stat(&p, false)?.st_ino == fstat(f.fd())?.st_ino,
        "stat did not resolve the symlink target",
    )
}

fn metadata_lstat_size() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let p = f.extra("link");
    fs::symlink(&f.path, &p)?;
    check(
        stat(&p, true)?.st_size == f.path.len() as i64,
        "symlink size was not target string length",
    )
}

fn metadata_missing() -> CaseResult {
    let f = Fixture::empty()?;
    expect_errno(stat(&f.path, false), 2, "stat missing path")
}

fn metadata_bad_fd() -> CaseResult {
    expect_errno(fstat(BAD), 9, "fstat invalid fd")
}

fn metadata_write_size() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    fs::lseek(f.fd(), 0, SEEK_END)?;
    write_all(f.fd(), b"XY")?;
    check(
        fstat(f.fd())?.st_size == 8,
        "fstat size did not grow after write",
    )
}

fn metadata_ftruncate_shrink() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    truncate_fd(f.fd(), 3)?;
    contents(f.fd(), b"abc")?;
    metadata_truncate_indirect()
}

// Sparse writes reach each pointer depth without constructing a huge fixture.
fn metadata_truncate_indirect() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let sectors = fstat(f.fd())?.st_blocks;
    check(sectors > 0, "fixture has no allocated block")?;
    let block_size = sectors * 512;
    let fanout = block_size / 4;
    let single = 12 * block_size;
    let double = (12 + fanout) * block_size;
    let triple = (12 + fanout + fanout * fanout) * block_size;
    for offset in [single, double, triple] {
        check(
            sc(
                PWRITE,
                f.fd().raw(),
                b"X".as_ptr() as u64,
                1,
                offset + 5,
                "sparse pwrite",
            )? == 1,
            "sparse pwrite returned wrong count",
        )?;
    }
    check(
        fstat(f.fd())?.st_blocks == 10 * sectors,
        "write omitted indirect block accounting",
    )?;
    for (offset, blocks) in [(triple, 10), (double, 6), (single, 3)] {
        truncate_fd(f.fd(), (offset + 6) as i64)?;
        check(
            fstat(f.fd())?.st_blocks == blocks * sectors,
            "shrink has wrong block accounting",
        )?;
        let mut b = [0; 6];
        check(
            sc(
                PREAD,
                f.fd().raw(),
                b.as_mut_ptr() as u64,
                6,
                offset,
                "sparse pread",
            )? == 6,
            "retained indirect data is missing",
        )?;
        check(b == *b"     X", "shrink damaged retained indirect data")?;
    }
    truncate_fd(f.fd(), 3)?;
    check(
        fstat(f.fd())?.st_blocks == sectors,
        "shrink did not reclaim pointer trees",
    )?;
    truncate_fd(f.fd(), 6)?;
    contents(f.fd(), b"abc   ")?;
    truncate_fd(f.fd(), 0)?;
    check(
        fstat(f.fd())?.st_blocks == 0,
        "zero truncate retained allocated blocks",
    )?;
    sync_fd(f.fd(), false)?;
    let reopened = f.open(O_RDONLY)?;
    let result = check(
        fstat(reopened)?.st_size == 0 && fstat(reopened)?.st_blocks == 0,
        "resized inode did not persist on reopen",
    );
    io::close(reopened)?;
    result
}

fn metadata_ftruncate_grow() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    truncate_fd(f.fd(), 8)?;
    contents(f.fd(), b"abcdef\0\0")
}

fn metadata_ftruncate_size() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    truncate_fd(f.fd(), 3)?;
    check(
        fstat(f.fd())?.st_size == 3,
        "fstat size did not change after ftruncate",
    )
}

fn metadata_ftruncate_offset() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    fs::lseek(f.fd(), 5, SEEK_SET)?;
    truncate_fd(f.fd(), 2)?;
    check(
        fs::lseek(f.fd(), 0, SEEK_CUR)? == 5,
        "ftruncate changed the offset",
    )
}

fn metadata_ftruncate_negative() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    expect_errno(truncate_fd(f.fd(), -1), 22, "negative ftruncate")
}

fn metadata_truncate() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let p = cpath(&f.path);
    sc(TRUNCATE, p.as_ptr() as u64, 2, 0, 0, "truncate")?;
    contents(f.fd(), b"ab")?;
    let normalized = cpath(&format!("/tmp/../{}", f.path.trim_start_matches('/')));
    sc(
        TRUNCATE,
        normalized.as_ptr() as u64,
        1,
        0,
        0,
        "normalized truncate",
    )?;
    contents(f.fd(), b"a")?;
    let device = cpath("/dev/null");
    expect_errno(
        sc(TRUNCATE, device.as_ptr() as u64, 0, 0, 0, "device truncate"),
        22,
        "device truncate",
    )?;
    let link = f.extra("link");
    fs::symlink(&link, &link)?;
    let loop_path = cpath(&link);
    expect_errno(
        sc(
            TRUNCATE,
            loop_path.as_ptr() as u64,
            0,
            0,
            0,
            "loop truncate",
        ),
        40,
        "loop truncate",
    )
}

fn metadata_mtime_advances() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let before = fstat(f.fd())?;
    time::sleep_ms(1100)?;
    write_all(f.fd(), b"X")?;
    let after = fstat(f.fd())?;
    check(
        (after.st_mtime, after.st_mtime_nsec) > (before.st_mtime, before.st_mtime_nsec),
        "mtime did not advance",
    )
}

fn metadata_ctime_advances() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let before = fstat(f.fd())?;
    time::sleep_ms(1100)?;
    write_all(f.fd(), b"X")?;
    let after = fstat(f.fd())?;
    check(
        (after.st_ctime, after.st_ctime_nsec) > (before.st_ctime, before.st_ctime_nsec),
        "ctime did not advance",
    )
}

fn metadata_atime_advances() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let before = fstat(f.fd())?;
    time::sleep_ms(1100)?;
    io::read(f.fd(), &mut [0; 1])?;
    let after = fstat(f.fd())?;
    check(
        (after.st_atime, after.st_atime_nsec) > (before.st_atime, before.st_atime_nsec),
        "atime did not advance",
    )
}

fn metadata_timestamp_nanos() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let s = fstat(f.fd())?;
    check(
        [s.st_atime_nsec, s.st_mtime_nsec, s.st_ctime_nsec]
            .iter()
            .all(|n| (0..1_000_000_000).contains(n)),
        "timestamp nanoseconds are outside 0..999999999",
    )
}

fn pipes_roundtrip() -> CaseResult {
    let (r, w) = io::pipe()?;
    write_all(w, b"abcdef")?;
    let mut b = [0; 6];
    check(
        io::read(r, &mut b)? == 6 && &b == b"abcdef",
        "pipe lost or reordered data",
    )
}

fn pipes_short_read() -> CaseResult {
    let (r, w) = io::pipe()?;
    write_all(w, b"XY")?;
    check(
        io::read(r, &mut [0; 16])? == 2,
        "pipe read did not return its available two bytes",
    )
}

fn pipes_eof() -> CaseResult {
    let (r, w) = io::pipe()?;
    io::close(w)?;
    check(
        io::read(r, &mut [0; 1])? == 0,
        "pipe did not return EOF after writer close",
    )
}

fn pipes_drain_before_eof() -> CaseResult {
    let (r, w) = io::pipe()?;
    write_all(w, b"XY")?;
    io::close(w)?;
    let mut b = [0; 2];
    check(
        io::read(r, &mut b)? == 2 && &b == b"XY",
        "writer close lost buffered bytes",
    )?;
    check(io::read(r, &mut b)? == 0, "drained pipe did not return EOF")
}

fn pipes_duplicate_writer() -> CaseResult {
    let (r, w) = io::pipe2(O_NONBLOCK as i32)?;
    let d = io::dup(w)?;
    io::close(w)?;
    expect_errno(
        io::read(r, &mut [0; 1]),
        11,
        "read with duplicate writer open",
    )?;
    io::close(d)?;
    check(
        io::read(r, &mut [0; 1])? == 0,
        "pipe did not reach EOF after duplicate close",
    )
}

fn pipes_epipe() -> CaseResult {
    signal::sigaction(signal::SIGPIPE, Some(&signal::Sigaction::ignore()), None)?;
    let (r, w) = io::pipe()?;
    io::close(r)?;
    expect_errno(io::write(w, b"X"), 32, "write without pipe readers")
}

fn pipes_sigpipe() -> CaseResult {
    pipe_sigpipe()
}

fn pipes_nonblock_empty() -> CaseResult {
    let (r, _w) = io::pipe2(O_NONBLOCK as i32)?;
    expect_errno(io::read(r, &mut [0; 1]), 11, "empty nonblocking pipe read")
}

fn pipes_nonblock_full() -> CaseResult {
    let (_r, w) = io::pipe2(O_NONBLOCK as i32)?;
    fill_pipe(w)
}

fn pipes_nonblock_flags() -> CaseResult {
    let (r, w) = io::pipe2(O_NONBLOCK as i32)?;
    check(
        io::fcntl_getfl(r)? & O_NONBLOCK as i64 != 0
            && io::fcntl_getfl(w)? & O_NONBLOCK as i64 != 0,
        "pipe2 did not set O_NONBLOCK on both ends",
    )
}

fn pipes_cloexec() -> CaseResult {
    let (r, w) = io::pipe2(CLOEXEC as i32)?;
    check(
        io::fcntl_getfd(r)? & 1 == 1 && io::fcntl_getfd(w)? & 1 == 1,
        "pipe2 did not set FD_CLOEXEC on both ends",
    )
}

fn pipes_atomic_small() -> CaseResult {
    pipe_atomic(512)
}

fn pipes_atomic_pipe_buf() -> CaseResult {
    pipe_atomic(4096)
}

fn pipes_atomic_nonblock() -> CaseResult {
    pipe_atomic_nonblock()
}

fn mmap_read() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let p =
        map(f.fd(), memory::MAP_SHARED, 0).map_err(|e| format!("file-backed mmap failed: {e}"))?;
    let result = (|| -> CaseResult {
        check(
            unsafe { std::slice::from_raw_parts(p, 6) } == b"abcdef",
            "mapping did not expose file bytes",
        )
    })();
    memory::munmap(p, 4096).map_err(|e| format!("file-backed mmap failed: {e}"))?;
    result
}

fn mmap_shared_writeback() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let p =
        map(f.fd(), memory::MAP_SHARED, 0).map_err(|e| format!("file-backed mmap failed: {e}"))?;
    let result = (|| -> CaseResult {
        check(
            unsafe { std::slice::from_raw_parts(p, 6) } == b"abcdef",
            "file mapping did not expose the original file bytes",
        )?;
        unsafe {
            p.write_volatile(b'X');
        }
        sync_mapping(p)?;
        contents(f.fd(), b"Xbcdef")
    })();
    memory::munmap(p, 4096).map_err(|e| format!("file-backed mmap failed: {e}"))?;
    result
}

fn mmap_shared_peer() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let p =
        map(f.fd(), memory::MAP_SHARED, 0).map_err(|e| format!("file-backed mmap failed: {e}"))?;
    let result = (|| -> CaseResult {
        check(
            unsafe { std::slice::from_raw_parts(p, 6) } == b"abcdef",
            "file mapping did not expose the original file bytes",
        )?;
        let q = map(f.fd(), memory::MAP_SHARED, 0)
            .map_err(|e| format!("file-backed mmap failed: {e}"))?;
        unsafe {
            p.write_volatile(b'X');
        }
        let result = (|| -> CaseResult {
            check(
                unsafe { q.read_volatile() } == b'X',
                "shared mappings did not share changes",
            )?;
            match process::fork()? {
                process::ForkResult::Child => {
                    let result = (|| -> CaseResult {
                        check(
                            unsafe { p.read_volatile() } == b'X',
                            "child did not inherit shared file bytes",
                        )?;
                        let peer = map(f.fd(), memory::MAP_SHARED, 0)?;
                        check(
                            unsafe { peer.read_volatile() } == b'X',
                            "child's new mapping did not see the parent's store",
                        )?;
                        unsafe { peer.add(1).write_volatile(b'Y') };
                        sync_mapping(peer)?;
                        memory::munmap(peer, 4096)?;
                        Ok(())
                    })();
                    process::exit(if result.is_ok() { 0 } else { 1 });
                }
                process::ForkResult::Parent(pid) => child_ok(
                    wait_child(pid.raw() as i32)?,
                    "child did not share and synchronize the file mapping",
                )?,
            }
            check(
                unsafe { p.add(1).read_volatile() == b'Y' && q.add(1).read_volatile() == b'Y' },
                "parent mappings did not see the child's store",
            )?;
            contents(f.fd(), b"XYcdef")
        })();
        memory::munmap(q, 4096)?;
        result
    })();
    memory::munmap(p, 4096).map_err(|e| format!("file-backed mmap failed: {e}"))?;
    result
}

fn mmap_private_copy() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let p =
        map(f.fd(), memory::MAP_PRIVATE, 0).map_err(|e| format!("file-backed mmap failed: {e}"))?;
    let result = (|| -> CaseResult {
        check(
            unsafe { std::slice::from_raw_parts(p, 6) } == b"abcdef",
            "file mapping did not expose the original file bytes",
        )?;
        unsafe {
            p.write_volatile(b'X');
        }
        contents(f.fd(), b"abcdef")
    })();
    memory::munmap(p, 4096).map_err(|e| format!("file-backed mmap failed: {e}"))?;
    result
}

fn mmap_private_readback() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let p =
        map(f.fd(), memory::MAP_PRIVATE, 0).map_err(|e| format!("file-backed mmap failed: {e}"))?;
    let result = (|| -> CaseResult {
        check(
            unsafe { std::slice::from_raw_parts(p, 6) } == b"abcdef",
            "file mapping did not expose the original file bytes",
        )?;
        unsafe {
            p.write_volatile(b'X');
        }
        check(
            unsafe { p.read_volatile() } == b'X',
            "private mapping did not retain its write",
        )
    })();
    memory::munmap(p, 4096).map_err(|e| format!("file-backed mmap failed: {e}"))?;
    result
}

fn mmap_close_fd() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    let d = f.open(O_RDWR)?;
    let p = map(d, memory::MAP_PRIVATE, 0).map_err(|e| format!("file-backed mmap failed: {e}"))?;
    let result = (|| -> CaseResult {
        io::close(d)?;
        check(
            unsafe { p.read_volatile() } == b'a',
            "closing the fd invalidated the mapping",
        )
    })();
    memory::munmap(p, 4096)?;
    result
}

fn mmap_unaligned_offset() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    expect_errno(
        map(f.fd(), memory::MAP_PRIVATE, 1),
        22,
        "mmap unaligned offset",
    )
}

fn mmap_bad_fd() -> CaseResult {
    expect_errno(map(BAD, memory::MAP_PRIVATE, 0), 9, "mmap invalid fd")
}

fn sync_fsync() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    sync_fd(f.fd(), false)?;
    let dir = fs::open_with_mode("/tmp", O_RDONLY | O_DIRECTORY, 0)?;
    let result = sync_fd(dir, false);
    io::close(dir)?;
    result?;
    Ok(())
}

fn sync_fsync_bad_fd() -> CaseResult {
    expect_errno(sync_fd(BAD, false), 9, "fsync invalid fd")
}

fn sync_fsync_pipe() -> CaseResult {
    let (r, _w) = io::pipe()?;
    expect_errno(sync_fd(r, false), 22, "fsync on pipe")
}

fn sync_fdatasync() -> CaseResult {
    let f = Fixture::new(b"abcdef")?;
    sync_fd(f.fd(), true)?;
    Ok(())
}

fn sync_fdatasync_bad_fd() -> CaseResult {
    expect_errno(sync_fd(BAD, true), 9, "fdatasync invalid fd")
}

fn sync_fdatasync_pipe() -> CaseResult {
    let (r, _w) = io::pipe()?;
    expect_errno(sync_fd(r, true), 22, "fdatasync on pipe")
}

fn main() {
    SUITE.run()
}
