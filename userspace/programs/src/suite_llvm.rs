//! Exercise the hosted LLVM compiler against the staged corpus.
#![deny(unsafe_op_in_unsafe_fn)]

use breenix_suite::suite::{case, category, suite, CaseError, CaseResult, Suite};
use libbreenix::{crypto::sha256::sha256, errno::Errno, error::Error, io, process, types::OwnedFd};
use serde_json::Value;
use std::ffi::{c_void, CString};
use std::fs::{self, File};
use std::io::Read;
use std::os::fd::FromRawFd;
use std::path::Path;
use std::ptr::{null, null_mut};

static SUITE: Suite = suite("llvm", "LLVM", &[category("llvm", "LLVM", &[
    case("llc-arithmetic-equals-mac", "LLC arithmetic object equals Mac", arithmetic_equals_mac),
    case("llc-hello-equals-mac", "LLC hello object equals Mac", hello_equals_mac),
    case("llc-libc-call-equals-mac", "LLC libc call object equals Mac", libc_call_equals_mac),
    case("llc-loop-equals-mac", "LLC loop object equals Mac", loop_equals_mac),
    case("llc-struct-equals-mac", "LLC struct object equals Mac", struct_equals_mac),
    case("llc-object-runs", "LLC object runs", object_runs),
])]);

const FIXTURE: &str = "/usr/libexec/breenix/llvm";

#[repr(C)]
struct FileActions { head: *mut c_void, tail: *mut c_void }
extern "C" {
    fn posix_spawn(pid: *mut i32, path: *const u8, actions: *const FileActions, attr: *const c_void,
                   argv: *const *const u8, envp: *const *const u8) -> i32;
    fn posix_spawn_file_actions_init(actions: *mut FileActions) -> i32;
    fn posix_spawn_file_actions_adddup2(actions: *mut FileActions, fd: i32, newfd: i32) -> i32;
    fn posix_spawn_file_actions_destroy(actions: *mut FileActions) -> i32;
}

fn fail(message: impl std::fmt::Display) -> CaseError { CaseError::Fail(message.to_string()) }

fn run(path: &str, args: &[String], capture: bool) -> Result<(i32, Vec<u8>), CaseError> {
    let strings: Vec<CString> = std::iter::once(path).chain(args.iter().map(String::as_str))
        .map(CString::new).collect::<Result<_, _>>().map_err(fail)?;
    let mut argv: Vec<*const u8> = strings.iter().map(|s| s.as_ptr().cast()).collect();
    argv.push(null());
    let envp = [null()];
    let mut actions = FileActions { head: null_mut(), tail: null_mut() };
    let mut reader = None;
    let mut writer = None;
    if capture {
        let (read_fd, write_fd) = io::pipe().map_err(fail)?;
        // SAFETY: the pipe supplies a newly owned descriptor.
        reader = Some(unsafe { File::from_raw_fd(read_fd.raw() as i32) });
        writer = Some(OwnedFd::new(write_fd));
        io::fcntl_setfd(read_fd, io::fd_flags::FD_CLOEXEC).map_err(fail)?;
        // SAFETY: actions is valid and initialized before use.
        let result = unsafe { posix_spawn_file_actions_init(&mut actions) };
        if result != 0 { return Err(fail(format!("spawn actions: {result}"))); }
        // SAFETY: the initialized actions and pipe descriptor remain valid.
        let result = unsafe { posix_spawn_file_actions_adddup2(&mut actions, write_fd.raw() as i32, 1) };
        if result != 0 {
            // SAFETY: actions was initialized above.
            unsafe { posix_spawn_file_actions_destroy(&mut actions); }
            return Err(fail(format!("spawn dup2: {result}")));
        }
    }
    let mut pid = 0;
    // SAFETY: path, actions and argument vectors remain valid for the call.
    let result = unsafe { posix_spawn(&mut pid, argv[0], if capture { &actions } else { null() }, null(), argv.as_ptr(), envp.as_ptr()) };
    if capture {
        // SAFETY: actions was initialized above and is no longer in use.
        unsafe { posix_spawn_file_actions_destroy(&mut actions); }
    }
    drop(writer);
    if result != 0 || pid <= 0 { return Err(fail(format!("spawn {path}: errno {result}, pid {pid}"))); }
    let mut output = Vec::new();
    let read_result = if let Some(mut reader) = reader { reader.read_to_end(&mut output).map(|_| ()) } else { Ok(()) };
    let mut status = 0;
    loop {
        match process::waitpid(pid, &mut status, 0) {
            Ok(_) => break,
            Err(Error::Os(Errno::EINTR)) => continue,
            Err(e) => return Err(fail(format!("wait {path}: {e}"))),
        }
    }
    read_result.map_err(fail)?;
    if !process::wifexited(status) { return Err(fail(format!("{path}: abnormal status {status:#x}"))); }
    Ok((process::wexitstatus(status), output))
}

fn expectations() -> Result<Value, CaseError> {
    let path = format!("{FIXTURE}/corpus/expect.json");
    serde_json::from_slice(&fs::read(path).map_err(fail)?).map_err(fail)
}
fn compile(entry: &Value, output: &str) -> CaseResult {
    let input = entry["input"].as_str().ok_or_else(|| fail("missing input"))?;
    let input = format!("{FIXTURE}/corpus/{input}");
    let arguments = entry["arguments"].as_array().ok_or_else(|| fail("missing arguments"))?;
    let args: Vec<String> = arguments.iter().map(|v| v.as_str().ok_or_else(|| fail("invalid argument")).map(|s| match s {
        "<input>" => input.clone(), "<output>" => output.to_string(), _ => s.to_string(),
    })).collect::<Result<_, _>>()?;
    let (status, _) = run(&format!("{FIXTURE}/bin/llc"), &args, false)?;
    if status != 0 { return Err(fail(format!("llc {input}: exit {status}"))); }
    Ok(())
}
fn temp(name: &str) -> String { format!("/tmp/llvm-{}-{name}", std::process::id()) }
/// Compile one corpus object with the box's llc (one llc run per case) and
/// compare it with the Mac llc's object.
fn object_equals(name: &str) -> CaseResult {
    let expectations = expectations()?;
    let objects = expectations["objects"].as_array().ok_or_else(|| fail("missing objects"))?;
    let entry = objects.iter().find(|entry| entry["input"].as_str() == Some(name))
        .ok_or_else(|| fail(format!("missing corpus object {name}")))?;
    let output = temp("object.o");
    let _ = fs::remove_file(&output);
    compile(entry, &output)?;
    let bytes = fs::read(&output).map_err(fail)?;
    let actual: String = sha256(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    if entry["sha256"].as_str() != Some(actual.as_str()) { return Err(fail(format!("object digest mismatch: {name}"))); }
    fs::remove_file(output).map_err(fail)?;
    Ok(())
}
fn arithmetic_equals_mac() -> CaseResult { object_equals("arithmetic.ll") }
fn hello_equals_mac() -> CaseResult { object_equals("hello.ll") }
fn libc_call_equals_mac() -> CaseResult { object_equals("libc-call.ll") }
fn loop_equals_mac() -> CaseResult { object_equals("loop.ll") }
fn struct_equals_mac() -> CaseResult { object_equals("struct.ll") }
fn object_runs() -> CaseResult {
    let expectations = expectations()?;
    let program = &expectations["program"];
    let name = program["input"].as_str().ok_or_else(|| fail("missing program input"))?;
    let objects = expectations["objects"].as_array().ok_or_else(|| fail("missing objects"))?;
    let entry = objects.iter().find(|entry| entry["input"].as_str() == Some(name)).ok_or_else(|| fail("missing program object"))?;
    let object = temp("program.o");
    let executable = temp("program");
    let _ = fs::remove_file(&object);
    let _ = fs::remove_file(&executable);
    compile(entry, &object)?;
    let args = vec!["--gc-sections".into(), "-T".into(), format!("{FIXTURE}/linker.ld"), "-o".into(), executable.clone(), object.clone(), format!("{FIXTURE}/lib/libc.a")];
    let (status, _) = run(&format!("{FIXTURE}/bin/bld"), &args, false)?;
    if status != 0 { return Err(fail(format!("bld: exit {status}"))); }
    let (status, output) = run(&executable, &[], true)?;
    let expected_status = program["exit_status"].as_i64().ok_or_else(|| fail("missing status"))?;
    let expected_output = program["stdout"].as_str().ok_or_else(|| fail("missing stdout"))?;
    if i64::from(status) != expected_status || output != expected_output.as_bytes() {
        return Err(fail(format!("program: exit {status}, output {:?}", String::from_utf8_lossy(&output))));
    }
    for path in [&object, &executable] { fs::remove_file(Path::new(path)).map_err(fail)?; }
    Ok(())
}
fn main() { SUITE.run() }
