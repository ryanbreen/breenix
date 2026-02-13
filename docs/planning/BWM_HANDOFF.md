# BWM (Breenix Window Manager) — Implementation Handoff

## Goal

Replace the **kernel-side terminal manager** (`kernel/src/graphics/terminal_manager.rs`) with a **userspace window manager** (`userspace/programs/src/bwm.rs`) that takes over the entire right pane (or full screen) and manages tabbed terminal sessions from userspace.

The kernel terminal manager currently handles Shell [F1], Logs [F2], and Monitor [F3] tabs with rendering done inside the kernel. This is architecturally wrong — terminal rendering belongs in userspace.

## Current State

### What Already Exists

**BWM binary** (`userspace/programs/src/bwm.rs`) — ~1035 lines, fully implemented:
- 3 tabs: Shell (spawns `/bin/bsh` via PTY), Logs (reads `/proc/kmsg`), Btop (spawns `/bin/btop` via PTY)
- Full ANSI terminal emulator per tab (`TermEmu` struct with escape sequence parser)
- Tab switching via F1/F2/F3 keys
- Framebuffer-mapped rendering (no per-character syscalls)
- Poll-based I/O loop (stdin + PTY master fds, 100ms timeout)
- Child process reaping and respawn
- Input parser for escape sequences (xterm and Linux F-key formats)

**Kernel syscalls** — all exist and work:
- `take_over_display()` (nr 431) — deactivates kernel terminal manager
- `fbinfo()` (nr 410) — returns framebuffer dimensions
- `fb_mmap()` (nr 412) — maps framebuffer into userspace (full-screen after takeover)
- `fb_flush()` (nr 411, op 6) — syncs double buffer to display
- PTY syscalls (nr 400-403) — posix_openpt, grantpt, unlockpt, ptsname
- poll() — with timeout, works on stdin and PTY fds

**Libbreenix wrappers** — all exist:
- `libbreenix::graphics` — fbinfo, fb_mmap, fb_flush, take_over_display, Framebuffer RAII wrapper
- `libbreenix::pty` — openpty() convenience function
- `libbreenix::termios` — raw mode for stdin
- `libbreenix::io` — poll, read, write, dup2, close

**Userspace btop** (`userspace/programs/src/btop.rs`) — standalone system monitor binary that reads from `/proc/pids` and `/proc/kmsg`, outputs ANSI-formatted text to stdout. BWM runs it as a child process connected via PTY.

### What Was Wrong Last Time

The previous attempt to launch BWM from init caused two problems:

1. **BWM crashed on ARM64** with DATA_ABORT (page fault). Root cause: `fb_mmap()` was patched to always return left-pane-only mapping (`width/2`), but BWM called `take_over_display()` first and expected full-screen. The DATA_ABORT occurred when BWM wrote past the mapped half-width buffer thinking it had full width.

2. **After the crash, init fell back to bsh but the system locked up**. The `take_over_display()` call had already deactivated the kernel terminal manager, so the shell had no visible output and the system appeared frozen.

Both were fixed by reverting: init spawns bsh directly, `fb_mmap()` always maps left pane only. BWM exists but is not spawned.

## Implementation Plan

### Phase 1: Fix fb_mmap for Full-Screen After Takeover

**File**: `kernel/src/syscall/graphics.rs`

The `sys_fbmmap()` function currently always maps `width/2`. After `take_over_display()`, it should map the full width. The code already has `is_display_active()` checks elsewhere — add the same conditional:

```rust
let map_width = if is_display_active() {
    // Kernel terminal manager still active → left pane only
    full_width / 2
} else {
    // Userspace took over → full framebuffer
    full_width
};
```

This was the change that was reverted. Re-apply it carefully.

### Phase 2: Update Init to Spawn BWM

**File**: `userspace/programs/src/init.rs`

Change init to spawn `/bin/bwm` instead of `/bin/bsh`. BWM will then spawn bsh as a child process connected via PTY.

```rust
const BWM_PATH: &[u8] = b"/bin/bwm\0";
// ...
let mut bwm_pid = spawn(BWM_PATH, "bwm");
```

**Critical**: BWM must be built and included in the disk image. The build script (`userspace/programs/build.sh`) needs to include bwm.

### Phase 3: Handle the Crash Recovery Problem

The biggest architectural issue: if BWM crashes, the kernel terminal manager is already deactivated, leaving the user with no visible output.

**Options** (choose one):

**Option A — Kernel fallback (recommended for now):**
Add a `give_back_display()` syscall that re-enables the kernel terminal manager. When init detects BWM died (via waitpid), it calls `give_back_display()` before respawning BWM or falling back to bsh.

```rust
// New syscall: nr 432
pub fn sys_give_back_display() -> SyscallResult {
    crate::graphics::terminal_manager::reactivate();
    Ok(0)
}
```

In `terminal_manager.rs`, add `reactivate()` that sets `DISPLAY_ACTIVE` back to true and redraws.

**Option B — BWM handles its own crash recovery:**
BWM installs a signal handler that calls `give_back_display()` on SIGSEGV/SIGBUS. More complex, requires signal infrastructure.

### Phase 4: Graceful Display Handoff

The current `take_over_display()` is abrupt. Consider:

1. **Kernel clears the right pane** before deactivating (optional, cosmetic)
2. **BWM waits for fbinfo** to confirm it has the right dimensions after takeover
3. **BWM does a full screen clear** before rendering its first frame
4. **Logging during handoff** should go to serial only (kernel terminal is off, BWM isn't rendering yet)

### Phase 5: Remove Kernel Terminal Manager (Long Term)

Once BWM is stable, the kernel terminal manager becomes dead code. However, keep it available as a fallback (gated behind a feature flag) for headless/debugging scenarios.

## Key Files to Modify

| File | Change |
|------|--------|
| `kernel/src/syscall/graphics.rs` | Restore full-screen mmap after takeover |
| `kernel/src/syscall/dispatcher.rs` | Register give_back_display syscall |
| `kernel/src/syscall/mod.rs` | Add SYS_GIVE_BACK_DISPLAY constant |
| `kernel/src/graphics/terminal_manager.rs` | Add reactivate() function |
| `userspace/programs/src/init.rs` | Spawn bwm instead of bsh |
| `userspace/programs/build.sh` | Ensure bwm binary is built and included |
| `libs/libbreenix/src/syscall.rs` | Add GIVE_BACK_DISPLAY syscall number |
| `libs/libbreenix/src/graphics.rs` | Add give_back_display() wrapper |

## Architecture After Implementation

```
Init (PID 1)
  ├── BWM (PID 2) — userspace window manager
  │     ├── bsh (PID 3) — shell via PTY [Shell tab, F1]
  │     └── btop (PID 4) — monitor via PTY [Btop tab, F3]
  │     └── [Logs tab reads /proc/kmsg directly, F2]
  └── telnetd (PID 5, optional)

Display Layout:
┌──────────────┬──────────────┐
│  Left Pane   │  Right Pane  │
│  (Demos:     │  (BWM:       │
│   confetti,  │   Shell/     │
│   bounce)    │   Logs/      │
│              │   Btop tabs) │
└──────────────┴──────────────┘
```

Left pane continues to be managed by kernel (visual demo programs with left-pane-only fbmmap). Right pane is BWM's territory after `take_over_display()`.

**Important**: The display split means BWM should only render to the right half. Currently `take_over_display()` + `fb_mmap()` returns the full screen. Either:
- BWM renders to full screen and kernel demos are disabled, OR
- `fb_mmap()` after takeover returns only the right pane (same width/2 but the RIGHT half), OR
- BWM is aware of the split and only renders to `width/2..width`

The simplest approach: BWM takes over the full screen. Left-pane demos stop when BWM starts. This avoids complex buffer sharing.

## Testing

1. Build: `cargo build --release --features testing,external_test_bins --bin qemu-uefi`
2. Build ARM64: `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`
3. Build userspace: `./userspace/programs/build.sh`
4. Run: `./run.sh --clean`
5. Verify BWM starts, tabs work (F1/F2/F3), shell is interactive
6. Kill BWM (if possible) and verify kernel terminal manager reactivates
7. Verify CPU usage is sane (~1-5%, not 300-400%)

## Known Issues to Watch For

1. **CPU usage**: The poll() loop with 100ms timeout should be efficient. If CPU spikes, check that poll() is actually sleeping (not busy-looping on EAGAIN).

2. **fb_flush() path**: On ARM64, flush goes through VirtIO GPU. On x86_64, flush copies to the shadow buffer. Both paths are tested.

3. **PTY data flow**: PTY master read should block in poll() when no data. If btop or bsh produces output faster than BWM can render, the PTY buffer will fill and the child will block on write — this is correct behavior (backpressure).

4. **Font rendering**: BWM uses libgfx 5x7 glyphs at 2x scale (10x14 px per cell). This is the same font as the kernel terminal manager.

5. **ANSI compatibility**: The TermEmu in bwm.rs handles ~20 CSI commands. If btop or bsh use unsupported sequences, they'll be silently ignored. Add support as needed.

6. **Memory**: Each TermEmu allocates `cols * rows * sizeof(Cell)` bytes. At 80x40 = 3200 cells × ~5 bytes = ~16KB per tab. Total ~48KB for 3 tabs, trivial.
