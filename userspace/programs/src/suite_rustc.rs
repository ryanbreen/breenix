//! Exercise the hosted Rust compiler and staged twin sysroot.
#![deny(unsafe_op_in_unsafe_fn)]

use breenix_suite::suite::{case, case_diagnostic, category, suite, CaseError, CaseResult, Suite};
use libbreenix::{crypto::sha256::sha256, errno::Errno, error::Error, io, process, types::OwnedFd};
use serde_json::Value;
use std::ffi::{c_void, CString};
use std::fs::{self, File};
use std::io::Read;
use std::os::fd::FromRawFd;
use std::ptr::{null, null_mut};

#[path = "suite_rustc/command_status.rs"]
mod command_status;

static SUITE: Suite = suite("rustc", "Rust compiler", &[category("rustc", "Rust compiler", &[
    case("identity-and-sysroot", "Compiler identity and sysroot", identity_and_sysroot),
    case("hello-object-equals-mac", "Hello object equals Mac", hello_object_equals_mac),
    case("diagnostic-located", "Diagnostic is located", diagnostic_located),
])]).case_limit_ms(60_000);

const FIXTURE: &str = "/usr/libexec/breenix/rustc";
// The twin rlibs carry the custom target identity from this JSON file.
const TARGET: &str = "../targets/x86_64-breenix.json";
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
    // The compiler still resolves its own hosted target by name.
    let target_path = CString::new(format!("RUST_TARGET_PATH={FIXTURE}/targets")).map_err(fail)?;
    let envp = [target_path.as_ptr().cast(), null()];
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
        // SAFETY: the initialized actions and pipe descriptor remain valid.
        let result = unsafe { posix_spawn_file_actions_adddup2(&mut actions, write_fd.raw() as i32, 2) };
        if result != 0 {
            // SAFETY: actions was initialized above.
            unsafe { posix_spawn_file_actions_destroy(&mut actions); }
            return Err(fail(format!("spawn stderr dup2: {result}")));
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
    if !process::wifexited(status) {
        let failure = command_status::abnormal_status(path, status, &output);
        if let Some(line) = failure.lines().next() {
            case_diagnostic(&format!("rustc compiler: {line}"));
        }
        return Err(fail(failure));
    }
    Ok((process::wexitstatus(status), output))
}


fn expectations() -> Result<Value, CaseError> {
    serde_json::from_slice(&fs::read(format!("{FIXTURE}/corpus/expect.json")).map_err(fail)?).map_err(fail)
}

fn compiler(args: &[String]) -> Result<(i32, Vec<u8>), CaseError> {
    run(&format!("{FIXTURE}/bin/rustc"), args, true)
}

fn compile(input: &str, extra: &[&str]) -> Result<(i32, Vec<u8>, String), CaseError> {
    std::env::set_current_dir(format!("{FIXTURE}/corpus")).map_err(fail)?;
    let output = format!("/tmp/rustc-{}-{input}.o", std::process::id());
    let _ = fs::remove_file(&output);
    let mut args: Vec<String> = ["--edition=2021", "--sysroot", &format!("{FIXTURE}/sysroot"),
        "-Dwarnings", "--emit=obj", "--target", TARGET]
        .iter().map(|s| (*s).to_string()).collect();
    args.extend(extra.iter().map(|s| (*s).to_string()));
    args.extend([input.to_string(), "-o".into(), output.clone()]);
    let (status, capture) = compiler(&args)?;
    Ok((status, capture, output))
}

fn identity_and_sysroot() -> CaseResult {
    let expected = expectations()?;
    let (status, output) = compiler(&["-vV".into()])?;
    let text = std::str::from_utf8(&output).map_err(fail)?;
    if status != 0 { return Err(fail(format!("rustc -vV: exit {status}"))); }
    let host = expected["hosted_host"].as_str().ok_or_else(|| fail("missing hosted host"))?;
    if host != "x86_64-breenix-hosted" { return Err(fail("unexpected hosted target in expectations")); }
    let host_line = format!("host: {host}");
    let identity = expected["rustc_vv"].as_array().ok_or_else(|| fail("missing identity"))?;
    for key in ["release: ", "commit-hash: ", "commit-date: ", "LLVM version: "] {
        if identity.iter().filter(|v| v.as_str().is_some_and(|s| s.starts_with(key))).count() != 1 {
            return Err(fail(format!("missing identity field {key}")));
        }
    }
    if identity.len() != 4 { return Err(fail("unexpected identity fields")); }
    for line in std::iter::once(host_line.as_str()).chain(
        identity.iter()
            .map(|v| v.as_str().unwrap_or(""))) {
        if line.is_empty() || text.lines().filter(|actual| *actual == line).count() != 1 {
            return Err(fail(format!("rustc identity missing {line:?}: {text}")));
        }
    }
    let (status, output, path) = compile("uses-std.rs", &[])?;
    if status != 0 { return Err(fail(format!("uses-std: exit {status}: {}", String::from_utf8_lossy(&output)))); }
    fs::remove_file(path).map_err(fail)?;
    Ok(())
}

fn hello_object_equals_mac() -> CaseResult {
    let expected = expectations()?;
    let (status, output, path) = compile("hello.rs", &["--crate-type=lib"])?;
    if status != 0 { return Err(fail(format!("hello: exit {status}: {}", String::from_utf8_lossy(&output)))); }
    let bytes = fs::read(&path).map_err(fail)?;
    let actual: String = sha256(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    if expected["hello_o_sha256"].as_str() != Some(actual.as_str()) { return Err(fail(format!("hello digest mismatch: {actual}"))); }
    fs::remove_file(path).map_err(fail)?;
    Ok(())
}

fn diagnostic_located() -> CaseResult {
    let expected = expectations()?;
    let (status, output, path) = compile("type-error.rs", &["--error-format=json"])?;
    if status != 1 || std::path::Path::new(&path).exists() { return Err(fail(format!("type error: exit {status} or unexpected object"))); }
    let location = expected["type_error"].as_str().ok_or_else(|| fail("missing diagnostic location"))?;
    let text = std::str::from_utf8(&output).map_err(fail)?;
    let mut matches = 0;
    for line in text.lines() {
        let diagnostic: Value = serde_json::from_str(line).map_err(fail)?;
        if diagnostic["level"] == "error" && diagnostic["code"]["code"] == "E0308" {
            for span in diagnostic["spans"].as_array().ok_or_else(|| fail("missing spans"))? {
                if span["is_primary"] == true {
                    let actual = format!("{}:{}:{}", span["file_name"].as_str().ok_or_else(|| fail("missing file"))?,
                        span["line_start"].as_u64().ok_or_else(|| fail("missing line"))?,
                        span["column_start"].as_u64().ok_or_else(|| fail("missing column"))?);
                    if actual == location { matches += 1; }
                }
            }
        }
    }
    if matches != 1 { return Err(fail(format!("expected primary diagnostic {location}: {text}"))); }
    Ok(())
}

fn main() { SUITE.run() }
