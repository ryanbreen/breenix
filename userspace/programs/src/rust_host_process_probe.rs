//! A standalone exec target keeps the suite runner out of the self-spawn path.
#![deny(unsafe_op_in_unsafe_fn)]

use libbreenix::errno::Errno;
use libbreenix::io::{fcntl_cmd, fd_flags::FD_CLOEXEC, status_flags::O_CLOEXEC};
use libbreenix::{process, signal};
use std::ffi::{c_void, CStr, CString};
use std::fs::{File, OpenOptions};
use std::io::{Error, ErrorKind, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::path::Path;
use std::ptr::null;

const SELF: &CStr = c"/usr/libexec/breenix/rust-host/process-probe";
const CONFLICTS_CHECKED: u8 = b'C';
const LOCKS_RELEASED: u8 = b'U';

// Values and LP64 layout from Breenix's <fcntl.h> and <sys/file.h>.
const F_SETLK: i32 = 6;
const F_WRLCK: i16 = 1;
const F_UNLCK: i16 = 2;
const LOCK_EX: i32 = 2;
const LOCK_NB: i32 = 4;
const LOCK_UN: i32 = 8;

#[repr(C)]
struct Flock {
    kind: i16,
    whence: i16,
    start: i64,
    len: i64,
    pid: i32,
}

extern "C" {
    fn posix_spawn(
        pid: *mut i32,
        path: *const u8,
        actions: *const c_void,
        attr: *const c_void,
        argv: *const *const u8,
        envp: *const *const u8,
    ) -> i32;
    fn pipe2(fds: *mut RawFd, flags: i32) -> i32;
    fn fcntl(fd: RawFd, cmd: i32, ...) -> i32;
    fn flock(fd: RawFd, operation: i32) -> i32;
    fn waitpid(pid: i32, status: *mut i32, flags: i32) -> i32;
    fn raise(sig: i32) -> i32;
}

fn spawn_self(args: &[&CStr]) -> Result<i32, String> {
    let argv: Vec<*const u8> = std::iter::once(SELF)
        .chain(args.iter().copied())
        .map(|arg| arg.as_ptr().cast())
        .chain(std::iter::once(null()))
        .collect();
    let envp = [null()];
    let mut pid = 0;
    // SAFETY: both argument vectors and the path are live and null-terminated.
    let result = unsafe {
        posix_spawn(
            &mut pid,
            argv[0],
            null(),
            null(),
            argv.as_ptr(),
            envp.as_ptr(),
        )
    };
    if result != 0 || pid <= 0 {
        return Err(format!("posix_spawn: result {result}, PID {pid}"));
    }
    Ok(pid)
}

fn wait_for(pid: i32) -> Result<i32, String> {
    loop {
        let mut status = 0;
        // SAFETY: pid is the positive PID returned by spawn, and status is writable.
        let result = unsafe { waitpid(pid, &mut status, 0) };
        if result == pid {
            return Ok(status);
        }
        let err = Error::last_os_error();
        if result != -1 || err.kind() != ErrorKind::Interrupted {
            return Err(format!("waitpid({pid}) returned {result}: {err}"));
        }
    }
}

fn fd_flags(fd: RawFd) -> std::io::Result<i32> {
    // SAFETY: F_GETFD accepts a descriptor number, including a closed descriptor.
    let flags = unsafe { fcntl(fd, fcntl_cmd::F_GETFD, 0i32) };
    if flags == -1 {
        return Err(Error::last_os_error());
    }
    Ok(flags)
}

fn cloexec_pipe() -> Result<(File, File), String> {
    let mut fds = [-1; 2];
    // SAFETY: fds holds two writable descriptor slots.
    if unsafe { pipe2(fds.as_mut_ptr(), O_CLOEXEC) } != 0 {
        return Err(format!("pipe2(O_CLOEXEC): {}", Error::last_os_error()));
    }
    // SAFETY: successful pipe2 returned two distinct, owned descriptors.
    let reader = unsafe { File::from_raw_fd(fds[0]) };
    // SAFETY: this is the distinct write descriptor returned by pipe2.
    let writer = unsafe { File::from_raw_fd(fds[1]) };
    for fd in fds {
        let flags = fd_flags(fd).map_err(|err| format!("F_GETFD on pipe {fd}: {err}"))?;
        if flags & FD_CLOEXEC == 0 {
            return Err(format!("pipe2 left FD_CLOEXEC clear on {fd}"));
        }
    }
    Ok((reader, writer))
}

fn inherit(file: &File) -> Result<(), String> {
    // SAFETY: F_SETFD takes a live descriptor and a flags value.
    if unsafe { fcntl(file.as_raw_fd(), fcntl_cmd::F_SETFD, 0i32) } == -1 {
        return Err(format!("clear FD_CLOEXEC: {}", Error::last_os_error()));
    }
    Ok(())
}

fn whole_file_flock(file: &File, operation: i32) -> std::io::Result<()> {
    // SAFETY: the caller holds the file open through the call.
    if unsafe { flock(file.as_raw_fd(), operation) } != 0 {
        return Err(Error::last_os_error());
    }
    Ok(())
}

fn whole_file_record_lock(file: &File, kind: i16) -> std::io::Result<()> {
    let mut lock = Flock {
        kind,
        whence: 0,
        start: 0,
        len: 0,
        pid: 0,
    };
    // SAFETY: file is open and lock is a live, initialized struct flock.
    if unsafe { fcntl(file.as_raw_fd(), F_SETLK, &mut lock as *mut Flock) } != 0 {
        return Err(Error::last_os_error());
    }
    Ok(())
}

fn lock_files(dir: &Path, create: bool) -> Result<(File, File), String> {
    let open = |name| {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(create)
            .open(dir.join(name))
            .map_err(|err| format!("open {name}: {err}"))
    };
    Ok((open("flock")?, open("fcntl")?))
}

fn require_byte(reader: &mut File, expected: u8) -> Result<(), String> {
    let mut byte = [0];
    reader
        .read_exact(&mut byte)
        .map_err(|err| format!("read pipe event: {err}"))?;
    if byte != [expected] {
        return Err(format!(
            "expected pipe event {expected:#x}, got {:#x}",
            byte[0]
        ));
    }
    Ok(())
}

fn check_locks(dir: &Path, fds: [RawFd; 4]) -> Result<(), String> {
    let [read_fd, write_fd, closed_read, closed_write] = fds;
    // Probe close-on-exec before opening anything that could reuse these numbers.
    for fd in [closed_read, closed_write] {
        if !matches!(fd_flags(fd), Err(err) if err.raw_os_error() == Some(Errno::EBADF as i32)) {
            return Err(format!("FD_CLOEXEC pipe descriptor {fd} survived exec"));
        }
    }
    if read_fd == write_fd || fd_flags(read_fd).is_err() || fd_flags(write_fd).is_err() {
        return Err("pipe endpoints expected from parent are missing".into());
    }
    // SAFETY: these are distinct, open endpoints in the child's descriptor table.
    let mut reader = unsafe { File::from_raw_fd(read_fd) };
    // SAFETY: the child's write endpoint has no other Rust owner.
    let mut writer = unsafe { File::from_raw_fd(write_fd) };
    let (bsd, record) = lock_files(dir, false)?;

    // LSB flock(LOCK_NB) reports EWOULDBLOCK. POSIX F_SETLK allows EACCES or EAGAIN.
    let denied = whole_file_flock(&bsd, LOCK_EX | LOCK_NB);
    if !matches!(&denied, Err(err) if err.kind() == ErrorKind::WouldBlock) {
        return Err(format!("flock under contention: {denied:?}"));
    }
    let denied = whole_file_record_lock(&record, F_WRLCK);
    let errno = denied.as_ref().err().and_then(Error::raw_os_error);
    if errno != Some(Errno::EACCES as i32) && errno != Some(Errno::EAGAIN as i32) {
        return Err(format!("F_SETLK under contention: {denied:?}"));
    }
    writer
        .write_all(&[CONFLICTS_CHECKED])
        .map_err(|err| format!("send conflicts checked: {err}"))?;
    require_byte(&mut reader, LOCKS_RELEASED)?;
    whole_file_flock(&bsd, LOCK_EX | LOCK_NB)
        .map_err(|err| format!("flock after release: {err}"))?;
    whole_file_record_lock(&record, F_WRLCK)
        .map_err(|err| format!("F_SETLK after release: {err}"))?;
    Ok(())
}

fn run_parent(dir: &Path) -> Result<(), String> {
    let (bsd, record) = lock_files(dir, true)?;
    whole_file_flock(&bsd, LOCK_EX | LOCK_NB).map_err(|err| format!("parent flock: {err}"))?;
    whole_file_record_lock(&record, F_WRLCK).map_err(|err| format!("parent F_SETLK: {err}"))?;

    let (child_read, mut parent_write) = cloexec_pipe()?;
    let (mut parent_read, child_write) = cloexec_pipe()?;
    inherit(&child_read)?;
    inherit(&child_write)?;
    let dir = CString::new(dir.as_os_str().as_encoded_bytes()).map_err(|err| err.to_string())?;
    let fds = [&child_read, &child_write, &parent_read, &parent_write]
        .map(|end| CString::new(end.as_raw_fd().to_string()).expect("decimal descriptor"));
    let pid = spawn_self(&[c"--check-locks", &dir, &fds[0], &fds[1], &fds[2], &fds[3]])?;
    drop(child_read);
    drop(child_write);
    let exchange = (|| {
        require_byte(&mut parent_read, CONFLICTS_CHECKED)?;
        whole_file_flock(&bsd, LOCK_UN).map_err(|err| format!("parent flock unlock: {err}"))?;
        whole_file_record_lock(&record, F_UNLCK)
            .map_err(|err| format!("parent F_SETLK unlock: {err}"))?;
        parent_write
            .write_all(&[LOCKS_RELEASED])
            .map_err(|err| format!("send locks released: {err}"))
    })();
    // EOF also releases a child waiting on the pipe if the parent hit an error.
    drop(parent_write);
    drop(parent_read);
    let status = wait_for(pid)?;
    exchange?;
    if !process::wifexited(status) || process::wexitstatus(status) != 3 {
        return Err(format!(
            "lock child wait status {status:#x}, expected exit 3"
        ));
    }

    let status = wait_for(spawn_self(&[c"--terminate"])?)?;
    if !process::wifsignaled(status) || process::wtermsig(status) != signal::SIGTERM {
        return Err(format!(
            "signal child wait status {status:#x}, expected SIGTERM"
        ));
    }
    Ok(())
}

fn run() -> Result<i32, String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [mode] if mode == "--terminate" => {
            let sig = signal::SIGTERM;
            signal::sigaction(sig, Some(&signal::Sigaction::default()), None)
                .map_err(|err| err.to_string())?;
            signal::sigprocmask(signal::SIG_UNBLOCK, Some(&signal::sigmask(sig)), None)
                .map_err(|err| err.to_string())?;
            // SAFETY: SIGTERM is a valid signal number with its default disposition.
            let result = unsafe { raise(sig) };
            Err(format!("raise(SIGTERM) returned {result}"))
        }
        [dir] => {
            run_parent(Path::new(dir))?;
            Ok(0)
        }
        [mode, dir, read, write, closed_read, closed_write] if mode == "--check-locks" => {
            let mut fds = [0; 4];
            for (fd, arg) in fds.iter_mut().zip([read, write, closed_read, closed_write]) {
                *fd = arg
                    .parse()
                    .map_err(|_| format!("invalid pipe descriptor {arg}"))?;
            }
            check_locks(Path::new(dir), fds)?;
            Ok(3)
        }
        _ => Err("invalid process probe arguments".into()),
    }
}

fn main() {
    let code = run().unwrap_or_else(|err| {
        eprintln!("{err}");
        1
    });
    std::process::exit(code);
}
