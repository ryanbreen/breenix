# Shell Enhancement & Third-Party Application Roadmap

## Vision

Transform Breenix from a kernel with embedded test programs into a proper operating system where users can:
- Log into an interactive shell
- Navigate the filesystem (`cd /bin`, `cd /home`)
- Run programs installed on disk (not hardcoded)
- Eventually: build and run third-party applications (GNU coreutils, etc.)

---

## Current State

### What We Have (init_shell.rs)

| Feature | Status | Notes |
|---------|--------|-------|
| Command parsing | ✅ | Pipelines, background jobs, redirection |
| Job control | ✅ | fg, bg, jobs, Ctrl+C handling |
| Signal handling | ✅ | SIGINT, SIGCHLD |
| Built-in commands | ✅ | help, exit, clear, echo, jobs, fg, bg |
| External programs | ⚠️ | Hardcoded PROGRAM_REGISTRY (17 programs) |
| Working directory | ❌ | No `cd`, no `pwd`, no cwd tracking |
| PATH resolution | ❌ | No PATH, no filesystem search |
| Paging/scrolling | ❌ | Output overflows QEMU window |

### Current Program Execution Model

```
User types: "hello"
     ↓
Shell looks up in PROGRAM_REGISTRY (static array)
     ↓
Finds binary_name = "hello_world\0"
     ↓
Calls exec_program() with hardcoded binary
     ↓
Kernel loads EMBEDDED program (not from disk)
```

**Problem**: Programs are compiled into the kernel, not loaded from filesystem.

---

## Phase 1: Basic Navigation (cd, pwd, cwd)

**Goal**: User can navigate the filesystem

### 1.1 Working Directory Tracking

Add per-process current working directory:

```rust
// In kernel process structure
pub struct Process {
    // ... existing fields
    pub cwd: PathBuf,  // Current working directory
}
```

Syscalls needed:
- `sys_getcwd(buf, size)` - Get current working directory
- `sys_chdir(path)` - Change current working directory

### 1.2 Shell Built-ins

Add to init_shell.rs:

```rust
fn builtin_cd(args: &[&str]) -> i32 {
    let path = args.get(0).unwrap_or(&"/");
    libbreenix::process::chdir(path)
}

fn builtin_pwd() -> i32 {
    let mut buf = [0u8; 256];
    let len = libbreenix::process::getcwd(&mut buf);
    io::print(core::str::from_utf8(&buf[..len]).unwrap());
    0
}
```

### 1.3 Relative Path Resolution

Update file syscalls to respect cwd:
- `sys_open("foo.txt")` → opens `{cwd}/foo.txt`
- `sys_stat("../bar")` → resolves relative to cwd

### Deliverables
- [ ] `sys_getcwd` and `sys_chdir` syscalls
- [ ] Per-process cwd in kernel
- [ ] `cd` and `pwd` shell built-ins
- [ ] Relative path resolution in VFS

---

## Phase 2: Filesystem-Based Program Loading

**Goal**: Load and execute ELF binaries from disk

### 2.1 Current exec() Flow

```
exec("hello_world") → Looks up in EMBEDDED_BINARIES → Loads from memory
```

### 2.2 Target exec() Flow

```
exec("/bin/hello") → Opens file from Ext2 → Reads ELF → Maps into memory → Jumps to entry
```

### 2.3 Implementation Steps

1. **Modify ELF loader** to accept file descriptor instead of memory slice:
   ```rust
   pub fn load_elf_from_file(fd: i32) -> Result<EntryPoint, ElfError>
   ```

2. **Update sys_execve** to:
   - Open file at path
   - Verify it's an ELF executable
   - Load into new address space
   - Close file, transfer control

3. **Add execute permission check** (if we have permissions):
   ```rust
   if !file.mode.contains(S_IXUSR) {
       return Err(EACCES);
   }
   ```

### Deliverables
- [ ] File-based ELF loader
- [ ] Updated sys_execve to load from path
- [ ] Execute permission checking
- [ ] Error handling for missing/invalid executables

---

## Phase 3: PATH Resolution

**Goal**: `ls` finds and runs `/bin/ls`

### 3.1 Environment Variables

Add environment variable support:

```rust
// Per-process environment
pub struct Process {
    pub env: BTreeMap<String, String>,  // PATH=/bin:/usr/bin
}
```

Syscalls:
- `sys_getenv(name, buf, size)` - Get environment variable
- `sys_setenv(name, value)` - Set environment variable

### 3.2 PATH Search Algorithm

In shell (not kernel):

```rust
fn find_executable(name: &str) -> Option<PathBuf> {
    // If absolute path, use directly
    if name.starts_with('/') {
        return Some(PathBuf::from(name));
    }

    // Search PATH directories
    let path = getenv("PATH").unwrap_or("/bin:/usr/bin");
    for dir in path.split(':') {
        let candidate = format!("{}/{}", dir, name);
        if file_exists(&candidate) && is_executable(&candidate) {
            return Some(PathBuf::from(candidate));
        }
    }
    None
}
```

### 3.3 Remove PROGRAM_REGISTRY

Replace hardcoded registry with PATH-based lookup:

```rust
// Before
fn execute_command(cmd: &str) {
    if let Some(entry) = PROGRAM_REGISTRY.iter().find(|e| e.name == cmd) {
        exec_program(entry.binary_name);
    }
}

// After
fn execute_command(cmd: &str) {
    if let Some(path) = find_executable(cmd) {
        exec(&path);
    } else {
        println!("{}: command not found", cmd);
    }
}
```

### Deliverables
- [ ] Environment variable syscalls
- [ ] PATH search in shell
- [ ] Remove PROGRAM_REGISTRY
- [ ] Default PATH set at login

---

## Phase 4: Filesystem Layout

**Goal**: Standard Unix directory structure

### 4.1 Directory Structure

```
/
├── bin/           # Essential user binaries
│   ├── ls
│   ├── cat
│   ├── echo
│   └── sh
├── sbin/          # System binaries
├── usr/
│   ├── bin/       # Non-essential user binaries
│   └── lib/       # Libraries
├── home/
│   └── user/      # User home directory
├── tmp/           # Temporary files
├── var/           # Variable data
│   └── log/
└── etc/           # Configuration
    └── passwd
```

### 4.2 Build System Integration

Modify xtask to:
1. Create filesystem image with proper layout
2. Install compiled programs to /bin
3. Copy test programs to /home/user

```rust
// In xtask build
fn create_filesystem_image() {
    create_dir("/bin");
    create_dir("/home/user");

    // Install coreutils
    copy("target/x86_64-breenix/release/ls", "/bin/ls");
    copy("target/x86_64-breenix/release/cat", "/bin/cat");
    // ...
}
```

### 4.3 Initial ramdisk or Ext2 Image

Options:
1. **Initramfs** - Embedded in kernel, unpacked to tmpfs at boot
2. **Ext2 image** - Separate disk image, already mostly working

Current: We have Ext2 driver and disk image support. Need to populate the image.

### Deliverables
- [ ] Standard directory layout in disk image
- [ ] xtask creates populated filesystem
- [ ] Programs installed to /bin at build time
- [ ] /home/user for user files

---

## Phase 5: Output Paging

**Goal**: Long output doesn't overflow screen

### 5.1 Simple Pager (less/more)

Implement a basic pager:

```rust
// /bin/more
fn main() {
    let lines_per_page = 24;
    let mut line_count = 0;

    for line in stdin.lines() {
        print!("{}", line);
        line_count += 1;

        if line_count >= lines_per_page {
            print!("--More--");
            wait_for_key();
            line_count = 0;
        }
    }
}
```

### 5.2 Pipe Integration

User can pipe long output:
```
help | more
ls -la /bin | more
```

### Deliverables
- [ ] `more` command implementation
- [ ] Pipe to pager works
- [ ] Optional: `less` with scrollback

---

## Phase 6: Coreutils

**Goal**: Basic Unix utilities

### Essential Commands

| Command | Complexity | Notes |
|---------|------------|-------|
| ls | Medium | Directory listing, requires stat |
| cat | Simple | Already have basic version |
| echo | Simple | Already implemented |
| mkdir | Simple | ✅ Already implemented |
| rmdir | Simple | ✅ Already implemented |
| rm | Simple | Unlink files |
| cp | Medium | Copy files |
| mv | Medium | Rename/move |
| head/tail | Simple | First/last N lines |
| wc | Simple | Word/line/char count |
| grep | Medium | Pattern matching |
| chmod | Simple | Change permissions |

### Implementation Order

1. **Already done**: mkdir, rmdir, echo
2. **Next**: ls (with -l option), cat, rm, cp
3. **Then**: head, tail, wc
4. **Later**: grep, chmod, more advanced tools

---

## Phase 7: Libc Foundation

**Goal**: C runtime for third-party applications

### 7.1 Options

| Option | Effort | Compatibility |
|--------|--------|---------------|
| Custom libc | High | Limited, but tailored |
| musl port | Medium | Good POSIX compliance |
| newlib port | Medium | Designed for embedded/OS |

**Recommendation**: Start with musl-libc port

### 7.2 Musl Port Steps

1. **Syscall layer**: Map musl syscalls to Breenix syscalls
2. **Thread support**: Requires pthread syscalls (clone, futex)
3. **Signal handling**: Full POSIX signals
4. **Stdio**: Already have basic file ops
5. **Memory**: malloc via brk/sbrk (already have)

### 7.3 Required Kernel Features

| Feature | Status | Priority |
|---------|--------|----------|
| fork/exec | ✅ | - |
| wait/waitpid | ✅ | - |
| brk/sbrk | ✅ | - |
| open/read/write/close | ✅ | - |
| stat/fstat | ✅ | - |
| mmap | ⚠️ Partial | High |
| signals | ⚠️ Basic | High |
| clone (threads) | ❌ | Medium |
| futex | ❌ | Medium |
| pipe | ✅ | - |
| dup/dup2 | ✅ | - |
| socket | ✅ | - |

### Deliverables
- [ ] Complete mmap implementation
- [ ] Full signal support (sigaction, sigprocmask)
- [ ] clone() for threading
- [ ] futex for synchronization
- [ ] musl syscall wrapper layer

---

## Phase 8: Cross-Compiler Toolchain

**Goal**: Build programs for Breenix from host

### 8.1 GCC/Clang Cross-Compiler

Create x86_64-breenix target:

```bash
# Configure GCC for Breenix
./configure --target=x86_64-breenix --with-sysroot=/path/to/breenix-sysroot
```

### 8.2 Sysroot Structure

```
breenix-sysroot/
├── usr/
│   ├── include/    # C headers (from musl)
│   └── lib/        # libc.a, crt0.o
└── lib/
    └── libc.so     # If dynamic linking
```

### 8.3 Build System

```bash
# Cross-compile hello.c for Breenix
x86_64-breenix-gcc -o hello hello.c

# Install to Breenix filesystem
cp hello /path/to/breenix-disk/bin/
```

---

## Phase 9: Third-Party Applications

**Goal**: Run GNU software

### 9.1 Porting Workflow

1. Download source (e.g., GNU coreutils)
2. Configure with cross-compiler
3. Fix any Breenix-specific issues
4. Build and install to sysroot

### 9.2 Initial Targets

| Application | Complexity | Notes |
|-------------|------------|-------|
| busybox | Medium | Many utils in one binary |
| GNU coreutils | Medium | Standard Unix tools |
| vim/vi | High | Requires termcap/ncurses |
| bash | High | Complex, but worth it |
| gcc | Very High | Self-hosting goal |

### 9.3 Self-Hosting Goal

Ultimate goal: Build Breenix on Breenix
- Requires: GCC, binutils, make, shell, coreutils
- This is a major milestone for any OS project

---

## Timeline & Dependencies

```
Phase 1: Navigation ──────┐
                          ↓
Phase 2: Disk Loading ────┼───→ Phase 3: PATH
                          ↓
Phase 4: FS Layout ───────┘
                          ↓
Phase 5: Paging ──────────→ Phase 6: Coreutils
                                    ↓
                          Phase 7: Libc ───→ Phase 8: Toolchain
                                                      ↓
                                            Phase 9: Third-Party Apps
```

**Critical Path**: Phases 1-4 are foundational. Phase 7 (libc) is the gate to third-party apps.

---

## Immediate Next Steps

1. **Implement `sys_getcwd` and `sys_chdir`** - Kernel syscalls for cwd
2. **Add `cd` and `pwd` to shell** - User-visible navigation
3. **Modify ELF loader** - Accept file path instead of embedded binary
4. **Create filesystem layout** - /bin, /home in disk image
5. **Install programs to /bin** - xtask builds and installs

---

## Success Metrics

### Milestone 1: Navigation
- [ ] User can `cd /bin` and `pwd` shows `/bin`
- [ ] `ls` shows directory contents

### Milestone 2: Disk Programs
- [ ] Programs loaded from /bin, not embedded
- [ ] PROGRAM_REGISTRY eliminated

### Milestone 3: Self-Sufficient Shell
- [ ] Boot → login → navigate → run programs from disk
- [ ] No hardcoded program list

### Milestone 4: Third-Party Ready
- [ ] C program compiles with cross-compiler
- [ ] C program runs on Breenix
- [ ] printf("Hello World") works
