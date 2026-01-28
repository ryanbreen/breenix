# Intra-Kernel Parallel Test Execution System

## Design Document v1.0

**Date:** 2026-01-28
**Status:** Design Phase
**Scope:** Single-boot parallel test execution with real-time graphical progress visualization

---

## 1. Executive Summary

This document describes a system for running all kernel boot tests concurrently within a **single kernel boot**, with real-time graphical progress visualization. The key insight is that many tests are I/O-bound (DNS, TCP, filesystem) and naturally yield, allowing the scheduler to interleave CPU-bound tests with minimal overhead.

### Vision

```
Single Kernel Boot
    |
    +-> [Memory Thread]     ||||||||.. 80%  (CPU-bound, fast)
    +-> [Network Thread]    ||........ 20%  (I/O-bound, waiting on DNS)
    +-> [Filesystem Thread] ||||...... 40%  (I/O-bound, ext2 reads)
    +-> [Process Thread]    ||||||.... 60%  (fork/exec, yields during wait)
    +-> [Scheduler Thread]  |||....... 30%  (context switch tests)

    All running concurrently, progress bars advancing in parallel
```

---

## 2. Current State Analysis

### 2.1 Boot Sequence (x86_64)

The current x86_64 boot sequence (`kernel/src/main.rs`):

1. **Early Init** (no heap, no interrupts):
   - Logger init (buffered)
   - Serial port init
   - GDT/IDT init
   - Per-CPU data init

2. **Memory Init** (heap available):
   - Physical memory mapping
   - Frame allocator init
   - Heap allocator init
   - Double-buffer upgrade for framebuffer

3. **Multi-terminal Display Init**:
   - Graphics demo on left pane
   - Terminal manager on right pane (F1: Shell, F2: Logs)
   - This happens at line ~260 in `kernel_main()`

4. **Driver Init**:
   - PCI enumeration
   - Network stack
   - ext2 filesystem
   - devfs/devptsfs

5. **Threading Init** (after stack switch):
   - Scheduler with idle task
   - Process manager
   - Workqueue subsystem
   - Softirq subsystem
   - Render thread (interactive mode)

6. **Test Execution**:
   - kthread tests (sequential)
   - User process creation (sequential)
   - Enter idle loop

### 2.2 Boot Sequence (ARM64)

The ARM64 boot sequence (`kernel/src/main_aarch64.rs`):

1. **Early Init**:
   - Physical memory offset init
   - Serial (PL011) init
   - Memory management init (frame allocator, heap)

2. **Interrupt Init**:
   - Generic Timer calibration
   - GICv2 init
   - UART interrupts

3. **Driver Init**:
   - VirtIO enumeration
   - Network stack
   - ext2 filesystem
   - VirtIO GPU

4. **Graphics Init** (line ~294):
   - `init_graphics()` initializes VirtIO GPU
   - Split-screen terminal UI
   - Graphics demo on left, terminal on right

5. **Threading Init**:
   - Per-CPU data
   - Process manager
   - Scheduler with idle task
   - Timer interrupt for preemption

6. **Userspace Launch**:
   - Load init_shell from ext2
   - Enter interactive loop with VirtIO keyboard

### 2.3 Framebuffer Infrastructure

**x86_64:**
- Uses bootloader-provided UEFI GOP framebuffer
- `SHELL_FRAMEBUFFER` in `logger.rs` wraps double-buffered framebuffer
- Supports RGB/BGR formats, multiple bytes-per-pixel
- Graphics primitives: lines, rectangles, circles, text

**ARM64:**
- Uses VirtIO GPU (MMIO transport)
- `SHELL_FRAMEBUFFER` in `graphics/arm64_fb.rs`
- Fixed 1280x800 @ 32bpp (B8G8R8A8_UNORM)
- GPU commands via virtqueue for flush operations

**Shared:**
- `Canvas` trait in `graphics/primitives.rs`
- `TerminalManager` for tabbed interface
- Font rendering with anti-aliasing
- Color blending for transparent text

### 2.4 Kthread Infrastructure

```rust
// kernel/src/task/kthread.rs

pub fn kthread_run<F>(func: F, name: &str) -> Result<KthreadHandle, KthreadError>
where
    F: FnOnce() + Send + 'static;

pub fn kthread_stop(handle: &KthreadHandle) -> Result<(), KthreadError>;
pub fn kthread_should_stop() -> bool;
pub fn kthread_park();
pub fn kthread_unpark(handle: &KthreadHandle);
pub fn kthread_join(handle: &KthreadHandle) -> Result<i32, KthreadError>;
pub fn kthread_exit(code: i32) -> !;
```

Key characteristics:
- Kthreads run with interrupts enabled (timer can preempt)
- `kthread_park()`/`kthread_unpark()` for sleep/wake coordination
- `kthread_join()` blocks caller until thread exits
- Thread registry uses `BTreeMap<u64, Arc<Kthread>>`

### 2.5 Existing Test Infrastructure

Tests are currently run sequentially:
- `test_kthread_lifecycle()` - verifies spawn/stop/join
- `test_workqueue()` - deferred work execution
- `test_softirq()` - bottom-half processing
- User process tests (`hello_time`, `clock_gettime_test`, etc.)

Test markers use `serial_println!()` for checkpoint output:
```rust
// kernel/src/test_checkpoints.rs
#[macro_export]
macro_rules! test_checkpoint {
    ($name:expr) => {
        #[cfg(feature = "testing")]
        log::info!("[CHECKPOINT:{}]", $name);
    };
}
```

---

## 3. Parallel Test Architecture

### 3.1 Test Subsystem Registry

Tests are organized by subsystem. Each subsystem has multiple tests that run in a single kthread:

```rust
// New file: kernel/src/test_runner/mod.rs

use alloc::sync::Arc;
use core::sync::atomic::{AtomicU32, AtomicBool, Ordering};

/// Test result status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestResult {
    Pass,
    Fail,
    Skip,
    Panic,
}

/// Individual test definition
pub struct TestDef {
    pub name: &'static str,
    pub func: fn() -> TestResult,
    /// Architecture filter (None = both, Some("x86_64") = x86_64 only)
    pub arch: Option<&'static str>,
}

/// A subsystem containing multiple tests
pub struct Subsystem {
    pub name: &'static str,
    pub tests: &'static [TestDef],
    /// Runtime progress tracking
    pub completed: AtomicU32,
    pub total: AtomicU32,
    pub failed: AtomicBool,
    pub current_test: AtomicU32,  // Index of currently running test
}

/// Global registry of all subsystems
pub static SUBSYSTEMS: &[&Subsystem] = &[
    &MEMORY_SUBSYSTEM,
    &SCHEDULER_SUBSYSTEM,
    &NETWORK_SUBSYSTEM,
    &FILESYSTEM_SUBSYSTEM,
    &PROCESS_SUBSYSTEM,
    &SIGNAL_SUBSYSTEM,
];
```

### 3.2 Subsystem Definitions

```rust
// kernel/src/test_runner/subsystems/memory.rs

pub static MEMORY_SUBSYSTEM: Subsystem = Subsystem {
    name: "Memory",
    tests: &[
        TestDef {
            name: "heap_allocation",
            func: test_heap_allocation,
            arch: None,
        },
        TestDef {
            name: "frame_allocator",
            func: test_frame_allocator,
            arch: None,
        },
        TestDef {
            name: "kernel_stack",
            func: test_kernel_stack,
            arch: None,
        },
        TestDef {
            name: "brk_syscall",
            func: test_brk_syscall,
            arch: None,
        },
        TestDef {
            name: "mmap_syscall",
            func: test_mmap_syscall,
            arch: None,
        },
    ],
    completed: AtomicU32::new(0),
    total: AtomicU32::new(5),
    failed: AtomicBool::new(false),
    current_test: AtomicU32::new(0),
};

fn test_heap_allocation() -> TestResult {
    use alloc::vec::Vec;
    let mut v: Vec<u64> = Vec::with_capacity(1000);
    for i in 0..1000 {
        v.push(i);
    }
    if v.iter().sum::<u64>() == 499500 {
        TestResult::Pass
    } else {
        TestResult::Fail
    }
}
```

### 3.3 Parallel Test Executor

```rust
// kernel/src/test_runner/executor.rs

use crate::task::kthread::{kthread_run, kthread_join, KthreadHandle};
use alloc::vec::Vec;

/// Spawn one kthread per subsystem, return handles
pub fn spawn_all_test_threads() -> Vec<(KthreadHandle, &'static Subsystem)> {
    let current_arch = if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else {
        "aarch64"
    };

    SUBSYSTEMS
        .iter()
        .filter_map(|subsystem| {
            // Count tests for this architecture
            let test_count = subsystem.tests.iter()
                .filter(|t| t.arch.map(|a| a == current_arch).unwrap_or(true))
                .count() as u32;

            if test_count == 0 {
                return None;
            }

            subsystem.total.store(test_count, Ordering::Release);
            subsystem.completed.store(0, Ordering::Release);
            subsystem.failed.store(false, Ordering::Release);

            let thread_name = alloc::format!("test/{}", subsystem.name);
            let handle = kthread_run(
                move || run_subsystem_tests(subsystem, current_arch),
                Box::leak(thread_name.into_boxed_str()),
            ).ok()?;

            Some((handle, *subsystem))
        })
        .collect()
}

fn run_subsystem_tests(subsystem: &'static Subsystem, arch: &str) {
    for (idx, test) in subsystem.tests.iter().enumerate() {
        // Skip tests not for this architecture
        if let Some(test_arch) = test.arch {
            if test_arch != arch {
                continue;
            }
        }

        subsystem.current_test.store(idx as u32, Ordering::Release);

        let result = (test.func)();

        match result {
            TestResult::Pass | TestResult::Skip => {}
            TestResult::Fail | TestResult::Panic => {
                subsystem.failed.store(true, Ordering::Release);
            }
        }

        subsystem.completed.fetch_add(1, Ordering::AcqRel);
    }
}

/// Wait for all test threads to complete
pub fn join_all(handles: Vec<(KthreadHandle, &'static Subsystem)>) -> (u32, u32) {
    let mut total_pass = 0u32;
    let mut total_fail = 0u32;

    for (handle, subsystem) in handles {
        let _ = kthread_join(&handle);

        let completed = subsystem.completed.load(Ordering::Acquire);
        let failed = subsystem.failed.load(Ordering::Acquire);

        if failed {
            total_fail += 1;
        } else {
            total_pass += completed;
        }
    }

    (total_pass, total_fail)
}
```

---

## 4. Framebuffer Fast Path

### 4.1 Early Display Strategy

The goal is to show progress visualization as early as possible. We need:

1. **x86_64:** Framebuffer is available immediately from bootloader (UEFI GOP)
2. **ARM64:** VirtIO GPU requires enumeration and initialization

**Proposal: Deferred Progress Display**

Rather than rushing framebuffer init, we:
1. Start tests immediately after threading is ready
2. Initialize graphics in parallel (separate kthread)
3. Show progress once graphics is ready
4. Update display at 10-20 Hz (50-100ms intervals)

This avoids blocking test execution on graphics init.

### 4.2 Minimal Graphics Init (x86_64)

```rust
// Already minimal - framebuffer from bootloader
fn init_progress_display_x86_64() -> Option<ProgressDisplay> {
    let fb = logger::SHELL_FRAMEBUFFER.get()?;
    let mut fb_guard = fb.lock();

    let (width, height) = {
        let w = graphics::primitives::Canvas::width(&*fb_guard);
        let h = graphics::primitives::Canvas::height(&*fb_guard);
        (w, h)
    };

    Some(ProgressDisplay::new(&mut *fb_guard, width, height))
}
```

### 4.3 Minimal Graphics Init (ARM64)

```rust
// kernel/src/drivers/virtio/gpu_mmio.rs modifications

/// Minimal early init - just display, no fancy features
pub fn init_minimal() -> Result<(), &'static str> {
    // Standard VirtIO GPU init (already exists)
    init()?;

    // Clear to dark background
    if let Some(buffer) = framebuffer() {
        let dark_blue: [u8; 4] = [50, 30, 20, 255]; // BGRA
        for chunk in buffer.chunks_exact_mut(4) {
            chunk.copy_from_slice(&dark_blue);
        }
    }

    flush()?;
    Ok(())
}
```

---

## 5. Progress Display System

### 5.1 Visual Layout

```
+------------------------------------------------------------------+
|                    BREENIX PARALLEL TEST RUNNER                   |
+------------------------------------------------------------------+
|                                                                    |
|  Memory      [||||||||||||||||||||..................] 80%  PASS   |
|  Scheduler   [|||||||||............................] 35%  RUN    |
|  Network     [||....................................] 10%  WAIT   |
|  Filesystem  [|||||||||||||.........................] 50%  RUN    |
|  Process     [||||||||||||||||||....................] 70%  PASS   |
|  Signal      [..........................................] 0%   PEND   |
|                                                                    |
|  Total: 245/500 tests | 3 subsystems complete | 0 failures       |
|                                                                    |
|  Current: Scheduler::test_context_switch_latency                  |
|           Network::test_dns_resolution (waiting for response)     |
|           Filesystem::test_ext2_read_large_file                   |
|                                                                    |
+------------------------------------------------------------------+
```

### 5.2 Progress Display Data Structure

```rust
// kernel/src/test_runner/display.rs

use crate::graphics::primitives::{Canvas, Color, Rect, TextStyle, fill_rect, draw_text};
use core::sync::atomic::Ordering;

const BAR_WIDTH: usize = 300;
const BAR_HEIGHT: usize = 16;
const ROW_HEIGHT: usize = 24;
const LEFT_MARGIN: usize = 20;
const LABEL_WIDTH: usize = 100;

#[derive(Debug, Clone, Copy)]
pub enum SubsystemState {
    Pending,    // Not started
    Running,    // Tests executing
    Waiting,    // I/O blocked (DNS, disk, etc.)
    Complete,   // All tests done
    Failed,     // At least one failure
}

pub struct ProgressDisplay {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    subsystem_count: usize,
}

impl ProgressDisplay {
    pub fn new(canvas: &mut impl Canvas, width: usize, height: usize) -> Self {
        let display = Self {
            x: 0,
            y: 0,
            width,
            height,
            subsystem_count: SUBSYSTEMS.len(),
        };

        // Draw initial frame
        display.draw_frame(canvas);
        display
    }

    fn draw_frame(&self, canvas: &mut impl Canvas) {
        // Dark background
        fill_rect(canvas, Rect {
            x: self.x as i32,
            y: self.y as i32,
            width: self.width as u32,
            height: self.height as u32,
        }, Color::rgb(20, 30, 50));

        // Title
        let title_style = TextStyle::new()
            .with_color(Color::WHITE)
            .with_background(Color::rgb(40, 60, 100));
        draw_text(canvas,
            (self.x + self.width / 2 - 150) as i32,
            (self.y + 10) as i32,
            "BREENIX PARALLEL TEST RUNNER",
            &title_style,
        );
    }

    pub fn update(&self, canvas: &mut impl Canvas) {
        let start_y = self.y + 50;

        for (idx, subsystem) in SUBSYSTEMS.iter().enumerate() {
            let row_y = start_y + idx * ROW_HEIGHT;

            let completed = subsystem.completed.load(Ordering::Acquire);
            let total = subsystem.total.load(Ordering::Acquire);
            let failed = subsystem.failed.load(Ordering::Acquire);

            let state = if total == 0 {
                SubsystemState::Pending
            } else if failed {
                SubsystemState::Failed
            } else if completed >= total {
                SubsystemState::Complete
            } else {
                SubsystemState::Running
            };

            self.draw_subsystem_row(canvas, row_y, subsystem.name, completed, total, state);
        }

        self.draw_summary(canvas, start_y + SUBSYSTEMS.len() * ROW_HEIGHT + 20);
    }

    fn draw_subsystem_row(
        &self,
        canvas: &mut impl Canvas,
        y: usize,
        name: &str,
        completed: u32,
        total: u32,
        state: SubsystemState
    ) {
        let x = self.x + LEFT_MARGIN;

        // Label
        let label_style = TextStyle::new().with_color(Color::WHITE);
        draw_text(canvas, x as i32, y as i32, name, &label_style);

        // Progress bar background
        let bar_x = x + LABEL_WIDTH;
        fill_rect(canvas, Rect {
            x: bar_x as i32,
            y: y as i32,
            width: BAR_WIDTH as u32,
            height: BAR_HEIGHT as u32,
        }, Color::rgb(50, 50, 50));

        // Progress bar fill
        let progress = if total > 0 {
            (completed as usize * BAR_WIDTH) / total as usize
        } else {
            0
        };

        let bar_color = match state {
            SubsystemState::Pending => Color::rgb(80, 80, 80),
            SubsystemState::Running => Color::rgb(100, 150, 255),
            SubsystemState::Waiting => Color::rgb(255, 200, 100),
            SubsystemState::Complete => Color::rgb(100, 255, 100),
            SubsystemState::Failed => Color::rgb(255, 100, 100),
        };

        if progress > 0 {
            fill_rect(canvas, Rect {
                x: bar_x as i32,
                y: y as i32,
                width: progress as u32,
                height: BAR_HEIGHT as u32,
            }, bar_color);
        }

        // Percentage
        let pct = if total > 0 { (completed * 100) / total } else { 0 };
        let pct_text = alloc::format!("{}%", pct);
        draw_text(canvas, (bar_x + BAR_WIDTH + 10) as i32, y as i32, &pct_text, &label_style);

        // State indicator
        let state_text = match state {
            SubsystemState::Pending => "PEND",
            SubsystemState::Running => "RUN",
            SubsystemState::Waiting => "WAIT",
            SubsystemState::Complete => "PASS",
            SubsystemState::Failed => "FAIL",
        };
        let state_color = bar_color;
        let state_style = TextStyle::new().with_color(state_color);
        draw_text(canvas, (bar_x + BAR_WIDTH + 50) as i32, y as i32, state_text, &state_style);
    }

    fn draw_summary(&self, canvas: &mut impl Canvas, y: usize) {
        let mut total_completed = 0u32;
        let mut total_tests = 0u32;
        let mut subsystems_complete = 0u32;
        let mut failures = 0u32;

        for subsystem in SUBSYSTEMS.iter() {
            let completed = subsystem.completed.load(Ordering::Acquire);
            let total = subsystem.total.load(Ordering::Acquire);
            let failed = subsystem.failed.load(Ordering::Acquire);

            total_completed += completed;
            total_tests += total;

            if completed >= total && total > 0 {
                subsystems_complete += 1;
            }
            if failed {
                failures += 1;
            }
        }

        let summary = alloc::format!(
            "Total: {}/{} tests | {} subsystems complete | {} failures",
            total_completed, total_tests, subsystems_complete, failures
        );

        let style = TextStyle::new().with_color(Color::rgb(200, 200, 200));
        draw_text(canvas, (self.x + LEFT_MARGIN) as i32, y as i32, &summary, &style);
    }
}
```

### 5.3 Display Update Loop

The display updates via a dedicated kthread that polls subsystem progress:

```rust
// kernel/src/test_runner/display_thread.rs

use crate::task::kthread::{kthread_run, kthread_should_stop, kthread_park, KthreadHandle};
use core::sync::atomic::{AtomicBool, Ordering};

static DISPLAY_READY: AtomicBool = AtomicBool::new(false);
static TESTS_COMPLETE: AtomicBool = AtomicBool::new(false);

pub fn spawn_display_thread() -> Option<KthreadHandle> {
    kthread_run(display_thread_fn, "test/display").ok()
}

fn display_thread_fn() {
    // Wait for graphics to be ready
    while !graphics_ready() && !kthread_should_stop() {
        kthread_park();
    }

    if kthread_should_stop() {
        return;
    }

    // Initialize progress display
    #[cfg(target_arch = "x86_64")]
    let fb = match crate::logger::SHELL_FRAMEBUFFER.get() {
        Some(fb) => fb,
        None => return,
    };

    #[cfg(target_arch = "aarch64")]
    let fb = match crate::graphics::arm64_fb::SHELL_FRAMEBUFFER.get() {
        Some(fb) => fb,
        None => return,
    };

    let display = {
        let mut fb_guard = fb.lock();
        let width = crate::graphics::primitives::Canvas::width(&*fb_guard);
        let height = crate::graphics::primitives::Canvas::height(&*fb_guard);
        ProgressDisplay::new(&mut *fb_guard, width, height)
    };

    DISPLAY_READY.store(true, Ordering::Release);

    // Update loop at ~20Hz
    let update_interval_ms = 50;
    let mut last_update = crate::time::monotonic_ms();

    while !kthread_should_stop() && !TESTS_COMPLETE.load(Ordering::Acquire) {
        let now = crate::time::monotonic_ms();
        if now - last_update >= update_interval_ms {
            {
                let mut fb_guard = fb.lock();
                display.update(&mut *fb_guard);

                // Flush
                #[cfg(target_arch = "x86_64")]
                if let Some(db) = fb_guard.double_buffer_mut() {
                    db.flush_if_dirty();
                }
                #[cfg(target_arch = "aarch64")]
                fb_guard.flush();
            }
            last_update = now;
        }

        // Yield to let test threads run
        crate::task::scheduler::yield_current();
    }

    // Final update showing completion
    {
        let mut fb_guard = fb.lock();
        display.update(&mut *fb_guard);

        #[cfg(target_arch = "x86_64")]
        if let Some(db) = fb_guard.double_buffer_mut() {
            db.flush_if_dirty();
        }
        #[cfg(target_arch = "aarch64")]
        fb_guard.flush();
    }
}

fn graphics_ready() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        crate::logger::SHELL_FRAMEBUFFER.get().is_some()
    }
    #[cfg(target_arch = "aarch64")]
    {
        crate::graphics::arm64_fb::SHELL_FRAMEBUFFER.get().is_some()
    }
}

pub fn signal_tests_complete() {
    TESTS_COMPLETE.store(true, Ordering::Release);
}
```

---

## 6. Test Registration API

### 6.1 Declarative Test Definition

Tests are defined using const statics for zero-overhead registration:

```rust
// Example: kernel/src/test_runner/subsystems/network.rs

pub static NETWORK_SUBSYSTEM: Subsystem = Subsystem {
    name: "Network",
    tests: &[
        TestDef {
            name: "loopback_ping",
            func: test_loopback_ping,
            arch: None,
        },
        TestDef {
            name: "udp_sendrecv",
            func: test_udp_sendrecv,
            arch: None,
        },
        TestDef {
            name: "tcp_connect",
            func: test_tcp_connect,
            arch: None,
        },
        TestDef {
            name: "dns_resolution",
            func: test_dns_resolution,
            arch: None, // Works on both
        },
    ],
    completed: AtomicU32::new(0),
    total: AtomicU32::new(4),
    failed: AtomicBool::new(false),
    current_test: AtomicU32::new(0),
};

fn test_loopback_ping() -> TestResult {
    // Send ICMP echo to 127.0.0.1
    // Wait for response with timeout
    // ...
    TestResult::Pass
}

fn test_dns_resolution() -> TestResult {
    // This naturally yields while waiting for DNS response
    let result = crate::net::dns::resolve("example.com");
    match result {
        Ok(_) => TestResult::Pass,
        Err(_) => TestResult::Fail,
    }
}
```

### 6.2 Architecture-Specific Tests

```rust
// Example: x86_64-specific memory tests

TestDef {
    name: "page_table_walk_x86",
    func: test_page_table_walk_x86,
    arch: Some("x86_64"),
}

TestDef {
    name: "mmu_arm64_granule",
    func: test_mmu_granule,
    arch: Some("aarch64"),
}
```

### 6.3 Adding New Subsystems

1. Create `kernel/src/test_runner/subsystems/<name>.rs`
2. Define `static <NAME>_SUBSYSTEM: Subsystem`
3. Add to `SUBSYSTEMS` array in `kernel/src/test_runner/mod.rs`

---

## 7. Failure Handling

### 7.1 Panic Interception

Panics in test threads should not crash the kernel:

```rust
// kernel/src/test_runner/panic_guard.rs

use core::panic::PanicInfo;

/// Wrapper that catches panics in test functions
pub fn run_test_guarded(test: &TestDef) -> TestResult {
    // Set thread-local panic handler that records failure
    // instead of halting

    // This requires panic=abort to be disabled for test builds
    // Alternative: use catch_unwind if available

    // For now, panics will crash the subsystem thread
    // The display thread can detect this via kthread_join() timeout
    (test.func)()
}
```

### 7.2 Subsystem Isolation

Each subsystem runs in its own kthread. A panic in one subsystem:
1. Terminates that subsystem's thread
2. Other subsystems continue running
3. Display thread marks subsystem as FAIL
4. Final report shows which subsystem panicked

### 7.3 Timeout Handling

For tests that might hang:

```rust
// Timeout wrapper (requires timer infrastructure)
fn run_with_timeout(test: fn() -> TestResult, timeout_ms: u64) -> TestResult {
    let start = crate::time::monotonic_ms();

    // Spawn test in current thread (blocking)
    // Check elapsed time periodically if test yields
    // For truly blocking tests, use watchdog thread

    let result = test();

    let elapsed = crate::time::monotonic_ms() - start;
    if elapsed > timeout_ms {
        serial_println!("Warning: test exceeded timeout ({}ms > {}ms)", elapsed, timeout_ms);
    }

    result
}
```

---

## 8. Cross-Architecture Considerations

### 8.1 Shared Code

The following is architecture-independent:
- `Subsystem` and `TestDef` definitions
- Progress tracking (atomic counters)
- Test executor logic
- Display layout calculations

### 8.2 Architecture-Specific Code

**x86_64:**
- Framebuffer access via `logger::SHELL_FRAMEBUFFER`
- Double-buffering with dirty region tracking
- HLT instruction for idle

**ARM64:**
- Framebuffer access via `arm64_fb::SHELL_FRAMEBUFFER`
- VirtIO GPU flush commands
- WFI instruction for idle

### 8.3 Conditional Compilation

```rust
#[cfg(target_arch = "x86_64")]
mod x86_tests {
    pub fn test_sse_disabled() -> TestResult { ... }
    pub fn test_syscall_instruction() -> TestResult { ... }
}

#[cfg(target_arch = "aarch64")]
mod arm64_tests {
    pub fn test_svc_instruction() -> TestResult { ... }
    pub fn test_timer_frequency() -> TestResult { ... }
}
```

---

## 9. Implementation Phases

### Phase 1: Infrastructure (2-3 days)

1. Create `kernel/src/test_runner/` module structure
2. Implement `Subsystem` and `TestDef` types
3. Implement parallel executor (`spawn_all_test_threads`, `join_all`)
4. Add feature flag `parallel_tests` to `Cargo.toml`

**Deliverable:** Tests can be spawned in parallel kthreads

### Phase 2: Progress Tracking (1-2 days)

1. Add atomic progress counters to subsystems
2. Implement `ProgressDisplay` struct
3. Add display update loop (serial output first)

**Deliverable:** Serial output shows parallel progress

### Phase 3: Graphical Display (2-3 days)

1. Implement progress bar rendering
2. Create display thread with 20Hz update
3. Test on both x86_64 and ARM64
4. Add color coding for states

**Deliverable:** Visual progress bars in framebuffer

### Phase 4: Test Migration (3-5 days)

1. Move existing tests into subsystem structure
2. Add architecture filtering
3. Convert userspace process tests
4. Verify test coverage maintained

**Deliverable:** All existing tests running in parallel framework

### Phase 5: Polish (1-2 days)

1. Add timeout handling
2. Improve panic reporting
3. Add test summary at boot
4. Documentation

**Deliverable:** Production-ready parallel test system

---

## 10. Appendices

### A. File Structure

```
kernel/src/test_runner/
    mod.rs              # Module root, SUBSYSTEMS array
    subsystem.rs        # Subsystem, TestDef types
    executor.rs         # spawn_all_test_threads, join_all
    display.rs          # ProgressDisplay
    display_thread.rs   # Display update kthread

    subsystems/
        mod.rs          # Subsystem module declarations
        memory.rs       # Memory subsystem tests
        scheduler.rs    # Scheduler subsystem tests
        network.rs      # Network subsystem tests
        filesystem.rs   # Filesystem subsystem tests
        process.rs      # Process subsystem tests
        signal.rs       # Signal subsystem tests
```

### B. Cargo.toml Changes

```toml
[features]
# Enable parallel test execution
parallel_tests = ["testing"]

# Enable with: cargo build --features parallel_tests
```

### C. Integration Point

In `kernel/src/main.rs` (x86_64) and `kernel/src/main_aarch64.rs`:

```rust
#[cfg(feature = "parallel_tests")]
fn run_parallel_tests() {
    use test_runner::{executor, display_thread};

    // Spawn display thread first
    let display_handle = display_thread::spawn_display_thread();

    // Spawn all test subsystem threads
    let test_handles = executor::spawn_all_test_threads();

    // Wait for all tests to complete
    let (passed, failed) = executor::join_all(test_handles);

    // Signal display thread to finish
    display_thread::signal_tests_complete();
    if let Some(handle) = display_handle {
        let _ = kthread_join(&handle);
    }

    serial_println!("=== PARALLEL TESTS COMPLETE: {} passed, {} failed ===", passed, failed);

    if failed > 0 {
        serial_println!("PARALLEL_TESTS_FAILED");
    } else {
        serial_println!("PARALLEL_TESTS_PASSED");
    }
}
```

### D. Example Serial Output

```
[test_runner] Spawning 6 subsystem test threads
[test/Memory] Starting 5 tests
[test/Scheduler] Starting 8 tests
[test/Network] Starting 4 tests
[test/Filesystem] Starting 6 tests
[test/Process] Starting 7 tests
[test/Signal] Starting 3 tests
[display] Progress display ready

Progress: Memory 2/5 | Scheduler 3/8 | Network 0/4 (waiting) | Filesystem 2/6 | Process 4/7 | Signal 1/3

[test/Memory] PASS: heap_allocation
[test/Scheduler] PASS: thread_spawn
[test/Filesystem] PASS: ext2_read_superblock
[test/Network] Waiting: dns_resolution (timeout 5000ms)
[test/Process] PASS: fork_basic

Progress: Memory 5/5 COMPLETE | Scheduler 5/8 | Network 1/4 | Filesystem 4/6 | Process 7/7 COMPLETE | Signal 3/3 COMPLETE

[test/Memory] All 5 tests passed
[test/Network] PASS: dns_resolution
[test/Scheduler] PASS: context_switch_latency

Progress: Memory PASS | Scheduler 8/8 COMPLETE | Network 4/4 COMPLETE | Filesystem 6/6 COMPLETE | Process PASS | Signal PASS

=== PARALLEL TESTS COMPLETE: 33 passed, 0 failed ===
PARALLEL_TESTS_PASSED
```

---

**End of Design Document**
