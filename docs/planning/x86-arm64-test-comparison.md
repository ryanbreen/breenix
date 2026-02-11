# Breenix Test Infrastructure: x86-64 vs ARM64 Comparison

**Last Updated**: 2026-02-11
**Branch**: `feat/test-parity-and-ci-fixes`

## Executive Summary

| Metric | x86-64 | ARM64 |
|--------|--------|-------|
| **Boot stages validated** | 252 | 184 |
| **Shared markers** | 163 (both architectures check identical userspace markers) |  |
| **Architecture-only markers** | 89 x86-only | 21 ARM64-only |
| **CI automation** | Every push/PR to main | Every push/PR to main |
| **Validation method** | Per-stage serial marker matching via xtask | Per-stage serial marker matching via xtask |
| **CI runner** | `ubuntu-latest` (x86_64) | `ubuntu-24.04-arm` (native ARM64) |
| **Test runner** | `cargo run -p xtask -- boot-stages` | `cargo run -p xtask -- arm64-boot-stages` |
| **CI status** | PASS (252/252) | PASS (184/184) locally — CI re-run pending |
| **Boot path** | UEFI bootloader → kernel | Direct kernel load (EL1) |
| **Disk format** | BXTEST embedded sectors | ext2 on virtio-blk |
| **Programs built** | 123 std binaries | 123 std binaries |

### Parity Progress

```
Feb 09: ARM64 had 2 CI checks (shell prompt + init_shell count)
Feb 10: ARM64 upgraded to 184 xtask boot stages (PR #187)
Feb 11: ARM64 CI 163/184 passing — 21 failures from ext2 Mutex contention
Feb 11: ext2 lock converted Mutex→RwLock — ARM64 184/184 passing locally
```

---

## Architecture Comparison

### Shared Infrastructure (163 stages)

Both architectures validate identical userspace test markers. The test binaries are compiled from the same Rust source (`userspace/tests/src/`) and emit the same `*_PASSED` / `*_FAILED` markers to serial output. Categories:

| Category | Shared Stages | Notes |
|----------|--------------|-------|
| Signal tests (handler, return, regs, altstack, pause, sigsuspend, kill, SIGCHLD, exec-reset, fork-inherit) | 14 | Identical behavior on both archs |
| UDP socket tests | 13 | Full coverage: bind, send, recv, EADDRINUSE, EAGAIN |
| TCP socket tests | 43 | Connect, accept, shutdown, data transfer, backlog, MSS, multi-cycle |
| DNS tests | 7 | Resolve, NXDOMAIN, edge cases |
| HTTP tests | 8 | URL validation, fetch |
| IPC tests (pipe, unix socket, dup, fcntl, cloexec, pipe2, poll, select, nonblock) | 12 | |
| TTY/session tests | 2 | |
| Filesystem tests (read, getdents, lseek, write, rename, large file, directory, link, access, devfs, CWD, exec-ext2, block-alloc) | 26 | |
| Coreutils (true, false, head, tail, wc, which, cat, ls) | 8 | |
| Rust std library tests (println, Vec, String, HashMap, getrandom, realloc, mmap, etc.) | 23 | |
| Advanced process tests (Ctrl-C, fork isolation, CoW, argv, exec-argv) | 12 | |
| FbInfo syscall test | 1 | |

### x86-64 Only Stages (89 stages, not in ARM64)

These fall into distinct categories:

#### Category 1: x86 Hardware Init (24 stages)
Hardware-specific initialization with no direct ARM64 equivalent.

| Stage | Marker | ARM64 Equivalent |
|-------|--------|-----------------|
| Kernel entry point | "Kernel entry point reached" | `[boot] Breenix ARM64 Kernel Starting` |
| Serial port initialized | "Serial port initialized and buffer flushed" | `[boot] UART initialized` |
| GDT/IDT initialized | "GDT and IDT initialized" | Exception vectors (no explicit marker) |
| GDT segment tests (5) | "GDT segment test passed" etc. | N/A — ARM64 uses EL0/EL1 privilege model |
| TSS descriptor/RSP0 tests (4) | "TSS descriptor test passed" etc. | N/A — no TSS on ARM64 |
| IST stacks updated | "Updated IST stacks with per-CPU emergency" | N/A |
| TLS initialized | "TLS initialized" | N/A — ARM64 uses TPIDR_EL0 |
| SWAPGS support enabled | "SWAPGS support enabled" | N/A |
| PIC initialized | "PIC initialized" | GICv2 init (no marker) |
| PCI bus enumerated | "PCI: Enumeration complete" | MMIO discovery (no marker) |
| E1000 device found/initialized | "E1000 network device found" etc. | virtio-net (no marker) |
| VirtIO block found/initialized/tested | "VirtIO block: Driver initialized" etc. | VirtIO MMIO (has markers) |

#### Category 2: Kernel Subsystem Init (11 stages)
Kernel-side initialization markers that ARM64 performs but doesn't emit.

| Stage | Marker | Status |
|-------|--------|--------|
| HAL per-CPU initialized | "HAL_PERCPU_INITIALIZED" | ARM64 does this, no marker |
| Physical memory available | "Physical memory offset available" | ARM64 does this, no marker |
| Network stack initialized | "Network stack initialized" | ARM64 does this, no marker |
| ARP request/reply (2) | "ARP request sent" / "NET: ARP resolved" | ARM64 does this, no marker |
| ICMP echo reply | "NET: ICMP echo reply received" | ARM64 does this, no marker |
| Syscall infrastructure ready | "System call infrastructure initialized" | ARM64 does this, no marker |
| Threading subsystem ready | "Threading subsystem initialized" | ARM64 does this, no marker |
| Process management ready | "Process management initialized" | ARM64 does this, no marker |
| Kernel tests starting | "Running kernel tests to create userspace" | ARM64 loads from ext2 instead |
| Kernel init complete | "Kernel initialization complete" | ARM64 has `[boot] Boot Complete!` |

#### Category 3: Precondition & Diagnostic Tests (17 stages)
x86-specific hardware validation and syscall regression tests.

| Stage | Marker |
|-------|--------|
| Preconditions 1-7 | IDT timer entry, Timer handler, PIT counter, PIC IRQ0, Runnable threads, Current thread, Interrupts disabled |
| All preconditions passed | "ALL PRECONDITIONS PASSED" |
| Timer resolution test | "Timer resolution test passed" |
| clock_gettime API test | "clock_gettime tests passed" |
| Breakpoint test | "Breakpoint test completed" (INT3, x86 only) |
| Ring 3 smoke test (2) | "RING3_SMOKE: created userspace PID" / "RING3_SYSCALL: First syscall" |
| Diagnostic tests 41a-41e (5) | Multiple getpid, write, clock_gettime, register preservation |

#### Category 4: Kthread / Workqueue / Softirq (27 stages)
Architecture-neutral kernel subsystem tests excluded from ARM64 xtask stages because they require the `kthread_test_only` feature flag.

| Subsystem | Stages | Status |
|-----------|--------|--------|
| Kthread (create, run, stop, exit, join, park, unpark) | 7 | Code works on ARM64, not in CI |
| Workqueue (init, kworker, basic, multiple, flush, re-queue, multi-flush, shutdown, error) | 10 | Code works on ARM64, not in CI |
| Softirq (init, register, Timer, NetRx, multiple, priority, nested, iteration, ksoftirqd, all-pass) | 10 | Code works on ARM64, not in CI |

#### Category 5: Process Scheduling Markers (10 stages)
Internal kernel scheduling markers for x86 test infrastructure.

| Stage | Marker |
|-------|--------|
| Direct execution test scheduled | "Direct execution test: process scheduled" |
| Fork test scheduled | "Fork test: process scheduled" |
| ENOSYS test scheduled | "ENOSYS test: process scheduled" |
| First userspace process scheduled | "RING3_SMOKE: created userspace PID" |
| Switched to kernel stack | "Successfully switched to kernel stack" |
| TSS.RSP0 verified | "TSS.RSP0 verified at" |
| HAL timer calibrated | "HAL_TIMER_CALIBRATED" |
| Interrupts enabled | N/A — tracked differently |
| Userspace hello | "Hello from userspace!" |
| Register init validated | "register_init_test_passed" |

### ARM64 Only Stages (21 stages, not in x86-64)

ARM64-specific boot infrastructure markers reflecting different hardware init:

| Stage | Marker | Category |
|-------|--------|----------|
| ARM64 kernel starting | `[boot] Breenix ARM64 Kernel Starting` | Boot |
| UART initialized | `[boot] UART initialized` | Hardware |
| GIC initialized | `[boot] GIC initialized` | Interrupts |
| Timer frequency detected | `[boot] Timer frequency:` | Timer |
| EL check | `[boot] Running at EL1` | Privilege |
| Memory init (3) | `[boot] Frame allocator initialized` etc. | Memory |
| Ext2 root mounted | `[boot] Ext2 root filesystem mounted` | Filesystem |
| Devfs/procfs/devptsfs init (3) | `[boot] devfs/procfs/devptsfs initialized` | VFS |
| TTY subsystem | `[boot] TTY subsystem initialized` | TTY |
| SMP boot (3) | `[smp] Secondary CPUs online` etc. | Multi-core |
| Test binary loading (2) | `[test] Loading test binaries` / `Test binaries loaded` | Test infra |
| Shell prompt | `breenix>` | Userspace |
| Init_shell count check | `init_shell` (validated ≤ 5) | Stability |

---

## CI Configuration

### Workflow: `.github/workflows/boot-tests.yml`

| Job | Arch | Runner | Stages | Status |
|-----|------|--------|--------|--------|
| `x86_64-boot-stages` | x86_64 | `ubuntu-latest` | 252 | PASS |
| `x86_64-kthread-stress` | x86_64 | `ubuntu-latest` | Kthread-specific | PASS |
| `arm64-boot` | ARM64 | `ubuntu-24.04-arm` | 184 | FAIL (163/184) |

All three jobs run on every push/PR to main.

### CI Build Pipeline Comparison

| Step | x86_64 | ARM64 |
|------|--------|-------|
| Build userspace | `build.sh` (x86_64) | `build.sh --arch aarch64` |
| Create disk | xtask embeds binaries in BXTEST sectors | `sudo create_ext2_disk.sh --arch aarch64` |
| Build kernel | `cargo build --features testing,external_test_bins` | `cargo build --target aarch64-breenix.json -Z build-std` |
| Run tests | `cargo run -p xtask -- boot-stages` | `cargo run -p xtask -- arm64-boot-stages` |
| Timeout | 20 minutes | 30 minutes |

---

## Known Issues

### ARM64 CI: Stage 125 Hang — FIXED

**Symptom**: `fs_write_test` marker never appeared in CI serial output. 21 ext2-dependent stages failed: filesystem write tests (125-129), exec-ext2 error paths (135-139), block-alloc (140), coreutils (141-148), exec-argv (182-183). 163/184 other stages passed.

**Root cause**: Global `spin::Mutex` on the ext2 filesystem (`ROOT_EXT2: Mutex<Option<Ext2Fs>>`) caused spinlock contention. All filesystem operations — reads AND writes — required exclusive access. Under slow QEMU TCG emulation in CI, a writing process held the mutex for extended I/O, blocking all concurrent exec/read/getdents operations. Multiple processes spinning on the mutex exhausted the 480s CI timeout.

**Fix**: Converted to `spin::RwLock` with separate accessor functions:
- `root_fs_read()` → shared lock for exec, file reads, getdents, stat, access, readlink
- `root_fs_write()` → exclusive lock for create, truncate, write, rename, link, unlink, mkdir, rmdir
- Refactored `sys_open` to use read lock for non-modifying opens (no O_CREAT/O_TRUNC)

**Result**: ARM64 184/184 passing locally. CI re-run pending.

---

## Gap Analysis: Path to Full Parity

### Current Gap: 68 stages (252 - 184)

| Category | Stages | Effort | Priority |
|----------|--------|--------|----------|
| **Kthread/workqueue/softirq** | 27 | Enable `kthread_test_only` in ARM64 CI | HIGH |
| **Architecture-specific init markers** | 24 | Add ARM64-equivalent markers or explicitly mark N/A | MEDIUM |
| **Kernel subsystem init markers** | 11 | Add `serial_println!` markers to `main_aarch64.rs` | MEDIUM |
| **Precondition/diagnostic tests** | 17 | Port applicable tests, mark x86-only as N/A | LOW |
| **Process scheduling markers** | 10 | Add equivalent markers or skip | LOW |

### Phase 1: Enable Kthread/Workqueue/Softirq on ARM64 (27 stages)

These tests are architecture-neutral Rust code that already works on ARM64. The only barrier is the `kthread_test_only` feature flag not being enabled in the standard ARM64 test build.

**Approach**: Either:
- Add a separate ARM64 kthread CI job (like x86_64 already has)
- Or integrate kthread/workqueue/softirq tests into the main ARM64 boot

### Phase 2: Add ARM64 Kernel Init Markers (11 stages)

Add `serial_println!` calls to `main_aarch64.rs` at the same initialization points where x86_64's `main.rs` emits them:
- HAL per-CPU init, physical memory, network stack, ARP/ICMP
- Syscall infrastructure, threading, process management

### Phase 3: Architecture-Specific Parity (24 stages)

For each x86-only hardware stage, decide:
- **Has ARM64 equivalent**: Add matching marker (e.g., GIC init → "GIC initialized")
- **No ARM64 equivalent**: Document as N/A, don't count toward parity target

### Phase 4: Fix CI Failures (21 stages) — DONE

Fixed by converting ext2 global lock from `spin::Mutex` to `spin::RwLock`. All 21 previously-failing stages now pass locally (184/184). CI re-run pending to confirm.

### Parity Target

After all phases:
- **Shared userspace stages**: 163 (identical on both)
- **ARM64-specific boot stages**: 21 (ARM64 hardware init)
- **x86-specific boot stages**: ~50 (GDT/TSS/PIC/PIT/SWAPGS/preconditions)
- **Kthread/workqueue/softirq**: 27 (on both via feature flag)
- **Added ARM64 kernel markers**: 11

**Target**: ARM64 reaches ~222 stages (184 + 27 kthread + 11 kernel markers), x86_64 stays at 252. The remaining ~30-stage gap is genuinely architecture-specific (GDT, TSS, PIC, INT3, IST) with no ARM64 equivalent.
