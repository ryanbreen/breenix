# Interactive Shell Implementation Plan

## Vision

Boot Breenix into an interactive shell (PID 1) that accepts keyboard input and executes commands. This is the transition from automated testing to actual OS usage.

**Design Philosophy:** Unix-idiomatic architecture where the kernel provides primitives via syscalls, and userspace programs (compiled against libbreenix) orchestrate user interaction.

## Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│                           USERSPACE                                  │
│                                                                      │
│  ┌────────────────────────────────────────────────────────────────┐ │
│  │  /init (PID 1) - Interactive Shell                              │ │
│  │                                                                  │ │
│  │  loop {                                                          │ │
│  │      write(stdout, "> ");           // prompt                    │ │
│  │      let line = read_line(stdin);   // BLOCKS on keyboard        │ │
│  │      let cmd = parse(line);         // tokenize                  │ │
│  │      if builtin(cmd) { handle(); }  // echo, ps, help            │ │
│  │      else { fork(); exec(cmd); wait(); }  // external            │ │
│  │  }                                                               │ │
│  └────────────────────────────────────────────────────────────────┘ │
│                              │                                       │
│                         libbreenix                                   │
│                    (syscall wrappers)                                │
│         read() write() fork() exec() wait() pipe() dup2()           │
├─────────────────────────────────────────────────────────────────────┤
│                            KERNEL                                    │
│                                                                      │
│  ┌─────────────┐    ┌─────────────┐    ┌──────────────────────┐    │
│  │  Keyboard   │    │    TTY      │    │   Process Manager    │    │
│  │   Driver    │ ─► │   Layer     │ ─► │   (fork/exec/wait)   │    │
│  │             │    │             │    │                      │    │
│  │ IRQ1 → scan │    │ line edit   │    │ program table        │    │
│  │ codes       │    │ cooked mode │    │ embedded ELFs        │    │
│  └─────────────┘    └─────────────┘    └──────────────────────┘    │
│         │                  │                      │                  │
│         ▼                  ▼                      ▼                  │
│  ┌─────────────────────────────────────────────────────────────────┐│
│  │                    Syscall Interface                             ││
│  │  SYS_read(0, buf, n)  →  blocks until line ready                ││
│  │  SYS_write(1, buf, n) →  output to TTY                          ││
│  │  SYS_fork()           →  create child process                   ││
│  │  SYS_execve()         →  replace process image                  ││
│  │  SYS_waitpid()        →  wait for child exit                    ││
│  └─────────────────────────────────────────────────────────────────┘│
└─────────────────────────────────────────────────────────────────────┘
```

---

## Phase 1: Kernel Input Infrastructure (CRITICAL)

**Goal:** `read(0, buf, n)` blocks and returns keyboard input to userspace.

### Current State

```
Keyboard IRQ1 → scancode → keyboard::stream::add_scancode()
                                    ↓
                         keyboard_task (async) → logs to serial
                                    ↓
                              ❌ DEAD END

Userspace read(stdin) → returns 0 immediately (no data)
```

### Target State

```
Keyboard IRQ1 → scancode → keyboard::stream::add_scancode()
                                    ↓
                         keyboard_task → TTY input buffer
                                    ↓
                         wake blocked readers
                                    ↓
Userspace read(stdin) → blocks → wakes → returns line
```

### Implementation Tasks

#### 1.1 Create TTY Input Buffer
**File:** `kernel/src/tty/mod.rs` (new)

```rust
pub struct TtyInputBuffer {
    /// Ring buffer for incoming characters
    buffer: ArrayQueue<u8>,
    /// Line buffer for cooked mode (accumulate until Enter)
    line_buffer: Vec<u8>,
    /// Waker for blocked readers
    reader_waker: Option<Waker>,
    /// Echo characters to output?
    echo: bool,
    /// Cooked mode (line-buffered)?
    canonical: bool,
}

impl TtyInputBuffer {
    pub const BUFFER_SIZE: usize = 256;

    pub fn new() -> Self { ... }

    /// Called by keyboard_task when character ready
    pub fn push_char(&mut self, c: u8) {
        if self.canonical {
            // Line mode: buffer until Enter
            if c == b'\n' {
                // Move line_buffer contents to ring buffer
                for byte in self.line_buffer.drain(..) {
                    self.buffer.push(byte);
                }
                self.buffer.push(b'\n');
                // Wake any blocked reader
                if let Some(waker) = self.reader_waker.take() {
                    waker.wake();
                }
            } else if c == 0x7f { // Backspace
                self.line_buffer.pop();
            } else {
                self.line_buffer.push(c);
            }
        } else {
            // Raw mode: immediate delivery
            self.buffer.push(c);
            if let Some(waker) = self.reader_waker.take() {
                waker.wake();
            }
        }

        if self.echo {
            // Echo to output (serial/framebuffer)
            serial_print!("{}", c as char);
        }
    }

    /// Called by sys_read for fd 0
    pub fn read(&mut self, buf: &mut [u8]) -> usize {
        let mut count = 0;
        while count < buf.len() {
            if let Some(byte) = self.buffer.pop() {
                buf[count] = byte;
                count += 1;
                if byte == b'\n' {
                    break; // Line complete
                }
            } else {
                break; // Buffer empty
            }
        }
        count
    }

    /// Check if data available (for polling)
    pub fn has_data(&self) -> bool {
        !self.buffer.is_empty()
    }

    /// Register waker for blocking read
    pub fn register_waker(&mut self, waker: Waker) {
        self.reader_waker = Some(waker);
    }
}
```

#### 1.2 Global TTY Instance
**File:** `kernel/src/tty/mod.rs`

```rust
use spin::Mutex;
use lazy_static::lazy_static;

lazy_static! {
    pub static ref TTY: Mutex<TtyInputBuffer> = Mutex::new(TtyInputBuffer::new());
}
```

#### 1.3 Connect keyboard_task to TTY
**File:** `kernel/src/keyboard/mod.rs`

```rust
// In keyboard_task, after processing scancode to character:
pub async fn keyboard_task() {
    let mut scancodes = ScancodeStream::new();
    let mut keyboard = Keyboard::new(...);

    while let Some(scancode) = scancodes.next().await {
        if let Ok(Some(key_event)) = keyboard.add_byte(scancode) {
            if let Some(key) = keyboard.process_keyevent(key_event) {
                match key {
                    DecodedKey::Unicode(c) => {
                        // Feed to TTY instead of just logging
                        crate::tty::TTY.lock().push_char(c as u8);
                    }
                    DecodedKey::RawKey(key) => {
                        // Handle special keys (arrows, function keys)
                    }
                }
            }
        }
    }
}
```

#### 1.4 Implement Blocking read() for stdin
**File:** `kernel/src/syscall/handlers.rs`

```rust
pub fn sys_read(fd: i32, buf_ptr: *mut u8, count: usize) -> SyscallResult {
    // ... existing validation ...

    match &fd_entry.kind {
        FdKind::StdIo(0) => {
            // stdin - read from TTY with blocking
            let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr, count) };

            loop {
                let mut tty = crate::tty::TTY.lock();

                if tty.has_data() {
                    let bytes_read = tty.read(buf);
                    return SyscallResult::Ok(bytes_read as i64);
                }

                // No data - need to block
                // Register current thread as waiting, yield, wake on keypress
                let thread_id = current_thread_id();
                tty.register_waker(create_thread_waker(thread_id));
                drop(tty); // Release lock before blocking

                // Block this thread until woken
                block_current_thread();
            }
        }
        // ... rest of existing handlers ...
    }
}
```

#### 1.5 Thread Blocking Infrastructure
**File:** `kernel/src/task/thread.rs`

```rust
/// Block current thread until explicitly woken
pub fn block_current_thread() {
    let thread_id = current_thread_id();

    // Mark thread as blocked
    SCHEDULER.lock().set_thread_state(thread_id, ThreadState::Blocked);

    // Yield to scheduler
    yield_current();
}

/// Wake a blocked thread
pub fn wake_thread(thread_id: ThreadId) {
    SCHEDULER.lock().set_thread_state(thread_id, ThreadState::Runnable);
}

/// Create a Waker that will wake the given thread
pub fn create_thread_waker(thread_id: ThreadId) -> Waker {
    // Implementation using RawWaker
    ...
}
```

### Phase 1 Test Plan

1. **Unit test:** TTY buffer push/read operations
2. **Integration test:** Keyboard IRQ → TTY buffer → read() returns data
3. **Boot stage:** "Stdin read returns keyboard input"

```rust
// userspace/tests/stdin_test.rs
#[no_mangle]
pub extern "C" fn _start() -> ! {
    io::print("Type something: ");

    let mut buf = [0u8; 64];
    let n = io::read(0, &mut buf);  // Should block until Enter

    io::print("You typed: ");
    io::write(1, &buf[..n]);

    if n > 0 {
        io::print("\nSTDIN_TEST_PASSED\n");
    }
    process::exit(0);
}
```

---

## Phase 2: TTY Abstraction Layer

**Goal:** POSIX terminal semantics - line editing, signal generation, mode control.

### Implementation Tasks

#### 2.1 Line Discipline (Cooked Mode)
- Backspace (0x7F) deletes previous character
- Ctrl+U kills entire line
- Ctrl+W deletes word
- Arrow keys (future: line editing with cursor)

#### 2.2 Signal Generation
- Ctrl+C → send SIGINT to foreground process
- Ctrl+D → send EOF (return 0 from read on empty line)
- Ctrl+Z → send SIGTSTP (job control, future)
- Ctrl+\ → send SIGQUIT

#### 2.3 Terminal Modes
```rust
pub struct Termios {
    pub iflag: u32,  // Input modes
    pub oflag: u32,  // Output modes
    pub cflag: u32,  // Control modes
    pub lflag: u32,  // Local modes (ECHO, ICANON, ISIG)
}

// Key flags:
const ECHO: u32 = 0x0008;    // Echo input characters
const ICANON: u32 = 0x0002;  // Canonical mode (line-buffered)
const ISIG: u32 = 0x0001;    // Enable signals (Ctrl+C, etc.)
```

#### 2.4 ioctl Syscall
```rust
pub fn sys_ioctl(fd: i32, request: u64, arg: u64) -> SyscallResult {
    match request {
        TCGETS => { /* get termios */ }
        TCSETS => { /* set termios */ }
        TIOCGWINSZ => { /* get window size */ }
        // ...
    }
}
```

---

## Phase 3: Init/Shell Userspace Program

**Goal:** PID 1 shell compiled against libbreenix.

### Shell Architecture

```
userspace/init/
├── Cargo.toml
├── src/
│   ├── main.rs        # Entry point, REPL loop
│   ├── parser.rs      # Command line tokenization
│   ├── builtins.rs    # Built-in commands
│   ├── exec.rs        # External command execution
│   └── jobs.rs        # (Future) Job control
```

### 3.1 Main REPL Loop
**File:** `userspace/init/src/main.rs`

```rust
#![no_std]
#![no_main]

use libbreenix::{io, process};

mod parser;
mod builtins;
mod exec;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    io::println("Breenix Shell v0.1");
    io::println("Type 'help' for available commands.\n");

    let mut line_buf = [0u8; 256];

    loop {
        // Print prompt
        io::print("breenix> ");

        // Read line (blocks until Enter)
        let n = io::read(0, &mut line_buf);
        if n == 0 {
            // EOF (Ctrl+D on empty line)
            io::println("\nGoodbye!");
            process::exit(0);
        }

        let line = core::str::from_utf8(&line_buf[..n])
            .unwrap_or("")
            .trim();

        if line.is_empty() {
            continue;
        }

        // Parse and execute
        let args = parser::tokenize(line);
        if args.is_empty() {
            continue;
        }

        let cmd = args[0];

        // Try builtin first
        if builtins::try_builtin(cmd, &args[1..]) {
            continue;
        }

        // External command
        exec::run_external(cmd, &args);
    }
}
```

### 3.2 Built-in Commands
**File:** `userspace/init/src/builtins.rs`

```rust
pub fn try_builtin(cmd: &str, args: &[&str]) -> bool {
    match cmd {
        "help" => { cmd_help(); true }
        "echo" => { cmd_echo(args); true }
        "ps" => { cmd_ps(); true }
        "exit" => { cmd_exit(args); true }
        "clear" => { cmd_clear(); true }
        "time" => { cmd_time(); true }
        _ => false
    }
}

fn cmd_help() {
    io::println("Available commands:");
    io::println("  help       - Show this help");
    io::println("  echo TEXT  - Print text");
    io::println("  ps         - List processes");
    io::println("  time       - Show current time");
    io::println("  clear      - Clear screen");
    io::println("  exit [N]   - Exit shell");
    io::println("");
    io::println("External programs:");
    io::println("  hello      - Hello world test");
    io::println("  counter    - Counter demo");
    // ... list embedded programs
}

fn cmd_echo(args: &[&str]) {
    for (i, arg) in args.iter().enumerate() {
        if i > 0 { io::print(" "); }
        io::print(arg);
    }
    io::println("");
}

fn cmd_ps() {
    // Syscall to get process list
    // For now, print PID of current process
    let pid = process::getpid();
    io::print("  PID  CMD\n");
    io::print("    ");
    // ... format process list
}

fn cmd_exit(args: &[&str]) {
    let code = args.get(0)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    process::exit(code);
}

fn cmd_clear() {
    // ANSI escape: clear screen, move cursor home
    io::print("\x1b[2J\x1b[H");
}

fn cmd_time() {
    let ts = libbreenix::time::now_monotonic();
    io::print("Uptime: ");
    // ... format time
}
```

### 3.3 External Command Execution
**File:** `userspace/init/src/exec.rs`

```rust
use libbreenix::{io, process};

/// Program registry - maps names to embedded ELF data
static PROGRAMS: &[(&str, &[u8])] = &[
    ("hello", include_bytes!("../../tests/hello_world.bin")),
    ("counter", include_bytes!("../../tests/counter.bin")),
    ("fork_test", include_bytes!("../../tests/fork_test.bin")),
    // ... more programs
];

pub fn run_external(cmd: &str, args: &[&str]) {
    // Look up program
    let program = PROGRAMS.iter().find(|(name, _)| *name == cmd);

    match program {
        Some((_, elf_data)) => {
            // Fork
            let pid = process::fork();

            if pid == 0 {
                // Child: exec the program
                process::exec_bytes(elf_data);
                // exec doesn't return on success
                io::println("exec failed");
                process::exit(1);
            } else {
                // Parent: wait for child
                let status = process::waitpid(pid, 0);
                if status != 0 {
                    io::print("Process exited with status: ");
                    // ... print status
                }
            }
        }
        None => {
            io::print("Unknown command: ");
            io::println(cmd);
            io::println("Type 'help' for available commands.");
        }
    }
}
```

### 3.4 libbreenix Additions Needed

```rust
// libs/libbreenix/src/process.rs

/// Execute program from raw ELF bytes (for embedded programs)
pub fn exec_bytes(elf_data: &[u8]) -> ! {
    unsafe {
        syscall2(SYS_EXECVE, elf_data.as_ptr() as u64, elf_data.len() as u64);
    }
    unreachable!()
}

/// Wait for child process
pub fn waitpid(pid: i32, options: i32) -> i32 {
    unsafe {
        syscall2(SYS_WAITPID, pid as u64, options as u64) as i32
    }
}
```

---

## Phase 4: Program Loading Enhancement

**Goal:** Run programs by name with arguments.

### 4.1 In-Memory Program Registry
Instead of embedding ELF in shell, create kernel-side registry:

```rust
// kernel/src/process/programs.rs
pub struct ProgramRegistry {
    programs: BTreeMap<&'static str, &'static [u8]>,
}

impl ProgramRegistry {
    pub fn lookup(&self, name: &str) -> Option<&[u8]> {
        self.programs.get(name).copied()
    }
}
```

### 4.2 execve() by Name
```rust
pub fn sys_execve(
    pathname: *const u8,  // Program name or path
    argv: *const *const u8,
    envp: *const *const u8,
) -> SyscallResult {
    let name = unsafe { cstr_to_str(pathname) };

    // Look up in program registry
    if let Some(elf_data) = PROGRAM_REGISTRY.lookup(name) {
        // Parse argv, envp
        let args = parse_string_array(argv);
        let env = parse_string_array(envp);

        // Load and execute
        exec_elf(elf_data, &args, &env)
    } else {
        SyscallResult::Err(ENOENT)
    }
}
```

### 4.3 Argument Passing
Set up initial stack with argc/argv:

```
+------------------+ <- Stack top (high address)
| envp[n] = NULL   |
| envp[n-1]        |
| ...              |
| envp[0]          |
| argv[argc] = NULL|
| argv[argc-1]     |
| ...              |
| argv[0]          |
| argc             |
+------------------+ <- Initial RSP
```

---

## Testing Strategy

### Boot Stages to Add

```rust
// xtask/src/main.rs - add new stages

// Phase 1 tests
Stage { marker: "STDIN_READ_BLOCKS", description: "Stdin read blocks for input" },
Stage { marker: "STDIN_RECEIVES_KEYBOARD", description: "Stdin receives keyboard data" },
Stage { marker: "TTY_ECHO_WORKS", description: "TTY echoes typed characters" },

// Phase 2 tests
Stage { marker: "TTY_BACKSPACE_WORKS", description: "Backspace deletes character" },
Stage { marker: "TTY_CTRL_C_SIGNAL", description: "Ctrl+C generates SIGINT" },
Stage { marker: "TTY_CTRL_D_EOF", description: "Ctrl+D sends EOF" },

// Phase 3 tests
Stage { marker: "SHELL_PROMPT_SHOWN", description: "Shell shows prompt" },
Stage { marker: "SHELL_ECHO_WORKS", description: "Shell echo command works" },
Stage { marker: "SHELL_EXTERNAL_CMD", description: "Shell runs external command" },
Stage { marker: "SHELL_PIPE_WORKS", description: "Shell pipe works (cmd1 | cmd2)" },
```

### Manual Testing

For interactive testing (not automated boot-stages):

```bash
# Build with interactive mode
cargo run -p xtask -- interactive

# This boots into the shell instead of running test suite
# Allows manual keyboard input testing
```

---

## Implementation Order

### Week 1: Phase 1 - Keyboard → Stdin Bridge
1. Create `kernel/src/tty/mod.rs` with TtyInputBuffer
2. Connect keyboard_task to TTY
3. Implement blocking read() for fd 0
4. Add thread blocking/waking infrastructure
5. Create stdin_test userspace program
6. Add boot stage for stdin test

### Week 2: Phase 2 - TTY Basics
1. Implement line editing (backspace)
2. Add echo mode
3. Implement Ctrl+C → SIGINT
4. Implement Ctrl+D → EOF
5. Add basic ioctl for termios

### Week 3: Phase 3 - Shell
1. Create `userspace/init/` shell program
2. Implement REPL loop with blocking read
3. Add built-in commands (help, echo, ps, exit)
4. Implement external command execution
5. Add basic pipe support (future)

### Week 4: Polish & Integration
1. Error handling and edge cases
2. Boot-as-default mode (kernel runs /init automatically)
3. Documentation and examples
4. Interactive demo mode for manual testing

---

## Dependencies

### Kernel Changes Required
- [ ] New `kernel/src/tty/` module
- [ ] Thread blocking infrastructure in scheduler
- [ ] Waker mechanism for blocked threads
- [ ] sys_waitpid implementation
- [ ] Enhanced sys_execve (name lookup, argv)

### libbreenix Changes Required
- [ ] `waitpid()` wrapper
- [ ] `exec_bytes()` for embedded ELFs
- [ ] Read helpers (read_line, etc.)

### Build System Changes
- [ ] New `userspace/init/` crate
- [ ] Include init binary in boot image
- [ ] Interactive boot mode in xtask

---

## Success Criteria

**Phase 1 Complete:**
- Boot stage passes: userspace program reads keyboard input via `read(0, ...)`
- Blocking works: read() doesn't return until data available

**Phase 2 Complete:**
- Backspace works in line-editing mode
- Ctrl+C terminates foreground process
- Ctrl+D sends EOF

**Phase 3 Complete:**
- Kernel boots into shell prompt
- Can type commands and see output
- Can run embedded test programs
- Basic pipes work (`echo hello | cat`)

**Full Success:**
- Interactive Breenix session where user can:
  - See shell prompt
  - Type commands
  - Run programs
  - See output
  - Exit cleanly
