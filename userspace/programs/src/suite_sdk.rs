//! The Developer SDK: Breenix's own applications built outside the kernel tree against a
//! packaged toolchain, headers and libraries, run here.
//!
//! The suite has a host side, scripts/sdk-suite.py, which runs before the boot: it builds
//! the SDK from a clean clone of the commit, builds the test apps in
//! userspace/sdk-suite/apps in fresh directories with nothing but the SDK, installs some
//! with the SDK's disk tool and follows the docs' quick start, then stages what it built
//! and one record per host step onto this disk under /usr/local/sdk-suite. A record,
//! /usr/local/sdk-suite/host/<category>/<case>, reads "PASS" or "FAIL <reason>".
//!
//! Cases that measure a host step report its record. Cases that run a program run the
//! staged or installed one in a scratch directory of their own with the environment
//! PATH=/usr/local/bin:/usr/bin:/bin, HOME=/tmp and SDK_SUITE_VAR, and compare its exact
//! output and exit status. A program or record that is not on the disk fails its case
//! with the host step's reason, so a host step that failed (the SDK does not exist yet)
//! is never a skip. The program a case starts gets until 1.5 seconds before the case's
//! 10-second limit; one still running then is killed and the case fails saying so.
use libbreenix::io::{self, fcntl_cmd, poll_events, status_flags, PollFd};
use libbreenix::process;
use libbreenix::signal;
use libbreenix::suite::{case, case_ms_left, category, check, fail, running_case, suite, CaseError, CaseResult, Suite};
use libbreenix::time;
use libbreenix::types::Fd;
use std::ffi::CString;
use std::os::unix::fs::MetadataExt;

/// What the host harness staged.
const STAGE: &str = "/usr/local/sdk-suite";
/// Where the SDK's disk tool installs programs.
const BIN: &str = "/usr/local/bin";
/// The whole environment every program the suite starts receives.
const ENV: &[&str] = &["PATH=/usr/local/bin:/usr/bin:/bin", "HOME=/tmp", "SDK_SUITE_VAR=sdk value"];
/// How much of the case's limit is kept back for killing and reporting a program that hangs.
const RESERVE_MS: u64 = 1500;

#[cfg(target_arch = "aarch64")]
const ARCH: &str = "aarch64";
#[cfg(target_arch = "aarch64")]
const MACHINE: u16 = 183;
#[cfg(target_arch = "x86_64")]
const ARCH: &str = "x86_64";
#[cfg(target_arch = "x86_64")]
const MACHINE: u16 = 62;

static SUITE: Suite = suite("sdk", "Developer SDK", &[
    category("toolchain", "packaged toolchains", &[
        case("build-script", "The checkout has one SDK build script, scripts/build-sdk.sh, and it is executable", host_case),
        case("clean-build", "scripts/build-sdk.sh --prefix DIR builds the SDK from a clean clone of the commit, offline, and exits 0", host_case),
        case("build-quiet", "The SDK build prints no compiler warnings (the pinned core library's NEON notice, #945, excepted)", host_case),
        case("version-commit", "The SDK's VERSION is one line, breenix-sdk <version> <commit>, naming the full commit it was built from", host_case),
        case("reproducible", "Two builds of the commit from clean clones at different paths give byte-identical SDK trees", host_case),
        case("cc-aarch64", "bin/aarch64-breenix-cc --version exits 0 and its first line names breenix-sdk, aarch64 and the commit", host_case),
        case("cc-x86-64", "bin/x86_64-breenix-cc --version exits 0 and its first line names breenix-sdk, x86_64 and the commit", host_case),
        case("cargo-aarch64", "bin/aarch64-breenix-cargo --version exits 0 and its first line names breenix-sdk, aarch64 and the commit", host_case),
        case("cargo-x86-64", "bin/x86_64-breenix-cargo --version exits 0 and its first line names breenix-sdk, x86_64 and the commit", host_case),
        case("libc-archives", "The SDK ships a libc.a for aarch64 and one for x86_64, each holding objects for its own machine only", host_case),
        case("rust-std", "The SDK ships a prebuilt Rust std (libstd-*.rlib) for both Breenix targets", host_case),
        case("std-fork-pin", "share/breenix-sdk/rust-fork.toml is the commit's rust-fork.toml, the pinned fork its std was built from", host_case),
        case("target-specs", "share/breenix-sdk/targets holds rustc target specs with os breenix for aarch64 and x86_64", host_case),
        case("headers-c17", "Each of the 29 ISO C17 headers compiles alone and all compile together with -std=c17, for both arches", host_case),
        case("headers-posix", "Each of the 86 POSIX.1-2024 headers compiles alone and all compile together under _POSIX_C_SOURCE 202405L, for both arches", host_case),
        case("header-symbols", "A program naming 78 common POSIX functions and 20 constants, from every major header, compiles and links for both arches", host_case),
        case("headers-self-contained", "A compile reads headers only from the SDK and the compiler's own resource directory, none from the host or the checkout", host_case),
        case("no-host-paths", "No SDK file holds the checkout's path, the clean clones' paths, the Rust fork's path or the builder's home directory", host_case),
        case("no-musl", "No SDK file carries musl's marks, and no archive defines compiler_builtins' libm math, a port of musl's libm, or holds its module or sources in metadata, bitcode or data", host_case),
        case("no-glibc", "No SDK file carries glibc's marks (__GLIBC__, GNU C Library, __THROW, libc.so.6, ld-linux, glibc)", host_case),
        case("no-other-libc", "No SDK file carries the marks of newlib, bionic, uClibc, dietlibc or picolibc", host_case),
        case("relocatable", "A copy of the SDK in another directory still compiles and links a C program for each arch", host_case),
    ]),
    category("c-app", "a C app built outside the tree", &[
        case("built", "The C app's eight source files compile and link with only the SDK's driver, in a directory outside the checkout", host_case),
        case("outside-tree", "No file of the C app's build (sources, dependency lists, objects, program) names a path into the checkout", host_case),
        case("warnings", "The C app compiles with -Wall -Wextra and no warnings", host_case),
        case("elf", "The C app is a static ELF executable for this machine (no PT_INTERP)", c_elf),
        case("hello", "c-app hello prints \"hello from c-app\" and exits 0", c_hello),
        case("args", "c-app receives its arguments, including an empty one, and getenv finds the environment it was given", c_args),
        case("exit-status", "c-app exit 42 ends with exit status 42 and prints nothing", c_exit),
        case("stdio-format", "printf and snprintf format integers, strings, characters and floats with widths, flags and truncation", c_stdio_format),
        case("stdio-stream", "fopen, fprintf, fputs, fgets, fseek and ftell write and read back a two-line file", c_stdio_stream),
        case("stderr", "fprintf(stderr) and printf reach standard error and standard output separately", c_stderr),
        case("files", "open, write, lseek, read, fstat, rename and unlink work on a file created with mode 0640", c_files),
        case("dirs", "mkdir, opendir, readdir, closedir and rmdir create, list and remove a directory of three files", c_dirs),
        case("spawn", "posix_spawn executes with actions and attributes, restores the parent mask and reports child failure as status 127", c_spawn),
        case("fork-wait", "fork and waitpid return a child's exit status 7", c_fork_wait),
        case("exec", "A forked child execv's the app, and the parent's waitpid gets the exit status 3 it asked for", c_exec),
        case("pipe", "A pipe carries a child's message to its parent, which reads it to end of file", c_pipe),
        case("signal-handler", "A sigaction handler runs once for raise and once more for kill to the process itself", c_signal_handler),
        case("signal-mask", "A signal blocked by sigprocmask stays pending, then is delivered when unblocked", c_signal_mask),
        case("threads", "Four pthreads each sum a range and pthread_join returns their results", c_threads),
        case("mutex", "Four pthreads incrementing a counter 10000 times each under a mutex reach 40000", c_mutex),
        case("condvar", "A producer thread hands 100 values through one slot guarded by a condition variable", c_condvar),
        case("socket-unix", "A socketpair carries a message to a forked child and its reply back", c_socket_unix),
        case("socket-tcp", "A TCP server on 127.0.0.1 accepts a forked client, reads its request and replies", c_socket_tcp),
        case("socket-udp", "A UDP datagram sent between two sockets on 127.0.0.1 arrives with the sender's port", c_socket_udp),
        case("math", "libm's sqrt, sin, cos, exp, log, pow, fmod, floor, ceil, atan2, hypot, cbrt and lround give the expected values", c_math),
    ]),
    category("rust-app", "a Rust app built outside the tree", &[
        case("built", "The Rust std app, a cargo project outside the checkout, builds with only the SDK's cargo wrapper, offline", host_case),
        case("nostd-built", "The no_std app builds with only the SDK's cargo wrapper, taking libbreenix from the SDK by version", host_case),
        case("lto", "The Rust std app also builds with cross-crate fat LTO, which needs bitcode in every rlib of the SDK's std", host_case),
        case("outside-tree", "Neither Rust app's build (dependency lists, lock file, program) names a path into the checkout", host_case),
        case("elf", "The Rust std app is a static ELF executable for this machine (no PT_INTERP)", rust_elf),
        case("hello", "rust-app hello prints \"hello from rust-app\" and exits 0", rust_hello),
        case("args", "std::env::args gives rust-app its arguments, including an empty one", rust_args),
        case("env", "std::env::var reads the environment it was given, and vars_os counts exactly its three variables", rust_env),
        case("exit-code", "std::process::exit(17) ends rust-app with exit status 17", rust_exit),
        case("fs-rw", "std::fs::write, OpenOptions append and read_to_string write and read back a file", rust_fs_rw),
        case("fs-dir", "std::fs::create_dir_all, read_dir, rename, remove_file, remove_dir and exists manage a directory tree", rust_fs_dir),
        case("fs-metadata", "std::fs::set_permissions and metadata report length, file type and mode 0640", rust_fs_metadata),
        case("process-status", "std::process::Command runs the app itself and status() returns its exit code 5", rust_process_status),
        case("process-output", "Command with piped stdin and stdout runs a child that copies its input back", rust_process_output),
        case("cwd", "std::env::current_dir and set_current_dir report and change the working directory", rust_cwd),
        case("threads", "std::thread::spawn and join return four threads' sums, and a Builder-named thread knows its name", rust_threads),
        case("sync", "Arc<Mutex> counts to 40000 across four threads, and an mpsc channel carries 1..=100", rust_sync),
        case("net-tcp", "std::net::TcpListener on 127.0.0.1 serves a TcpStream client in another thread", rust_net_tcp),
        case("net-udp", "std::net::UdpSocket sends a datagram on 127.0.0.1 that arrives with the sender's address", rust_net_udp),
        case("net-unix", "std::os::unix::net::UnixStream::pair carries a message from another thread", rust_net_unix),
        case("float", "f64 and f32 maximum, minimum and round_ties_even, libc's sqrtf128, fmaximumf128, roundevenf128, fmodf128 and fmaf128, and f16's methods give IEEE 754-2019 results", rust_float),
        case("nostd-hello", "The no_std libbreenix app prints \"hello from rust-nostd-app\" and exits 0", nostd_hello),
        case("nostd-syscalls", "The no_std app's getpid, file round trip and monotonic clock work through libbreenix", nostd_syscalls),
        case("nostd-exit", "The no_std app's main returns 9, and libbreenix's runtime exits with status 9", nostd_exit),
    ]),
    category("install", "installing apps onto a Breenix disk", &[
        case("tool", "The SDK ships bin/breenix-image, and breenix-image --version names the commit", host_case),
        case("installed", "breenix-image install puts the C, Rust and no_std apps onto the suite's disk, every install exiting 0", host_case),
        case("path", "/usr/local/bin/sdk-c-app, the documented install path, is a regular file on the booted disk", install_path),
        case("contents", "The installed program is byte-identical to the one the SDK built", install_contents),
        case("mode-default", "An app installed without --mode has mode 0755", install_mode_default),
        case("owner", "An installed app is owned by root (uid 0, gid 0)", install_owner),
        case("mode-explicit", "An app installed with --mode 0750 has mode 0750", install_mode_explicit),
        case("bindir", "/usr/local/bin is a directory of mode 0755 owned by root", install_bindir),
        case("runs-absolute", "/usr/local/bin/sdk-c-app hello runs by its absolute path", install_runs_absolute),
        case("runs-path", "bsh finds sdk-c-app by its bare name through the disk shell configuration and runs it", install_runs_path),
        case("runs-shell", "sh -c 'sdk-c-app hello' finds the app through PATH and runs it", install_runs_shell),
        case("rust-app", "The installed Rust std app runs from PATH", install_rust_app),
        case("nostd-app", "The installed no_std app runs from PATH", install_nostd_app),
        case("replaced", "Installing the C app over the no_std app's name replaces it: the name runs the C app", install_replaced),
        case("removed", "breenix-image remove takes an installed app off the disk: it exited 0 and the name is gone", install_removed),
        case("list", "breenix-image list prints exactly the five installed apps' names, sorted", host_case),
        case("fsck", "e2fsck -fn finds no errors on the disk after the installs, the removal and the quick start", host_case),
        case("arch-refused", "breenix-image refuses a program built for the other architecture and leaves the disk without it", host_case),
        case("remove-missing", "breenix-image remove of a name that was never installed exits non-zero", host_case),
        case("several-apps", "The booted disk carries all five installed apps and each runs from PATH", install_several),
    ]),
    category("templates", "project templates & docs", &[
        case("tool", "The SDK ships bin/breenix-new, and breenix-new --list names the c, rust and rust-no-std templates", host_case),
        case("c-create", "breenix-new c tmpl-c creates a project directory and exits 0", host_case),
        case("c-build", "The c template's project builds unchanged with breenix-build --arch, which prints the program's path", host_case),
        case("c-runs", "The c template's program prints \"hello from tmpl-c\" and exits 0", template_c_runs),
        case("rust-create", "breenix-new rust tmpl-rust creates a project directory and exits 0", host_case),
        case("rust-build", "The rust template's project builds unchanged with breenix-build --arch", host_case),
        case("rust-runs", "The rust template's program prints \"hello from tmpl-rust\" and exits 0", template_rust_runs),
        case("nostd-create", "breenix-new rust-no-std tmpl-rust-no-std creates a project directory and exits 0", host_case),
        case("nostd-build", "The rust-no-std template's project builds unchanged with breenix-build --arch", host_case),
        case("nostd-runs", "The rust-no-std template's program prints \"hello from tmpl-rust-no-std\" and exits 0", template_nostd_runs),
        case("every-template", "Every template breenix-new --list names builds unchanged, and each program prints hello from its project", template_every),
        case("outside-tree", "No generated project's files or build output name a path into the checkout", host_case),
        case("no-clobber", "breenix-new refuses a directory that already holds a file, exiting non-zero and changing nothing", host_case),
        case("quick-start-doc", "docs/sdk/quick-start.md has sh quick-start blocks, one breenix quick-start-run line and one text quick-start-output block", host_case),
        case("quick-start-host", "The quick start's sh blocks, run literally in one shell in an empty directory, exit 0", host_case),
        case("quick-start-runs", "The quick start's program, run in Breenix as the doc shows, prints what the doc says it prints", quick_start_runs),
        case("sdk-docs", "The SDK ships the quick start as share/doc/breenix-sdk/quick-start.md, identical to docs/sdk/quick-start.md", host_case),
    ]),
]);

fn now_ms() -> u64 {
    time::now_monotonic().map(|t| (t.as_nanos() / 1_000_000) as u64).unwrap_or(0)
}

fn this_case() -> Result<&'static str, CaseError> {
    Ok(running_case().ok_or("the case could not tell which case it is")?)
}

// ---------------------------------------------------------------- host records

/// The record the host harness left for `id` (category/case): Ok for PASS, its reason
/// for FAIL, and a failure saying so when the disk carries no record for this machine.
fn host(id: &str) -> CaseResult {
    let run = match std::fs::read_to_string(format!("{STAGE}/run")) {
        Ok(text) => text,
        Err(_) => return fail(format!("{STAGE}/run is not on the disk: scripts/sdk-suite.py did not stage this disk")),
    };
    if !run.lines().any(|line| line == format!("arch {ARCH}")) {
        return fail(format!("the disk was staged for another architecture, not {ARCH}"));
    }
    let path = format!("{STAGE}/host/{id}");
    let text = std::fs::read_to_string(&path).map_err(|_| format!("the host harness left no record {path}"))?;
    let line = text.lines().next().unwrap_or("");
    if line == "PASS" {
        return Ok(());
    }
    match line.strip_prefix("FAIL ") {
        Some(why) => fail(format!("host: {why}")),
        None => fail(format!("{path} reads {line:?}, not PASS or FAIL")),
    }
}

/// A case that measures a host step: its own record.
fn host_case() -> CaseResult {
    host(this_case()?)
}

/// The path of a staged program, or a failure giving the host step that should have built it.
fn staged(name: &str, built_by: &str) -> Result<String, CaseError> {
    let path = format!("{STAGE}/apps/{name}");
    if std::fs::metadata(&path).is_ok() {
        return Ok(path);
    }
    host(built_by)?;
    Err(format!("{path} is not on the disk although the host recorded {built_by} as passing").into())
}

/// The path of an installed program, or a failure giving the install step's record.
fn installed(name: &str) -> Result<String, CaseError> {
    let path = format!("{BIN}/{name}");
    if std::fs::metadata(&path).is_ok() {
        return Ok(path);
    }
    host("install/installed")?;
    Err(format!("{path} is not on the disk although the host recorded every install as passing").into())
}

// ---------------------------------------------------------------- running programs

/// A directory of the case's own under /tmp, removed with everything in it at the end.
struct Scratch {
    dir: String,
}

impl Scratch {
    fn new() -> Result<Scratch, CaseError> {
        let pid = process::getpid()?.raw();
        let dir = format!("/tmp/sdk-{pid}");
        remove_tree(std::path::Path::new(&dir));
        std::fs::create_dir(&dir).map_err(|e| format!("could not create the scratch directory {dir}: {e}"))?;
        Ok(Scratch { dir })
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        remove_tree(std::path::Path::new(&self.dir));
    }
}

/// Remove `path` and, for a directory, everything in it, using only read_dir, unlink
/// and rmdir.
fn remove_tree(path: &std::path::Path) {
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if meta.is_dir() {
            if let Ok(entries) = std::fs::read_dir(path) {
                for entry in entries.flatten() {
                    remove_tree(&entry.path());
                }
            }
            let _ = std::fs::remove_dir(path);
        } else {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// What a program the suite started left behind.
struct Ran {
    status: i32,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl Ran {
    fn ended(&self) -> String {
        if process::wifexited(self.status) {
            format!("exit status {}", process::wexitstatus(self.status))
        } else if process::wifsignaled(self.status) {
            format!("signal {}", process::wtermsig(self.status))
        } else {
            format!("wait status {:#x}", self.status)
        }
    }

    fn out(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    fn err_text(&self) -> String {
        let text = String::from_utf8_lossy(&self.stderr);
        let text = text.trim();
        if text.is_empty() { String::from("nothing on stderr") } else { format!("stderr {:?}", text) }
    }
}

fn cstrings(items: &[String]) -> (Vec<CString>, Vec<*const u8>) {
    let owned: Vec<CString> = items.iter().map(|s| CString::new(s.as_str()).unwrap_or_default()).collect();
    let mut ptrs: Vec<*const u8> = owned.iter().map(|c| c.as_ptr() as *const u8).collect();
    ptrs.push(core::ptr::null());
    (owned, ptrs)
}

/// Close-on-exec pipe ends.
fn pipe() -> Result<(Fd, Fd), CaseError> {
    io::pipe2(status_flags::O_CLOEXEC).map_err(|e| format!("pipe failed: {e}").into())
}

fn nonblocking(fd: Fd) {
    if let Ok(flags) = io::fcntl(fd, fcntl_cmd::F_GETFL, 0) {
        let _ = io::fcntl(fd, fcntl_cmd::F_SETFL, flags | status_flags::O_NONBLOCK as i64);
    }
}

/// Run `path` with `argv` (argv[0] included) in `dir`, collecting its stdout and stderr
/// until it exits, or killing it 1.5 seconds before the case's limit.
fn run(path: &str, argv: &[&str], dir: &str) -> Result<Ran, CaseError> {
    let (out_r, out_w) = pipe()?;
    let (err_r, err_w) = pipe()?;
    let (exec_r, exec_w) = pipe()?;
    let argv: Vec<String> = argv.iter().map(|a| a.to_string()).collect();
    let env: Vec<String> = ENV.iter().map(|e| e.to_string()).collect();
    let (_argv_owned, argv_ptrs) = cstrings(&argv);
    let (_env_owned, env_ptrs) = cstrings(&env);
    let cpath = CString::new(path).map_err(|_| "program path holds a NUL")?;
    let cdir = CString::new(dir).map_err(|_| "directory path holds a NUL")?;

    let child = match process::fork() {
        Ok(process::ForkResult::Child) => {
            let why = (|| -> Result<(), String> {
                process::chdir(cdir.as_bytes_with_nul()).map_err(|e| format!("chdir {dir} failed: {e}"))?;
                let null = libbreenix::fs::open("/dev/null", libbreenix::fs::O_RDONLY)
                    .map_err(|e| format!("open /dev/null failed: {e}"))?;
                io::dup2(null, Fd::STDIN).map_err(|e| format!("dup2 failed: {e}"))?;
                io::dup2(out_w, Fd::STDOUT).map_err(|e| format!("dup2 failed: {e}"))?;
                io::dup2(err_w, Fd::STDERR).map_err(|e| format!("dup2 failed: {e}"))?;
                match process::execve(cpath.as_bytes_with_nul(), argv_ptrs.as_ptr(), env_ptrs.as_ptr()) {
                    Ok(never) => match never {},
                    Err(e) => Err(format!("execve {path} failed: {e}")),
                }
            })();
            if let Err(why) = why {
                let _ = io::write(exec_w, why.as_bytes());
            }
            process::exit(127);
        }
        Ok(process::ForkResult::Parent(pid)) => pid.raw() as i32,
        Err(e) => {
            for fd in [out_r, out_w, err_r, err_w, exec_r, exec_w] {
                let _ = io::close(fd);
            }
            return Err(format!("fork failed: {e}").into());
        }
    };
    for fd in [out_w, err_w, exec_w] {
        let _ = io::close(fd);
    }
    for fd in [out_r, err_r, exec_r] {
        nonblocking(fd);
    }

    let allowed = case_ms_left().saturating_sub(RESERVE_MS);
    let deadline = now_ms() + allowed;
    let mut readers = [(out_r, Vec::new()), (err_r, Vec::new()), (exec_r, Vec::new())];
    let mut open = [true; 3];
    let mut status = None;
    let mut exited_at = 0;
    loop {
        let mut fds: Vec<PollFd> = Vec::new();
        for (i, (fd, _)) in readers.iter().enumerate() {
            if open[i] {
                fds.push(PollFd::new(*fd, poll_events::POLLIN));
            }
        }
        if !fds.is_empty() {
            let _ = io::poll(&mut fds, 10);
        }
        for (i, (fd, buf)) in readers.iter_mut().enumerate() {
            if !open[i] {
                continue;
            }
            let mut chunk = [0u8; 4096];
            loop {
                match io::read(*fd, &mut chunk) {
                    Ok(0) => {
                        open[i] = false;
                        let _ = io::close(*fd);
                        break;
                    }
                    Ok(n) => {
                        if buf.len() < 1 << 20 {
                            buf.extend_from_slice(&chunk[..n]);
                        }
                    }
                    Err(_) => break,
                }
            }
        }
        if status.is_none() {
            let mut s = 0;
            if let Ok(pid) = process::waitpid(child, &mut s, process::WNOHANG) {
                if pid.raw() as i32 == child {
                    status = Some(s);
                    exited_at = now_ms();
                }
            }
        }
        if let Some(s) = status {
            // Done once every pipe is closed; a descendant still holding one gets 200 ms.
            if !open.iter().any(|o| *o) || now_ms() > exited_at + 200 {
                for (i, (fd, _)) in readers.iter().enumerate() {
                    if open[i] {
                        let _ = io::close(*fd);
                    }
                }
                let [(_, stdout), (_, stderr), (_, exec_err)] = readers;
                if !exec_err.is_empty() {
                    return Err(String::from_utf8_lossy(&exec_err).into_owned().into());
                }
                return Ok(Ran { status: s, stdout, stderr });
            }
        } else if now_ms() > deadline {
            let _ = signal::kill(child, signal::SIGKILL);
            let mut s = 0;
            let _ = process::waitpid(child, &mut s, 0);
            for (i, (fd, _)) in readers.iter().enumerate() {
                if open[i] {
                    let _ = io::close(*fd);
                }
            }
            return Err(format!("{path} did not finish within {allowed} ms and was killed").into());
        }
    }
}

/// Pass when the program exited with `status` and printed exactly `stdout`.
fn expect(what: &str, ran: &Ran, stdout: &str, status: i32) -> CaseResult {
    let exited = process::wifexited(ran.status) && process::wexitstatus(ran.status) == status;
    if !exited {
        return fail(format!("{what} ended with {}, expected exit status {status}; it printed {:?} ({})",
                            ran.ended(), ran.out(), ran.err_text()));
    }
    if ran.out() != stdout {
        return fail(format!("{what} printed {:?}, expected {stdout:?} ({})", ran.out(), ran.err_text()));
    }
    Ok(())
}

/// Run the staged program `name` with `args` in a scratch directory and expect `stdout`
/// and exit status 0.
fn staged_prints(name: &str, built_by: &str, args: &[&str], stdout: &str) -> CaseResult {
    staged_ends(name, built_by, args, stdout, 0)
}

fn staged_ends(name: &str, built_by: &str, args: &[&str], stdout: &str, status: i32) -> CaseResult {
    let path = staged(name, built_by)?;
    let scratch = Scratch::new()?;
    let mut argv = vec![path.as_str()];
    argv.extend_from_slice(args);
    let ran = run(&path, &argv, &scratch.dir)?;
    expect(&format!("{name} {}", args.join(" ")), &ran, stdout, status)
}

/// Check that the ELF file at `path` is an executable for this machine with no
/// interpreter: a static program.
fn static_elf(path: &str) -> CaseResult {
    let data = std::fs::read(path).map_err(|e| format!("could not read {path}: {e}"))?;
    if data.len() < 64 || &data[..4] != b"\x7fELF" {
        return fail(format!("{path} is not an ELF file"));
    }
    check(data[4] == 2 && data[5] == 1, &format!("{path} is not a 64-bit little-endian ELF file"))?;
    let u16_at = |at: usize| u16::from_le_bytes([data[at], data[at + 1]]);
    let e_type = u16_at(16);
    let machine = u16_at(18);
    check(machine == MACHINE, &format!("{path} is for ELF machine {machine}, not {ARCH} ({MACHINE})"))?;
    check(e_type == 2 || e_type == 3, &format!("{path} has ELF type {e_type}, not an executable"))?;
    let phoff = u64::from_le_bytes(data[32..40].try_into().unwrap_or([0; 8])) as usize;
    let phentsize = u16_at(54) as usize;
    let phnum = u16_at(56) as usize;
    for i in 0..phnum {
        let at = phoff + i * phentsize;
        if at + 4 > data.len() {
            return fail(format!("{path}'s program headers run past the end of the file"));
        }
        let p_type = u32::from_le_bytes(data[at..at + 4].try_into().unwrap_or([0; 4]));
        if p_type == 3 {
            return fail(format!("{path} names a program interpreter (PT_INTERP): it is not static"));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- c-app

fn c(args: &[&str], stdout: &str) -> CaseResult {
    staged_prints("c-app", "c-app/built", args, stdout)
}

fn c_elf() -> CaseResult { static_elf(&staged("c-app", "c-app/built")?) }

fn c_hello() -> CaseResult { c(&["hello"], "hello from c-app\n") }

fn c_args() -> CaseResult {
    c(&["args", "one", "two words", ""],
      "argc=4\n[one]\n[two words]\n[]\nSDK_SUITE_VAR=sdk value\nSDK_SUITE_UNSET=(null)\n")
}

fn c_exit() -> CaseResult { staged_ends("c-app", "c-app/built", &["exit", "42"], "", 42) }

fn c_stdio_format() -> CaseResult {
    c(&["stdio-format"],
      "-42|   42|42   |00042|ff|FF|10|str|abc|z|%\n3.14|1.234568e+04|0.0001|1e+20\nsnprintf=8 buf=abcd\n1234567890123|4|-7\n")
}

fn c_stdio_stream() -> CaseResult { c(&["stdio-stream"], "read: line one\nread: line two\nsize=18\n") }

fn c_stderr() -> CaseResult {
    let path = staged("c-app", "c-app/built")?;
    let scratch = Scratch::new()?;
    let ran = run(&path, &[&path, "stderr"], &scratch.dir)?;
    expect("c-app stderr", &ran, "to stdout\n", 0)?;
    let err = String::from_utf8_lossy(&ran.stderr).into_owned();
    check(err == "to stderr\n", &format!("c-app stderr wrote {err:?} to standard error, expected \"to stderr\\n\""))
}

fn c_files() -> CaseResult { c(&["files"], "read=456 size=10 mode=640\nrenamed\nunlinked\n") }

fn c_dirs() -> CaseResult { c(&["dirs"], "entries=a,b,c\nrmdir ok\n") }

fn c_spawn() -> CaseResult { c(&["spawn"], "spawn actions, attributes, exec and status 127 passed\n") }
fn c_fork_wait() -> CaseResult { c(&["fork-wait"], "child exited 7\n") }

fn c_exec() -> CaseResult { c(&["exec"], "exec child exited 3\n") }

fn c_pipe() -> CaseResult { c(&["pipe"], "pipe: through the pipe\n") }

fn c_signal_handler() -> CaseResult { c(&["signal-handler"], "count after raise=1\ncount after kill=2\n") }

fn c_signal_mask() -> CaseResult { c(&["signal-mask"], "pending=1 delivered=0\ndelivered=1\n") }

fn c_threads() -> CaseResult { c(&["threads"], "threads=4 total=7998000\n") }

fn c_mutex() -> CaseResult { c(&["mutex"], "counter=40000\n") }

fn c_condvar() -> CaseResult { c(&["condvar"], "received=100 sum=5050\n") }

fn c_socket_unix() -> CaseResult { c(&["socket-unix"], "unix: pong\nchild status 0\n") }

fn c_socket_tcp() -> CaseResult { c(&["socket-tcp"], "tcp server got: hello tcp\ntcp client status 0\n") }

fn c_socket_udp() -> CaseResult { c(&["socket-udp"], "udp: datagram from-port-matches=1\n") }

fn c_math() -> CaseResult {
    c(&["math"], "sqrt=1.414214 sin=0.841471 cos=1.000000 exp=2.718282 log=2.302585\n\
                  pow=1024.000000 fmod=1.500000 floor=-2.000000 ceil=-1.000000\n\
                  pi=3.141593 hypot=5.000000 cbrt=3.000000 lround=3\n")
}

// ---------------------------------------------------------------- rust-app

fn rust(args: &[&str], stdout: &str) -> CaseResult {
    staged_prints("rust-app", "rust-app/built", args, stdout)
}

fn nostd(args: &[&str], stdout: &str, status: i32) -> CaseResult {
    staged_ends("rust-nostd-app", "rust-app/nostd-built", args, stdout, status)
}

fn rust_elf() -> CaseResult { static_elf(&staged("rust-app", "rust-app/built")?) }

fn rust_hello() -> CaseResult { rust(&["hello"], "hello from rust-app\n") }

fn rust_args() -> CaseResult { rust(&["args", "one", "two words", ""], "argc=5\n[one]\n[two words]\n[]\n") }

fn rust_env() -> CaseResult { rust(&["env"], "SDK_SUITE_VAR=sdk value\nSDK_SUITE_UNSET=absent\nvars=3\n") }

fn rust_exit() -> CaseResult { staged_ends("rust-app", "rust-app/built", &["exit", "17"], "", 17) }

fn rust_fs_rw() -> CaseResult { rust(&["fs-rw"], "contents=rust fs|more|\nlen=13\n") }

fn rust_fs_dir() -> CaseResult { rust(&["fs-dir"], "a: b,x,y\nrenamed=true\nexists=false\n") }

fn rust_fs_metadata() -> CaseResult { rust(&["fs-metadata"], "len=5 file=true dir=false mode=640\ndot is_dir=true\n") }

fn rust_process_status() -> CaseResult { rust(&["process-status"], "status=Some(5)\n") }

fn rust_process_output() -> CaseResult {
    rust(&["process-output"], "child said: piped through a child\nchild status=Some(0)\n")
}

fn rust_cwd() -> CaseResult {
    let path = staged("rust-app", "rust-app/built")?;
    let scratch = Scratch::new()?;
    let ran = run(&path, &[&path, "cwd"], &scratch.dir)?;
    expect("rust-app cwd", &ran, &format!("cwd={0}\ncwd after={0}/sub\n", scratch.dir), 0)
}

fn rust_threads() -> CaseResult { rust(&["threads"], "threads=4 total=7998000\nname=Some(\"worker\")\n") }

fn rust_sync() -> CaseResult { rust(&["sync"], "counter=40000\nchannel sum=5050\n") }

fn rust_net_tcp() -> CaseResult { rust(&["net-tcp"], "tcp server got: hello tcp\ntcp client got: ack:hello tcp\n") }

fn rust_net_udp() -> CaseResult { rust(&["net-udp"], "udp: datagram from-matches=true\n") }

fn rust_net_unix() -> CaseResult { rust(&["net-unix"], "unix: unix ping\n") }

fn rust_float() -> CaseResult {
    rust(&["float"], "maximum=NaN 0 1.25\nminimum=NaN -0 -3.5\nround_ties_even=2 4 -2 0 6\nf128=1.5 0 2 1.5 7\nf16=1.5 0 2 1.5 -3\n")
}

fn nostd_hello() -> CaseResult { nostd(&["hello"], "hello from rust-nostd-app\n", 0) }

fn nostd_syscalls() -> CaseResult { nostd(&["syscalls"], "pid ok\nfile ok\nclock ok\n", 0) }

fn nostd_exit() -> CaseResult { nostd(&["exit", "9"], "", 9) }

// ---------------------------------------------------------------- install

/// The mode bits, owner and group of `path`.
fn mode_of(path: &str) -> Result<(u32, u32, u32, bool, bool), CaseError> {
    let meta = std::fs::metadata(path).map_err(|e| format!("stat {path}: {e}"))?;
    Ok((meta.mode() & 0o7777, meta.uid(), meta.gid(), meta.is_file(), meta.is_dir()))
}

fn install_path() -> CaseResult {
    let path = installed("sdk-c-app")?;
    let (_, _, _, file, _) = mode_of(&path)?;
    check(file, &format!("{path} is not a regular file"))
}

fn install_contents() -> CaseResult {
    let path = installed("sdk-c-app")?;
    let built = staged("c-app", "c-app/built")?;
    let a = std::fs::read(&path).map_err(|e| format!("read {path}: {e}"))?;
    let b = std::fs::read(&built).map_err(|e| format!("read {built}: {e}"))?;
    check(a == b, &format!("{path} ({} bytes) differs from the built program {built} ({} bytes)", a.len(), b.len()))
}

fn install_mode_default() -> CaseResult {
    let path = installed("sdk-c-app")?;
    let (mode, ..) = mode_of(&path)?;
    check(mode == 0o755, &format!("{path} has mode {mode:04o}, expected 0755"))
}

fn install_owner() -> CaseResult {
    let path = installed("sdk-c-app")?;
    let (_, uid, gid, ..) = mode_of(&path)?;
    check(uid == 0 && gid == 0, &format!("{path} is owned by uid {uid} gid {gid}, expected root (0, 0)"))
}

fn install_mode_explicit() -> CaseResult {
    let path = installed("sdk-private")?;
    let (mode, ..) = mode_of(&path)?;
    check(mode == 0o750, &format!("{path} has mode {mode:04o}, expected 0750"))
}

fn install_bindir() -> CaseResult {
    if std::fs::metadata(BIN).is_err() {
        host("install/installed")?;
        return fail(format!("{BIN} is not on the disk although the host recorded every install as passing"));
    }
    let (mode, uid, gid, _, dir) = mode_of(BIN)?;
    check(dir && mode == 0o755 && uid == 0 && gid == 0,
          &format!("{BIN} is {} with mode {mode:04o} owned by {uid}:{gid}, expected a directory 0755 owned by 0:0",
                  if dir { "a directory" } else { "not a directory" }))
}

/// Run through the disk's shell configuration, exactly as an interactive bsh
/// session does. The suite's private ENV must not supply the PATH being tested.
fn configured_shell(words: &[&str], dir: &str) -> Result<Ran, CaseError> {
    let config = std::fs::read_to_string("/etc/bshrc")
        .map_err(|e| format!("could not read the shell configuration: {e}"))?;
    let arguments = words.iter().map(|word| format!("{word:?}")).collect::<Vec<_>>().join(", ");
    // Capture the command independently from startup output (bshrc prints a
    // greeting). Missing output files are errors, never a fallback to shell output.
    let stdout = format!("{dir}/command-stdout");
    let stderr = format!("{dir}/command-stderr");
    let script = format!("{config}\nlet result = exec({arguments});\n\
        writeFile({stdout:?}, result.stdout);\n\
        writeFile({stderr:?}, result.stderr);\nexit(result.exitCode);");
    let mut ran = run("/bin/bsh", &["bsh", "-e", &script], dir)?;
    ran.stdout = std::fs::read(&stdout).map_err(|e| format!("could not read command stdout: {e}"))?;
    let mut command_stderr = std::fs::read(&stderr)
        .map_err(|e| format!("could not read command stderr: {e}"))?;
    ran.stderr.append(&mut command_stderr);
    Ok(ran)
}

/// Run `name args` by its bare name through the configured shell's PATH search.
fn from_path(name: &str, args: &[&str], stdout: &str) -> CaseResult {
    installed(name)?;
    let scratch = Scratch::new()?;
    let mut argv = vec![name];
    argv.extend_from_slice(args);
    let ran = configured_shell(&argv, &scratch.dir)?;
    expect(&format!("bsh {name} {}", args.join(" ")), &ran, stdout, 0)
}

fn install_runs_absolute() -> CaseResult {
    let path = installed("sdk-c-app")?;
    let scratch = Scratch::new()?;
    let ran = run(&path, &[&path, "hello"], &scratch.dir)?;
    expect(&format!("{path} hello"), &ran, "hello from c-app\n", 0)
}

fn install_runs_path() -> CaseResult { from_path("sdk-c-app", &["hello"], "hello from c-app\n") }

fn install_runs_shell() -> CaseResult {
    installed("sdk-c-app")?;
    let scratch = Scratch::new()?;
    let ran = run("/bin/sh", &["sh", "-c", "sdk-c-app hello"], &scratch.dir)?;
    expect("sh -c 'sdk-c-app hello'", &ran, "hello from c-app\n", 0)
}

fn install_rust_app() -> CaseResult { from_path("sdk-rust-app", &["hello"], "hello from rust-app\n") }

fn install_nostd_app() -> CaseResult { from_path("sdk-nostd-app", &["hello"], "hello from rust-nostd-app\n") }

fn install_replaced() -> CaseResult { from_path("sdk-replaced", &["hello"], "hello from c-app\n") }

fn install_removed() -> CaseResult {
    host("install/installed")?;
    host("install/removed")?;
    let path = format!("{BIN}/sdk-removed");
    check(std::fs::symlink_metadata(&path).is_err(), &format!("{path} is still on the disk after breenix-image remove"))
}

fn install_several() -> CaseResult {
    for (name, said) in [("sdk-c-app", "c-app"), ("sdk-rust-app", "rust-app"), ("sdk-nostd-app", "rust-nostd-app"),
                         ("sdk-private", "c-app"), ("sdk-replaced", "c-app")] {
        from_path(name, &["hello"], &format!("hello from {said}\n"))?;
    }
    Ok(())
}

// ---------------------------------------------------------------- templates

fn template_runs(project: &str, built_by: &str) -> CaseResult {
    staged_prints(&format!("templates/{project}"), built_by, &[], &format!("hello from {project}\n"))
}

fn template_c_runs() -> CaseResult { template_runs("tmpl-c", "templates/c-build") }

fn template_rust_runs() -> CaseResult { template_runs("tmpl-rust", "templates/rust-build") }

fn template_nostd_runs() -> CaseResult { template_runs("tmpl-rust-no-std", "templates/nostd-build") }

fn template_every() -> CaseResult {
    host("templates/every-template")?;
    let list = std::fs::read_to_string(format!("{STAGE}/templates"))
        .map_err(|e| format!("could not read the template list {STAGE}/templates: {e}"))?;
    let projects: Vec<&str> = list.split_whitespace().collect();
    check(!projects.is_empty(), "the template list is empty")?;
    for project in projects {
        template_runs(project, "templates/every-template")?;
    }
    Ok(())
}

fn quick_start_runs() -> CaseResult {
    host("templates/quick-start-host")?;
    let line = std::fs::read_to_string(format!("{STAGE}/quick-start/run"))
        .map_err(|e| format!("could not read the quick start's run line: {e}"))?;
    let expected = std::fs::read_to_string(format!("{STAGE}/quick-start/expected"))
        .map_err(|e| format!("could not read the quick start's expected output: {e}"))?;
    let words: Vec<&str> = line.split_whitespace().collect();
    check(!words.is_empty(), "the quick start's run line is empty")?;
    let scratch = Scratch::new()?;
    let ran = configured_shell(&words, &scratch.dir)?;
    expect(line.trim(), &ran, &expected, 0)
}

fn main() { SUITE.run() }
