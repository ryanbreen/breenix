# Init System & Virtual Consoles Implementation Plan

## Overview

Replace `init_shell` as PID 1 with a proper init system that:
1. Manages system services (telnetd, etc.)
2. Supports multiple interaction modes (console, telnet, serial)
3. Spawns shells on virtual consoles like Linux
4. Handles process supervision and cleanup

## Current State

- `init_shell` runs as PID 1 (interactive mode)
- No service management
- No virtual console support
- telnetd exists but has test limits and must be manually started

## Architecture

### Init Process Responsibilities

```
/sbin/init (PID 1)
├── Parse /etc/inittab or built-in config
├── Mount filesystems (future)
├── Start services
│   ├── telnetd on port 2323
│   └── (future: sshd, httpd, etc.)
├── Spawn getty on virtual consoles
│   ├── /dev/tty1 → getty → login → shell
│   ├── /dev/tty2 → getty → login → shell
│   └── /dev/ttyS0 → getty (serial console)
├── Reap orphaned processes (wait for zombies)
└── Handle shutdown signals
```

### Virtual Console Model (Linux-style)

```
/dev/console  - System console (kernel messages)
/dev/tty0     - Current virtual console
/dev/tty1-6   - Virtual text consoles (Alt+F1 through Alt+F6)
/dev/ttyS0    - Serial console (QEMU -serial stdio)
/dev/pts/*    - Pseudo-terminals (telnet, ssh, screen)
```

## Implementation Phases

### Phase 1: Basic Init Structure

**Goal:** Create init binary that starts services and shells

**Files:**
- `userspace/tests/init.rs` - Main init process
- `kernel/src/main.rs` - Load init instead of init_shell

**Behavior:**
```rust
// init.rs pseudocode
fn main() {
    // We are PID 1
    println!("Breenix init starting...");

    // Start telnetd in background
    if fork() == 0 {
        exec("/bin/telnetd");
    }

    // Start shell on console
    if fork() == 0 {
        exec("/bin/init_shell");
    }

    // Reap zombies forever
    loop {
        waitpid(-1, WNOHANG);
        yield();
    }
}
```

### Phase 2: Virtual Console Infrastructure

**Goal:** Implement /dev/ttyN virtual consoles

**Kernel Changes:**
- `kernel/src/tty/vt.rs` - Virtual terminal multiplexer
- `kernel/src/tty/console.rs` - Console output routing
- Support Alt+Fn switching between consoles

**Each VT has:**
- Input buffer (keyboard → VT)
- Output buffer (VT → screen)
- Foreground process group
- Termios settings

### Phase 3: Getty/Login

**Goal:** Proper login flow on consoles

**Files:**
- `userspace/tests/getty.rs` - Open TTY, prompt for login
- `userspace/tests/login.rs` - Authenticate and exec shell (future)

**Flow:**
```
init spawns: getty /dev/tty1
getty: opens /dev/tty1, sets termios, prints "login: "
getty: reads username, execs login
login: (future: authenticate), execs shell
shell: runs as user session
```

For now (single-user): getty → shell directly

### Phase 4: Service Management

**Goal:** Structured service start/stop

**Config format (simple):**
```
# /etc/inittab or built-in
::sysinit:/bin/mount -a
::respawn:/sbin/getty /dev/tty1
::respawn:/sbin/getty /dev/tty2
::once:/sbin/telnetd
```

**Respawn logic:**
- If service exits, restart it
- Rate-limit restarts (don't spin)

### Phase 5: Serial Console Support

**Goal:** Shell accessible via QEMU serial

**Current:** Kernel output goes to serial
**Needed:** Bidirectional serial I/O for shell

**Approach:**
- `/dev/ttyS0` backed by UART 0x3F8
- getty spawns on ttyS0
- Serial becomes interactive shell

## File Structure

```
userspace/tests/
├── init.rs          # PID 1 init process
├── getty.rs         # TTY login prompt
├── init_shell.rs    # Interactive shell (unchanged)
└── telnetd.rs       # Telnet server (remove test limits)

kernel/src/tty/
├── mod.rs           # TTY subsystem
├── pty/             # Pseudo-terminals (done)
├── vt.rs            # Virtual terminal multiplexer (new)
└── console.rs       # Console driver (new)
```

## Migration Path

### Step 1: Create init binary
- Copy minimal logic from init_shell
- Fork telnetd and shell
- Reap zombies

### Step 2: Update kernel to load init
- Change `kernel_main_continue()` to load `/sbin/init`
- Keep init_shell as fallback

### Step 3: Fix telnetd
- Remove MAX_ATTEMPTS/MAX_CYCLES limits
- Run as proper daemon

### Step 4: Add virtual consoles
- Implement /dev/tty1-6
- Add console switching

### Step 5: Add getty
- Simple program: open tty, print prompt, exec shell

## Success Criteria

1. **Boot sequence:**
   ```
   Kernel starts
   init (PID 1) starts
   telnetd starts (PID 2)
   getty/shell starts on console (PID 3)
   ```

2. **Telnet works:**
   ```bash
   # From host
   telnet localhost 2323
   # Get shell prompt
   breenix>
   ```

3. **Process tree:**
   ```
   PID 1: init
   ├── PID 2: telnetd
   │   └── PID N: init_shell (per connection)
   └── PID 3: init_shell (console)
   ```

4. **Zombie reaping:**
   - Exited processes don't accumulate
   - init waits on children

## Open Questions

1. **Config format:** Built-in vs /etc/inittab file?
   - Start built-in, add file support later

2. **Console switching:** How to handle Alt+Fn in QEMU?
   - QEMU may intercept these; may need different keys

3. **Serial vs VGA console:** Which is primary?
   - Serial for now (easier in QEMU)
   - VGA console requires framebuffer work

## References

- Linux init(8) man page
- systemd architecture (for modern comparison)
- SysV init behavior
- FreeBSD init/rc system
