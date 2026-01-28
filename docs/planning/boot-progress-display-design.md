# Boot Progress Display Design

## Executive Summary

This document describes the design for a graphical boot progress display that visualizes POST (Power-On Self-Test) and kernel initialization progress during Breenix boot. The display supports both graphical mode (framebuffer) and ASCII mode (terminal), with parallel test execution and independent progress bars per subsystem.

## Research Findings

### Current Breenix Graphics Infrastructure

**Framebuffer Initialization Timeline (x86_64):**
1. `logger::init_early()` - Buffers log messages
2. `serial::init()` - Serial port available
3. `logger::serial_ready()` - Flushes buffered messages to serial
4. `logger::init_framebuffer()` - Direct framebuffer writes enabled
5. `memory::init()` - Heap allocator available
6. `logger::upgrade_to_double_buffer()` - Tear-free rendering
7. `graphics::render_queue::init()` - Deferred rendering queue
8. `graphics::terminal_manager::init_terminal_manager()` - Full terminal UI

**Key Files:**
- `/Users/wrb/fun/code/breenix/kernel/src/framebuffer.rs` - Low-level pixel operations using bootloader_api
- `/Users/wrb/fun/code/breenix/kernel/src/logger.rs` - ShellFrameBuffer with ANSI escape codes
- `/Users/wrb/fun/code/breenix/kernel/src/graphics/primitives.rs` - Canvas trait, fill_rect, draw_text
- `/Users/wrb/fun/code/breenix/kernel/src/graphics/terminal.rs` - TerminalPane with ANSI support
- `/Users/wrb/fun/code/breenix/kernel/src/graphics/double_buffer.rs` - DirtyRect tracking

**ARM64 Graphics:**
- `/Users/wrb/fun/code/breenix/kernel/src/graphics/arm64_fb.rs` - VirtIO GPU wrapper implementing Canvas
- `/Users/wrb/fun/code/breenix/kernel/src/drivers/virtio/gpu_mmio.rs` - VirtIO GPU driver
- Framebuffer available after VirtIO MMIO enumeration (~300 lines into kernel_main)

### External Boot Visualization Patterns

**Linux Plymouth** (from [Ubuntu Wiki](https://wiki.ubuntu.com/Plymouth), [ArchWiki](https://wiki.archlinux.org/title/Plymouth)):
- Uses KMS (Kernel Mode Setting) or framebuffer fallback
- Started as early as possible in initramfs
- Falls back to text mode if no graphics available
- UEFI systems use SimpleDRM for immediate splash before GPU driver loads

**FreeBSD** (from [FreeBSD Handbook](https://docs-archive.freebsd.org/doc/11.4-RELEASE/usr/local/share/doc/freebsd/handbook/boot-splash.html)):
- Splash screen as alternate to boot messages
- Uses VESA framebuffer or EFI framebuffer
- Lua bootloader scripting for customization

**UEFI GOP** (from [OSDev Wiki](https://wiki.osdev.org/GOP), [UEFI Spec](https://uefi.org/specs/UEFI/2.9_A/12_Protocols_Console_Support.html)):
- Graphics Output Protocol provides framebuffer before kernel takes over
- Blt() function for block transfers (progress bar drawing)
- Framebuffer persists after ExitBootServices()

### Synchronization Primitives Available

From `/Users/wrb/fun/code/breenix/kernel/src/`:
- `core::sync::atomic::{AtomicBool, AtomicUsize, AtomicU64}` - Lock-free counters
- `spin::Mutex` - Spinlock-based mutex
- `conquer_once::spin::OnceCell` - One-time initialization

---

## Architecture Overview

### Display Timeline

```
Phase 0: Pre-Framebuffer (Serial Only)
========================================
  serial::init()
  |
  |  [Serial: "Breenix Boot Progress" header]
  |
  logger::init_framebuffer()
  |
  v
Phase 1: Early Framebuffer (Direct Writes)
==========================================
  |  [Graphics: Simple solid-color progress bars]
  |  [Serial: ASCII progress bars with cursor positioning]
  |
  memory::init()  <-- Heap available
  |
  v
Phase 2: Full Graphics (Double Buffered)
========================================
  logger::upgrade_to_double_buffer()
  graphics::render_queue::init()
  |
  |  [Graphics: Smooth animated progress bars]
  |  [Serial: Same ASCII representation]
  |
  ... subsystem tests run in parallel ...
  |
  v
Phase 3: Transition to Normal Boot
==================================
  All tests complete
  |
  [Clear progress display]
  [Show normal shell/terminal]
```

### Component Diagram

```
+------------------------------------------------------------------+
|                    Boot Progress Manager                          |
|  - Tracks overall boot progress                                  |
|  - Coordinates display modes (graphics vs ASCII)                 |
|  - Manages subsystem registration                                |
+------------------------------------------------------------------+
              |                           |
              v                           v
+---------------------------+  +---------------------------+
|   Graphical Renderer      |  |    ASCII Renderer         |
|   (Canvas-based)          |  |    (ANSI escape codes)    |
|   - fill_rect for bars    |  |    - cursor positioning   |
|   - draw_text for labels  |  |    - box drawing chars    |
|   - double buffer flush   |  |    - serial output        |
+---------------------------+  +---------------------------+
              |                           |
              v                           v
+---------------------------+  +---------------------------+
|   DoubleBufferedFrameBuffer|  |    Serial Port            |
|   or direct framebuffer   |  |    (COM1/COM2)            |
+---------------------------+  +---------------------------+

+------------------------------------------------------------------+
|                    Progress State (Atomic)                        |
|  - subsystem_progress[N]: AtomicU32                              |
|  - subsystem_total[N]: u32                                       |
|  - subsystem_name[N]: &'static str                               |
+------------------------------------------------------------------+
              ^
              |
+---------------------------+  +---------------------------+
|   Subsystem Test Runner   |  |   Subsystem Test Runner   |
|   (e.g., Memory Tests)    |  |   (e.g., Timer Tests)     |
|   - Increments progress   |  |   - Increments progress   |
|   - Runs on kthread       |  |   - Runs on kthread       |
+---------------------------+  +---------------------------+
```

---

## ASCII Mockup

### Graphical Mode (Framebuffer)

```
+------------------------------------------------------------------+
|                         BREENIX OS                                |
|                    Boot Progress v0.1.0                           |
+------------------------------------------------------------------+

  Memory Manager    [####################..........] 67% (8/12)
  Timer Subsystem   [##############################] 100% (5/5)
  Interrupt Setup   [################..............] 53% (4/8)
  PCI Enumeration   [####..........................] 15% (1/7)
  Filesystem        [..............................] 0% (0/6)
  Network Stack     [..............................] 0% (0/4)
  Process Manager   [..............................] 0% (0/3)
  TTY Subsystem     [..............................] 0% (0/2)

  Overall Progress: [#########.....................] 31%

+------------------------------------------------------------------+
|  Press F12 for verbose boot log                                  |
+------------------------------------------------------------------+
```

### ASCII Mode (Serial Terminal)

```
======================== BREENIX BOOT ========================

 Memory      [========........] 50%  4/8
 Timer       [================] 100% 5/5
 Interrupts  [====............] 25%  2/8
 PCI         [................]  0%  0/7
 Filesystem  [................]  0%  0/6
 Network     [................]  0%  0/4
 Process     [................]  0%  0/3
 TTY         [................]  0%  0/2

 OVERALL     [====............] 27%

==============================================================
```

Uses ANSI escape codes:
- `\x1B[H` - Home cursor
- `\x1B[nA` / `\x1B[nB` - Move cursor up/down n lines
- `\x1B[nG` - Move cursor to column n
- `\x1B[2K` - Erase entire line
- `\x1B[?25l` / `\x1B[?25h` - Hide/show cursor

---

## Data Structures

### Core Types

```rust
// kernel/src/boot_progress/mod.rs

use core::sync::atomic::{AtomicU32, AtomicBool, Ordering};

/// Maximum number of subsystems that can register for progress tracking
pub const MAX_SUBSYSTEMS: usize = 16;

/// Unique identifier for a registered subsystem
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubsystemId(u8);

/// Progress state for a single subsystem (lock-free)
#[repr(C)]
pub struct SubsystemProgress {
    /// Current test count (completed)
    current: AtomicU32,
    /// Total test count
    total: u32,
    /// Human-readable name (max 15 chars for alignment)
    name: &'static str,
    /// Whether this slot is in use
    active: AtomicBool,
    /// Subsystem status
    status: AtomicU32, // 0=pending, 1=running, 2=passed, 3=failed
}

/// Global progress tracker
pub struct BootProgressTracker {
    /// Per-subsystem progress
    subsystems: [SubsystemProgress; MAX_SUBSYSTEMS],
    /// Number of registered subsystems
    count: AtomicU32,
    /// Whether display has been initialized
    display_ready: AtomicBool,
    /// Display mode
    mode: DisplayMode,
}

#[derive(Debug, Clone, Copy)]
pub enum DisplayMode {
    /// No display available yet
    None,
    /// Serial-only ASCII mode
    SerialOnly,
    /// Graphics mode (framebuffer available)
    Graphics,
    /// Both graphics and serial
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum SubsystemStatus {
    Pending = 0,
    Running = 1,
    Passed = 2,
    Failed = 3,
}
```

### API

```rust
impl BootProgressTracker {
    /// Register a subsystem for progress tracking.
    /// Returns None if MAX_SUBSYSTEMS reached.
    pub fn register(&self, name: &'static str, total: u32) -> Option<SubsystemId>;

    /// Increment progress for a subsystem (called from test code).
    /// This is lock-free and can be called from any context.
    pub fn increment(&self, id: SubsystemId);

    /// Set total count (if known after registration).
    pub fn set_total(&self, id: SubsystemId, total: u32);

    /// Mark subsystem as complete (passed or failed).
    pub fn complete(&self, id: SubsystemId, passed: bool);

    /// Get current progress for a subsystem.
    pub fn progress(&self, id: SubsystemId) -> (u32, u32);

    /// Get overall progress as percentage.
    pub fn overall_percent(&self) -> u8;

    /// Trigger a display refresh (called periodically or on progress change).
    pub fn refresh_display(&self);
}

// Global instance
pub static BOOT_PROGRESS: OnceCell<BootProgressTracker> = OnceCell::uninit();

// Convenience macros
#[macro_export]
macro_rules! boot_test_start {
    ($name:expr, $total:expr) => {
        $crate::boot_progress::BOOT_PROGRESS
            .get()
            .and_then(|bp| bp.register($name, $total))
    };
}

#[macro_export]
macro_rules! boot_test_progress {
    ($id:expr) => {
        if let Some(bp) = $crate::boot_progress::BOOT_PROGRESS.get() {
            bp.increment($id);
        }
    };
}
```

---

## Implementation Approach

### Phase 1: Serial-Only ASCII Mode

**Location:** `kernel/src/boot_progress/ascii.rs`

1. Initialize early (right after `serial::init()`):
   ```rust
   boot_progress::init_serial_mode();
   ```

2. Use ANSI escape codes for cursor positioning
3. Redraw only changed lines to minimize serial traffic
4. Support hidden cursor during updates

**Key Considerations:**
- No heap required
- Static buffers only
- Lock-free progress updates

### Phase 2: Early Framebuffer Mode

**Location:** `kernel/src/boot_progress/graphics.rs`

1. Initialize after `logger::init_framebuffer()`:
   ```rust
   boot_progress::init_graphics_mode(&mut framebuffer);
   ```

2. Direct framebuffer writes (no double buffer yet)
3. Simple filled rectangles for progress bars
4. Fixed-width font for text labels

**Progress Bar Drawing:**
```rust
fn draw_progress_bar(
    canvas: &mut impl Canvas,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    progress: u8,  // 0-100
    fg_color: Color,
    bg_color: Color,
) {
    // Background
    fill_rect(canvas, Rect { x, y, width, height }, bg_color);

    // Filled portion
    let filled_width = (width as u32 * progress as u32) / 100;
    fill_rect(canvas, Rect { x, y, width: filled_width, height }, fg_color);
}
```

### Phase 3: Double-Buffered Graphics

**Location:** `kernel/src/boot_progress/graphics.rs`

1. Upgrade after `logger::upgrade_to_double_buffer()`:
   ```rust
   boot_progress::upgrade_to_double_buffer();
   ```

2. Use dirty region tracking for efficient updates
3. Smooth animations possible (optional)
4. Flush only when progress changes

### Phase 4: Parallel Test Execution

**Key:** All progress updates are atomic and lock-free.

```rust
// Example: Memory subsystem tests
fn run_memory_tests() {
    let id = boot_test_start!("Memory", 8).expect("Failed to register");

    // These can run in parallel via kthreads
    test_frame_allocator();      boot_test_progress!(id);
    test_heap_allocator();       boot_test_progress!(id);
    test_kernel_stack();         boot_test_progress!(id);
    test_page_tables();          boot_test_progress!(id);
    // ... etc

    BOOT_PROGRESS.get().unwrap().complete(id, true);
}
```

### Display Refresh Strategy

1. **Timer-based:** Refresh every 50ms during boot
2. **Event-based:** Refresh on progress increment (debounced)
3. **Hybrid:** Whichever fires first

```rust
// In timer interrupt or dedicated kthread
fn boot_progress_tick() {
    static LAST_REFRESH: AtomicU64 = AtomicU64::new(0);

    let now = time::ticks();
    let last = LAST_REFRESH.load(Ordering::Relaxed);

    // Refresh at most every 50ms
    if now.saturating_sub(last) >= 50 {
        if let Some(bp) = BOOT_PROGRESS.get() {
            bp.refresh_display();
        }
        LAST_REFRESH.store(now, Ordering::Relaxed);
    }
}
```

---

## Kernel Modifications Needed

### New Files

1. `kernel/src/boot_progress/mod.rs` - Core tracker and API
2. `kernel/src/boot_progress/ascii.rs` - Serial ASCII renderer
3. `kernel/src/boot_progress/graphics.rs` - Framebuffer renderer
4. `kernel/src/boot_progress/subsystems.rs` - Subsystem definitions

### Modified Files

1. **`kernel/src/main.rs`** (x86_64):
   - Add `boot_progress::init_serial_mode()` after `serial::init()`
   - Add `boot_progress::init_graphics_mode()` after `logger::init_framebuffer()`
   - Add `boot_progress::upgrade_to_double_buffer()` after logger upgrade
   - Add `boot_progress::finish()` before showing normal terminal
   - Wrap existing init calls with progress tracking

2. **`kernel/src/main_aarch64.rs`**:
   - Same integration points as x86_64
   - Use `arm64_fb::SHELL_FRAMEBUFFER` for graphics mode

3. **`kernel/src/lib.rs`**:
   - Add `pub mod boot_progress;`

4. **`kernel/src/graphics/mod.rs`**:
   - Ensure `primitives` is accessible for progress bar drawing

5. **`kernel/Cargo.toml`**:
   - Add feature flag: `boot_progress = []`
   - Default disabled for normal builds, enabled for testing/demo

### Feature Integration

```rust
// In kernel/src/main.rs

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    logger::init_early();
    serial::init();
    logger::serial_ready();

    #[cfg(feature = "boot_progress")]
    boot_progress::init_serial_mode();

    let framebuffer = boot_info.framebuffer.as_mut().unwrap();
    logger::init_framebuffer(framebuffer.buffer_mut(), framebuffer.info());

    #[cfg(feature = "boot_progress")]
    boot_progress::init_graphics_mode();

    // ... existing initialization ...

    // Example: wrap memory init
    #[cfg(feature = "boot_progress")]
    let mem_id = boot_test_start!("Memory", 4);

    memory::init(physical_memory_offset, memory_regions);

    #[cfg(feature = "boot_progress")]
    boot_test_progress!(mem_id.unwrap());

    // ... etc ...

    #[cfg(feature = "boot_progress")]
    boot_progress::finish();

    // Normal terminal display begins
}
```

---

## Risks and Alternatives

### Risks

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Framebuffer not available early enough | Medium | High | Serial ASCII fallback is primary mode; graphics is enhancement |
| Progress display slows boot | Low | Medium | Lock-free atomics, batched updates, 50ms refresh cap |
| Display corruption with concurrent updates | Medium | Low | All display updates go through single refresh function |
| ARM64 VirtIO GPU initialization too late | High | Medium | For ARM64, may need to wait longer for graphics; serial works immediately |
| Stack overflow in render path | Low | High | Reuse existing render_queue architecture |

### Alternatives Considered

1. **EFI-Stage Boot Splash (Not Chosen)**
   - Would require modifying bootloader
   - More complex, less portable
   - Current approach works with existing bootloader

2. **Text-Only Progress (Simpler)**
   - Just print progress to serial
   - No visual appeal
   - Chosen as fallback mode

3. **Plymouth-Style External Daemon (Not Applicable)**
   - Requires userspace
   - Too late in boot process
   - We want kernel-level visibility

4. **Polling-Based Display Update (Alternative)**
   - Timer-driven refresh
   - Simpler implementation
   - May miss rapid progress changes
   - **Chosen as hybrid approach**

5. **Single Progress Bar (Simpler)**
   - One bar for overall boot
   - Less informative
   - Not chosen; per-subsystem visibility is valuable

---

## Testing Strategy

1. **Unit Tests:**
   - Progress tracker atomic operations
   - ASCII rendering output validation
   - Graphics rendering to mock canvas

2. **Integration Tests:**
   - Full boot with progress enabled
   - Verify all subsystems register and complete
   - Timing: ensure boot time increase < 100ms

3. **Visual Verification:**
   - VNC connection to verify graphics output
   - Serial log verification for ASCII output

---

## Implementation Order

1. **Week 1:** Core data structures and serial ASCII mode
2. **Week 2:** Early framebuffer graphics mode
3. **Week 3:** Double-buffered mode and smooth updates
4. **Week 4:** ARM64 support and testing

---

## References

- [Plymouth - Ubuntu Wiki](https://wiki.ubuntu.com/Plymouth)
- [Plymouth - ArchWiki](https://wiki.archlinux.org/title/Plymouth)
- [FreeBSD Boot Splash](https://docs-archive.freebsd.org/doc/11.4-RELEASE/usr/local/share/doc/freebsd/handbook/boot-splash.html)
- [UEFI GOP - OSDev Wiki](https://wiki.osdev.org/GOP)
- [UEFI Console Protocols](https://uefi.org/specs/UEFI/2.9_A/12_Protocols_Console_Support.html)
