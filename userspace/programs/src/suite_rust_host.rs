//! Observe the hosted probe's startup and exit across an exec and pipe.
#![deny(unsafe_op_in_unsafe_fn)]

use breenix_suite::suite::{case, category, suite, CaseError, CaseResult, Suite};
use libbreenix::{errno::Errno, error::Error, io, process, types::OwnedFd};
use std::ffi::{c_void, CString};
use std::fs::File;
use std::io::Read;
use std::os::fd::FromRawFd;
use std::ptr::null;

static SUITE: Suite = suite(
    "rust-host",
    "Rust host runtime",
    &[category(
        "libc-surface",
        "Hosted libc surface",
        &[case(
            "startup-order",
            "Startup, exit, thread destructors and program headers",
            startup_order,
        )],
    )],
);

extern "C" {
    fn posix_spawn(
        pid: *mut i32,
        path: *const u8,
        actions: *const c_void,
        attr: *const c_void,
        argv: *const *const u8,
        envp: *const *const u8,
    ) -> i32;
}

fn startup_order() -> CaseResult {
    let (read_fd, write_fd) =
        io::pipe().map_err(|err| CaseError::Fail(format!("startup pipe: {err}")))?;
    // SAFETY: pipe returned two distinct descriptors owned by this process.
    let reader = unsafe { File::from_raw_fd(read_fd.raw() as i32) };
    let writer = OwnedFd::new(write_fd);
    io::fcntl_setfd(read_fd, io::fd_flags::FD_CLOEXEC)
        .map_err(|err| CaseError::Fail(format!("startup pipe close-on-exec: {err}")))?;

    let probe = c"/usr/libexec/breenix/rust-host/startup-probe";
    let fd_arg = CString::new(write_fd.raw().to_string()).expect("decimal descriptor");
    let argv = [probe.as_ptr().cast(), fd_arg.as_ptr().cast(), null()];
    let envp = [null()];
    let mut pid = 0;
    // SAFETY: the path and both null-terminated vectors live through the call.
    // Null actions and attributes request POSIX defaults.
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
    drop(writer);
    if result != 0 {
        return Err(CaseError::Fail(format!(
            "posix_spawn startup probe: errno {result}"
        )));
    }
    if pid <= 0 {
        return Err(CaseError::Fail(format!(
            "posix_spawn returned invalid child PID {pid}"
        )));
    }

    // ELF gABI initialization, joined thread destruction, process LIFO and reverse fini.
    const EXPECTED: &[u8] = b"abcdmstxrjkehzy";
    let mut order = Vec::new();
    let mut reader = reader.take((EXPECTED.len() + 1) as u64);
    let read_result = reader.read_to_end(&mut order);
    drop(reader);

    let mut status = 0;
    loop {
        match process::waitpid(pid, &mut status, 0) {
            Ok(_) => break,
            Err(Error::Os(Errno::EINTR)) => continue,
            Err(err) => return Err(CaseError::Fail(format!("wait for startup probe: {err}"))),
        }
    }
    read_result.map_err(|err| CaseError::Fail(format!("read startup pipe: {err}")))?;
    if !process::wifexited(status) || process::wexitstatus(status) != 0 || order != EXPECTED {
        return Err(CaseError::Fail(format!(
            "startup probe wait status {status:#x}; events {:?}, expected {:?}",
            String::from_utf8_lossy(&order),
            String::from_utf8_lossy(EXPECTED),
        )));
    }
    Ok(())
}

fn main() {
    SUITE.run()
}
