# Unified Platform-Agnostic Test Architecture Plan

## Problem Statement

x86_64 and ARM64 have deeply asymmetric kernel entry, test infrastructure, and boot visualization. The root cause is that x86_64's `kernel/src/main.rs` is a **standalone binary crate** that re-declares ALL modules via `mod` statements, creating a separate module tree from `kernel/src/lib.rs`. ARM64's `main_aarch64.rs` uses `extern crate kernel;` and calls into `lib.rs` directly.

This asymmetry means:
- `crate::exit_qemu()` works on ARM64 but not on x86_64 (different module trees)
- Tests are gated behind `#[cfg(target_arch = "x86_64")]` instead of being shared
- Boot visualization works on ARM64 natively but requires the `interactive` feature on x86_64
- Xtask has completely separate code paths per architecture
- Every new feature requires two implementations

**Goal**: Everything platform-specific is behind a HAL layer or embodied in the QEMU launch configuration. Kernel entry, testing, evaluation, kthread tests, and graphical boot progress are all platform-agnostic.

---

## Current State Inventory

### Duplicate Module Tree (Root Cause)

| Module | Declared in main.rs (binary) | Declared in lib.rs (library) |
|--------|------------------------------|------------------------------|
| serial | `mod serial` | `pub mod serial` |
| task | `mod task` | `pub mod task` |
| memory | `mod memory` | `pub mod memory` |
| process | `mod process` | `pub mod process` |
| syscall | `mod syscall` | `pub mod syscall` |
| drivers | `mod drivers` | `pub mod drivers` |
| test_framework | `mod test_framework` | `pub mod test_framework` |
| test_exec | `pub mod test_exec` | `pub mod test_exec` |

Every module is compiled **twice** -- once for the binary crate and once for the library crate.

### Test Functions Trapped in main.rs (x86_64-only)

| Function | Location | x86_64-specific APIs used |
|----------|----------|--------------------------|
| `test_kthread_exit_code()` | main.rs:1803 | `interrupts::enable()`, `hlt()` |
| `test_kthread_park_unpark()` | main.rs:1836 | `interrupts::enable()`, `hlt()` |
| `test_kthread_double_stop()` | main.rs:1945 | `interrupts::enable()`, `hlt()` |
| `test_kthread_should_stop_non_kthread()` | main.rs:2013 | `interrupts::enable()` |
| `test_kthread_stop_after_exit()` | main.rs:2024 | `interrupts::enable()`, `hlt()` |
| `test_kthread_stress()` | main.rs:2533 | `interrupts::enable()`, `hlt()` |
| `test_workqueue()` | main.rs:2065 | `interrupts::enable()`, `hlt()` |
| `test_softirq()` | main.rs:2295 | `interrupts::enable()`, `hlt()` |
| `test_threading()` | main.rs:2410 | `interrupts::enable()`, `hlt()` |
| `test_syscalls()` | main.rs:2449 | `interrupts::enable()`, `hlt()` |
| `test_exception_handlers()` | main.rs:2497 | `interrupts::enable()` |

All use only `interrupts::enable()` and `hlt()` -- both already have arch-generic wrappers in `kthread.rs`.

### Platform Differences Requiring HAL

| Concern | x86_64 | ARM64 | Abstraction |
|---------|--------|-------|-------------|
| Serial output | Dual COM (COM1 user + COM2 kernel) | Single PL011 UART | Serial port config |
| QEMU exit | I/O port 0xf4 | PSCI HVC SYSTEM_OFF | Already in lib.rs |
| Test binary loading | VirtIO PCI disk | ext2 filesystem | Unified loader |
| Interrupts enable/disable | `x86_64::instructions` | `msr daifclr/daifset` | Already in kthread.rs |
| Halt | `hlt` instruction | `wfi` instruction | Already in kthread.rs |
| Timestamp | `_rdtsc` (TSC) | `cntvct_el0` | Already in btrt.rs |
| Graphical boot progress | Requires `interactive` feature | Always available (VirtIO GPU) | See Phase 6 |
| Framebuffer | `logger::SHELL_FRAMEBUFFER` | `arm64_fb::SHELL_FRAMEBUFFER` | FB trait |

### Graphical Init Progress (Currently Asymmetric)

ARM64 shows a split-screen during boot:
- **Left pane (50%)**: Graphical progress panel showing 11 subsystems with color-coded progress bars
- **Right pane (50%)**: Shell/terminal

The rendering code is shared in `kernel/src/test_framework/display.rs` and `progress.rs`, but:
- ARM64: Always available (VirtIO GPU initialized early in `init_graphics()`)
- x86_64: Only available with `interactive` feature flag, using `logger::SHELL_FRAMEBUFFER`

The progress system tracks 11 subsystems (Memory, Scheduler, Interrupts, Filesystem, Network, IPC, Process, Syscall, Timer, Logging, System) across 4 boot stages (EarlyBoot, PostScheduler, ProcessContext, Userspace) with atomic counters.

---

## Phase 1: Shared Test Infrastructure (LOW RISK)

**Goal**: Move ALL remaining tests out of main.rs into shared modules.

### 1A: Extract remaining kthread tests

Move 6 kthread test functions from `main.rs` to `kernel/src/task/kthread_tests.rs`. Replace `x86_64::instructions::interrupts::enable()` with `arch_enable_interrupts()` and `hlt()` with `arch_halt()`.

**Files**:
- `kernel/src/task/kthread_tests.rs` -- add 6 new test functions
- `kernel/src/main.rs` -- remove functions, call shared versions
- `kernel/src/main_aarch64.rs` -- call new shared tests

### 1B: Extract workqueue and softirq tests

Create `kernel/src/task/workqueue_tests.rs` and `kernel/src/task/softirq_tests.rs`. These use no x86_64-specific APIs except interrupt control.

**Files**:
- `kernel/src/task/workqueue_tests.rs` (new)
- `kernel/src/task/softirq_tests.rs` (new)
- `kernel/src/task/mod.rs` -- add modules
- `kernel/src/main.rs` -- remove inline tests
- `kernel/src/main_aarch64.rs` -- call shared tests

### 1C: Unified test binary loading

Create a shared binary loading function that abstracts VirtIO PCI (x86_64) vs ext2 (ARM64):

```rust
// kernel/src/boot/test_loader.rs
pub fn load_test_binary(name: &str) -> Result<&[u8], &'static str> {
    #[cfg(target_arch = "x86_64")]
    { userspace_test::get_test_binary(name) }
    #[cfg(target_arch = "aarch64")]
    { ext2_loader::load_binary(name) }
}
```

**Files**:
- `kernel/src/boot/test_loader.rs` (new)
- `kernel/src/boot/mod.rs` (new or updated)

### 1D: Shared BTRT milestone recording

Both entry points have nearly identical BTRT milestone recording patterns. Extract to a shared function.

**Files**:
- `kernel/src/test_framework/milestones.rs` (new)

**Dependencies**: None between sub-tasks. All can be done in parallel.

---

## Phase 2: Unified Kernel Entry (HIGH RISK)

**Goal**: Eliminate the duplicate module tree by converting x86_64 main.rs to `extern crate kernel`.

### Why This Is Necessary

The dual module tree is the root cause of:
- `crate::exit_qemu()` not working from main.rs
- Every source file being compiled twice (longer builds, larger binary)
- Functions in lib.rs being invisible to the binary crate
- Test modules needing to be declared in BOTH main.rs AND lib.rs

### 2A: Preparatory refactoring (LOW RISK)

Move test-only modules that exist ONLY in main.rs to lib.rs with appropriate `#[cfg]` guards:
- `clock_gettime_test`, `rtc_test`, `time_test`, `preempt_count_test`
- `gdt_tests`, `contracts`, `contract_runner`
- `test_checkpoints`, `test_userspace`, `userspace_fault_tests`
- `stack_switch`, `spinlock`, `terminal_emulator`

**Files**:
- `kernel/src/lib.rs` -- add `pub mod` declarations with cfg guards
- `kernel/src/main.rs` -- verify modules still resolve

### 2B: The conversion (HIGH RISK)

1. Remove ALL `mod` statements from main.rs (~90 lines)
2. Add `extern crate kernel;` at the top
3. Change every `crate::` to `kernel::` (26+ occurrences)
4. Change unqualified module references to `kernel::` paths
5. Move `serial_print!`/`serial_println!` macros to lib.rs (or re-export)
6. Keep `kernel_main` function in main.rs (required by `bootloader_api::entry_point!`)

**Risk mitigation**:
- Do NOT change any logic -- only change module resolution
- Build after removing each batch of `mod` declarations
- Keep a mapping of old→new paths
- Run full test suite after conversion

### 2C: Remove QemuExitCode duplication

main.rs defines its own `QemuExitCode` and `test_exit_qemu()` separate from lib.rs. After conversion, use `kernel::exit_qemu()` directly.

**Files**:
- `kernel/src/main.rs` -- complete rewrite of imports (~200 lines changed)
- `kernel/src/lib.rs` -- ensure all modules are `pub`
- `kernel/Cargo.toml` -- dependency adjustments if needed

---

## Phase 3: HAL Abstraction (MEDIUM RISK)

**Goal**: Replace scattered `#[cfg(target_arch)]` blocks with trait-based dispatch.

### 3A: Serial abstraction

Formalize a `SerialOutput` trait. x86_64 dual-serial (COM1 user + COM2 kernel) stays as an implementation detail. ARM64 routes both to PL011.

**Files**:
- `kernel/src/arch_impl/traits.rs` -- add `SerialOutput` trait
- `kernel/src/serial.rs` -- implement for x86_64
- `kernel/src/serial_aarch64.rs` -- implement for ARM64

### 3B: QEMU exit

Already abstracted in `lib.rs` (`exit_qemu()`). After Phase 2, both entry points call `kernel::exit_qemu()`. No further work needed.

### 3C: Timer/Timestamp

The `TimerOps` trait already exists in `arch_impl/traits.rs`. Wire it up to replace cfg blocks in `btrt.rs` and elsewhere.

### 3D: Interrupt control

Already partially done in `kthread.rs`. Replace all direct `x86_64::instructions::interrupts::*` calls in shared code with `CpuOps` trait calls.

**Dependencies**: Phase 3B depends on Phase 2.

---

## Phase 4: Xtask Unification (LOW RISK)

**Goal**: Single code path for all test commands regardless of architecture.

### 4A: QemuConfig abstraction

```rust
// xtask/src/qemu_config.rs
struct QemuConfig {
    arch: Arch,
    build_command: Vec<String>,
    qemu_binary: String,
    qemu_args: Vec<String>,
    serial_files: Vec<PathBuf>,
    kernel_log_file: PathBuf,    // COM2 on x86, same file on ARM
    user_output_file: PathBuf,   // COM1 on x86, same file on ARM
    completion_marker: String,
    timeout: Duration,
}

impl QemuConfig {
    fn for_btrt(arch: Arch) -> Self { ... }
    fn for_kthread(arch: Arch) -> Self { ... }
    fn for_boot_stages(arch: Arch) -> Self { ... }
}
```

### 4B: Unified boot stage definitions

Merge `get_boot_stages()` and `get_arm64_boot_stages()` into a single function with per-arch marker strings.

### 4C: Unified test monitor

Extract the serial output monitoring loop (marker detection, timeout, BTRT extraction) into a reusable `TestMonitor`.

**Files**:
- `xtask/src/qemu_config.rs` (new)
- `xtask/src/test_monitor.rs` (new)
- `xtask/src/main.rs` -- refactor all test commands

**Dependencies**: None from kernel phases. Can be done in parallel with Phases 1-3.

---

## Phase 5: Convergence (MEDIUM RISK)

**Goal**: Eliminate remaining arch-specific test configuration.

### 5A: Unified userspace test loading

After Phase 1C creates a shared binary loader, create a single `create_all_test_processes()` function. Currently x86_64 creates ~60 processes inline in `kernel_main_continue()` (lines 960-1397) and ARM64 has a similar list in `load_test_binaries_from_ext2()` (lines 664-788).

**Files**:
- `kernel/src/boot/test_runner.rs` (new) -- shared test process creation
- `kernel/src/main.rs` -- call shared function
- `kernel/src/main_aarch64.rs` -- call shared function

### 5B: Feature flag reduction

After extracting tests to shared modules, feature flags like `kthread_stress_test`, `workqueue_test_only`, `dns_test_only` work on both architectures without per-arch gates.

### 5C: Unified idle loop

x86_64 idle loop uses `enable_and_hlt()` + scheduler yield. ARM64 uses `wfi`. Unify into `kernel_idle_loop()` in lib.rs.

### 5D: Single test catalog for xtask

`btrt_catalog.rs` becomes the single source of truth for both architectures.

**Dependencies**: 5A requires 1C + 2B. 5B requires 1A + 1B.

---

## Phase 6: Unified Graphical Boot Progress (MEDIUM RISK)

**Goal**: Both architectures show identical subsystem-by-subsystem init progress when booting graphically.

### Current State

The rendering code is already shared in `kernel/src/test_framework/display.rs`:
- 11 subsystems tracked via atomic counters in `progress.rs`
- Color-coded progress bars (Green=EarlyBoot, Blue=PostScheduler, Yellow=ProcessContext, Purple=Userspace)
- Status indicators (PEND/RUN/PASS/FAIL) per subsystem
- 600x400px panel at (40, 40) on the left side of a split screen

ARM64 initializes this in `init_graphics()` (main_aarch64.rs:383) with a split-screen layout:
- **Left pane (50%)**: Progress panel + animations
- **Divider**: 4px vertical line
- **Right pane (50%)**: Shell/terminal

x86_64 only enables this with the `interactive` feature flag, checking `logger::SHELL_FRAMEBUFFER`.

### 6A: Always-on framebuffer initialization for x86_64

The `interactive` feature gate on x86_64 graphics should be relaxed. When a framebuffer is available (UEFI provides one via bootloader), initialize the progress display unconditionally.

**Change**: In `display.rs`, replace:
```rust
#[cfg(all(target_arch = "x86_64", feature = "interactive"))]
```
with:
```rust
#[cfg(target_arch = "x86_64")]
```

And ensure x86_64's framebuffer is initialized early enough for progress rendering.

**Files**:
- `kernel/src/test_framework/display.rs` -- remove `interactive` gate for x86_64
- `kernel/src/main.rs` -- initialize framebuffer and split-screen layout early in boot (matching ARM64's `init_graphics()` pattern)

### 6B: Unified split-screen setup

Create a shared `init_boot_display()` function in lib.rs that both entry points call:

```rust
pub fn init_boot_display() -> Result<(), &'static str> {
    // 1. Detect framebuffer (UEFI fb on x86, VirtIO GPU on ARM)
    // 2. Set up split-screen: left = progress, right = terminal
    // 3. Initialize progress counters
    // 4. Render initial state
}
```

ARM64's `init_graphics()` and x86_64's framebuffer setup both call into this shared function.

**Files**:
- `kernel/src/graphics/boot_display.rs` (new) -- shared split-screen setup
- `kernel/src/main.rs` -- call `init_boot_display()` early
- `kernel/src/main_aarch64.rs` -- refactor `init_graphics()` to use shared code

### 6C: Unified progress update integration

After each subsystem initializes, call `progress::update()` and `display::request_refresh()`. This is already wired up for BTRT test results. Extend to cover non-test init milestones (memory init, scheduler init, filesystem mount, etc.) so the graphical display shows the full boot sequence, not just test results.

**Files**:
- `kernel/src/test_framework/progress.rs` -- add init milestone tracking
- `kernel/src/test_framework/display.rs` -- render init milestones alongside test progress

### 6D: Headless degradation

When running headless (no framebuffer available), the display module already degrades to a no-op. Ensure serial output continues to show KTAP progress lines regardless of graphics availability.

**Dependencies**: Phase 6B benefits from Phase 2 (shared lib.rs access). Phase 6C is independent.

---

## Implementation Ordering

```
Phase 1A (kthread tests)            ─┐
Phase 1B (workqueue/softirq tests)  ─┤─→ Phase 5B (feature flag reduction)
Phase 1C (unified binary loading)   ─┤─→ Phase 5A (unified test loading)
Phase 1D (shared BTRT milestones)   ─┘

Phase 2A (prep refactoring)   → Phase 2B (extern crate) → Phase 2C (dedup)
                                        │
                                        ├→ Phase 3B (exit_qemu done)
                                        ├→ Phase 5A (unified test loading)
                                        └→ Phase 6B (shared boot display)

Phase 3A (serial trait)       ─┐
Phase 3C (timer trait)        ─┤─→ All independent, LOW RISK
Phase 3D (interrupt control)  ─┘

Phase 4A (QemuConfig)        ─┐
Phase 4B (boot stages)       ─┤─→ Phase 5D (single catalog)
Phase 4C (test monitor)      ─┘

Phase 6A (x86 fb always-on)  ─┐
Phase 6B (split-screen setup) ─┤─→ Unified graphical boot
Phase 6C (progress updates)  ─┘
```

**Recommended execution order**:
1. **Phase 1** (all 4 sub-tasks in parallel) -- quick wins, zero risk
2. **Phase 4** (all 3 sub-tasks) -- xtask cleanup, independent of kernel
3. **Phase 2A → 2B → 2C** -- the big migration, done carefully with builds after each step
4. **Phase 3** (all 4 sub-tasks) -- clean up remaining cfg blocks
5. **Phase 6** -- unified graphical boot progress
6. **Phase 5** -- final convergence

---

## Risk Summary

| Phase | Risk | Mitigation |
|-------|------|------------|
| 1 (shared tests) | LOW | Arch wrappers already exist and are proven |
| 2 (extern crate) | HIGH | Staged migration, build after each change, no logic changes |
| 3 (HAL traits) | LOW | Mechanical replacements with defined semantics |
| 4 (xtask) | LOW | Independent of kernel, easy to test |
| 5 (convergence) | MEDIUM | Depends on earlier phases being solid |
| 6 (graphics) | MEDIUM | Framebuffer availability varies; headless degradation required |

---

## Success Criteria

When complete, the following commands produce identical output formats and test the same code paths (only QEMU launch differs):

```bash
cargo run -p xtask -- boot-test-btrt --arch x86_64
cargo run -p xtask -- boot-test-btrt --arch arm64
cargo run -p xtask -- kthread-test --arch x86_64
cargo run -p xtask -- kthread-test --arch arm64
```

Both architectures, when booted graphically, show the same split-screen layout:
- Left pane: Subsystem-by-subsystem init progress with color-coded bars
- Right pane: Shell/terminal

The kernel source has:
- Zero `#[cfg(target_arch = "x86_64")]` gates on test functions
- A single `kernel_init_common()` path called by both entry points
- All platform specifics behind HAL traits or `#[cfg]` blocks in arch_impl modules
- A single BTRT catalog and test runner shared across architectures
