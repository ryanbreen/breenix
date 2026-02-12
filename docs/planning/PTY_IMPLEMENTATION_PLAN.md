# PTY (Pseudo-Terminal) Implementation Plan

## Overview

PTY (pseudo-terminal) support enables remote shell access via telnet/SSH by providing terminal emulation over network connections. A PTY creates a master/slave device pair where:
- **Master**: Controlled by the server (telnetd/sshd), connected to network socket
- **Slave**: Appears as a regular terminal to the shell process

## Current State

### What We Have
- Basic TTY infrastructure (Phase 1-2 complete)
- Line discipline with canonical mode, echo, signals
- ioctl for termios (TCGETS, TCSETS, TIOCGPGRP, TIOCSPGRP)
- Full TCP stack with socket/bind/listen/accept/read/write
- fork/exec/dup2 for process management

### What's Missing for PTY
- PTY master/slave device pairs
- `/dev/ptmx` multiplexer device
- `/dev/pts/` filesystem for slave devices
- PTY-specific syscalls (posix_openpt, grantpt, unlockpt, ptsname)
- PTY allocation and lifecycle management

## Architecture

### Device Hierarchy

```
/dev/
├── ptmx              # PTY multiplexer (open to get new PTY master)
├── pts/              # PTY slave devices directory
│   ├── 0             # First PTY slave
│   ├── 1             # Second PTY slave
│   └── ...
├── tty0              # Physical console (existing)
└── console           # Kernel console (existing)
```

### Data Flow

```
[telnet client] <--> [socket] <--> [telnetd]
                                      |
                                      v
                               [PTY master fd]
                                      |
                            (kernel PTY layer)
                                      |
                               [PTY slave fd]
                                      |
                                      v
                                   [shell]
                                      |
                                      v
                              [child processes]
```

### Kernel Components

```
kernel/src/
├── tty/
│   ├── mod.rs           # Existing TTY infrastructure
│   ├── line_discipline.rs  # Existing line discipline
│   ├── driver.rs        # Existing TTY driver
│   └── pty/             # NEW: PTY subsystem
│       ├── mod.rs       # PTY module entry
│       ├── master.rs    # PTY master device
│       ├── slave.rs     # PTY slave device
│       ├── pair.rs      # PTY pair management
│       └── multiplexer.rs  # /dev/ptmx implementation
├── fs/
│   └── devpts/          # NEW: devpts filesystem
│       ├── mod.rs       # devpts mount point
│       └── ops.rs       # PTY slave file operations
└── syscall/
    └── pty.rs           # NEW: PTY syscalls
```

## Implementation Phases

### Phase 1: PTY Core Infrastructure (Foundation)

**Goal**: Create basic PTY master/slave pair mechanism

#### 1.1 PTY Pair Structure

```rust
// kernel/src/tty/pty/pair.rs

/// A PTY pair consists of a master and slave device
pub struct PtyPair {
    /// Unique PTY number (0, 1, 2, ...)
    pub pty_num: u32,

    /// Master side state
    pub master: PtyMaster,

    /// Slave side state
    pub slave: PtySlave,

    /// Shared ring buffer for data transfer
    pub master_to_slave: RingBuffer<u8, 4096>,
    pub slave_to_master: RingBuffer<u8, 4096>,

    /// Line discipline for the slave
    pub ldisc: LineDiscipline,

    /// Terminal attributes (termios)
    pub termios: Termios,

    /// Controlling session/process group
    pub session: Option<SessionId>,
    pub foreground_pgid: Option<ProcessGroupId>,
}

pub struct PtyMaster {
    /// Reference count
    pub refcount: AtomicU32,
    /// Locked state (unlockpt not yet called)
    pub locked: AtomicBool,
}

pub struct PtySlave {
    /// Reference count
    pub refcount: AtomicU32,
    /// Path: /dev/pts/N
    pub path: [u8; 16],
}
```

#### 1.2 PTY Allocator

```rust
// kernel/src/tty/pty/mod.rs

/// Global PTY allocator
pub struct PtyAllocator {
    /// Next PTY number to allocate
    next_pty_num: AtomicU32,

    /// Active PTY pairs (pty_num -> PtyPair)
    pairs: SpinLock<BTreeMap<u32, Arc<PtyPair>>>,

    /// Maximum PTY pairs allowed
    max_ptys: u32,
}

impl PtyAllocator {
    pub fn allocate(&self) -> Result<Arc<PtyPair>, Errno> {
        let pty_num = self.next_pty_num.fetch_add(1, Ordering::SeqCst);
        if pty_num >= self.max_ptys {
            return Err(Errno::ENOSPC);
        }

        let pair = Arc::new(PtyPair::new(pty_num));
        self.pairs.lock().insert(pty_num, pair.clone());
        Ok(pair)
    }

    pub fn get(&self, pty_num: u32) -> Option<Arc<PtyPair>> {
        self.pairs.lock().get(&pty_num).cloned()
    }

    pub fn release(&self, pty_num: u32) {
        self.pairs.lock().remove(&pty_num);
    }
}
```

#### 1.3 File Descriptor Types

```rust
// kernel/src/syscall/fd.rs - extend FdKind enum

pub enum FdKind {
    // ... existing variants ...

    /// PTY master file descriptor
    PtyMaster(u32),  // pty_num

    /// PTY slave file descriptor
    PtySlave(u32),   // pty_num
}
```

**Deliverables**:
- [ ] `PtyPair` structure with master/slave
- [ ] `PtyAllocator` for managing PTY lifecycle
- [ ] Ring buffers for bidirectional data transfer
- [ ] `FdKind::PtyMaster` and `FdKind::PtySlave` variants

---

### Phase 2: PTY Syscalls

**Goal**: Implement POSIX PTY syscalls

#### 2.1 posix_openpt() - Open PTY Master

```rust
// kernel/src/syscall/pty.rs

/// Open a new PTY master device
///
/// # Arguments
/// * `flags` - O_RDWR | O_NOCTTY | O_CLOEXEC
///
/// # Returns
/// * File descriptor for PTY master on success
/// * -EMFILE if too many open files
/// * -ENOSPC if no PTY slots available
pub fn sys_posix_openpt(flags: i32) -> SyscallResult {
    // Validate flags (must have O_RDWR)
    if flags & O_RDWR != O_RDWR {
        return Err(Errno::EINVAL);
    }

    // Allocate new PTY pair
    let pair = PTY_ALLOCATOR.allocate()?;

    // Create file descriptor for master
    let fd = current_process().fd_table.alloc(FdKind::PtyMaster(pair.pty_num))?;

    // Handle O_CLOEXEC
    if flags & O_CLOEXEC != 0 {
        current_process().fd_table.set_cloexec(fd, true);
    }

    Ok(fd as i64)
}
```

#### 2.2 grantpt() - Grant Access to Slave

```rust
/// Grant access to the slave PTY
///
/// In a full implementation, this would change ownership/permissions.
/// For Breenix, we just validate the fd is a PTY master.
pub fn sys_grantpt(fd: i32) -> SyscallResult {
    let process = current_process();
    let fd_entry = process.fd_table.get(fd as u64)?;

    match fd_entry.kind {
        FdKind::PtyMaster(pty_num) => {
            // In a real system: chown slave to current user
            // For now, just succeed if it's a valid PTY master
            Ok(0)
        }
        _ => Err(Errno::ENOTTY),
    }
}
```

#### 2.3 unlockpt() - Unlock Slave for Opening

```rust
/// Unlock the slave PTY for opening
pub fn sys_unlockpt(fd: i32) -> SyscallResult {
    let process = current_process();
    let fd_entry = process.fd_table.get(fd as u64)?;

    match fd_entry.kind {
        FdKind::PtyMaster(pty_num) => {
            let pair = PTY_ALLOCATOR.get(pty_num).ok_or(Errno::EIO)?;
            pair.master.locked.store(false, Ordering::SeqCst);
            Ok(0)
        }
        _ => Err(Errno::ENOTTY),
    }
}
```

#### 2.4 ptsname_r() - Get Slave Device Path

```rust
/// Get the path to the slave PTY device
pub fn sys_ptsname(fd: i32, buf: u64, buflen: u64) -> SyscallResult {
    let process = current_process();
    let fd_entry = process.fd_table.get(fd as u64)?;

    match fd_entry.kind {
        FdKind::PtyMaster(pty_num) => {
            let path = format!("/dev/pts/{}", pty_num);
            let path_bytes = path.as_bytes();

            if path_bytes.len() + 1 > buflen as usize {
                return Err(Errno::ERANGE);
            }

            // Copy path to user buffer
            copy_to_user(buf, path_bytes)?;
            copy_to_user(buf + path_bytes.len() as u64, &[0u8])?; // null terminator

            Ok(0)
        }
        _ => Err(Errno::ENOTTY),
    }
}
```

**Deliverables**:
- [ ] `sys_posix_openpt()` syscall
- [ ] `sys_grantpt()` syscall
- [ ] `sys_unlockpt()` syscall
- [ ] `sys_ptsname()` syscall (or ptsname_r variant)
- [ ] Syscall number assignments in dispatcher

---

### Phase 3: /dev/ptmx and /dev/pts/ Filesystem

**Goal**: Create device nodes for PTY access

#### 3.1 /dev/ptmx Device

```rust
// kernel/src/tty/pty/multiplexer.rs

/// The /dev/ptmx device - opening it creates a new PTY pair
pub struct PtmxDevice;

impl DeviceOperations for PtmxDevice {
    fn open(&self, flags: i32) -> Result<FdKind, Errno> {
        // Allocate new PTY pair
        let pair = PTY_ALLOCATOR.allocate()?;
        Ok(FdKind::PtyMaster(pair.pty_num))
    }
}
```

#### 3.2 devpts Filesystem

```rust
// kernel/src/fs/devpts/mod.rs

/// The devpts filesystem mounted at /dev/pts/
pub struct DevptsFilesystem {
    /// Reference to PTY allocator
    allocator: &'static PtyAllocator,
}

impl Filesystem for DevptsFilesystem {
    fn lookup(&self, name: &str) -> Result<VfsInode, Errno> {
        // Parse PTY number from name (e.g., "0", "1", "2")
        let pty_num: u32 = name.parse().map_err(|_| Errno::ENOENT)?;

        // Check if PTY exists and is unlocked
        let pair = self.allocator.get(pty_num).ok_or(Errno::ENOENT)?;
        if pair.master.locked.load(Ordering::SeqCst) {
            return Err(Errno::EIO); // Not yet unlocked
        }

        Ok(VfsInode::PtySlave(pty_num))
    }

    fn readdir(&self) -> Vec<DirEntry> {
        // List all active PTY slaves
        self.allocator.pairs.lock()
            .keys()
            .filter(|&num| {
                self.allocator.get(*num)
                    .map(|p| !p.master.locked.load(Ordering::SeqCst))
                    .unwrap_or(false)
            })
            .map(|num| DirEntry {
                name: format!("{}", num),
                inode: *num as u64,
                file_type: FileType::CharDevice,
            })
            .collect()
    }
}
```

**Deliverables**:
- [ ] `/dev/ptmx` device node
- [ ] devpts filesystem implementation
- [ ] Mount devpts at `/dev/pts/` during boot
- [ ] PTY slave lookup by number

---

### Phase 4: PTY I/O Operations

**Goal**: Implement read/write through PTY pairs

#### 4.1 Master Read/Write

```rust
// kernel/src/tty/pty/master.rs

impl PtyMaster {
    /// Write data to master (goes to slave's input)
    pub fn write(&self, pair: &PtyPair, data: &[u8]) -> Result<usize, Errno> {
        // Data written to master goes through line discipline to slave
        let mut written = 0;
        for &byte in data {
            // Process through line discipline (handles echo, signals, etc.)
            pair.ldisc.process_input(byte, &pair.termios)?;
            written += 1;
        }
        Ok(written)
    }

    /// Read data from master (slave's output)
    pub fn read(&self, pair: &PtyPair, buf: &mut [u8]) -> Result<usize, Errno> {
        // Read data that slave has written
        pair.slave_to_master.read(buf)
    }
}
```

#### 4.2 Slave Read/Write

```rust
// kernel/src/tty/pty/slave.rs

impl PtySlave {
    /// Write data from slave (goes to master's read buffer)
    pub fn write(&self, pair: &PtyPair, data: &[u8]) -> Result<usize, Errno> {
        // Data written by slave process goes to master
        pair.slave_to_master.write(data)
    }

    /// Read data for slave (from master's write, via line discipline)
    pub fn read(&self, pair: &PtyPair, buf: &mut [u8]) -> Result<usize, Errno> {
        // Read from line discipline output buffer
        pair.ldisc.read(buf)
    }
}
```

#### 4.3 Integrate with sys_read/sys_write

```rust
// kernel/src/syscall/handlers.rs - extend sys_read/sys_write

// In sys_read match:
FdKind::PtyMaster(pty_num) => {
    let pair = PTY_ALLOCATOR.get(pty_num).ok_or(Errno::EBADF)?;
    let n = pair.master_read(&mut buf)?;
    copy_to_user(buf_ptr, &buf[..n])?;
    Ok(n as i64)
}
FdKind::PtySlave(pty_num) => {
    let pair = PTY_ALLOCATOR.get(pty_num).ok_or(Errno::EBADF)?;
    let n = pair.slave_read(&mut buf)?;
    copy_to_user(buf_ptr, &buf[..n])?;
    Ok(n as i64)
}

// In sys_write match:
FdKind::PtyMaster(pty_num) => {
    let pair = PTY_ALLOCATOR.get(pty_num).ok_or(Errno::EBADF)?;
    pair.master_write(&buf)
}
FdKind::PtySlave(pty_num) => {
    let pair = PTY_ALLOCATOR.get(pty_num).ok_or(Errno::EBADF)?;
    pair.slave_write(&buf)
}
```

**Deliverables**:
- [ ] Master read/write operations
- [ ] Slave read/write operations
- [ ] Line discipline integration for PTY
- [ ] sys_read/sys_write support for PTY fds

---

### Phase 5: PTY ioctl Support

**Goal**: Support terminal control on PTY devices

#### 5.1 PTY-Specific ioctls

```rust
// kernel/src/syscall/ioctl.rs

// PTY-specific ioctl commands
pub const TIOCGPTN: u64 = 0x80045430;     // Get PTY number
pub const TIOCSPTLCK: u64 = 0x40045431;   // Lock/unlock PTY
pub const TIOCGPTLCK: u64 = 0x80045439;   // Get lock status

pub fn sys_ioctl_pty(fd: i32, cmd: u64, arg: u64) -> SyscallResult {
    let process = current_process();
    let fd_entry = process.fd_table.get(fd as u64)?;

    let pty_num = match fd_entry.kind {
        FdKind::PtyMaster(n) | FdKind::PtySlave(n) => n,
        _ => return Err(Errno::ENOTTY),
    };

    let pair = PTY_ALLOCATOR.get(pty_num).ok_or(Errno::EIO)?;

    match cmd {
        TIOCGPTN => {
            // Get PTY number
            copy_to_user(arg, &pty_num.to_ne_bytes())?;
            Ok(0)
        }
        TIOCSPTLCK => {
            // Set lock state
            let lock: i32 = copy_from_user(arg)?;
            pair.master.locked.store(lock != 0, Ordering::SeqCst);
            Ok(0)
        }
        TIOCGPTLCK => {
            // Get lock state
            let locked = pair.master.locked.load(Ordering::SeqCst) as i32;
            copy_to_user(arg, &locked.to_ne_bytes())?;
            Ok(0)
        }
        // Standard terminal ioctls (TCGETS, TCSETS, etc.)
        TCGETS | TCSETS | TCSETSW | TCSETSF => {
            // Delegate to existing termios handling
            handle_termios_ioctl(&pair.termios, cmd, arg)
        }
        TIOCGPGRP | TIOCSPGRP => {
            // Process group handling
            handle_pgrp_ioctl(pair, cmd, arg)
        }
        _ => Err(Errno::EINVAL),
    }
}
```

**Deliverables**:
- [ ] TIOCGPTN - get PTY number
- [ ] TIOCSPTLCK - lock/unlock PTY
- [ ] TIOCGPTLCK - get lock status
- [ ] Standard termios ioctls on PTY
- [ ] Process group ioctls on PTY

---

### Phase 6: libbreenix PTY Wrappers

**Goal**: Userspace API for PTY operations

```rust
// libs/libbreenix/src/pty.rs

use crate::syscall::{nr, raw};

/// Open a new PTY master
pub fn posix_openpt(flags: i32) -> Result<i32, i32> {
    let result = unsafe { raw::syscall1(nr::POSIX_OPENPT, flags as u64) };
    if result < 0 {
        Err(result as i32)
    } else {
        Ok(result as i32)
    }
}

/// Grant access to slave PTY
pub fn grantpt(fd: i32) -> Result<(), i32> {
    let result = unsafe { raw::syscall1(nr::GRANTPT, fd as u64) };
    if result < 0 {
        Err(result as i32)
    } else {
        Ok(())
    }
}

/// Unlock slave PTY
pub fn unlockpt(fd: i32) -> Result<(), i32> {
    let result = unsafe { raw::syscall1(nr::UNLOCKPT, fd as u64) };
    if result < 0 {
        Err(result as i32)
    } else {
        Ok(())
    }
}

/// Get slave PTY path
pub fn ptsname(fd: i32, buf: &mut [u8]) -> Result<usize, i32> {
    let result = unsafe {
        raw::syscall3(nr::PTSNAME, fd as u64, buf.as_mut_ptr() as u64, buf.len() as u64)
    };
    if result < 0 {
        Err(result as i32)
    } else {
        Ok(result as usize)
    }
}

/// Convenience function: open PTY pair and return (master_fd, slave_path)
pub fn openpty() -> Result<(i32, [u8; 32]), i32> {
    let master_fd = posix_openpt(O_RDWR | O_NOCTTY)?;
    grantpt(master_fd)?;
    unlockpt(master_fd)?;

    let mut path = [0u8; 32];
    ptsname(master_fd, &mut path)?;

    Ok((master_fd, path))
}
```

**Deliverables**:
- [ ] `posix_openpt()` wrapper
- [ ] `grantpt()` wrapper
- [ ] `unlockpt()` wrapper
- [ ] `ptsname()` wrapper
- [ ] `openpty()` convenience function

---

### Phase 7: Telnet Server

**Goal**: Create telnetd using PTY infrastructure

```rust
// userspace/programs/telnetd.rs

//! Simple telnet server using PTY
//!
//! 1. Bind to port 23
//! 2. Accept connections
//! 3. For each connection:
//!    - Create PTY pair
//!    - Fork child process
//!    - Child: open PTY slave, dup2 to stdin/stdout/stderr, exec shell
//!    - Parent: relay data between socket and PTY master

use libbreenix::{
    io::{close, dup2, read, write},
    process::{execv, exit, fork},
    pty::openpty,
    socket::{accept, bind, listen, socket, SockAddrIn, AF_INET, SOCK_STREAM},
};

const TELNET_PORT: u16 = 23;

fn handle_connection(client_fd: i32) {
    // Create PTY pair
    let (master_fd, slave_path) = match openpty() {
        Ok(pair) => pair,
        Err(_) => {
            close(client_fd as u64);
            return;
        }
    };

    let pid = fork();

    if pid == 0 {
        // Child process
        close(master_fd as u64);
        close(client_fd as u64);

        // Open slave PTY
        let slave_fd = open(&slave_path, O_RDWR);
        if slave_fd < 0 {
            exit(1);
        }

        // Set up stdin/stdout/stderr
        dup2(slave_fd as u64, 0); // stdin
        dup2(slave_fd as u64, 1); // stdout
        dup2(slave_fd as u64, 2); // stderr
        close(slave_fd as u64);

        // Exec shell
        let shell = b"/bin/init_shell\0";
        let argv: [*const u8; 2] = [shell.as_ptr(), core::ptr::null()];
        execv(shell, argv.as_ptr());
        exit(127);
    }

    // Parent process - relay data
    close(/* slave is not opened by parent */);

    loop {
        // Poll both client socket and PTY master
        // Relay data bidirectionally

        let mut buf = [0u8; 1024];

        // Read from client, write to PTY master
        let n = read(client_fd as u64, &mut buf);
        if n > 0 {
            write(master_fd as u64, &buf[..n as usize]);
        } else if n == 0 {
            break; // Client disconnected
        }

        // Read from PTY master, write to client
        let n = read(master_fd as u64, &mut buf);
        if n > 0 {
            write(client_fd as u64, &buf[..n as usize]);
        }
    }

    close(master_fd as u64);
    close(client_fd as u64);
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Create listening socket
    let listen_fd = socket(AF_INET, SOCK_STREAM, 0).unwrap();
    let addr = SockAddrIn::new([0, 0, 0, 0], TELNET_PORT);
    bind(listen_fd, &addr).unwrap();
    listen(listen_fd, 128).unwrap();

    println!("Telnet server listening on port {}", TELNET_PORT);

    loop {
        match accept(listen_fd, None) {
            Ok(client_fd) => {
                // Fork to handle connection (or handle inline for simplicity)
                handle_connection(client_fd);
            }
            Err(_) => continue,
        }
    }
}
```

**Deliverables**:
- [ ] `telnetd.rs` userspace program
- [ ] PTY creation and management
- [ ] Socket-to-PTY relay loop
- [ ] Shell spawning via fork/exec
- [ ] Boot stage test for telnetd

---

## Testing Strategy

### Unit Tests

1. **PTY Allocation Test**
   - Allocate PTY pair
   - Verify pty_num is unique
   - Release and verify cleanup

2. **PTY Data Transfer Test**
   - Write to master, read from slave
   - Write to slave, read from master
   - Verify data integrity

3. **Line Discipline Test**
   - Echo handling through PTY
   - Canonical mode line editing
   - Signal generation (Ctrl+C)

### Integration Tests

1. **PTY Syscall Test** (`pty_test.rs`)
   - posix_openpt returns valid fd
   - grantpt succeeds on master fd
   - unlockpt succeeds
   - ptsname returns valid path
   - Can open slave after unlock

2. **PTY Shell Test** (`pty_shell_test.rs`)
   - Fork child with PTY
   - Child execs shell
   - Parent can send commands via PTY master
   - Receive output via PTY master

3. **Telnet Integration Test**
   - Start telnetd
   - Connect via TCP
   - Send command, receive output
   - Disconnect cleanly

### Boot Stage Markers

```
PTY_ALLOC_OK         - PTY allocator initialized
PTY_SYSCALL_OK       - PTY syscalls registered
DEVPTS_MOUNTED       - /dev/pts/ mounted
PTY_TEST_PASSED      - PTY integration test passed
TELNETD_LISTENING    - Telnet server ready
```

---

## Dependencies

### Required Before PTY

| Dependency | Status | Notes |
|------------|--------|-------|
| TTY line discipline | ✅ | Already implemented |
| ioctl infrastructure | ✅ | TCGETS/TCSETS working |
| fork/exec | ✅ | Full implementation |
| dup2 | ✅ | Working |
| TCP sockets | ✅ | Full stack |
| devfs concept | ❌ | Need /dev/ptmx device |

### Enables After PTY

| Feature | Notes |
|---------|-------|
| Telnet server | Direct application |
| SSH server | Requires crypto (future) |
| Screen/tmux | Terminal multiplexing |
| Remote debugging | GDB over network |

---

## Timeline Estimate

| Phase | Complexity | Dependencies |
|-------|------------|--------------|
| Phase 1: Core Infrastructure | Medium | None |
| Phase 2: Syscalls | Low | Phase 1 |
| Phase 3: Filesystem | Medium | Phase 1, devfs concept |
| Phase 4: I/O Operations | Medium | Phases 1-3 |
| Phase 5: ioctl Support | Low | Phase 4 |
| Phase 6: libbreenix | Low | Phase 5 |
| Phase 7: Telnet Server | Medium | All above |

**Critical Path**: Phases 1 → 2 → 3 → 4 → 6 → 7

---

## Success Criteria

### Milestone 1: PTY Works
- [ ] `posix_openpt()` returns valid fd
- [ ] Can open `/dev/pts/N` after unlock
- [ ] Data flows master ↔ slave

### Milestone 2: Shell Over PTY
- [ ] Fork child, exec shell on PTY slave
- [ ] Parent reads/writes via PTY master
- [ ] Shell commands execute correctly

### Milestone 3: Telnet Access
- [ ] `telnet localhost 23` from host connects
- [ ] Shell prompt appears
- [ ] Commands execute and output returns
- [ ] Clean disconnect

---

## References

- Linux PTY implementation: `drivers/tty/pty.c`
- POSIX.1-2017: openpt, grantpt, unlockpt, ptsname
- FreeBSD PTY: `sys/kern/tty_pts.c`
- "The TTY Demystified": https://www.linusakesson.net/programming/tty/
