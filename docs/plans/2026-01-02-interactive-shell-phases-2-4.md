# Interactive Shell Phases 2-4 Implementation Plan

**Goal:** Complete the interactive shell with raw mode, external commands, piping, and job control.

**Architecture:** Build on existing TTY/line discipline infrastructure. Add raw mode support to termios, implement external command execution via fork+exec, add shell piping using existing pipe syscalls, and implement POSIX job control with process groups and sessions.

**Tech Stack:** Rust (kernel + libbreenix), x86_64 assembly for signal trampolines, POSIX-compatible APIs.

---

## Current State

**Phase 2 (Line Discipline): 85% complete**
- ✅ TTY device abstraction
- ✅ Canonical mode (line buffering)
- ✅ Echo handling
- ✅ Erase character (backspace)
- ✅ POSIX termios
- ✅ Signal generation (Ctrl+C, Ctrl+\, Ctrl+Z)
- ✅ ioctl syscall
- ❌ **Raw mode toggle (ICANON flag)**

**Phase 3 (Shell Integration): 60% complete**
- ✅ /init binary with libbreenix
- ✅ Read-eval-print loop
- ✅ Command parsing
- ✅ Built-in commands (help, echo, ps, exit, clear, time)
- ✅ Line editing with backspace
- ✅ Ctrl+C interrupt handling
- ✅ ANSI escape sequence parser
- ❌ **Program table (command → ELF mapping)**
- ❌ **fork() + exec() for external commands**
- ❌ **Basic pipe support (cmd1 | cmd2)**

**Phase 4 (Job Control): 20% complete**
- ✅ Signal delivery (Ctrl+C, Ctrl+Z)
- ❌ **Background process execution (&)**
- ❌ **fg/bg commands**
- ❌ **Process groups and session management**
- ❌ **Job status tracking**

---

## Task 1: Raw Mode Toggle (ICANON flag)

**Files:**
- Modify: `kernel/src/tty/line_discipline.rs`
- Modify: `kernel/src/tty/driver.rs`
- Modify: `libs/libbreenix/src/termios.rs`
- Test: `userspace/shell_test` or manual testing

**Step 1: Add ICANON flag check to line discipline**

The line discipline currently always buffers input until newline. Add a check for the ICANON flag in termios to bypass line buffering when disabled.

```rust
// In line_discipline.rs, modify process_input()
pub fn process_input(&mut self, c: u8, echo_fn: impl Fn(u8)) {
    // Check if canonical mode is enabled
    if !self.termios.is_canonical() {
        // Raw mode: pass character directly without buffering
        self.output_buffer.push(c);
        if self.termios.echo_enabled() {
            echo_fn(c);
        }
        return;
    }

    // Existing canonical mode processing...
}
```

**Step 2: Add is_canonical() helper to Termios**

```rust
// In termios struct
pub fn is_canonical(&self) -> bool {
    self.c_lflag & ICANON != 0
}

pub fn set_canonical(&mut self, enable: bool) {
    if enable {
        self.c_lflag |= ICANON;
    } else {
        self.c_lflag &= !ICANON;
    }
}
```

**Step 3: Add cfmakeraw() to libbreenix**

```rust
// libs/libbreenix/src/termios.rs
pub fn cfmakeraw(termios: &mut Termios) {
    termios.c_iflag &= !(IGNBRK | BRKINT | PARMRK | ISTRIP | INLCR | IGNCR | ICRNL | IXON);
    termios.c_oflag &= !OPOST;
    termios.c_lflag &= !(ECHO | ECHONL | ICANON | ISIG | IEXTEN);
    termios.c_cflag &= !(CSIZE | PARENB);
    termios.c_cflag |= CS8;
    termios.c_cc[VMIN] = 1;
    termios.c_cc[VTIME] = 0;
}
```

**Step 4: Test raw mode**

Create a simple test that:
1. Gets current termios with tcgetattr
2. Calls cfmakeraw
3. Sets termios with tcsetattr
4. Reads single characters without waiting for Enter
5. Restores original termios

**Commit:** `feat(tty): implement raw mode (ICANON flag toggle)`

---

## Task 2: Program Table for External Commands

**Files:**
- Create: `userspace/programs/mod.rs`
- Modify: `userspace/init_shell/src/main.rs`
- Modify: `kernel/src/process/loader.rs` (if needed)

**Step 1: Define program table structure**

```rust
// userspace/programs/mod.rs
pub struct ProgramEntry {
    pub name: &'static str,
    pub elf_data: &'static [u8],
}

pub static PROGRAMS: &[ProgramEntry] = &[
    ProgramEntry { name: "hello", elf_data: include_bytes!("../../target/x86_64-breenix/release/hello") },
    ProgramEntry { name: "cat", elf_data: include_bytes!("../../target/x86_64-breenix/release/cat") },
    // Add more programs as they're built
];

pub fn find_program(name: &str) -> Option<&'static [u8]> {
    PROGRAMS.iter()
        .find(|p| p.name == name)
        .map(|p| p.elf_data)
}
```

**Step 2: Create simple external programs**

Start with a minimal `hello` program:
```rust
// userspace/hello/src/main.rs
#![no_std]
#![no_main]

use libbreenix::{io::println, process::exit};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    println("Hello from external program!");
    exit(0);
}
```

**Step 3: Modify shell to lookup external commands**

```rust
// In shell command dispatch
fn execute_command(cmd: &str, args: &[&str]) {
    match cmd {
        "help" => builtin_help(),
        "echo" => builtin_echo(args),
        // ... other builtins
        _ => {
            // Try external command
            if let Some(elf_data) = programs::find_program(cmd) {
                execute_external(elf_data, args);
            } else {
                println!("{}: command not found", cmd);
            }
        }
    }
}
```

**Commit:** `feat(shell): add program table for external command lookup`

---

## Task 3: Fork + Exec for External Commands

**Files:**
- Modify: `userspace/init_shell/src/main.rs`
- Modify: `libs/libbreenix/src/process.rs` (add exec wrapper if missing)
- Test: Run external `hello` command

**Step 1: Implement execute_external() in shell**

```rust
fn execute_external(elf_data: &[u8], args: &[&str]) {
    let pid = fork();

    if pid == 0 {
        // Child process: exec the program
        // For now, we'll use a syscall that loads ELF from memory
        exec_from_memory(elf_data, args);
        // If exec fails, exit with error
        exit(127);
    } else if pid > 0 {
        // Parent process: wait for child
        let mut status: i32 = 0;
        waitpid(pid, &mut status, 0);

        if let Some(exit_code) = wexitstatus(status) {
            if exit_code != 0 {
                println!("Process exited with code {}", exit_code);
            }
        }
    } else {
        println!("fork failed");
    }
}
```

**Step 2: Add exec_from_memory syscall if not present**

Check if we have a way to exec an ELF from memory. If not, add:
- Syscall that takes pointer to ELF data + length
- Kernel loads ELF into process address space
- Jumps to entry point

**Step 3: Test external command execution**

```
breenix> hello
Hello from external program!
breenix>
```

**Commit:** `feat(shell): implement fork+exec for external commands`

---

## Task 4: Basic Pipe Support (cmd1 | cmd2)

**Files:**
- Modify: `userspace/init_shell/src/main.rs` (command parsing)
- Modify: `userspace/init_shell/src/main.rs` (pipe execution)
- Use: existing `pipe()`, `dup2()`, `close()` syscalls

**Step 1: Add pipe detection to command parser**

```rust
fn parse_pipeline(input: &str) -> Vec<Vec<&str>> {
    input.split('|')
        .map(|cmd| cmd.trim().split_whitespace().collect())
        .collect()
}
```

**Step 2: Implement pipeline execution**

```rust
fn execute_pipeline(commands: Vec<Vec<&str>>) {
    if commands.len() == 1 {
        // Single command, no piping needed
        execute_command(&commands[0]);
        return;
    }

    let mut prev_read_fd: Option<i32> = None;

    for (i, cmd) in commands.iter().enumerate() {
        let is_last = i == commands.len() - 1;

        // Create pipe for all but last command
        let (read_fd, write_fd) = if !is_last {
            let mut fds = [0i32; 2];
            pipe(&mut fds);
            (Some(fds[0]), Some(fds[1]))
        } else {
            (None, None)
        };

        let pid = fork();

        if pid == 0 {
            // Child process

            // Connect stdin to previous pipe's read end
            if let Some(fd) = prev_read_fd {
                dup2(fd, 0); // stdin
                close(fd);
            }

            // Connect stdout to current pipe's write end
            if let Some(fd) = write_fd {
                dup2(fd, 1); // stdout
                close(fd);
            }

            // Close unused read end in child
            if let Some(fd) = read_fd {
                close(fd);
            }

            // Execute command
            execute_single_command(cmd);
            exit(0);
        }

        // Parent: close write end (child has it)
        if let Some(fd) = write_fd {
            close(fd);
        }

        // Close previous read end (we've passed it to child)
        if let Some(fd) = prev_read_fd {
            close(fd);
        }

        // Save read end for next iteration
        prev_read_fd = read_fd;
    }

    // Wait for all children
    for _ in &commands {
        let mut status = 0;
        wait(&mut status);
    }
}
```

**Step 3: Test piping**

```
breenix> echo hello | cat
hello
breenix>
```

**Commit:** `feat(shell): implement basic pipe support`

---

## Task 5: Background Process Execution (&)

**Files:**
- Modify: `userspace/init_shell/src/main.rs` (command parsing)
- Modify: `userspace/init_shell/src/main.rs` (background execution)

**Step 1: Detect background operator in parser**

```rust
fn parse_command(input: &str) -> (Vec<&str>, bool) {
    let trimmed = input.trim();
    let background = trimmed.ends_with('&');
    let cmd_str = if background {
        trimmed.trim_end_matches('&').trim()
    } else {
        trimmed
    };

    (cmd_str.split_whitespace().collect(), background)
}
```

**Step 2: Execute without waiting for background jobs**

```rust
fn execute_with_background(cmd: &[&str], background: bool) {
    let pid = fork();

    if pid == 0 {
        // Child
        execute_command(cmd);
        exit(0);
    } else if pid > 0 {
        if background {
            // Don't wait, add to job list
            add_job(pid, cmd);
            println!("[{}] {}", job_id, pid);
        } else {
            // Foreground: wait for completion
            let mut status = 0;
            waitpid(pid, &mut status, 0);
        }
    }
}
```

**Commit:** `feat(shell): implement background process execution (&)`

---

## Task 6: Job Tracking Data Structure

**Files:**
- Create: `userspace/init_shell/src/jobs.rs`
- Modify: `userspace/init_shell/src/main.rs`

**Step 1: Define job structure**

```rust
// jobs.rs
#[derive(Clone, Copy, PartialEq)]
pub enum JobStatus {
    Running,
    Stopped,
    Done,
}

pub struct Job {
    pub id: u32,
    pub pid: i32,
    pub pgid: i32,
    pub status: JobStatus,
    pub command: [u8; 256], // Command string
    pub command_len: usize,
}

pub struct JobTable {
    jobs: [Option<Job>; 32],
    next_id: u32,
}

impl JobTable {
    pub const fn new() -> Self {
        JobTable {
            jobs: [None; 32],
            next_id: 1,
        }
    }

    pub fn add(&mut self, pid: i32, pgid: i32, command: &str) -> u32 {
        let id = self.next_id;
        self.next_id += 1;

        for slot in &mut self.jobs {
            if slot.is_none() {
                let mut cmd_buf = [0u8; 256];
                let len = command.len().min(255);
                cmd_buf[..len].copy_from_slice(&command.as_bytes()[..len]);

                *slot = Some(Job {
                    id,
                    pid,
                    pgid,
                    status: JobStatus::Running,
                    command: cmd_buf,
                    command_len: len,
                });
                return id;
            }
        }
        0 // Table full
    }

    pub fn get_by_id(&self, id: u32) -> Option<&Job> {
        self.jobs.iter()
            .filter_map(|j| j.as_ref())
            .find(|j| j.id == id)
    }

    pub fn update_status(&mut self, pid: i32, status: JobStatus) {
        for slot in &mut self.jobs {
            if let Some(job) = slot {
                if job.pid == pid {
                    job.status = status;
                    return;
                }
            }
        }
    }

    pub fn remove(&mut self, id: u32) {
        for slot in &mut self.jobs {
            if let Some(job) = slot {
                if job.id == id {
                    *slot = None;
                    return;
                }
            }
        }
    }

    pub fn list(&self) -> impl Iterator<Item = &Job> {
        self.jobs.iter().filter_map(|j| j.as_ref())
    }
}
```

**Step 2: Integrate job table with shell**

```rust
static mut JOB_TABLE: JobTable = JobTable::new();

fn add_job(pid: i32, command: &str) -> u32 {
    unsafe { JOB_TABLE.add(pid, pid, command) }
}
```

**Commit:** `feat(shell): add job tracking data structure`

---

## Task 7: Process Groups (setpgid, getpgid)

**Files:**
- Modify: `kernel/src/syscall/mod.rs` (add syscall numbers)
- Create: `kernel/src/syscall/pgrp.rs`
- Modify: `kernel/src/process/mod.rs` (add pgid field)
- Modify: `libs/libbreenix/src/process.rs` (add wrappers)

**Step 1: Add pgid field to Process**

```rust
// In Process struct
pub struct Process {
    pub pid: Pid,
    pub pgid: Pid,  // Process group ID
    pub sid: Pid,   // Session ID
    // ... existing fields
}
```

**Step 2: Implement setpgid syscall**

```rust
// kernel/src/syscall/pgrp.rs
pub fn sys_setpgid(pid: i32, pgid: i32) -> i64 {
    let target_pid = if pid == 0 {
        current_pid()
    } else {
        Pid::new(pid as u64)
    };

    let new_pgid = if pgid == 0 {
        target_pid
    } else {
        Pid::new(pgid as u64)
    };

    // Validate: can only set pgid for self or children
    // ... validation logic

    if let Some(proc) = get_process_mut(target_pid) {
        proc.pgid = new_pgid;
        0
    } else {
        -ESRCH
    }
}

pub fn sys_getpgid(pid: i32) -> i64 {
    let target_pid = if pid == 0 {
        current_pid()
    } else {
        Pid::new(pid as u64)
    };

    if let Some(proc) = get_process(target_pid) {
        proc.pgid.as_u64() as i64
    } else {
        -ESRCH
    }
}
```

**Step 3: Add libbreenix wrappers**

```rust
// libs/libbreenix/src/process.rs
pub fn setpgid(pid: i32, pgid: i32) -> i32 {
    unsafe { syscall2(SYS_SETPGID, pid as u64, pgid as u64) as i32 }
}

pub fn getpgid(pid: i32) -> i32 {
    unsafe { syscall1(SYS_GETPGID, pid as u64) as i32 }
}

pub fn setpgrp() -> i32 {
    setpgid(0, 0)
}

pub fn getpgrp() -> i32 {
    getpgid(0)
}
```

**Commit:** `feat(process): implement setpgid/getpgid syscalls`

---

## Task 8: Session Management (setsid, getsid)

**Files:**
- Modify: `kernel/src/syscall/pgrp.rs`
- Modify: `libs/libbreenix/src/process.rs`

**Step 1: Implement setsid syscall**

```rust
pub fn sys_setsid() -> i64 {
    let current = current_process();

    // Cannot create new session if already a process group leader
    if current.pid == current.pgid {
        return -EPERM;
    }

    // Create new session: pid becomes sid and pgid
    current.sid = current.pid;
    current.pgid = current.pid;

    // Detach from controlling terminal
    current.controlling_tty = None;

    current.pid.as_u64() as i64
}

pub fn sys_getsid(pid: i32) -> i64 {
    let target_pid = if pid == 0 {
        current_pid()
    } else {
        Pid::new(pid as u64)
    };

    if let Some(proc) = get_process(target_pid) {
        proc.sid.as_u64() as i64
    } else {
        -ESRCH
    }
}
```

**Step 2: Add libbreenix wrappers**

```rust
pub fn setsid() -> i32 {
    unsafe { syscall0(SYS_SETSID) as i32 }
}

pub fn getsid(pid: i32) -> i32 {
    unsafe { syscall1(SYS_GETSID, pid as u64) as i32 }
}
```

**Commit:** `feat(process): implement setsid/getsid syscalls`

---

## Task 9: Shell Job Control - fg Command

**Files:**
- Modify: `userspace/init_shell/src/main.rs`
- Modify: `userspace/init_shell/src/jobs.rs`

**Step 1: Implement fg builtin**

```rust
fn builtin_fg(args: &[&str]) {
    let job_id = if args.is_empty() {
        // Default to most recent job
        get_current_job_id()
    } else {
        // Parse %N or N
        parse_job_spec(args[0])
    };

    if let Some(job) = get_job(job_id) {
        // Put job in foreground
        tcsetpgrp(0, job.pgid); // Give terminal to job

        // If stopped, send SIGCONT
        if job.status == JobStatus::Stopped {
            kill(-job.pgid, SIGCONT);
            update_job_status(job_id, JobStatus::Running);
        }

        // Wait for job
        let mut status = 0;
        waitpid(-job.pgid, &mut status, WUNTRACED);

        // Take terminal back
        tcsetpgrp(0, getpgrp());

        // Update job status based on wait result
        if WIFSTOPPED(status) {
            update_job_status(job_id, JobStatus::Stopped);
            println!("[{}]+ Stopped    {}", job_id, job.command_str());
        } else {
            remove_job(job_id);
        }
    } else {
        println!("fg: no such job");
    }
}
```

**Commit:** `feat(shell): implement fg builtin command`

---

## Task 10: Shell Job Control - bg Command

**Files:**
- Modify: `userspace/init_shell/src/main.rs`

**Step 1: Implement bg builtin**

```rust
fn builtin_bg(args: &[&str]) {
    let job_id = if args.is_empty() {
        get_current_job_id()
    } else {
        parse_job_spec(args[0])
    };

    if let Some(job) = get_job(job_id) {
        if job.status != JobStatus::Stopped {
            println!("bg: job {} not stopped", job_id);
            return;
        }

        // Send SIGCONT to resume in background
        kill(-job.pgid, SIGCONT);
        update_job_status(job_id, JobStatus::Running);
        println!("[{}]+ {} &", job_id, job.command_str());
    } else {
        println!("bg: no such job");
    }
}
```

**Commit:** `feat(shell): implement bg builtin command`

---

## Task 11: Shell Job Control - jobs Command

**Files:**
- Modify: `userspace/init_shell/src/main.rs`

**Step 1: Implement jobs builtin**

```rust
fn builtin_jobs(_args: &[&str]) {
    for job in list_jobs() {
        let status_str = match job.status {
            JobStatus::Running => "Running",
            JobStatus::Stopped => "Stopped",
            JobStatus::Done => "Done",
        };

        let current_marker = if job.id == get_current_job_id() { "+" } else { "-" };

        println!("[{}]{} {}    {}", job.id, current_marker, status_str, job.command_str());
    }
}
```

**Commit:** `feat(shell): implement jobs builtin command`

---

## Task 12: SIGCHLD Handler for Job Status Updates

**Files:**
- Modify: `userspace/init_shell/src/main.rs`
- Modify: `userspace/init_shell/src/jobs.rs`

**Step 1: Install SIGCHLD handler**

```rust
fn sigchld_handler(_sig: i32) {
    // Non-blocking wait for any child
    loop {
        let mut status = 0;
        let pid = waitpid(-1, &mut status, WNOHANG | WUNTRACED | WCONTINUED);

        if pid <= 0 {
            break;
        }

        // Update job status
        if WIFEXITED(status) || WIFSIGNALED(status) {
            mark_job_done(pid);
        } else if WIFSTOPPED(status) {
            mark_job_stopped(pid);
        } else if WIFCONTINUED(status) {
            mark_job_running(pid);
        }
    }
}

fn setup_signal_handlers() {
    let sa = Sigaction {
        handler: sigchld_handler as usize,
        flags: SA_RESTART | SA_NOCLDSTOP,
        mask: 0,
    };
    sigaction(SIGCHLD, &sa, None);
}
```

**Step 2: Report completed jobs at prompt**

```rust
fn check_and_report_jobs() {
    for job in list_jobs() {
        if job.status == JobStatus::Done {
            println!("[{}]+ Done    {}", job.id, job.command_str());
            remove_job(job.id);
        }
    }
}

// In main loop, before printing prompt:
check_and_report_jobs();
print!("breenix> ");
```

**Commit:** `feat(shell): add SIGCHLD handler for job status updates`

---

## Task 13: tcsetpgrp/tcgetpgrp for Terminal Control

**Files:**
- Modify: `kernel/src/syscall/ioctl.rs` (if not already present)
- Modify: `libs/libbreenix/src/termios.rs`

**Step 1: Verify TIOCSPGRP/TIOCGPGRP work**

These should already be implemented from Phase 2. Verify:
```rust
pub fn tcsetpgrp(fd: i32, pgrp: i32) -> i32 {
    ioctl(fd, TIOCSPGRP, &pgrp as *const i32 as usize)
}

pub fn tcgetpgrp(fd: i32) -> i32 {
    let mut pgrp: i32 = 0;
    ioctl(fd, TIOCGPGRP, &mut pgrp as *mut i32 as usize);
    pgrp
}
```

**Step 2: Test terminal control switching**

Verify the shell can give/take terminal control from child processes.

**Commit:** `feat(shell): verify and test tcsetpgrp/tcgetpgrp`

---

## Execution Order

### Phase 2 Completion (1 task)
1. Task 1: Raw mode toggle

### Phase 3 Completion (3 tasks)
2. Task 2: Program table
3. Task 3: Fork + exec
4. Task 4: Basic pipe support

### Phase 4 Completion (9 tasks)
5. Task 5: Background execution (&)
6. Task 6: Job tracking structure
7. Task 7: Process groups (setpgid/getpgid)
8. Task 8: Sessions (setsid/getsid)
9. Task 9: fg command
10. Task 10: bg command
11. Task 11: jobs command
12. Task 12: SIGCHLD handler
13. Task 13: Terminal control verification

---

## Agent Dispatch Strategy

Each task should be assigned to a codex agent with:
1. Clear task boundaries (one feature per agent)
2. Specific files to modify
3. Test criteria
4. Commit message format

Agents should be dispatched in dependency order:
- Tasks 1-4 can run partially in parallel
- Tasks 5-6 depend on 1-4
- Tasks 7-8 are kernel work (can run in parallel)
- Tasks 9-13 depend on 5-8
