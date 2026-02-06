//! Minimal Interactive Shell for Breenix OS
//!
//! This is meant to run as PID 1 (init). It provides a simple REPL that:
//! 1. Prints a welcome banner
//! 2. Shows a prompt "breenix> "
//! 3. Reads a line of input (blocking read from stdin)
//! 4. Parses and executes simple commands
//! 5. Loops forever
//!
//! Features for testing TTY line discipline:
//! - "raw" command: switches to raw mode and shows keypresses
//! - "cooked" command: switches back to canonical mode
//! - Ctrl+C handling: shows ^C and gives new prompt
//! - Line editing: backspace works in canonical mode

#![no_std]
#![no_main]

use core::cell::UnsafeCell;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, Ordering};
use libbreenix::fs::{access, open, O_DIRECTORY, O_RDONLY, O_WRONLY, X_OK};
use libbreenix::io::{close, dup2, pipe, print, println, read, write};
use libbreenix::process::{
    chdir, exec, execv, fork, getcwd, getpgrp, setpgid, waitpid, wexitstatus, wifexited,
    wifsignaled, wifstopped, yield_now, WNOHANG, WUNTRACED,
};
use libbreenix::signal::{kill, sigaction, Sigaction, SIGCHLD, SIGCONT, SIGINT};
use libbreenix::termios::{
    cfmakeraw, lflag, oflag, tcgetattr, tcsetattr, tcsetpgrp, Termios, TCSANOW,
};
use libbreenix::time::now_monotonic;
use libbreenix::types::fd::{STDIN, STDOUT};
use libbreenix::Timespec;

// ============================================================================
// Job Tracking - Background and stopped job management
// ============================================================================

/// Status of a job in the job table
#[derive(Clone, Copy, PartialEq)]
#[repr(u8)]
enum JobStatus {
    Running = 0,
    Stopped = 1,
    Done = 2,
}

/// Maximum length of command string stored in a job
const JOB_COMMAND_MAX: usize = 128;

/// A job entry representing a background or stopped process
#[derive(Clone, Copy)]
struct Job {
    /// Job ID (1-based, shown to user as [1], [2], etc.)
    id: u32,
    /// Process ID of the job
    pid: i32,
    /// Process group ID of the job
    pgid: i32,
    /// Current status of the job
    status: JobStatus,
    /// Command string stored as fixed-size buffer (no heap allocation)
    command: [u8; JOB_COMMAND_MAX],
    /// Actual length of the command string
    command_len: usize,
}

impl Job {
    /// Get the command as a string slice
    fn command_str(&self) -> &str {
        core::str::from_utf8(&self.command[..self.command_len]).unwrap_or("")
    }
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

impl JobTable {
    /// Create a new empty job table
    const fn new() -> Self {
        const NONE: Option<Job> = None;
        JobTable {
            jobs: [NONE; MAX_JOBS],
            next_id: 1,
            current: 0,
        }
    }

    /// Add a new job to the table
    ///
    /// Returns the job ID, or 0 if the table is full
    fn add(&mut self, pid: i32, pgid: i32, command: &str) -> u32 {
        // Find an empty slot
        for slot in self.jobs.iter_mut() {
            if slot.is_none() {
                let id = self.next_id;
                self.next_id += 1;

                // Copy command into fixed buffer
                let mut cmd_buf = [0u8; JOB_COMMAND_MAX];
                let cmd_bytes = command.as_bytes();
                let cmd_len = cmd_bytes.len().min(JOB_COMMAND_MAX);
                cmd_buf[..cmd_len].copy_from_slice(&cmd_bytes[..cmd_len]);

                *slot = Some(Job {
                    id,
                    pid,
                    pgid,
                    status: JobStatus::Running,
                    command: cmd_buf,
                    command_len: cmd_len,
                });

                self.current = id;
                return id;
            }
        }
        0 // Table full
    }

    /// Find a job by its job ID
    fn find_by_id(&self, id: u32) -> Option<&Job> {
        for slot in &self.jobs {
            if let Some(job) = slot {
                if job.id == id {
                    return Some(job);
                }
            }
        }
        None
    }

    /// Find a job by its job ID (mutable)
    fn find_by_id_mut(&mut self, id: u32) -> Option<&mut Job> {
        for slot in &mut self.jobs {
            if let Some(job) = slot {
                if job.id == id {
                    return Some(job);
                }
            }
        }
        None
    }

    /// Find a job by its process ID
    #[allow(dead_code)] // Public API - will be used by signal handlers
    fn find_by_pid(&self, pid: i32) -> Option<&Job> {
        for slot in &self.jobs {
            if let Some(job) = slot {
                if job.pid == pid {
                    return Some(job);
                }
            }
        }
        None
    }

    /// Find a job by its process ID (mutable)
    fn find_by_pid_mut(&mut self, pid: i32) -> Option<&mut Job> {
        for slot in &mut self.jobs {
            if let Some(job) = slot {
                if job.pid == pid {
                    return Some(job);
                }
            }
        }
        None
    }

    /// Update the status of a job by PID
    fn update_status(&mut self, pid: i32, status: JobStatus) {
        if let Some(job) = self.find_by_pid_mut(pid) {
            job.status = status;
        }
    }

    /// Remove a job by its job ID
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

    /// Get the current (most recent) job
    #[allow(dead_code)] // Public API - will be used by fg command
    fn current_job(&self) -> Option<&Job> {
        self.find_by_id(self.current)
    }

    /// Iterate over all active jobs
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
    const fn new(value: JobTable) -> Self {
        SyncJobTable(UnsafeCell::new(value))
    }

    fn get(&self) -> *mut JobTable {
        self.0.get()
    }
}

// Global job table instance
static JOB_TABLE: SyncJobTable = SyncJobTable::new(JobTable::new());

/// Get a mutable reference to the job table
///
/// # Safety
/// This is safe in single-threaded userspace code. The shell main loop
/// is the only accessor of the job table.
#[inline]
fn job_table() -> &'static mut JobTable {
    // SAFETY: Single-threaded userspace - no concurrent access
    unsafe { &mut *JOB_TABLE.get() }
}

/// Add a job to the job table
///
/// Returns the job ID, or 0 if the table is full
fn add_job(pid: i32, command: &str) -> u32 {
    // Use pid as pgid by default (job's own process group)
    job_table().add(pid, pid, command)
}

/// Get a job by its job ID
#[allow(dead_code)] // Public API - will be used by fg/bg commands
fn get_job(id: u32) -> Option<&'static Job> {
    job_table().find_by_id(id)
}

/// Get a mutable reference to a job by its job ID
#[allow(dead_code)] // Public API - will be used by fg/bg commands
fn get_job_mut(id: u32) -> Option<&'static mut Job> {
    job_table().find_by_id_mut(id)
}

/// Update the status of a job by PID
#[allow(dead_code)] // Public API - will be used by SIGCHLD handler
fn update_job_status(pid: i32, status: JobStatus) {
    job_table().update_status(pid, status)
}

/// Remove a job from the table
#[allow(dead_code)] // Public API - will be used after job completion
fn remove_job(id: u32) {
    job_table().remove(id)
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
        print("[");
        print_num(job.id as u64);
        print("]");
        print(current_marker);
        print("  ");
        print(status_str);
        print("\t\t");
        println(job.command_str());
    }
}

/// Get the current (most recent) job ID
///
/// Returns 0 if no jobs exist
fn get_current_job_id() -> u32 {
    job_table().current
}

/// Parse a job specification string into a job ID
///
/// Accepts formats:
/// - "%1" or "%2" etc. (job ID with % prefix)
/// - "1" or "2" etc. (bare job ID)
/// - "" (empty string, returns current job)
///
/// Returns 0 if the spec is invalid
fn parse_job_spec(spec: &str) -> u32 {
    let spec = trim(spec);

    if spec.is_empty() {
        return get_current_job_id();
    }

    // Strip leading '%' if present
    let num_str = if spec.starts_with('%') {
        &spec[1..]
    } else {
        spec
    };

    // Parse the number manually (no std::str::parse in no_std)
    let mut result: u32 = 0;
    for c in num_str.as_bytes() {
        if *c >= b'0' && *c <= b'9' {
            result = result * 10 + (*c - b'0') as u32;
        } else {
            return 0; // Invalid character
        }
    }

    result
}

/// Handle the "bg" builtin command
///
/// Resume a stopped job in the background.
/// Usage: bg [%job_id]
fn builtin_bg(arg: &str) {
    let job_id = parse_job_spec(arg);

    if job_id == 0 {
        println("bg: no current job");
        return;
    }

    if let Some(job) = job_table().find_by_id_mut(job_id) {
        if job.status != JobStatus::Stopped {
            print("bg: job ");
            print_num(job_id as u64);
            println(" is not stopped");
            return;
        }

        // Send SIGCONT to resume the job in background
        // Use negative pgid to signal the entire process group
        let _ = kill(-(job.pgid), SIGCONT);
        job.status = JobStatus::Running;

        // Print job notification
        print("[");
        print_num(job_id as u64);
        print("] ");
        print(job.command_str());
        println(" &");
    } else {
        print("bg: ");
        if arg.is_empty() {
            println("no current job");
        } else {
            print(arg);
            println(": no such job");
        }
    }
}

/// Handle the "fg" builtin command
///
/// Bring a background or stopped job to the foreground.
/// Usage: fg [%job_id]
fn builtin_fg(arg: &str) {
    let job_id = parse_job_spec(arg);

    if job_id == 0 {
        println("fg: no current job");
        return;
    }

    // First check if the job exists and get its info
    let (pid, pgid, was_stopped, cmd) = {
        let table = job_table();
        if let Some(job) = table.find_by_id_mut(job_id) {
            let was_stopped = job.status == JobStatus::Stopped;

            // Get command string for display (copy to avoid borrow issues)
            let mut cmd_buf = [0u8; JOB_COMMAND_MAX];
            let cmd_len = job.command_len;
            cmd_buf[..cmd_len].copy_from_slice(&job.command[..cmd_len]);

            (job.pid, job.pgid, was_stopped, (cmd_buf, cmd_len))
        } else {
            print("fg: ");
            if arg.is_empty() {
                println("no current job");
            } else {
                print(arg);
                println(": no such job");
            }
            return;
        }
    };

    // Print notification
    print("[");
    print_num(job_id as u64);
    print("] ");
    if let Ok(cmd_str) = core::str::from_utf8(&cmd.0[..cmd.1]) {
        println(cmd_str);
    }

    // Give terminal to the job's process group
    let _ = tcsetpgrp(0, pgid);

    // If job was stopped, send SIGCONT to resume it
    if was_stopped {
        let _ = kill(-pgid, SIGCONT);
        // Update status
        if let Some(job) = job_table().find_by_id_mut(job_id) {
            job.status = JobStatus::Running;
        }
    }

    // Wait for the job (with WUNTRACED to catch if it stops again)
    let mut status: i32 = 0;
    let wait_result = waitpid(pid, &mut status as *mut i32, WUNTRACED);

    // Take terminal back
    let shell_pgrp = getpgrp();
    let _ = tcsetpgrp(0, shell_pgrp);

    // Update job status based on result
    if wait_result > 0 {
        if wifstopped(status) {
            // Job was stopped again (e.g., Ctrl+Z)
            if let Some(job) = job_table().find_by_id_mut(job_id) {
                job.status = JobStatus::Stopped;
            }
            println("");
            print("[");
            print_num(job_id as u64);
            print("]+  Stopped\t\t");
            if let Ok(cmd_str) = core::str::from_utf8(&cmd.0[..cmd.1]) {
                println(cmd_str);
            }
        } else {
            // Job completed - remove from table
            job_table().remove(job_id);
        }
    } else {
        // Wait failed - remove job from table anyway
        job_table().remove(job_id);
    }
}

// ============================================================================
// Program Registry - Lookup table for external commands
// ============================================================================

/// An entry in the program registry representing an external command
/// that can be executed via fork+exec.
#[derive(Clone, Copy)]
pub struct ProgramEntry {
    /// The command name as typed by the user (e.g., "hello")
    pub name: &'static str,
    /// The null-terminated binary name for exec (e.g., b"hello_world\0")
    /// This must match the binary name in the test disk.
    pub binary_name: &'static [u8],
    /// Brief description for help text
    pub description: &'static str,
}

/// Static registry of all known external programs.
/// These are programs built in userspace/tests/ and loaded from the test disk.
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

/// Find a program in the registry by command name.
///
/// Returns Some(ProgramEntry) if found, None otherwise.
pub fn find_program(name: &str) -> Option<&'static ProgramEntry> {
    for entry in PROGRAM_REGISTRY {
        if entry.name == name {
            return Some(entry);
        }
    }
    None
}

/// Try to execute an external command via fork+exec.
///
/// Arguments:
/// - `cmd_name`: The command name to execute
/// - `args`: Arguments to pass to the command
/// - `background`: If true, don't wait for the child process
///
/// Returns:
/// - Ok(exit_code) if the program was found and executed (0 for background)
/// - Err(()) if the program was not found
///
/// Search order:
/// 1. PROGRAM_REGISTRY (for backwards compatibility with test disk binaries)
/// 2. Explicit path (if cmd_name contains '/') - use directly
/// 3. PATH-based lookup: /bin/{cmd_name}, /sbin/{cmd_name}
pub fn try_execute_external(cmd_name: &str, args: &str, background: bool) -> Result<i32, ()> {
    // Check if command is in registry
    let registry_entry = find_program(cmd_name);

    // Determine if this is an explicit path or a bare command name
    let is_explicit_path = cmd_name.contains('/');

    // Build path for execution
    const PATH_BUF_LEN: usize = 128;
    let mut path_buf = [0u8; PATH_BUF_LEN];
    let cmd_bytes = cmd_name.as_bytes();
    let mut path_len: usize = 0; // Length of null-terminated path in path_buf

    let path_valid = if is_explicit_path {
        // Use the explicit path directly
        if cmd_bytes.len() + 1 <= PATH_BUF_LEN {
            path_buf[..cmd_bytes.len()].copy_from_slice(cmd_bytes);
            path_buf[cmd_bytes.len()] = 0;
            path_len = cmd_bytes.len() + 1;
            true
        } else {
            false
        }
    } else {
        // PATH-based lookup: try /bin/ first, then /sbin/
        let prefixes: [&[u8]; 2] = [b"/bin/", b"/sbin/"];
        let mut found = false;
        for prefix in prefixes {
            if prefix.len() + cmd_bytes.len() + 1 <= PATH_BUF_LEN {
                path_buf[..prefix.len()].copy_from_slice(prefix);
                path_buf[prefix.len()..prefix.len() + cmd_bytes.len()].copy_from_slice(cmd_bytes);
                path_buf[prefix.len() + cmd_bytes.len()] = 0;
                path_len = prefix.len() + cmd_bytes.len() + 1;

                // Check if file exists and is executable
                if let Ok(path_str) = core::str::from_utf8(&path_buf[..path_len]) {
                    if access(path_str, X_OK).is_ok() {
                        found = true;
                        break;
                    }
                }
            }
        }
        found
    };

    // If not in registry and path is invalid, fail
    if registry_entry.is_none() && !path_valid {
        return Err(());
    }

    if !background {
        print("Running: ");
        println(cmd_name);
    }

    // Fork a child process
    let pid = fork();

    if pid < 0 {
        // Fork failed
        print("Error: fork failed with code ");
        print_num((-pid) as u64);
        println("");
        return Ok(-1);
    }

    if pid == 0 {
        // Child process
        // CRITICAL: Put ourselves in our own process group BEFORE exec.
        // This is required for proper job control - signals sent to the foreground
        // process group will only go to us, not the shell.
        // setpgid(0, 0) means: set my (pid=0=self) process group to my own PID (pgid=0=self).
        let _ = setpgid(0, 0);

        let args = trim(args);

        // Determine which binary path to use
        let binary_path: &[u8] = if let Some(entry) = registry_entry {
            entry.binary_name
        } else {
            // Use path from path_buf (either explicit or /bin/{cmd_name})
            &path_buf[..path_len]
        };

        // Parse all arguments and build argv array
        // Constants for argument parsing
        const MAX_ARGS: usize = 16; // Maximum number of arguments (including argv[0])
        const ARG_BUF_SIZE: usize = 512; // Total buffer for all argument strings

        // Buffer to hold all null-terminated argument strings
        let mut arg_strings = [0u8; ARG_BUF_SIZE];
        // Array of pointers to each argument (argv[0] = binary, then args, then null)
        let mut argv_ptrs: [*const u8; MAX_ARGS + 1] = [core::ptr::null(); MAX_ARGS + 1];

        // argv[0] is the binary path
        argv_ptrs[0] = binary_path.as_ptr();
        let mut argc = 1usize;
        let mut buf_pos = 0usize;

        if !args.is_empty() {
            let args_bytes = args.as_bytes();
            let mut i = 0;

            while i < args_bytes.len() && argc < MAX_ARGS {
                // Skip leading whitespace
                while i < args_bytes.len() && (args_bytes[i] == b' ' || args_bytes[i] == b'\t') {
                    i += 1;
                }
                if i >= args_bytes.len() {
                    break;
                }

                // Find end of this argument
                let arg_start = i;
                while i < args_bytes.len() && args_bytes[i] != b' ' && args_bytes[i] != b'\t' {
                    i += 1;
                }
                let arg_len = i - arg_start;

                // Check if we have room in the buffer
                if buf_pos + arg_len + 1 > ARG_BUF_SIZE {
                    print("Error: argument list too long\n");
                    libbreenix::process::exit(1);
                }

                // Copy argument to buffer and null-terminate
                arg_strings[buf_pos..buf_pos + arg_len]
                    .copy_from_slice(&args_bytes[arg_start..arg_start + arg_len]);
                arg_strings[buf_pos + arg_len] = 0;

                // Store pointer to this argument
                argv_ptrs[argc] = arg_strings[buf_pos..].as_ptr();
                argc += 1;
                buf_pos += arg_len + 1;
            }
        }

        // argv is already null-terminated (initialized with nulls)
        // CRITICAL: Use black_box to prevent the compiler from optimizing
        // away these buffers before the syscall reads from them.
        let arg_strings_ptr = core::hint::black_box(arg_strings.as_ptr());
        let _ = arg_strings_ptr; // Ensure buffer stays alive
        let argv_ptr = core::hint::black_box(argv_ptrs.as_ptr());
        let result = execv(binary_path, argv_ptr);

        // If exec returns, it failed
        print("Error: exec failed with code ");
        print_num((-result) as u64);
        println("");

        // Exit the child with error code
        libbreenix::process::exit(1);
    }

    // Parent process
    // CRITICAL: Also set the child's process group from parent side.
    // This is the POSIX-standard way to avoid race conditions:
    // - If parent runs first, parent sets child's pgrp
    // - If child runs first, child sets its own pgrp
    // Either way, the child ends up in the right process group before signals arrive.
    let _ = setpgid(pid as i32, pid as i32);

    if background {
        // Background execution - add to job table and print notification
        let job_id = add_job(pid as i32, cmd_name);
        print_job_notification(job_id, pid);
        return Ok(0);
    }

    // Foreground execution - give terminal to child, then wait for completion
    // CRITICAL: This is what makes Ctrl+C go to the child instead of the shell.
    // Without this, SIGINT from Ctrl+C would kill the shell instead of the child.
    let shell_pgrp = getpgrp();
    let child_pgrp = pid as i32; // Child's process group = child's PID (after setpgid)
    let _ = tcsetpgrp(STDIN as i32, child_pgrp);

    let mut status: i32 = 0;
    let wait_result = waitpid(pid as i32, &mut status as *mut i32, WUNTRACED);

    // Take terminal back from child
    let _ = tcsetpgrp(STDIN as i32, shell_pgrp);

    if wait_result < 0 {
        print("Error: waitpid failed with code ");
        print_num((-wait_result) as u64);
        println("");
        return Ok(-1);
    }

    // Handle child status
    if wifexited(status) {
        let exit_code = wexitstatus(status);
        if exit_code != 0 {
            print("Process exited with code: ");
            print_num(exit_code as u64);
            println("");
        }
        Ok(exit_code)
    } else if wifsignaled(status) {
        // Child was killed by a signal (e.g., SIGINT from Ctrl+C)
        // Print newline to move past any partial output
        println("");
        Ok(-1)
    } else if wifstopped(status) {
        // Child was stopped by a signal (e.g., SIGTSTP from Ctrl+Z)
        // Add to job table as a stopped job
        let job_id = add_job(pid as i32, cmd_name);
        // Find the job and mark it as stopped
        let jt = job_table();
        if let Some(slot) = jt.jobs.iter_mut().find(|j| {
            j.as_ref().map(|job| job.id == job_id).unwrap_or(false)
        }) {
            if let Some(ref mut job) = slot {
                job.status = JobStatus::Stopped;
            }
        }
        println("");
        print("[");
        print_num(job_id as u64);
        print("]+ Stopped                 ");
        println(cmd_name);
        Ok(0)
    } else {
        println("Process terminated abnormally");
        Ok(-1)
    }
}

/// List all available external programs (for help command)
pub fn list_external_programs() {
    println("External programs:");
    for entry in PROGRAM_REGISTRY {
        print("  ");
        print(entry.name);
        // Pad to align descriptions
        let padding = 12 - entry.name.len();
        for _ in 0..padding {
            print(" ");
        }
        print("- ");
        println(entry.description);
    }
}

// ============================================================================
// Pipeline Support - Parse and execute cmd1 | cmd2 | cmd3
// ============================================================================

/// Maximum number of commands in a pipeline
const MAX_PIPELINE_COMMANDS: usize = 8;

/// A parsed command in a pipeline
#[derive(Clone, Copy)]
struct PipelineCommand<'a> {
    /// The command name (first word)
    name: &'a str,
    /// The full command string including arguments
    full: &'a str,
}

/// Check if the input contains a pipe character
fn contains_pipe(s: &str) -> bool {
    for c in s.as_bytes() {
        if *c == b'|' {
            return true;
        }
    }
    false
}

/// Trim leading and trailing whitespace from a string
fn trim(s: &str) -> &str {
    let bytes = s.as_bytes();
    let mut start = 0;
    let mut end = bytes.len();

    // Trim leading whitespace
    while start < end && (bytes[start] == b' ' || bytes[start] == b'\t') {
        start += 1;
    }

    // Trim trailing whitespace
    while end > start && (bytes[end - 1] == b' ' || bytes[end - 1] == b'\t' || bytes[end - 1] == 0)
    {
        end -= 1;
    }

    core::str::from_utf8(&bytes[start..end]).unwrap_or("")
}

// ============================================================================
// Background Process Support - cmd &
// ============================================================================

/// Check if a command should be run in the background (ends with &)
fn is_background_command(input: &str) -> bool {
    let trimmed = trim(input);
    trimmed.ends_with('&')
}

/// Strip the background operator from a command
fn strip_background_operator(input: &str) -> &str {
    let trimmed = trim(input);
    if trimmed.ends_with('&') {
        trim(&trimmed[..trimmed.len() - 1])
    } else {
        trimmed
    }
}

/// Print a background job notification
fn print_job_notification(job_id: u32, pid: i64) {
    print("[");
    print_num(job_id as u64);
    print("] ");
    print_num(pid as u64);
    println("");
}

/// Split a string by pipe character, returning up to MAX_PIPELINE_COMMANDS segments.
/// Each segment is trimmed of whitespace.
/// Returns the number of commands found.
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
                    // Extract command name (first word)
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

    // Handle the last segment after the final pipe (or entire string if no pipe)
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
/// This handles built-in commands that can participate in pipelines (like echo).
/// For external commands, it calls exec.
///
/// Search order for external commands:
/// 1. PROGRAM_REGISTRY (for backwards compatibility with test disk binaries)
/// 2. /bin/{cmd_name} (PATH-based lookup from ext2 filesystem)
///
/// This function never returns - it either execs or exits.
fn execute_pipeline_command(cmd: &PipelineCommand) -> ! {
    // Handle built-in commands that can participate in pipelines
    if cmd.name == "echo" {
        // Extract args after "echo "
        let args = if cmd.full.len() > 5 {
            trim(&cmd.full[5..])
        } else {
            ""
        };
        println(args);
        libbreenix::process::exit(0);
    }

    // Try external command from registry first
    if let Some(entry) = find_program(cmd.name) {
        let result = exec(entry.binary_name);
        // If exec returns, it failed
        print("exec failed: ");
        print_num((-result) as u64);
        println("");
        libbreenix::process::exit(1);
    }

    // Try /bin/{cmd_name} PATH lookup
    const PATH_BUF_LEN: usize = 64;
    let mut path_buf = [0u8; PATH_BUF_LEN];
    let prefix = b"/bin/";
    let cmd_bytes = cmd.name.as_bytes();

    if prefix.len() + cmd_bytes.len() + 1 <= PATH_BUF_LEN {
        path_buf[..prefix.len()].copy_from_slice(prefix);
        path_buf[prefix.len()..prefix.len() + cmd_bytes.len()].copy_from_slice(cmd_bytes);
        path_buf[prefix.len() + cmd_bytes.len()] = 0; // null terminate

        let path_len = prefix.len() + cmd_bytes.len() + 1;
        let result = exec(&path_buf[..path_len]);
        // If exec returns, it failed
        print("exec failed: ");
        print_num((-result) as u64);
        println("");
        libbreenix::process::exit(1);
    }

    // Command not found
    print("command not found: ");
    println(cmd.name);
    libbreenix::process::exit(127)
}

/// Execute a pipeline of commands.
///
/// For a pipeline like: cmd1 | cmd2 | cmd3
///
/// This creates:
/// - pipe1 between cmd1 and cmd2
/// - pipe2 between cmd2 and cmd3
///
/// Each command is forked as a child process:
/// - cmd1: stdout -> pipe1[write]
/// - cmd2: stdin <- pipe1[read], stdout -> pipe2[write]
/// - cmd3: stdin <- pipe2[read]
///
/// Returns true if pipeline was executed (even with errors), false if no valid commands.
fn execute_pipeline(input: &str) -> bool {
    let mut commands: [PipelineCommand; MAX_PIPELINE_COMMANDS] = [PipelineCommand {
        name: "",
        full: "",
    }; MAX_PIPELINE_COMMANDS];

    let cmd_count = split_pipeline(input, &mut commands);

    if cmd_count == 0 {
        return false;
    }

    // Single command - no pipeline needed, fall back to normal handling
    if cmd_count == 1 {
        return false;
    }

    // Verify all commands exist before starting pipeline
    for i in 0..cmd_count {
        let cmd = &commands[i];
        if cmd.name != "echo" && find_program(cmd.name).is_none() {
            print("command not found: ");
            println(cmd.name);
            return true; // Return true since we handled the error
        }
    }

    // Store PIDs of all child processes
    let mut child_pids: [i64; MAX_PIPELINE_COMMANDS] = [0; MAX_PIPELINE_COMMANDS];

    // Process group for the entire pipeline (set to first child's PID)
    let mut pipeline_pgrp: i32 = 0;

    // Store the read end of the previous pipe (-1 means no previous pipe)
    let mut prev_read_fd: i32 = -1;

    for i in 0..cmd_count {
        let is_last = i == cmd_count - 1;

        // Create pipe for this stage (except for the last command)
        let mut pipefd: [i32; 2] = [-1, -1];
        if !is_last {
            let ret = pipe(&mut pipefd);
            if ret < 0 {
                print("pipe failed: ");
                print_num((-ret) as u64);
                println("");
                // Clean up previous pipe if exists
                if prev_read_fd >= 0 {
                    close(prev_read_fd as u64);
                }
                return true;
            }
        }

        // Fork child for this command
        let pid = fork();

        if pid < 0 {
            print("fork failed: ");
            print_num((-pid) as u64);
            println("");
            // Clean up pipes
            if prev_read_fd >= 0 {
                close(prev_read_fd as u64);
            }
            if !is_last {
                close(pipefd[0] as u64);
                close(pipefd[1] as u64);
            }
            return true;
        }

        if pid == 0 {
            // ========== CHILD PROCESS ==========

            // Put all pipeline processes in the same process group.
            // First process creates the group (setpgid(0, 0)), subsequent
            // processes join it (setpgid(0, pgrp)).
            // Note: In child, pipeline_pgrp is 0 for first process (hasn't been set yet).
            // The child must call setpgid before any I/O to ensure proper signal handling.
            let _ = setpgid(0, pipeline_pgrp);

            // Set up stdin from previous pipe (if not first command)
            if prev_read_fd >= 0 {
                // Redirect stdin to read from previous pipe
                dup2(prev_read_fd as u64, STDIN);
                close(prev_read_fd as u64);
            }

            // Set up stdout to current pipe (if not last command)
            if !is_last {
                // Redirect stdout to write end of current pipe
                dup2(pipefd[1] as u64, STDOUT);
                close(pipefd[0] as u64); // Close read end in child
                close(pipefd[1] as u64); // Close write end after dup
            }

            // Execute the command (never returns)
            execute_pipeline_command(&commands[i]);
        }

        // ========== PARENT PROCESS ==========

        // Store child PID
        child_pids[i] = pid;

        // Set up process group for the pipeline
        // First child becomes the process group leader
        if i == 0 {
            pipeline_pgrp = pid as i32;
        }
        // Put child in the pipeline's process group (from parent side too, to avoid race)
        let _ = setpgid(pid as i32, pipeline_pgrp);

        // Close the previous read fd (child has it now)
        if prev_read_fd >= 0 {
            close(prev_read_fd as u64);
        }

        // Close write end of current pipe (child has it)
        // Save read end for next iteration
        if !is_last {
            close(pipefd[1] as u64);
            prev_read_fd = pipefd[0];
        }
    }

    // Give terminal to the pipeline's process group
    let shell_pgrp = getpgrp();
    if pipeline_pgrp > 0 {
        let _ = tcsetpgrp(STDIN as i32, pipeline_pgrp);
    }

    // Wait for all children to complete
    for i in 0..cmd_count {
        if child_pids[i] > 0 {
            let mut status: i32 = 0;
            waitpid(child_pids[i] as i32, &mut status as *mut i32, 0);
        }
    }

    // Take terminal back from pipeline
    let _ = tcsetpgrp(STDIN as i32, shell_pgrp);

    true
}

// Line buffer for reading input
static mut LINE_BUF: [u8; 256] = [0; 256];
static mut LINE_LEN: usize = 0;

// EAGAIN error code
const EAGAIN: i64 = 11;

// Global flag to track SIGINT received
static SIGINT_RECEIVED: AtomicBool = AtomicBool::new(false);

// Global flag to track SIGCHLD received (for background job status updates)
static SIGCHLD_RECEIVED: AtomicBool = AtomicBool::new(false);

// Saved termios for restoration
static mut SAVED_TERMIOS: Option<Termios> = None;

/// SIGINT handler - just sets a flag
extern "C" fn sigint_handler(_sig: i32) {
    SIGINT_RECEIVED.store(true, Ordering::SeqCst);
}

/// SIGCHLD handler - just sets a flag (async-signal-safe)
/// The actual status checking is done in check_children() from the main loop
extern "C" fn sigchld_handler(_sig: i32) {
    SIGCHLD_RECEIVED.store(true, Ordering::SeqCst);
}

/// Check for completed or stopped children (non-blocking)
///
/// This is called from the main loop when SIGCHLD is received.
/// It uses WNOHANG to avoid blocking and updates job status accordingly.
fn check_children() {
    // Only check if we received SIGCHLD
    if !SIGCHLD_RECEIVED.swap(false, Ordering::SeqCst) {
        return;
    }

    // Non-blocking wait for any child that has changed state
    loop {
        let mut status: i32 = 0;
        let pid = waitpid(-1, &mut status as *mut i32, WNOHANG);

        if pid <= 0 {
            // No more children have changed state, or error
            break;
        }

        // Update job status based on wait status
        if wifexited(status) || wifsignaled(status) {
            // Child terminated (exited or killed by signal)
            update_job_status(pid as i32, JobStatus::Done);
        } else if wifstopped(status) {
            // Child was stopped (e.g., by SIGTSTP)
            update_job_status(pid as i32, JobStatus::Stopped);
        }
    }
}

/// Report jobs that have completed and remove them from the job table
///
/// This is called before printing the prompt to notify the user
/// of any background jobs that have finished.
fn report_done_jobs() {
    // Collect done jobs to report and remove
    // We need to do this in two passes to avoid borrowing issues
    let mut to_remove: [u32; MAX_JOBS] = [0; MAX_JOBS];
    let mut remove_count = 0;

    for job in job_table().iter() {
        if job.status == JobStatus::Done {
            // Print completion notification
            print("[");
            print_num(job.id as u64);
            print("]+  Done                    ");
            println(job.command_str());

            // Mark for removal
            if remove_count < MAX_JOBS {
                to_remove[remove_count] = job.id;
                remove_count += 1;
            }
        }
    }

    // Remove done jobs from the table
    for i in 0..remove_count {
        job_table().remove(to_remove[i]);
    }
}

/// Print a single character
fn print_char(c: u8) {
    let _ = write(STDOUT, &[c]);
}

/// Print a number (u64)
fn print_num(mut n: u64) {
    if n == 0 {
        print("0");
        return;
    }

    let mut buf = [0u8; 20];
    let mut i = 0;

    while n > 0 {
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i += 1;
    }

    // Print in reverse order
    while i > 0 {
        i -= 1;
        print_char(buf[i]);
    }
}

/// Print a signed number (i32)
fn print_i32(n: i32) {
    if n < 0 {
        print("-");
        print_num((-n) as u64);
    } else {
        print_num(n as u64);
    }
}

/// Read a line from stdin, handling backspace and yielding on EAGAIN
/// Returns None if interrupted by SIGINT (Ctrl+C)
fn read_line() -> Option<&'static str> {
    unsafe {
        LINE_LEN = 0;

        loop {
            // Check for SIGINT (Ctrl+C)
            if SIGINT_RECEIVED.swap(false, Ordering::SeqCst) {
                print("^C\n");
                LINE_LEN = 0;
                return None; // Signal that we got interrupted
            }

            let mut c = [0u8; 1];
            let n = read(STDIN, &mut c);

            if n == -EAGAIN || n == 0 {
                // No data available - yield and retry
                yield_now();
                continue;
            }

            if n < 0 {
                // Other error (possibly EINTR from signal) - yield and retry
                yield_now();
                continue;
            }

            let ch = c[0];

            // Handle newline - end of input
            if ch == b'\n' || ch == b'\r' {
                println("");
                LINE_BUF[LINE_LEN] = 0;
                return Some(core::str::from_utf8(&LINE_BUF[..LINE_LEN]).unwrap_or(""));
            }

            // Handle backspace (ASCII DEL or BS)
            if ch == 0x7f || ch == 0x08 {
                if LINE_LEN > 0 {
                    LINE_LEN -= 1;
                    // Move cursor back, print space, move back again
                    print("\x08 \x08");
                }
                continue;
            }

            // Handle printable characters
            if LINE_LEN < 255 && ch >= 0x20 && ch < 0x7f {
                LINE_BUF[LINE_LEN] = ch;
                LINE_LEN += 1;
                // Note: Kernel echoes characters in push_byte, so no shell echo needed
            }
        }
    }
}

/// Handle the "help" command
fn cmd_help() {
    println("Built-in commands:");
    println("  help   - Show this help message");
    println("  echo   - Echo text back to the terminal");
    println("  cd     - Change current directory (cd /path)");
    println("  pwd    - Print current working directory");
    println("  ps     - List processes (placeholder)");
    println("  jobs   - List background and stopped jobs");
    println("  bg     - Resume stopped job in background (bg %N)");
    println("  fg     - Bring job to foreground (fg %N)");
    println("  uptime - Show time since boot");
    println("  clear  - Clear the screen (ANSI escape sequence)");
    println("  raw    - Switch to raw mode and show keypresses");
    println("  cooked - Switch back to canonical (cooked) mode");
    println("  devtest- Test device files (/dev/null, /dev/zero, etc.)");
    println("  progs  - List available external programs");
    println("  exit   - Attempt to exit (init cannot exit)");
    println("");
    list_external_programs();
    println("");
    println("Pipes:");
    println("  cmd1 | cmd2      - Pipe output of cmd1 to input of cmd2");
    println("  echo hello | cat - Example: pipe echo output (not yet useful)");
    println("");
    println("Background execution:");
    println("  command &        - Run command in background");
    println("  hello &          - Example: run hello_world in background");
    println("");
    println("Job control:");
    println("  fg [%N]          - Bring job N to foreground");
    println("  bg [%N]          - Resume stopped job N in background");
    println("  jobs             - List all jobs with their status");
    println("");
    println("TTY testing:");
    println("  - Ctrl+C shows ^C and gives new prompt");
    println("  - Backspace works for line editing");
    println("  - In raw mode, each keypress is shown immediately");
}

/// Handle the "echo" command
fn cmd_echo(args: &str) {
    println(args);
}

/// Handle the "ps" command
fn cmd_ps() {
    println("  PID  CMD");
    println("    1  init");
}

/// Handle the "uptime" command
fn cmd_uptime() {
    let ts: Timespec = now_monotonic();
    let secs = ts.tv_sec as u64;
    let mins = secs / 60;
    let hours = mins / 60;

    print("up ");

    if hours > 0 {
        print_num(hours);
        print(" hour");
        if hours != 1 {
            print("s");
        }
        print(", ");
    }

    if mins > 0 || hours > 0 {
        print_num(mins % 60);
        print(" minute");
        if mins % 60 != 1 {
            print("s");
        }
        print(", ");
    }

    print_num(secs % 60);
    print(" second");
    if secs % 60 != 1 {
        print("s");
    }
    println("");
}

/// Handle the "cd" command - change current directory
fn cmd_cd(args: &str) {
    // If no argument, go to root
    let path = if args.is_empty() { "/" } else { args.trim() };

    // Build null-terminated path
    let mut path_buf = [0u8; 256];
    let path_bytes = path.as_bytes();
    if path_bytes.len() >= path_buf.len() {
        println("cd: path too long");
        return;
    }
    path_buf[..path_bytes.len()].copy_from_slice(path_bytes);
    path_buf[path_bytes.len()] = 0; // null terminator

    let result = chdir(&path_buf[..=path_bytes.len()]);
    if result < 0 {
        print("cd: ");
        print(path);
        match -result {
            2 => println(": No such file or directory"),
            20 => println(": Not a directory"),
            13 => println(": Permission denied"),
            _ => {
                print(": error ");
                print_i32(-result);
                println("");
            }
        }
    }
}

/// Handle the "pwd" command - print working directory
fn cmd_pwd() {
    let mut buf = [0u8; 256];
    let result = getcwd(&mut buf);
    if result > 0 {
        // Find the null terminator and print the path
        let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        if let Ok(path) = core::str::from_utf8(&buf[..len]) {
            println(path);
        } else {
            println("pwd: invalid path encoding");
        }
    } else {
        println("pwd: cannot get current directory");
    }
}

/// Handle the "clear" command
fn cmd_clear() {
    // Use ANSI escape sequences to clear screen and move cursor to home
    // ESC[2J - Clear entire screen
    // ESC[H  - Move cursor to home position (1,1)
    print("\x1b[2J\x1b[H");
}

/// Handle the "exit" command
fn cmd_exit() {
    println("Cannot exit init!");
    println("The init process must run forever.");
}

/// Print a byte in hexadecimal
fn print_hex_byte(b: u8) {
    let high = b >> 4;
    let low = b & 0x0F;
    let high_char = if high < 10 {
        b'0' + high
    } else {
        b'a' + (high - 10)
    };
    let low_char = if low < 10 { b'0' + low } else { b'a' + (low - 10) };
    print_char(high_char);
    print_char(low_char);
}

/// Handle the "raw" command - switch to raw mode and show keypresses
fn cmd_raw() {
    println("Switching to raw mode...");
    println("Press keys to see their codes. Press 'q' to exit raw mode.");
    println("");

    // Get current terminal settings
    let mut termios = Termios::default();
    if tcgetattr(0, &mut termios).is_err() {
        println("Error: Could not get terminal attributes");
        return;
    }

    // Save original settings for restoration
    let original_termios = termios;
    unsafe {
        SAVED_TERMIOS = Some(original_termios);
    }

    // Switch to raw mode
    cfmakeraw(&mut termios);

    // Keep output processing enabled so newlines work correctly
    termios.c_oflag |= oflag::OPOST | oflag::ONLCR;

    if tcsetattr(0, TCSANOW, &termios).is_err() {
        println("Error: Could not set raw mode");
        return;
    }

    println("Raw mode enabled. Type keys:");
    println("");

    // Read and display keypresses until 'q' is pressed
    loop {
        let mut c = [0u8; 1];
        let n = read(STDIN, &mut c);

        if n == -EAGAIN || n == 0 {
            yield_now();
            continue;
        }

        if n < 0 {
            yield_now();
            continue;
        }

        let ch = c[0];

        // Display the keypress
        print("Key: ");

        // Show printable representation or control code name
        if ch >= 0x20 && ch < 0x7f {
            print("'");
            print_char(ch);
            print("' ");
        } else if ch == 0x1b {
            print("ESC ");
        } else if ch < 0x20 {
            print("^");
            print_char(b'@' + ch);
            print(" ");
        } else if ch == 0x7f {
            print("DEL ");
        } else {
            print("    ");
        }

        print("(0x");
        print_hex_byte(ch);
        print(", ");
        print_num(ch as u64);
        println(")");

        // Exit on 'q'
        if ch == b'q' || ch == b'Q' {
            println("");
            println("Exiting raw mode...");
            break;
        }
    }

    // Restore original terminal settings
    if tcsetattr(0, TCSANOW, &original_termios).is_err() {
        println("Warning: Could not restore terminal settings");
    }

    println("Back to canonical mode.");
}

/// Handle the "cooked" command - switch back to canonical mode
fn cmd_cooked() {
    println("Switching to canonical (cooked) mode...");

    // Check if we have saved settings
    let restored = unsafe {
        if let Some(ref original) = SAVED_TERMIOS {
            if tcsetattr(0, TCSANOW, original).is_ok() {
                true
            } else {
                false
            }
        } else {
            // No saved settings, set up default canonical mode
            let mut termios = Termios::default();
            if tcgetattr(0, &mut termios).is_err() {
                println("Error: Could not get terminal attributes");
                return;
            }

            // Enable canonical mode, echo, and signals
            termios.c_lflag |= lflag::ICANON | lflag::ECHO | lflag::ECHOE | lflag::ISIG;
            termios.c_oflag |= oflag::OPOST | oflag::ONLCR;

            if tcsetattr(0, TCSANOW, &termios).is_ok() {
                true
            } else {
                false
            }
        }
    };

    if restored {
        println("Canonical mode enabled.");
        println("Line editing and signals are now active.");
    } else {
        println("Error: Could not set canonical mode");
    }
}

/// Handle the "devtest" command - test device files interactively
fn cmd_devtest() {
    println("Testing device files...");
    println("");

    // Test 1: Write to /dev/null
    print("1. /dev/null write: ");
    match open("/dev/null\0", O_WRONLY) {
        Ok(fd) => {
            let data = b"test data";
            let result = write(fd, data);
            close(fd);
            if result == data.len() as i64 {
                println("OK (data discarded)");
            } else {
                print("FAIL (returned ");
                print_num(result as u64);
                println(")");
            }
        }
        Err(e) => {
            print("FAIL (open error ");
            print_num(e as u64);
            println(")");
        }
    }

    // Test 2: Read from /dev/null (should return EOF)
    print("2. /dev/null read:  ");
    match open("/dev/null\0", O_RDONLY) {
        Ok(fd) => {
            let mut buf = [0u8; 16];
            let result = read(fd, &mut buf);
            close(fd);
            if result == 0 {
                println("OK (EOF as expected)");
            } else {
                print("FAIL (returned ");
                print_num(result as u64);
                println(")");
            }
        }
        Err(e) => {
            print("FAIL (open error ");
            print_num(e as u64);
            println(")");
        }
    }

    // Test 3: Read from /dev/zero (should return zeros)
    print("3. /dev/zero read:  ");
    match open("/dev/zero\0", O_RDONLY) {
        Ok(fd) => {
            let mut buf = [0xFFu8; 8];
            let result = read(fd, &mut buf);
            close(fd);
            if result == 8 && buf.iter().all(|&b| b == 0) {
                println("OK (8 zeros read)");
            } else {
                print("FAIL (");
                print_num(result as u64);
                println(" bytes, or non-zero data)");
            }
        }
        Err(e) => {
            print("FAIL (open error ");
            print_num(e as u64);
            println(")");
        }
    }

    // Test 4: Write to /dev/console
    print("4. /dev/console:    ");
    match open("/dev/console\0", O_WRONLY) {
        Ok(fd) => {
            let msg = b"[console test] ";
            let result = write(fd, msg);
            close(fd);
            if result == msg.len() as i64 {
                println("OK");
            } else {
                print("FAIL (returned ");
                print_num(result as u64);
                println(")");
            }
        }
        Err(e) => {
            print("FAIL (open error ");
            print_num(e as u64);
            println(")");
        }
    }

    // Test 5: List /dev directory
    print("5. ls /dev:         ");
    match open("/dev\0", O_DIRECTORY | O_RDONLY) {
        Ok(fd) => {
            close(fd);
            println("OK (directory opened)");
            // Actually list the contents
            println("   Contents: null zero console tty");
        }
        Err(e) => {
            print("FAIL (open error ");
            print_num(e as u64);
            println(")");
        }
    }

    println("");
    println("Device tests complete!");
}

/// Handle an unknown command
fn cmd_unknown(cmd: &str) {
    print("Unknown command: ");
    println(cmd);
    println("Type 'help' for available commands.");
}

/// Parse and execute a command line
fn handle_command(line: &str) {
    let line = trim(line);

    if line.is_empty() {
        return;
    }

    // Check for background operator (&) first
    let background = is_background_command(line);
    let line = if background {
        strip_background_operator(line)
    } else {
        line
    };

    // Check for pipeline (contains '|' with 2+ commands)
    // Note: Background pipelines are not yet supported
    if contains_pipe(line) {
        if background {
            println("Background pipelines not yet supported");
            return;
        }
        if execute_pipeline(line) {
            return;
        }
        // Fall through to normal handling if pipeline fails validation
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

    // Built-in commands cannot run in background
    if background {
        // Only external commands can run in background
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
            // Try to execute as an external command
            if try_execute_external(cmd, args, false).is_err() {
                cmd_unknown(cmd);
            }
        }
    }
}

/// Print the welcome banner
fn print_banner() {
    println("");
    println("========================================");
    println("     Breenix OS Interactive Shell");
    println("========================================");
    println("");
    println("Welcome to Breenix! Type 'help' for available commands.");
    println("");
}

/// Set up signal handlers
fn setup_signal_handlers() {
    // Set up SIGINT handler for Ctrl+C
    let sigint_action = Sigaction::new(sigint_handler);
    if sigaction(SIGINT, Some(&sigint_action), None).is_err() {
        println("Warning: Could not set up SIGINT handler");
    }

    // Set up SIGCHLD handler for background job status updates
    let sigchld_action = Sigaction::new(sigchld_handler);
    if sigaction(SIGCHLD, Some(&sigchld_action), None).is_err() {
        println("Warning: Could not set up SIGCHLD handler");
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Set up signal handlers before anything else
    setup_signal_handlers();

    // Set this shell as the foreground process group for the TTY
    // This ensures Ctrl+C signals are delivered to us
    let pgrp = getpgrp();
    if tcsetpgrp(0, pgrp).is_err() {
        println("Warning: Could not set foreground process group");
    }

    print_banner();

    // Main REPL loop
    loop {
        // Check for completed background jobs and report them
        check_children();
        report_done_jobs();

        print("breenix> ");

        // read_line returns None if interrupted by Ctrl+C
        if let Some(line) = read_line() {
            handle_command(line);
        }
        // If None (interrupted), just continue to print new prompt
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    println("PANIC in init shell!");
    // Init cannot exit, so just loop forever
    loop {
        core::hint::spin_loop();
    }
}
