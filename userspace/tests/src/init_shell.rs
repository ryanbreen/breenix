//! Interactive Shell for Breenix OS (std version)
//!
//! This is meant to run as PID 1 (init). It provides a REPL that:
//! 1. Prints a welcome banner
//! 2. Shows a prompt "breenix> "
//! 3. Reads a line of input (blocking read from stdin)
//! 4. Parses and executes simple commands
//! 5. Loops forever
//!
//! Features:
//! - Job table with background/stopped job management
//! - SIGCHLD handler for async child reaping
//! - SIGINT handler for Ctrl+C
//! - Command parsing (pipes, background &, builtins)
//! - Built-in commands: cd, exit, jobs, fg, bg, pwd, export, env, raw, cooked,
//!   help, clear, echo, kill, time, uptime, test, devtest, progs, ps
//! - Pipeline execution (multi-stage pipes)
//! - Environment variable support
//! - TTY/termios control (raw mode, cooked mode, tcsetpgrp)
//! - Line reading with prompt

#![allow(clippy::needless_return)]

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, Ordering};

use libbreenix::io::{close, dup2, pipe, read, write};
use libbreenix::process::{
    fork, getpid, getpgid, setpgid, waitpid, yield_now,
    wifexited, wexitstatus, wifsignaled, wifstopped,
    ForkResult, WNOHANG, WUNTRACED,
};
use libbreenix::signal::{sigaction, Sigaction, SIGINT, SIGCHLD, SIGCONT};
use libbreenix::termios::{
    self, Termios, tcgetattr, tcsetattr, tcsetpgrp, TCSANOW,
    lflag, iflag, oflag,
};
use libbreenix::types::Fd;

// ============================================================================
// Constants
// ============================================================================

const STDIN: Fd = Fd::from_raw(0);
const STDOUT: Fd = Fd::from_raw(1);

// Access mode
const X_OK: i32 = 1;

// File open flags
const O_RDONLY: i32 = 0;
const O_WRONLY: i32 = 1;
const O_DIRECTORY: i32 = 0o200000;

// Additional termios flags not in libbreenix convenience module
const IGNBRK: u32 = 0x0001;
const BRKINT: u32 = 0x0002;
const PARMRK: u32 = 0x0008;
const ISTRIP: u32 = 0x0020;
const INLCR: u32 = 0x0040;
const IGNCR: u32 = 0x0080;

// c_cflag bits
const CSIZE: u32 = 0x0030;
const CS8: u32 = 0x0030;
const PARENB: u32 = 0x0100;

// Syscall numbers for calls not yet in libbreenix
const SYS_OPEN: u64 = 2;

/// Raw execve wrapper using libbreenix syscall primitives.
/// This replaces the `extern "C" { fn execve(...) }` FFI import.
fn sys_execve(path: *const u8, argv: *const *const u8, envp: *const *const u8) -> i32 {
    unsafe {
        libbreenix::raw::syscall3(
            libbreenix::syscall::nr::EXEC,
            path as u64,
            argv as u64,
            envp as u64,
        ) as i32
    }
}

// ============================================================================
// Raw syscall helper for open (fs::open takes &str, we sometimes need *const u8)
// ============================================================================

fn sys_open(path: *const u8, flags: i32) -> i64 {
    unsafe { libbreenix::raw::syscall3(SYS_OPEN, path as u64, flags as u64, 0) as i64 }
}

fn sys_access(pathname: *const u8, mode: i32) -> i32 {
    unsafe {
        libbreenix::raw::syscall2(
            libbreenix::syscall::nr::ACCESS,
            pathname as u64,
            mode as u64,
        ) as i32
    }
}

fn getpgrp() -> i32 {
    match getpgid(0) {
        Ok(pid) => pid.raw() as i32,
        Err(_) => -1,
    }
}

fn shell_cfmakeraw(t: &mut Termios) {
    t.c_iflag &= !(IGNBRK | BRKINT | PARMRK | ISTRIP | INLCR | IGNCR | iflag::ICRNL | iflag::IXON);
    t.c_oflag &= !oflag::OPOST;
    t.c_lflag &= !(lflag::ECHO | lflag::ICANON | lflag::ISIG | lflag::IEXTEN);
    t.c_cflag &= !(CSIZE | PARENB);
    t.c_cflag |= CS8;
    t.c_cc[termios::cc::VMIN] = 1;
    t.c_cc[termios::cc::VTIME] = 0;
}

// ============================================================================
// Job Tracking - Background and stopped job management
// ============================================================================

/// Status of a job in the job table
#[derive(Clone, PartialEq)]
#[repr(u8)]
enum JobStatus {
    Running = 0,
    Stopped = 1,
    Done = 2,
}

/// A job entry representing a background or stopped process
#[derive(Clone)]
struct Job {
    /// Job ID (1-based, shown to user as [1], [2], etc.)
    id: u32,
    /// Process ID of the job
    pid: i32,
    /// Process group ID of the job
    pgid: i32,
    /// Current status of the job
    status: JobStatus,
    /// Command string (heap-allocated String instead of fixed buffer)
    command: String,
}

/// Maximum number of concurrent jobs
const MAX_JOBS: usize = 16;

/// Job table tracking all background and stopped jobs
struct JobTable {
    /// Array of job slots (None = empty slot)
    jobs: [Option<Job>; MAX_JOBS],
    /// Next job ID to assign
    next_id: u32,
    /// ID of the current (most recent) job
    current: u32,
}

// Helper to create the array of None values
fn empty_jobs() -> [Option<Job>; MAX_JOBS] {
    // Cannot use array init with non-Copy types, so build it manually
    [
        None, None, None, None, None, None, None, None, None, None, None, None, None, None, None,
        None,
    ]
}

impl JobTable {
    fn new() -> Self {
        JobTable {
            jobs: empty_jobs(),
            next_id: 1,
            current: 0,
        }
    }

    /// Add a new job to the table. Returns the job ID, or 0 if the table is full.
    fn add(&mut self, pid: i32, pgid: i32, command: &str) -> u32 {
        for slot in self.jobs.iter_mut() {
            if slot.is_none() {
                let id = self.next_id;
                self.next_id += 1;
                *slot = Some(Job {
                    id,
                    pid,
                    pgid,
                    status: JobStatus::Running,
                    command: command.to_string(),
                });
                self.current = id;
                return id;
            }
        }
        0 // Table full
    }

    fn find_by_id_mut(&mut self, id: u32) -> Option<&mut Job> {
        self.jobs
            .iter_mut()
            .filter_map(|s| s.as_mut())
            .find(|j| j.id == id)
    }

    fn find_by_pid_mut(&mut self, pid: i32) -> Option<&mut Job> {
        self.jobs
            .iter_mut()
            .filter_map(|s| s.as_mut())
            .find(|j| j.pid == pid)
    }

    fn update_status(&mut self, pid: i32, status: JobStatus) {
        if let Some(job) = self.find_by_pid_mut(pid) {
            job.status = status;
        }
    }

    fn remove(&mut self, id: u32) {
        for slot in &mut self.jobs {
            if let Some(job) = slot {
                if job.id == id {
                    *slot = None;
                    return;
                }
            }
        }
    }

    fn iter(&self) -> impl Iterator<Item = &Job> {
        self.jobs.iter().filter_map(|slot| slot.as_ref())
    }
}

/// A wrapper type that allows an `UnsafeCell` to be shared between threads.
///
/// # Safety
/// This is safe because our shell is single-threaded. The shell main loop is
/// the only code that accesses the job table, and signal handlers only set
/// atomic flags (they don't access the job table directly).
#[repr(transparent)]
struct SyncJobTable(UnsafeCell<JobTable>);

// SAFETY: The shell is single-threaded, so no concurrent access occurs.
unsafe impl Sync for SyncJobTable {}

impl SyncJobTable {
    fn new(value: JobTable) -> Self {
        SyncJobTable(UnsafeCell::new(value))
    }

    fn get(&self) -> *mut JobTable {
        self.0.get()
    }
}

// We use std::sync::LazyLock to initialize the global job table (requires allocation).
// Since signal handlers only set atomic flags (not touch the job table), this is safe.
static JOB_TABLE: std::sync::LazyLock<SyncJobTable> =
    std::sync::LazyLock::new(|| SyncJobTable::new(JobTable::new()));

/// Get a mutable reference to the job table
///
/// # Safety
/// This is safe in single-threaded userspace code. The shell main loop
/// is the only accessor of the job table.
#[inline]
fn job_table() -> &'static mut JobTable {
    unsafe { &mut *JOB_TABLE.get() }
}

/// Add a job to the job table. Returns the job ID, or 0 if the table is full.
fn add_job(pid: i32, command: &str) -> u32 {
    job_table().add(pid, pid, command)
}

/// Update the status of a job by PID
fn update_job_status(pid: i32, status: JobStatus) {
    job_table().update_status(pid, status)
}

/// List all jobs to stdout (for "jobs" command)
fn list_jobs() {
    let table = job_table();
    for job in table.iter() {
        let status_str = match job.status {
            JobStatus::Running => "Running",
            JobStatus::Stopped => "Stopped",
            JobStatus::Done => "Done",
        };
        let current_marker = if job.id == table.current { "+" } else { "-" };
        println!(
            "[{}]{}  {}\t\t{}",
            job.id, current_marker, status_str, job.command
        );
    }
}

/// Get the current (most recent) job ID. Returns 0 if no jobs exist.
fn get_current_job_id() -> u32 {
    job_table().current
}

/// Parse a job specification string into a job ID.
///
/// Accepts formats:
/// - "%1" or "%2" etc. (job ID with % prefix)
/// - "1" or "2" etc. (bare job ID)
/// - "" (empty string, returns current job)
///
/// Returns 0 if the spec is invalid.
fn parse_job_spec(spec: &str) -> u32 {
    let spec = spec.trim();

    if spec.is_empty() {
        return get_current_job_id();
    }

    let num_str = if let Some(stripped) = spec.strip_prefix('%') {
        stripped
    } else {
        spec
    };

    num_str.parse::<u32>().unwrap_or(0)
}

/// Handle the "bg" builtin command
fn builtin_bg(arg: &str) {
    let job_id = parse_job_spec(arg);

    if job_id == 0 {
        println!("bg: no current job");
        return;
    }

    if let Some(job) = job_table().find_by_id_mut(job_id) {
        if job.status != JobStatus::Stopped {
            println!("bg: job {} is not stopped", job_id);
            return;
        }

        // Send SIGCONT to resume the job in background
        let _ = libbreenix::signal::kill(-(job.pgid), SIGCONT);
        job.status = JobStatus::Running;

        println!("[{}] {} &", job_id, job.command);
    } else if arg.is_empty() {
        println!("bg: no current job");
    } else {
        println!("bg: {}: no such job", arg);
    }
}

/// Handle the "fg" builtin command
fn builtin_fg(arg: &str) {
    let job_id = parse_job_spec(arg);

    if job_id == 0 {
        println!("fg: no current job");
        return;
    }

    // First check if the job exists and get its info
    let (pid, pgid, was_stopped, cmd) = {
        let table = job_table();
        if let Some(job) = table.find_by_id_mut(job_id) {
            let was_stopped = job.status == JobStatus::Stopped;
            let cmd = job.command.clone();
            (job.pid, job.pgid, was_stopped, cmd)
        } else {
            if arg.is_empty() {
                println!("fg: no current job");
            } else {
                println!("fg: {}: no such job", arg);
            }
            return;
        }
    };

    // Print notification
    println!("[{}] {}", job_id, cmd);

    // Give terminal to the job's process group
    let _ = tcsetpgrp(STDIN, pgid);

    // If job was stopped, send SIGCONT to resume it
    if was_stopped {
        let _ = libbreenix::signal::kill(-pgid, SIGCONT);
        if let Some(job) = job_table().find_by_id_mut(job_id) {
            job.status = JobStatus::Running;
        }
    }

    // Wait for the job (with WUNTRACED to catch if it stops again)
    let mut status: i32 = 0;
    let wait_result = waitpid(pid, &mut status as *mut i32, WUNTRACED);

    // Take terminal back
    let shell_pgrp = getpgrp();
    let _ = tcsetpgrp(STDIN, shell_pgrp);

    // Update job status based on result
    match wait_result {
        Ok(reaped) if reaped.raw() > 0 => {
            if wifstopped(status) {
                // Job was stopped again (e.g., Ctrl+Z)
                if let Some(job) = job_table().find_by_id_mut(job_id) {
                    job.status = JobStatus::Stopped;
                }
                println!();
                println!("[{}]+  Stopped\t\t{}", job_id, cmd);
            } else {
                // Job completed - remove from table
                job_table().remove(job_id);
            }
        }
        _ => {
            // Wait failed - remove job from table anyway
            job_table().remove(job_id);
        }
    }
}

// ============================================================================
// Program Registry - Lookup table for external commands
// ============================================================================

struct ProgramEntry {
    name: &'static str,
    binary_name: &'static [u8],
    description: &'static str,
}

static PROGRAM_REGISTRY: &[ProgramEntry] = &[
    ProgramEntry {
        name: "hello",
        binary_name: b"hello_world\0",
        description: "Print hello world message",
    },
    ProgramEntry {
        name: "hello_world",
        binary_name: b"hello_world\0",
        description: "Print hello world message",
    },
    ProgramEntry {
        name: "counter",
        binary_name: b"counter\0",
        description: "Count from 0 to 9",
    },
    ProgramEntry {
        name: "spinner",
        binary_name: b"spinner\0",
        description: "Display spinning animation",
    },
    ProgramEntry {
        name: "demo",
        binary_name: b"demo\0",
        description: "Animated graphics demo on left pane",
    },
    ProgramEntry {
        name: "bounce",
        binary_name: b"bounce\0",
        description: "Bouncing balls with collision detection (for Gus!)",
    },
    ProgramEntry {
        name: "hello_time",
        binary_name: b"hello_time\0",
        description: "Print hello with timestamp",
    },
    ProgramEntry {
        name: "fork_test",
        binary_name: b"fork_test\0",
        description: "Test fork syscall",
    },
    ProgramEntry {
        name: "pipe_test",
        binary_name: b"pipe_test\0",
        description: "Test pipe syscall",
    },
    ProgramEntry {
        name: "signal_test",
        binary_name: b"signal_test\0",
        description: "Test signal handling",
    },
    // === Coreutils ===
    ProgramEntry {
        name: "cat",
        binary_name: b"cat\0",
        description: "Concatenate and print files",
    },
    ProgramEntry {
        name: "ls",
        binary_name: b"ls\0",
        description: "List directory contents",
    },
    ProgramEntry {
        name: "mkdir",
        binary_name: b"mkdir\0",
        description: "Create directories",
    },
    ProgramEntry {
        name: "rmdir",
        binary_name: b"rmdir\0",
        description: "Remove empty directories",
    },
    ProgramEntry {
        name: "rm",
        binary_name: b"rm\0",
        description: "Remove files",
    },
    ProgramEntry {
        name: "cp",
        binary_name: b"cp\0",
        description: "Copy files",
    },
    ProgramEntry {
        name: "mv",
        binary_name: b"mv\0",
        description: "Move/rename files",
    },
    ProgramEntry {
        name: "echo",
        binary_name: b"echo\0",
        description: "Print arguments to stdout",
    },
    // === Network Tools ===
    ProgramEntry {
        name: "dns_test",
        binary_name: b"dns_test\0",
        description: "Test DNS resolution (google.com, example.com)",
    },
    ProgramEntry {
        name: "tcpclient",
        binary_name: b"tcp_client_test\0",
        description: "Send TCP message to 10.0.2.2:18888",
    },
    ProgramEntry {
        name: "telnetd",
        binary_name: b"/sbin/telnetd\0",
        description: "Telnet server on port 2323",
    },
    // === PTY Test ===
    ProgramEntry {
        name: "pty_test",
        binary_name: b"pty_test\0",
        description: "Test PTY functionality",
    },
];

fn find_program(name: &str) -> Option<&'static ProgramEntry> {
    PROGRAM_REGISTRY.iter().find(|e| e.name == name)
}

/// List all available external programs (for help command)
fn list_external_programs() {
    println!("External programs:");
    for entry in PROGRAM_REGISTRY {
        let padding = if entry.name.len() < 12 {
            12 - entry.name.len()
        } else {
            1
        };
        let pad: String = " ".repeat(padding);
        println!("  {}{}- {}", entry.name, pad, entry.description);
    }
}

// ============================================================================
// External command execution
// ============================================================================

/// Try to execute an external command via fork+exec.
///
/// Search order:
/// 1. PROGRAM_REGISTRY (for backwards compatibility with test disk binaries)
/// 2. Explicit path (if cmd_name contains '/') - use directly
/// 3. PATH-based lookup: /bin/{cmd_name}, /sbin/{cmd_name}
///
/// Returns Ok(exit_code) if executed, Err(()) if command not found.
fn try_execute_external(cmd_name: &str, args: &str, background: bool) -> Result<i32, ()> {
    let registry_entry = find_program(cmd_name);
    let is_explicit_path = cmd_name.contains('/');

    // Build path for execution
    let mut path_buf = Vec::new();

    let path_valid = if is_explicit_path {
        path_buf.extend_from_slice(cmd_name.as_bytes());
        path_buf.push(0);
        true
    } else {
        // PATH-based lookup: try /bin/ first, then /sbin/
        let prefixes: [&[u8]; 2] = [b"/bin/", b"/sbin/"];
        let mut found = false;
        for prefix in prefixes {
            let mut candidate = Vec::new();
            candidate.extend_from_slice(prefix);
            candidate.extend_from_slice(cmd_name.as_bytes());
            candidate.push(0);

            if sys_access(candidate.as_ptr(), X_OK) == 0 {
                path_buf = candidate;
                found = true;
                break;
            }
        }
        found
    };

    // If not in registry and path is invalid, fail
    if registry_entry.is_none() && !path_valid {
        return Err(());
    }

    if !background {
        println!("Running: {}", cmd_name);
    }

    match fork() {
        Ok(ForkResult::Child) => {
            // Child process
            // Put ourselves in our own process group BEFORE exec.
            let _ = setpgid(0, 0);

            let args = args.trim();

            // Determine which binary path to use
            let binary_path: &[u8] = if let Some(entry) = registry_entry {
                entry.binary_name
            } else {
                &path_buf
            };

            // Build argv: [binary_path, arg1, arg2, ..., null]
            // We need to keep the CString-like data alive through the execve call
            let mut arg_strings: Vec<Vec<u8>> = Vec::new();

            // argv[0] = binary path (already null-terminated)
            // For the rest, parse the args string
            if !args.is_empty() {
                let mut i = 0;
                let args_bytes = args.as_bytes();
                while i < args_bytes.len() {
                    // Skip whitespace
                    while i < args_bytes.len() && (args_bytes[i] == b' ' || args_bytes[i] == b'\t')
                    {
                        i += 1;
                    }
                    if i >= args_bytes.len() {
                        break;
                    }
                    // Find end of arg
                    let start = i;
                    while i < args_bytes.len() && args_bytes[i] != b' ' && args_bytes[i] != b'\t' {
                        i += 1;
                    }
                    let mut arg = args_bytes[start..i].to_vec();
                    arg.push(0); // null-terminate
                    arg_strings.push(arg);
                }
            }

            // Build argv pointer array
            let mut argv_ptrs: Vec<*const u8> = Vec::new();
            argv_ptrs.push(binary_path.as_ptr()); // argv[0]
            for arg in &arg_strings {
                argv_ptrs.push(arg.as_ptr());
            }
            argv_ptrs.push(std::ptr::null()); // null terminator

            let envp: [*const u8; 1] = [std::ptr::null()];

            // Use black_box to prevent compiler from optimizing away buffers
            let _keep_alive = std::hint::black_box(&arg_strings);
            let _keep_argv = std::hint::black_box(&argv_ptrs);

            sys_execve(binary_path.as_ptr(), argv_ptrs.as_ptr(), envp.as_ptr());

            // If exec returns, it failed
            eprintln!("Error: exec failed");
            std::process::exit(1);
        }
        Ok(ForkResult::Parent(child_pid)) => {
            let pid = child_pid.raw() as i32;

            // Also set the child's process group from parent side (POSIX race avoidance)
            let _ = setpgid(pid, pid);

            if background {
                let job_id = add_job(pid, cmd_name);
                println!("[{}] {}", job_id, pid);
                return Ok(0);
            }

            // Foreground execution - give terminal to child, then wait
            let shell_pgrp = getpgrp();
            let child_pgrp = pid;
            let _ = tcsetpgrp(STDIN, child_pgrp);

            let mut status: i32 = 0;
            let wait_result = waitpid(pid, &mut status as *mut i32, WUNTRACED);

            // Take terminal back from child
            let _ = tcsetpgrp(STDIN, shell_pgrp);

            match wait_result {
                Err(_) => {
                    println!("Error: waitpid failed");
                    Ok(-1)
                }
                Ok(_) => {
                    if wifexited(status) {
                        let exit_code = wexitstatus(status);
                        if exit_code != 0 {
                            println!("Process exited with code: {}", exit_code);
                        }
                        Ok(exit_code)
                    } else if wifsignaled(status) {
                        println!();
                        Ok(-1)
                    } else if wifstopped(status) {
                        let job_id = add_job(pid, cmd_name);
                        // Mark as stopped
                        if let Some(job) = job_table().find_by_id_mut(job_id) {
                            job.status = JobStatus::Stopped;
                        }
                        println!();
                        println!("[{}]+ Stopped                 {}", job_id, cmd_name);
                        Ok(0)
                    } else {
                        println!("Process terminated abnormally");
                        Ok(-1)
                    }
                }
            }
        }
        Err(_) => {
            println!("Error: fork failed");
            Ok(-1)
        }
    }
}

// ============================================================================
// Pipeline Support
// ============================================================================

const MAX_PIPELINE_COMMANDS: usize = 8;

#[derive(Clone, Copy)]
struct PipelineCommand<'a> {
    name: &'a str,
    full: &'a str,
}

fn contains_pipe(s: &str) -> bool {
    s.as_bytes().contains(&b'|')
}

/// Trim leading/trailing whitespace and null bytes
fn trim(s: &str) -> &str {
    let bytes = s.as_bytes();
    let mut start = 0;
    let mut end = bytes.len();

    while start < end && (bytes[start] == b' ' || bytes[start] == b'\t') {
        start += 1;
    }

    while end > start
        && (bytes[end - 1] == b' ' || bytes[end - 1] == b'\t' || bytes[end - 1] == 0)
    {
        end -= 1;
    }

    std::str::from_utf8(&bytes[start..end]).unwrap_or("")
}

fn is_background_command(input: &str) -> bool {
    trim(input).ends_with('&')
}

fn strip_background_operator(input: &str) -> &str {
    let trimmed = trim(input);
    if trimmed.ends_with('&') {
        trim(&trimmed[..trimmed.len() - 1])
    } else {
        trimmed
    }
}

fn split_pipeline<'a>(
    input: &'a str,
    commands: &mut [PipelineCommand<'a>; MAX_PIPELINE_COMMANDS],
) -> usize {
    let bytes = input.as_bytes();
    let mut count = 0;
    let mut start = 0;

    for (i, &c) in bytes.iter().enumerate() {
        if c == b'|' {
            if count < MAX_PIPELINE_COMMANDS {
                let segment = trim(&input[start..i]);
                if !segment.is_empty() {
                    let name_end = segment
                        .as_bytes()
                        .iter()
                        .position(|&ch| ch == b' ')
                        .unwrap_or(segment.len());
                    commands[count] = PipelineCommand {
                        name: &segment[..name_end],
                        full: segment,
                    };
                    count += 1;
                }
            }
            start = i + 1;
        }
    }

    if count < MAX_PIPELINE_COMMANDS && start < bytes.len() {
        let segment = trim(&input[start..]);
        if !segment.is_empty() {
            let name_end = segment
                .as_bytes()
                .iter()
                .position(|&ch| ch == b' ')
                .unwrap_or(segment.len());
            commands[count] = PipelineCommand {
                name: &segment[..name_end],
                full: segment,
            };
            count += 1;
        }
    }

    count
}

/// Execute a single command in a pipeline context.
/// This function never returns - it either execs or exits.
fn execute_pipeline_command(cmd: &PipelineCommand) -> ! {
    // Handle built-in "echo" in pipelines
    if cmd.name == "echo" {
        let args = if cmd.full.len() > 5 {
            trim(&cmd.full[5..])
        } else {
            ""
        };
        println!("{}", args);
        std::process::exit(0);
    }

    // Try external command from registry first
    if let Some(entry) = find_program(cmd.name) {
        let argv: [*const u8; 2] = [entry.binary_name.as_ptr(), std::ptr::null()];
        let envp: [*const u8; 1] = [std::ptr::null()];
        sys_execve(entry.binary_name.as_ptr(), argv.as_ptr(), envp.as_ptr());
        eprintln!("exec failed");
        std::process::exit(1);
    }

    // Try /bin/{cmd_name} and /sbin/{cmd_name} PATH lookup
    let prefixes: [&[u8]; 2] = [b"/bin/", b"/sbin/"];
    for prefix in prefixes {
        let mut path_buf: Vec<u8> = Vec::new();
        path_buf.extend_from_slice(prefix);
        path_buf.extend_from_slice(cmd.name.as_bytes());
        path_buf.push(0);

        if sys_access(path_buf.as_ptr(), X_OK) == 0 {
            let argv: [*const u8; 2] = [path_buf.as_ptr(), std::ptr::null()];
            let envp: [*const u8; 1] = [std::ptr::null()];
            let _keep = std::hint::black_box(&path_buf);
            sys_execve(path_buf.as_ptr(), argv.as_ptr(), envp.as_ptr());
            eprintln!("exec failed");
            std::process::exit(1);
        }
    }

    eprintln!("command not found: {}", cmd.name);
    std::process::exit(127)
}

/// Execute a pipeline of commands.
fn execute_pipeline(input: &str) -> bool {
    let mut commands: [PipelineCommand; MAX_PIPELINE_COMMANDS] = [PipelineCommand {
        name: "",
        full: "",
    }; MAX_PIPELINE_COMMANDS];

    let cmd_count = split_pipeline(input, &mut commands);

    if cmd_count <= 1 {
        return false;
    }

    // Verify all commands exist before starting pipeline
    for i in 0..cmd_count {
        let cmd = &commands[i];
        if cmd.name != "echo" && find_program(cmd.name).is_none() {
            // Check PATH too
            let mut found = false;
            let prefixes: [&[u8]; 2] = [b"/bin/", b"/sbin/"];
            for prefix in prefixes {
                let mut path_buf: Vec<u8> = Vec::new();
                path_buf.extend_from_slice(prefix);
                path_buf.extend_from_slice(cmd.name.as_bytes());
                path_buf.push(0);
                if sys_access(path_buf.as_ptr(), X_OK) == 0 {
                    found = true;
                    break;
                }
            }
            if !found {
                println!("command not found: {}", cmd.name);
                return true;
            }
        }
    }

    let mut child_pids: [i32; MAX_PIPELINE_COMMANDS] = [0; MAX_PIPELINE_COMMANDS];
    let mut pipeline_pgrp: i32 = 0;
    let mut prev_read_fd: Option<Fd> = None;

    for i in 0..cmd_count {
        let is_last = i == cmd_count - 1;

        let pipe_fds = if !is_last {
            match pipe() {
                Ok(fds) => Some(fds),
                Err(_) => {
                    println!("pipe failed");
                    if let Some(fd) = prev_read_fd {
                        let _ = close(fd);
                    }
                    return true;
                }
            }
        } else {
            None
        };

        match fork() {
            Ok(ForkResult::Child) => {
                // ========== CHILD PROCESS ==========
                let _ = setpgid(0, pipeline_pgrp);

                if let Some(fd) = prev_read_fd {
                    let _ = dup2(fd, STDIN);
                    let _ = close(fd);
                }

                if let Some((read_fd, write_fd)) = pipe_fds {
                    let _ = dup2(write_fd, STDOUT);
                    let _ = close(read_fd);
                    let _ = close(write_fd);
                }

                execute_pipeline_command(&commands[i]);
            }
            Ok(ForkResult::Parent(child_pid)) => {
                // ========== PARENT PROCESS ==========
                let pid = child_pid.raw() as i32;
                child_pids[i] = pid;

                if i == 0 {
                    pipeline_pgrp = pid;
                }
                let _ = setpgid(pid, pipeline_pgrp);

                if let Some(fd) = prev_read_fd {
                    let _ = close(fd);
                }

                if let Some((read_fd, write_fd)) = pipe_fds {
                    let _ = close(write_fd);
                    prev_read_fd = Some(read_fd);
                } else {
                    prev_read_fd = None;
                }
            }
            Err(_) => {
                println!("fork failed");
                if let Some(fd) = prev_read_fd {
                    let _ = close(fd);
                }
                if let Some((read_fd, write_fd)) = pipe_fds {
                    let _ = close(read_fd);
                    let _ = close(write_fd);
                }
                return true;
            }
        }
    }

    // Give terminal to the pipeline's process group
    let shell_pgrp = getpgrp();
    if pipeline_pgrp > 0 {
        let _ = tcsetpgrp(STDIN, pipeline_pgrp);
    }

    // Wait for all children
    for i in 0..cmd_count {
        if child_pids[i] > 0 {
            let mut status: i32 = 0;
            let _ = waitpid(child_pids[i], &mut status as *mut i32, 0);
        }
    }

    // Take terminal back
    let _ = tcsetpgrp(STDIN, shell_pgrp);

    true
}

// ============================================================================
// Signal handling
// ============================================================================

static SIGINT_RECEIVED: AtomicBool = AtomicBool::new(false);
static SIGCHLD_RECEIVED: AtomicBool = AtomicBool::new(false);

// Saved termios for restoration
static mut SAVED_TERMIOS: Option<Termios> = None;

/// SIGINT handler - just sets a flag
extern "C" fn sigint_handler(_sig: i32) {
    SIGINT_RECEIVED.store(true, Ordering::SeqCst);
}

/// SIGCHLD handler - just sets a flag (async-signal-safe)
extern "C" fn sigchld_handler(_sig: i32) {
    SIGCHLD_RECEIVED.store(true, Ordering::SeqCst);
}

/// Check for completed or stopped children (non-blocking)
fn check_children() {
    if !SIGCHLD_RECEIVED.swap(false, Ordering::SeqCst) {
        return;
    }

    loop {
        let mut status: i32 = 0;
        match waitpid(-1, &mut status as *mut i32, WNOHANG) {
            Ok(pid) if pid.raw() > 0 => {
                let pid_i32 = pid.raw() as i32;
                if wifexited(status) || wifsignaled(status) {
                    update_job_status(pid_i32, JobStatus::Done);
                } else if wifstopped(status) {
                    update_job_status(pid_i32, JobStatus::Stopped);
                }
            }
            _ => break,
        }
    }
}

/// Report jobs that have completed and remove them from the job table
fn report_done_jobs() {
    let mut to_remove: Vec<u32> = Vec::new();

    for job in job_table().iter() {
        if job.status == JobStatus::Done {
            println!("[{}]+  Done                    {}", job.id, job.command);
            to_remove.push(job.id);
        }
    }

    for id in to_remove {
        job_table().remove(id);
    }
}

// ============================================================================
// Line reading
// ============================================================================

/// Read a line from stdin, handling backspace and yielding on EAGAIN.
/// Returns None if interrupted by SIGINT (Ctrl+C).
fn read_line() -> Option<String> {
    let mut line = String::new();

    loop {
        // Check for SIGINT (Ctrl+C)
        if SIGINT_RECEIVED.swap(false, Ordering::SeqCst) {
            print!("^C\n");
            return None;
        }

        let mut c = [0u8; 1];
        match read(STDIN, &mut c) {
            Ok(n) if n > 0 => {
                let ch = c[0];

                // Handle newline
                if ch == b'\n' || ch == b'\r' {
                    println!();
                    return Some(line);
                }

                // Handle backspace (ASCII DEL or BS)
                if ch == 0x7f || ch == 0x08 {
                    if !line.is_empty() {
                        line.pop();
                        print!("\x08 \x08");
                    }
                    continue;
                }

                // Handle printable characters
                if line.len() < 255 && ch >= 0x20 && ch < 0x7f {
                    line.push(ch as char);
                    // Note: Kernel echoes characters in push_byte, so no shell echo needed
                }
            }
            Ok(_) => {
                // EOF or no data
                if getpid().map(|p| p.raw()).unwrap_or(0) != 1 {
                    std::process::exit(0);
                }
                let _ = yield_now();
            }
            Err(_) => {
                // EAGAIN, EINTR, or other transient error - just yield and retry
                let _ = yield_now();
            }
        }
    }
}

// ============================================================================
// Built-in commands
// ============================================================================

fn cmd_help() {
    println!("Built-in commands:");
    println!("  help   - Show this help message");
    println!("  echo   - Echo text back to the terminal");
    println!("  cd     - Change current directory (cd /path)");
    println!("  pwd    - Print current working directory");
    println!("  ps     - List processes (placeholder)");
    println!("  jobs   - List background and stopped jobs");
    println!("  bg     - Resume stopped job in background (bg %N)");
    println!("  fg     - Bring job to foreground (fg %N)");
    println!("  uptime - Show time since boot");
    println!("  clear  - Clear the screen (ANSI escape sequence)");
    println!("  raw    - Switch to raw mode and show keypresses");
    println!("  cooked - Switch back to canonical (cooked) mode");
    println!("  devtest- Test device files (/dev/null, /dev/zero, etc.)");
    println!("  progs  - List available external programs");
    println!("  exit   - Attempt to exit (init cannot exit)");
    println!();
    list_external_programs();
    println!();
    println!("Pipes:");
    println!("  cmd1 | cmd2      - Pipe output of cmd1 to input of cmd2");
    println!("  echo hello | cat - Example: pipe echo output (not yet useful)");
    println!();
    println!("Background execution:");
    println!("  command &        - Run command in background");
    println!("  hello &          - Example: run hello_world in background");
    println!();
    println!("Job control:");
    println!("  fg [%N]          - Bring job N to foreground");
    println!("  bg [%N]          - Resume stopped job N in background");
    println!("  jobs             - List all jobs with their status");
    println!();
    println!("TTY testing:");
    println!("  - Ctrl+C shows ^C and gives new prompt");
    println!("  - Backspace works for line editing");
    println!("  - In raw mode, each keypress is shown immediately");
}

fn cmd_echo(args: &str) {
    println!("{}", args);
}

fn cmd_ps() {
    println!("  PID  CMD");
    println!("    1  init");
}

fn cmd_uptime() {
    // Use libbreenix::time::now_monotonic() for CLOCK_MONOTONIC
    let ts = match libbreenix::time::now_monotonic() {
        Ok(ts) => ts,
        Err(_) => {
            println!("Error: could not get monotonic time");
            return;
        }
    };

    let secs = ts.tv_sec as u64;
    let mins = secs / 60;
    let hours = mins / 60;

    let mut parts: Vec<String> = Vec::new();

    if hours > 0 {
        let suffix = if hours != 1 { "s" } else { "" };
        parts.push(format!("{} hour{}", hours, suffix));
    }

    if mins > 0 || hours > 0 {
        let m = mins % 60;
        let suffix = if m != 1 { "s" } else { "" };
        parts.push(format!("{} minute{}", m, suffix));
    }

    let s = secs % 60;
    let suffix = if s != 1 { "s" } else { "" };
    parts.push(format!("{} second{}", s, suffix));

    println!("up {}", parts.join(", "));
}

fn cmd_cd(args: &str) {
    let path = if args.is_empty() { "/" } else { args.trim() };

    if let Err(e) = std::env::set_current_dir(path) {
        println!("cd: {}: {}", path, e);
    }
}

fn cmd_pwd() {
    match std::env::current_dir() {
        Ok(path) => println!("{}", path.display()),
        Err(_) => println!("pwd: cannot get current directory"),
    }
}

fn cmd_clear() {
    print!("\x1b[2J\x1b[H");
}

fn cmd_exit() {
    println!("Cannot exit init!");
    println!("The init process must run forever.");
}

/// Handle the "raw" command - switch to raw mode and show keypresses
fn cmd_raw() {
    println!("Switching to raw mode...");
    println!("Press keys to see their codes. Press 'q' to exit raw mode.");
    println!();

    let mut t = Termios::default();
    if tcgetattr(STDIN, &mut t).is_err() {
        println!("Error: Could not get terminal attributes");
        return;
    }

    let original_termios = t;
    unsafe {
        SAVED_TERMIOS = Some(original_termios);
    }

    shell_cfmakeraw(&mut t);
    // Keep output processing enabled so newlines work correctly
    t.c_oflag |= oflag::OPOST | oflag::ONLCR;

    if tcsetattr(STDIN, TCSANOW, &t).is_err() {
        println!("Error: Could not set raw mode");
        return;
    }

    println!("Raw mode enabled. Type keys:");
    println!();

    loop {
        let mut c = [0u8; 1];
        match read(STDIN, &mut c) {
            Ok(n) if n > 0 => {
                let ch = c[0];

                // Display the keypress
                if ch >= 0x20 && ch < 0x7f {
                    println!("Key: '{}' (0x{:02x}, {})", ch as char, ch, ch);
                } else if ch == 0x1b {
                    println!("Key: ESC (0x{:02x}, {})", ch, ch);
                } else if ch < 0x20 {
                    println!(
                        "Key: ^{} (0x{:02x}, {})",
                        (b'@' + ch) as char,
                        ch,
                        ch
                    );
                } else if ch == 0x7f {
                    println!("Key: DEL (0x{:02x}, {})", ch, ch);
                } else {
                    println!("Key:     (0x{:02x}, {})", ch, ch);
                }

                if ch == b'q' || ch == b'Q' {
                    println!();
                    println!("Exiting raw mode...");
                    break;
                }
            }
            Ok(_) => {
                let _ = yield_now();
            }
            Err(_) => {
                let _ = yield_now();
            }
        }
    }

    if tcsetattr(STDIN, TCSANOW, &original_termios).is_err() {
        println!("Warning: Could not restore terminal settings");
    }

    println!("Back to canonical mode.");
}

/// Handle the "cooked" command - switch back to canonical mode
fn cmd_cooked() {
    println!("Switching to canonical (cooked) mode...");

    let restored = unsafe {
        if let Some(ref original) = SAVED_TERMIOS {
            tcsetattr(STDIN, TCSANOW, original).is_ok()
        } else {
            let mut t = Termios::default();
            if tcgetattr(STDIN, &mut t).is_err() {
                println!("Error: Could not get terminal attributes");
                return;
            }
            t.c_lflag |= lflag::ICANON | lflag::ECHO | lflag::ECHOE | lflag::ISIG;
            t.c_oflag |= oflag::OPOST | oflag::ONLCR;
            tcsetattr(STDIN, TCSANOW, &t).is_ok()
        }
    };

    if restored {
        println!("Canonical mode enabled.");
        println!("Line editing and signals are now active.");
    } else {
        println!("Error: Could not set canonical mode");
    }
}

/// Handle the "devtest" command - test device files interactively
fn cmd_devtest() {
    println!("Testing device files...");
    println!();

    // Test 1: Write to /dev/null
    print!("1. /dev/null write: ");
    let fd = sys_open(b"/dev/null\0".as_ptr(), O_WRONLY);
    if fd >= 0 {
        let data = b"test data";
        let fd_handle = Fd::from_raw(fd as u64);
        let result = write(fd_handle, data);
        let _ = close(fd_handle);
        match result {
            Ok(n) if n == data.len() => println!("OK (data discarded)"),
            Ok(n) => println!("FAIL (returned {})", n),
            Err(_) => println!("FAIL (write error)"),
        }
    } else {
        println!("FAIL (open error {})", -fd);
    }

    // Test 2: Read from /dev/null (should return EOF)
    print!("2. /dev/null read:  ");
    let fd = sys_open(b"/dev/null\0".as_ptr(), O_RDONLY);
    if fd >= 0 {
        let mut buf = [0u8; 16];
        let fd_handle = Fd::from_raw(fd as u64);
        let result = read(fd_handle, &mut buf);
        let _ = close(fd_handle);
        match result {
            Ok(0) => println!("OK (EOF as expected)"),
            Ok(n) => println!("FAIL (returned {})", n),
            Err(_) => println!("FAIL (read error)"),
        }
    } else {
        println!("FAIL (open error {})", -fd);
    }

    // Test 3: Read from /dev/zero (should return zeros)
    print!("3. /dev/zero read:  ");
    let fd = sys_open(b"/dev/zero\0".as_ptr(), O_RDONLY);
    if fd >= 0 {
        let mut buf = [0xFFu8; 8];
        let fd_handle = Fd::from_raw(fd as u64);
        let result = read(fd_handle, &mut buf);
        let _ = close(fd_handle);
        match result {
            Ok(8) if buf.iter().all(|&b| b == 0) => println!("OK (8 zeros read)"),
            Ok(n) => println!("FAIL ({} bytes, or non-zero data)", n),
            Err(_) => println!("FAIL (read error)"),
        }
    } else {
        println!("FAIL (open error {})", -fd);
    }

    // Test 4: Write to /dev/console
    print!("4. /dev/console:    ");
    let fd = sys_open(b"/dev/console\0".as_ptr(), O_WRONLY);
    if fd >= 0 {
        let msg = b"[console test] ";
        let fd_handle = Fd::from_raw(fd as u64);
        let result = write(fd_handle, msg);
        let _ = close(fd_handle);
        match result {
            Ok(n) if n == msg.len() => println!("OK"),
            Ok(n) => println!("FAIL (returned {})", n),
            Err(_) => println!("FAIL (write error)"),
        }
    } else {
        println!("FAIL (open error {})", -fd);
    }

    // Test 5: List /dev directory
    print!("5. ls /dev:         ");
    let fd = sys_open(b"/dev\0".as_ptr(), O_DIRECTORY | O_RDONLY);
    if fd >= 0 {
        let _ = close(Fd::from_raw(fd as u64));
        println!("OK (directory opened)");
        println!("   Contents: null zero console tty");
    } else {
        println!("FAIL (open error {})", -fd);
    }

    println!();
    println!("Device tests complete!");
}

fn cmd_unknown(cmd: &str) {
    println!("Unknown command: {}", cmd);
    println!("Type 'help' for available commands.");
}

// ============================================================================
// Command parsing and execution
// ============================================================================

fn handle_command(line: &str) {
    let line = trim(line);

    if line.is_empty() {
        return;
    }

    // Check for background operator (&)
    let background = is_background_command(line);
    let line = if background {
        strip_background_operator(line)
    } else {
        line
    };

    // Check for pipeline
    if contains_pipe(line) {
        if background {
            println!("Background pipelines not yet supported");
            return;
        }
        if execute_pipeline(line) {
            return;
        }
    }

    // Extract command name and arguments
    let cmd_end = line
        .as_bytes()
        .iter()
        .position(|&c| c == b' ')
        .unwrap_or(line.len());
    let cmd = &line[..cmd_end];
    let args = if cmd_end < line.len() {
        trim(&line[cmd_end + 1..])
    } else {
        ""
    };

    // Background commands can only be external
    if background {
        if try_execute_external(cmd, args, true).is_err() {
            cmd_unknown(cmd);
        }
        return;
    }

    // Match built-in commands first (foreground only)
    match cmd {
        "help" => cmd_help(),
        "echo" => cmd_echo(args),
        "cd" => cmd_cd(args),
        "pwd" => cmd_pwd(),
        "ps" => cmd_ps(),
        "jobs" => list_jobs(),
        "bg" => builtin_bg(args),
        "fg" => builtin_fg(args),
        "uptime" => cmd_uptime(),
        "clear" => cmd_clear(),
        "raw" => cmd_raw(),
        "cooked" => cmd_cooked(),
        "devtest" => cmd_devtest(),
        "progs" => list_external_programs(),
        "exit" | "quit" => cmd_exit(),
        _ => {
            if try_execute_external(cmd, args, false).is_err() {
                cmd_unknown(cmd);
            }
        }
    }
}

// ============================================================================
// Welcome banner
// ============================================================================

fn print_banner() {
    println!();
    println!("========================================");
    println!("     Breenix OS Interactive Shell");
    println!("========================================");
    println!();
    println!("Welcome to Breenix! Type 'help' for available commands.");
    println!();
}

// ============================================================================
// Signal handler setup
// ============================================================================

fn setup_signal_handlers() {
    // Set up SIGINT handler for Ctrl+C using libbreenix::Sigaction::new()
    // which automatically sets SA_RESTORER and uses the libbreenix restorer
    let sigint_action = Sigaction::new(sigint_handler);
    if sigaction(SIGINT, Some(&sigint_action), None).is_err() {
        println!("Warning: Could not set up SIGINT handler");
    }

    // Set up SIGCHLD handler for background job status updates
    let sigchld_action = Sigaction::new(sigchld_handler);
    if sigaction(SIGCHLD, Some(&sigchld_action), None).is_err() {
        println!("Warning: Could not set up SIGCHLD handler");
    }
}

// ============================================================================
// Entry point
// ============================================================================

fn main() {
    // Set up signal handlers before anything else
    setup_signal_handlers();

    // Set this shell as the foreground process group for the TTY
    let pgrp = getpgrp();
    if tcsetpgrp(STDIN, pgrp).is_err() {
        println!("Warning: Could not set foreground process group");
    }

    print_banner();

    // Main REPL loop
    loop {
        check_children();
        report_done_jobs();

        print!("breenix> ");
        // Flush stdout so prompt appears before blocking read
        use std::io::Write;
        let _ = std::io::stdout().flush();

        if let Some(line) = read_line() {
            handle_command(&line);
        }
        // If None (interrupted by Ctrl+C), just continue to print new prompt
    }
}
