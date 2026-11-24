//! Exercise the hosted C++ runtime through separate probe programs.
#![deny(unsafe_op_in_unsafe_fn)]

use breenix_suite::suite::{case, category, suite, CaseError, CaseResult, Suite};
use libbreenix::{errno::Errno, error::Error, io, process, types::OwnedFd};
use std::ffi::{c_void, CString};
use std::fs::File;
use std::io::Read;
use std::os::fd::FromRawFd;
use std::ptr::null;

static SUITE: Suite = suite("cxx-runtime", "C++ runtime", &[category("cxx-runtime", "C++ runtime", &[
    case("static-init-order", "Static initialization and destruction order", static_init_order),
    case("guarded-static-8-threads", "Guarded static initialization with eight threads", guarded_static),
    case("containers-and-threads", "Containers and threads", containers_threads),
])]);

extern "C" {
    fn posix_spawn(pid: *mut i32, path: *const u8, actions: *const c_void, attr: *const c_void,
                   argv: *const *const u8, envp: *const *const u8) -> i32;
}

fn run_probe(name: &str) -> CaseResult {
    let expectations: serde_json::Value = serde_json::from_str(include_str!("../cxx/expect.json"))
        .map_err(|e| CaseError::Fail(format!("invalid expectations: {e}")))?;
    let expected = expectations[name].as_str().ok_or_else(|| CaseError::Fail(format!("missing expectation: {name}")))?;
    let (read_fd, write_fd) = io::pipe().map_err(|e| CaseError::Fail(format!("pipe: {e}")))?;
    // SAFETY: the pipe supplies a newly owned descriptor.
    let mut reader = unsafe { File::from_raw_fd(read_fd.raw() as i32) };
    let writer = OwnedFd::new(write_fd);
    io::fcntl_setfd(read_fd, io::fd_flags::FD_CLOEXEC).map_err(|e| CaseError::Fail(format!("fcntl: {e}")))?;
    let path = CString::new(format!("/usr/libexec/breenix/cxx-runtime/{name}")).expect("probe name");
    let fd = CString::new(write_fd.raw().to_string()).expect("descriptor");
    let argv = [path.as_ptr().cast(), fd.as_ptr().cast(), null()];
    let envp = [null()];
    let mut pid = 0;
    // SAFETY: path and null-terminated vectors remain valid for the call.
    let result = unsafe { posix_spawn(&mut pid, argv[0], null(), null(), argv.as_ptr(), envp.as_ptr()) };
    drop(writer);
    if result != 0 || pid <= 0 { return Err(CaseError::Fail(format!("spawn {name}: errno {result}, pid {pid}"))); }
    let mut events = Vec::new();
    let read_result = reader.read_to_end(&mut events);
    drop(reader);
    let mut status = 0;
    loop {
        match process::waitpid(pid, &mut status, 0) {
            Ok(_) => break,
            Err(Error::Os(Errno::EINTR)) => continue,
            Err(e) => return Err(CaseError::Fail(format!("wait {name}: {e}"))),
        }
    }
    read_result.map_err(|e| CaseError::Fail(format!("read {name}: {e}")))?;
    if !process::wifexited(status) || process::wexitstatus(status) != 0 || events != expected.as_bytes() {
        return Err(CaseError::Fail(format!("{name}: status {status:#x}, events {:?}, expected {:?}", String::from_utf8_lossy(&events), expected)));
    }
    Ok(())
}
fn static_init_order() -> CaseResult { run_probe("static-init-order") }
fn guarded_static() -> CaseResult { run_probe("guarded-static") }
fn containers_threads() -> CaseResult { run_probe("containers-threads") }
fn main() { SUITE.run() }
