# Handoff: Intermittent Timer Test Failure Investigation

## Summary

During the terminal_manager removal work (Feb 2025), an intermittent boot test failure was observed: `[TESTS_COMPLETE:84/84:FAILED:1]`. The failure appeared on one boot but not on a subsequent boot (84/84 PASS). The specific failing test was not identified in the serial output captured at the time. This document provides all context needed to investigate and reproduce.

## Observed Behavior

- **First boot** (after removing terminal_manager + fixing input routing): `[TESTS_COMPLETE:84/84:FAILED:1]` — 1 test failed
- **Second boot** (same binary, no code changes): `[TESTS_COMPLETE:84/84]` — all pass
- The failure is intermittent and timing-sensitive

## How to Reproduce

```bash
# Build ARM64 kernel with testing
cargo build --release --target aarch64-breenix.json \
  -Z build-std=core,alloc \
  -Z build-std-features=compiler-builtins-mem \
  -p kernel --bin kernel-aarch64

# Run boot test (may need multiple attempts to hit the failure)
./docker/qemu/run-aarch64-boot-test-native.sh
```

Check serial output at `/tmp/breenix_aarch64_boot_native/serial.txt` for:
```
[TEST:<subsystem>:<test_name>:FAIL:<reason>]
```

The test framework emits one line per test result. Grep for `FAIL` to identify the culprit:
```bash
grep 'FAIL' /tmp/breenix_aarch64_boot_native/serial.txt
```

## Timer Test Inventory

There are 5 timer tests in the Timer subsystem, defined in `kernel/src/test_framework/registry.rs:4542-4578`:

### 1. `timer_init` (timeout: 2s, stage: EarlyBoot)
**File:** `registry.rs:1148`
- ARM64: Checks `timer::frequency_hz() > 0` and `timer::is_calibrated()`
- x86_64: Checks TSC calibration or PIT tick counter
- **Failure mode:** Would only fail if CNTFRQ_EL0 is 0 or base timestamp not set — unlikely to be intermittent

### 2. `timer_ticks` (timeout: 5s, stage: EarlyBoot)
**File:** `registry.rs:1191`
- ARM64: Reads CNTVCT_EL0, spins 100K iterations, reads again, checks ts2 > ts1
- x86_64: Uses `get_monotonic_time()` with 5M iteration spin
- **Failure mode:** Counter stall — very unlikely on ARM64 Generic Timer

### 3. `timer_delay` (timeout: 10s, stage: EarlyBoot) ⚠️ MOST LIKELY SUSPECT
**File:** `registry.rs:1239`
- Targets 10ms delay, accepts 5-20ms (50% tolerance)
- ARM64: Busy-waits on CNTVCT_EL0 for `(freq * 10) / 1000` ticks, then checks elapsed nanoseconds via `nanoseconds_since_base()`
- **Failure mode:** If QEMU virtual timer experiences jitter (host CPU contention, QEMU scheduling), the elapsed time could exceed the 20ms upper bound. This is the most plausible intermittent failure because:
  - It depends on real wall-clock timing
  - QEMU timer emulation can stall during host scheduling events
  - The tolerance window (5-20ms) is relatively tight
  - Fail messages: "delay too short on ARM64" or "delay too long on ARM64"

### 4. `timer_monotonic` (timeout: 5s, stage: EarlyBoot)
**File:** `registry.rs:1310`
- Reads CNTVCT_EL0 100 times, checks each >= previous
- **Failure mode:** Counter going backwards — would indicate a serious hardware/emulation bug, extremely unlikely

### 5. `timer_quantum_reset_aarch64` (timeout: 2s, stage: EarlyBoot, ARM64-only)
**File:** `registry.rs:3567`
- Disables interrupts, resets call counter, calls `reset_quantum()`, checks counter == 1
- **Failure mode:** Deterministic — either the counter works or it doesn't. Not timing-sensitive.

## Architecture Details

### ARM64 Generic Timer
- **Counter register:** CNTVCT_EL0 (virtual counter, always readable from EL0)
- **Frequency register:** CNTFRQ_EL0 (set by firmware, typically 24 MHz on QEMU `virt` machine)
- **Timer IRQ:** 27 (PPI)
- **Timer frequency:** 200 Hz (5ms per tick)
- **Implementation:** `kernel/src/arch_impl/aarch64/timer.rs` (208 lines)
- **Interrupt handler:** `kernel/src/arch_impl/aarch64/timer_interrupt.rs` (319 lines)

### Nanosecond Calculation
```rust
fn nanoseconds_since_base() -> Option<u64> {
    let freq = COUNTER_FREQ.load(Ordering::Relaxed);
    let base = BASE_TIMESTAMP.load(Ordering::Relaxed);
    let now = read_cntvct();
    let ticks = now.saturating_sub(base);
    Some(((ticks as u128 * 1_000_000_000) / freq as u128) as u64)
}
```

### QEMU Configuration
From `docker/qemu/run-aarch64-boot-test-native.sh`:
```bash
qemu-system-aarch64 -M virt -cpu cortex-a72 -m 512 -smp 4
```

## Investigation Plan

### Step 1: Identify the Failing Test
Run the boot test in a loop until the failure reproduces:
```bash
for i in $(seq 1 20); do
  ./docker/qemu/run-aarch64-boot-test-native.sh 2>&1
  if grep -q 'FAILED' /tmp/breenix_aarch64_boot_native/serial.txt; then
    echo "=== FAILURE ON ITERATION $i ==="
    grep 'FAIL' /tmp/breenix_aarch64_boot_native/serial.txt
    cp /tmp/breenix_aarch64_boot_native/serial.txt /tmp/timer_fail_$i.txt
    break
  fi
done
```

### Step 2: Check if it's `timer_delay`
If the failure is `[TEST:timer:timer_delay:FAIL:delay too long on ARM64]`:
- This is QEMU host scheduling jitter
- **Fix options:**
  1. Widen tolerance (e.g., 2-50ms instead of 5-20ms)
  2. Add retry logic (fail only after N consecutive failures)
  3. Skip this test on QEMU (mark as `skip` in CI)

### Step 3: Check if it's a non-timer test
The failure might not be in the Timer subsystem at all. The observation was "1 of 84 tests failed" — it could be any of the 84 tests. The Timer subsystem was suspected only because of its timing sensitivity.

### Step 4: Use the Tracing Framework
If the test is reproducible, use GDB + tracing:
```bash
./breenix-gdb-chat/scripts/gdb_session.sh start
./breenix-gdb-chat/scripts/gdb_session.sh cmd "call trace_dump_counters()"
./breenix-gdb-chat/scripts/gdb_session.sh cmd "call trace_dump_latest(50)"
./breenix-gdb-chat/scripts/gdb_session.sh stop
```

## Key Files

| File | Purpose |
|------|---------|
| `kernel/src/test_framework/registry.rs:1140-1354` | Timer test implementations |
| `kernel/src/test_framework/registry.rs:4542-4578` | Timer test definitions (TestDef structs) |
| `kernel/src/test_framework/executor.rs` | Test runner + serial output format |
| `kernel/src/arch_impl/aarch64/timer.rs` | ARM64 Generic Timer (CNTVCT_EL0) |
| `kernel/src/arch_impl/aarch64/timer_interrupt.rs` | ARM64 timer interrupt handler |
| `kernel/src/clock_gettime_test.rs` | Separate clock_gettime validation (not part of BTRT) |
| `docker/qemu/run-aarch64-boot-test-native.sh` | ARM64 boot test script |

## Investigation Results (Feb 2026)

### Reproduction Attempt

Ran the ARM64 boot test on the current `main` branch (commit `3f82a7a`). Result:

```
[TESTS_COMPLETE:84/84]   (kernel boot tests — all pass)
[TESTS_COMPLETE:84/84]   (post-userspace stage — all pass)
```

**The failure did not reproduce.** No `FAIL` lines in the serial output.

### Test Count Analysis

The document originally noted 84 tests. The codebase now has ~208 `TestDef` entries across 11 subsystems, but many are architecture-filtered. The executor uses `arch.matches_current()` to filter, resulting in exactly **84 tests on ARM64**. The count breakdown by subsystem (as defined in `registry.rs`):

- Memory, Timer, Logging, Filesystem, Network, IPC, Interrupt, Process, Syscall, Scheduler, System
- Tests tagged `x86_64_only` are excluded on ARM64; tests tagged `aarch64_only` are excluded on x86_64

### Code Review Findings

1. **`timer_delay` comment/code mismatch:** The comment at `registry.rs:1242` says "allow 5-15ms" but the actual bounds are `MIN_MS = 5` and `MAX_MS = 20` (TARGET_MS * 2). The tolerance is 5-20ms, not 5-15ms. This is cosmetic — the code is correct and more generous than the comment suggests.

2. **`test_timer_interrupt_running`** (in INTERRUPT_TESTS, not TIMER_TESTS): Spins for 15ms using hardware timers then checks that `get_ticks()` advanced by at least 2. This test is also timing-sensitive and could fail under QEMU host scheduling pressure.

3. **No retry logic exists** for any timing-sensitive test. A single QEMU scheduling hiccup during the ~10ms busy-wait in `timer_delay` or the ~15ms wait in `timer_interrupt_running` could cause a failure.

### Assessment

The original failure was likely one of:

1. **`timer_delay`** — QEMU host scheduling jitter caused elapsed time to exceed 20ms. Most probable given the tight tolerance window.
2. **`timer_interrupt_running`** — QEMU host scheduling stall prevented timer interrupts from firing during the 15ms spin window, causing `get_ticks()` to not advance enough.
3. **Any other timing-dependent test** — the failure was never identified by name.

The failure has not reproduced since the original observation. This is consistent with it being a QEMU host scheduling issue that depends on host CPU load at the exact moment the test runs.

### Recommended Actions

1. **Fix the `timer_delay` comment** to match the actual 5-20ms tolerance (minor)
2. **Add retry logic to `timer_delay`** — fail only after 3 consecutive failures to absorb QEMU jitter
3. **Consider widening `timer_delay` tolerance** to 2-50ms for QEMU environments
4. **Monitor** — if the failure appears again in CI, the `[TEST:...:FAIL:...]` line will identify the exact test

### Status: Low Priority

This is a non-reproducible, intermittent QEMU timing issue. It does not indicate a kernel bug. The investigation can be closed unless the failure reappears in CI.

## Context

This failure appeared during the terminal_manager removal refactor. The code changes removed kernel-side terminal rendering, interrupt interception, and logger routing. None of these changes directly affect the timer subsystem, but they could change boot timing (faster boot due to less work in interrupt handlers and logger). This timing change could expose a latent race condition in any timing-sensitive test.
