//! Exercise hosted dynamic executables through the kernel's execve path.

use breenix_suite::suite::{case, category, check, fail, suite, CaseError, CaseResult, Suite};
use libbreenix::{errno::Errno, error::Error, process};
use std::ffi::CStr;
use std::ptr::null;

const PROBE: &CStr = c"/usr/libexec/breenix/dynamic/dynamic-probe";
const PANIC_PROBE: &CStr = c"/usr/libexec/breenix/dynamic/dynamic-panic-probe";
const MISSING_INTERP: &CStr = c"/usr/libexec/breenix/dynamic/missing-interp";

static SUITE: Suite = suite(
    "dynamic",
    "Dynamic linking",
    &[category(
        "dynamic",
        "Hosted dynamic executables",
        &[
            case(
                "interp-runs-program",
                "Interpreter enters a two-DSO program with its auxv",
                interp_runs_program,
            ),
            case(
                "missing-interp-fails",
                "Missing interpreter returns ENOENT in the caller",
                missing_interp_fails,
            ),
            case(
                "dso-relocs-and-tls",
                "X2 relocation values and independent zeroed TLS",
                dso_relocs_and_tls,
            ),
            case(
                "panic-across-dso",
                "Rust panic crosses a loaded DSO and runs its cleanup",
                panic_across_dso,
            ),
        ],
    )],
);

fn main() {
    SUITE.run()
}

fn run_probe(probe: &CStr, mode: Option<&CStr>) -> CaseResult {
    match process::fork()? {
        process::ForkResult::Child => {
            let argv = [
                probe.as_ptr().cast(),
                mode.map_or(null(), |arg| arg.as_ptr().cast()),
                null(),
            ];
            let error =
                process::execve(probe.to_bytes_with_nul(), argv.as_ptr(), [null()].as_ptr())
                    .unwrap_err();
            eprintln!("{probe:?} execve: {error}");
            process::exit(126);
        }
        process::ForkResult::Parent(pid) => {
            let mut status = 0;
            loop {
                match process::waitpid(pid.raw() as i32, &mut status, 0) {
                    Err(Error::Os(Errno::EINTR)) => continue,
                    result => {
                        result?;
                        break;
                    }
                }
            }
            if process::wifexited(status) && process::wexitstatus(status) == 0 {
                Ok(())
            } else {
                fail(format!("{probe:?} {mode:?} wait status {status:#x}"))
            }
        }
    }
}

fn interp_runs_program() -> CaseResult {
    run_probe(PROBE, Some(c"auxv"))
}

fn missing_interp_fails() -> CaseResult {
    let path = MISSING_INTERP.to_str().expect("ASCII fixture path");
    let metadata = std::fs::metadata(path)
        .map_err(|error| CaseError::Fail(format!("missing-interpreter program: {error}")))?;
    check(
        metadata.is_file(),
        "missing-interpreter program is not a file",
    )?;
    let caller = process::getpid()?;
    let argv = [MISSING_INTERP.as_ptr().cast(), null()];
    let error = process::execve(
        MISSING_INTERP.to_bytes_with_nul(),
        argv.as_ptr(),
        [null()].as_ptr(),
    )
    .unwrap_err();
    if !matches!(error, Error::Os(Errno::ENOENT)) {
        return fail(format!(
            "missing interpreter returned {error}; expected ENOENT"
        ));
    }
    check(
        process::getpid()? == caller,
        "execve changed the caller PID",
    )?;
    let mut status = 0;
    let waited = process::waitpid(-1, &mut status, process::WNOHANG);
    check(
        matches!(waited, Err(Error::Os(Errno::ECHILD))),
        "failed execve left a waitable child or a live child",
    )
}

fn dso_relocs_and_tls() -> CaseResult {
    run_probe(PROBE, Some(c"relocs"))
}

fn panic_across_dso() -> CaseResult {
    run_probe(PANIC_PROBE, None)
}
