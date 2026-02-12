# Structured CI Boot Test Architecture: Boot Test Result Table (BTRT)

**Date**: 2026-02-10
**Status**: Design complete, ready for implementation
**Branch**: Create `feat/structured-boot-tests` from `main`

## Problem

The current CI boot test system is brittle. It works by:

1. Defining 252 (x86) / 184 (ARM64) boot stages as string markers
2. Launching QEMU, capturing serial output to a file
3. Polling the file every iteration, doing substring matches against each marker
4. Tracking which markers appeared, applying pass thresholds and per-stage timeouts

This approach has fundamental problems:

- **Timing-dependent**: Serial output is asynchronous. Markers can appear out of order, be split across buffer flushes, or be interleaved with other output.
- **Fragile parsing**: Any change to log format, whitespace, or wording breaks stage detection. Alternative markers (pipe `|` syntax) are a band-aid.
- **No error context**: When a stage fails, we know which string didn't appear, but not why. The `failure_meaning` and `check_hint` fields are human guesses, not runtime data.
- **Sequential assumption**: Stages are checked in order, but many tests run concurrently. A test that completes quickly still has to wait for all prior stages to be checked.
- **Buffer flush races**: QEMU buffers ~4KB of serial output. On timeout, we send SIGTERM and wait 2 seconds hoping buffers flush. Sometimes they don't.
- **Pass thresholds are a smell**: ARM64 uses `min_stages = 120` out of 184 because we can't reliably distinguish "test hasn't run yet" from "test failed silently."

## Solution: Boot Test Result Table (BTRT)

Replace serial string matching with a structured in-memory result table that the kernel populates during boot, then exfiltrate via QMP physical memory dump.

### Core Concept

1. Kernel allocates a fixed-layout `BootTestResultTable` at a known physical address
2. Each boot test atomically writes its result (pass/fail/skip + error info) to its assigned slot
3. All tests run in a single pass, as parallel as the kernel allows
4. After boot completes, kernel writes a completion sentinel
5. Host uses QMP `pmemsave` to dump the result table from guest physical memory
6. xtask parses the binary blob and produces human-readable + KTAP output

### Why QMP `pmemsave`

The user selected QMP physical memory dump as the exfiltration mechanism. This is the right choice because:

- **No serial pollution**: The result table never touches the serial port. Serial remains available for human-readable KTAP output and kernel logs.
- **Atomic snapshot**: QMP can pause the guest, dump memory, then resume (or let it exit). No buffer flush races.
- **Binary-safe**: No base64 encoding overhead, no delimiter escaping, no character encoding issues.
- **Architecture-independent**: `pmemsave` works identically on x86_64 and aarch64 QEMU.
- **Zero kernel complexity for exfiltration**: The kernel just writes to memory. It doesn't need a base64 encoder, serial framing, or any output logic for test results.

### Why Also KTAP Serial Output

The user also wants full KTAP-formatted text on serial alongside the blob. This gives:

- **Live progress monitoring**: Watch tests complete in real-time during development
- **GitHub Actions log readability**: KTAP lines appear in the workflow log, parseable by CI tools
- **Debugging without QMP**: If QMP setup fails, serial KTAP is a fallback
- **Human-friendly**: Developers can `cat` the serial log and see results immediately

## Architecture

### 1. In-Kernel Data Structures

#### Boot Test Result Table (BTRT)

```
Offset  Size    Field               Description
──────────────────────────────────────────────────────────────────
0x0000  8       magic               0x4254_5254_0001_0001 ("BTRT" + version 1.1)
0x0008  4       total_tests         Number of test slots allocated
0x000C  4       tests_completed     Atomic: incremented as each test finishes
0x0010  4       tests_passed        Atomic: incremented on PASS
0x0014  4       tests_failed        Atomic: incremented on FAIL
0x0018  8       boot_start_ns       Monotonic nanoseconds at kernel entry
0x0020  8       boot_end_ns         Monotonic nanoseconds at boot complete
0x0028  8       reserved            Future use (checksum, flags, etc.)
0x0030  varies  results[]           Array of TestResult entries
```

Header size: 48 bytes (0x30).

#### Test Result Entry

```
Offset  Size    Field               Description
──────────────────────────────────────────────────────────────────
0x00    2       test_id             Unique test identifier (0-511)
0x02    1       status              0=PENDING, 1=RUNNING, 2=PASS, 3=FAIL, 4=SKIP, 5=TIMEOUT
0x03    1       error_code          Error category (0=none, 1=panic, 2=assertion, 3=timeout, ...)
0x04    4       duration_us         Microseconds from test start to completion
0x08    4       error_detail        Error-specific value (e.g., expected vs actual, errno, address)
0x0C    4       reserved            Alignment padding / future use
```

Entry size: 16 bytes (matches TraceEvent alignment).

For 512 test slots: 512 * 16 = 8,192 bytes.
Total BTRT size: 48 + 8,192 = 8,240 bytes (~8KB).

#### Error Detail Buffer (Optional Extension)

For tests that need to report string-level error context (e.g., "expected X got Y"), a separate 4KB error string buffer indexed by test_id. Each test gets up to 64 bytes of error text. This is optional for v1 -- the error_code + error_detail u32 covers most cases.

```
Offset  Size    Field
──────────────────────────────────────────────────────────────────
0x0000  64      error_strings[0]    NUL-terminated error text for test 0
0x0040  64      error_strings[1]    NUL-terminated error text for test 1
...
0x7FC0  64      error_strings[511]  NUL-terminated error text for test 511
```

Size: 512 * 64 = 32,768 bytes (32KB). Combined with BTRT: ~40KB total.

### 2. Physical Memory Reservation

The BTRT needs a known physical address that both the kernel and QMP can reference.

**Approach**: Reserve a page-aligned region in the kernel's BSS section with `#[no_mangle]` so the physical address is discoverable. The kernel's linker script already places BSS at deterministic addresses.

For QMP `pmemsave`, we need the **guest physical** address, not virtual. Two options:

**Option A (simpler)**: The kernel prints the physical address of the BTRT to serial once during init:
```
[btrt] Boot Test Result Table at phys 0x00000000_42100000 (8240 bytes)
```
xtask captures this single line and uses it for the QMP `pmemsave` command.

**Option B (zero-serial)**: Use a well-known fixed physical address. Reserve a region in the kernel's memory map (e.g., `0x1000_0000` on ARM64, or a specific frame on x86). This requires linker script changes but eliminates any serial dependency.

**Recommendation**: Option A for v1. It's one serial line, easily parsed, and avoids linker script complexity. Option B can be pursued later.

### 3. Tracing Integration

The BTRT is the **authoritative result store** for CI. The existing tracing system provides **debugging context**.

#### New Provider: BOOT_TEST_PROVIDER (ID: 0x07)

```
Event Type  Name              Payload
──────────────────────────────────────────────────────────────────
0x0700      TEST_REGISTER     test_id (u16) | category (u16)
0x0701      TEST_START        test_id (u16) | 0
0x0702      TEST_PASS         test_id (u16) | duration_ms (u16)
0x0703      TEST_FAIL         test_id (u16) | error_code (u16)
0x0704      TEST_SKIP         test_id (u16) | reason_code (u16)
0x0705      TEST_TIMEOUT      test_id (u16) | elapsed_ms (u16)
```

These trace events go into the existing per-CPU ring buffers. They provide:
- **Timeline**: When exactly did each test start/finish?
- **Ordering**: What was running concurrently?
- **Debugging**: On failure, `trace_dump_latest(50)` shows the events leading up to it.

#### New Counters

```
BOOT_TEST_TOTAL      - Total tests registered
BOOT_TEST_PASS       - Tests passed
BOOT_TEST_FAIL       - Tests failed
BOOT_TEST_SKIP       - Tests skipped
BOOT_TEST_TIMEOUT    - Tests timed out
```

### 4. Kernel-Side API

#### Test Registration (Compile-Time)

```rust
/// Static test catalog -- one entry per boot test, defined at compile time.
/// test_id values are stable across builds (used as BTRT array indices).
pub struct BootTestDef {
    pub id: u16,
    pub name: &'static str,
    pub category: BootTestCategory,
}

pub enum BootTestCategory {
    KernelInit,      // GDT, IDT, memory, PCI, etc.
    DriverInit,      // VirtIO, E1000, UART, etc.
    Subsystem,       // Scheduler, process manager, networking, etc.
    UserspaceExec,   // Userspace binary execution tests
    UserspaceResult, // Userspace test pass/fail results
}
```

Tests are defined in a `const` array (the catalog). Each test has a stable `id` that maps directly to a BTRT slot index.

#### Recording Results (Runtime)

```rust
/// Record a test result. Lock-free, safe from any context.
pub fn btrt_record(test_id: u16, status: TestStatus, error_code: u8, error_detail: u32);

/// Convenience wrappers
pub fn btrt_pass(test_id: u16, duration_us: u32);
pub fn btrt_fail(test_id: u16, error_code: u8, error_detail: u32);
pub fn btrt_skip(test_id: u16);
pub fn btrt_timeout(test_id: u16);
```

These are simple indexed writes to the global BTRT array. No locks needed -- each test_id maps to a unique slot, and the atomic `tests_completed` counter is incremented with `fetch_add`.

#### KTAP Serial Emission

Each `btrt_record` call also emits a KTAP line to serial:

```
ok 1 kernel_entry
ok 2 serial_init
ok 3 gdt_idt_init
not ok 4 pci_enumeration # FAIL error_code=2 detail=0x1af40001
ok 5 timer_calibration
```

This is the "full KTAP serial" output the user requested. It provides live progress on serial while the BTRT accumulates the structured data.

#### Boot Completion

After all tests complete (or a watchdog timer fires):

```rust
pub fn btrt_finalize() {
    btrt.boot_end_ns = monotonic_ns();
    btrt.magic |= BTRT_COMPLETE_FLAG; // Set high bit to signal completion
    // Emit KTAP summary to serial
    serial_println!("KTAP version 1");
    serial_println!("1..{}", btrt.total_tests);
    // ... summary lines ...
    serial_println!("# {} passed, {} failed, {} skipped",
        btrt.tests_passed, btrt.tests_failed, btrt.tests_skipped);
    serial_println!("===BTRT_READY==="); // Signal to xtask
}
```

### 5. QMP Exfiltration Flow

#### QEMU Launch (xtask)

```bash
qemu-system-aarch64 \
  -machine virt -cpu cortex-a72 -m 512M -smp 1 \
  -kernel target/aarch64-breenix/release/kernel-aarch64 \
  -drive file=target/ext2-aarch64.img,format=raw,if=virtio \
  -serial file:target/serial_output.txt \
  -qmp unix:/tmp/breenix-qmp.sock,server,nowait \
  -nographic
```

The key addition is `-qmp unix:/tmp/breenix-qmp.sock,server,nowait`.

#### Extraction Sequence (xtask)

```
1. Launch QEMU with QMP socket
2. Connect to QMP socket, send {"execute": "qmp_capabilities"}
3. Monitor serial output for "===BTRT_READY===" or "[btrt]...phys 0x..." line
4. Parse physical address from serial
5. Send: {"execute": "stop"}                     # Pause guest
6. Send: {"execute": "pmemsave", "arguments":    # Dump BTRT
           {"val": <phys_addr>, "size": 40960,
            "filename": "/tmp/btrt-results.bin"}}
7. Send: {"execute": "quit"}                     # Terminate QEMU
8. Parse /tmp/btrt-results.bin
9. Produce human-readable summary + exit code
```

#### QMP Client in xtask

xtask already has full control of the QEMU process. Adding a QMP client requires:
- Unix socket connection (Rust `std::os::unix::net::UnixStream`)
- JSON serialization (xtask already depends on `serde_json` or we add it)
- Simple request-response protocol (send JSON line, read JSON line)

This is ~100-150 lines of Rust code. No external dependencies needed beyond what xtask already has.

### 6. xtask Parser

The binary blob parser:

1. Validates magic number and version
2. Checks `tests_completed == total_tests` (all tests ran)
3. Iterates result entries, cross-references with the test catalog
4. Produces output:
   - **Console summary**: Table of pass/fail/skip with timing
   - **KTAP file**: For GitHub Actions test reporting
   - **JSON file**: Machine-readable results for dashboards/trends
   - **Exit code**: 0 if all required tests pass, 1 otherwise

### 7. CI Workflow Changes

The GitHub Actions workflow changes minimally:

```yaml
- name: Run boot tests
  run: cargo run -p xtask -- boot-test-btrt --arch arm64
  timeout-minutes: 10  # Down from 30 -- no more polling/waiting
```

The `boot-test-btrt` xtask command:
1. Builds kernel + userspace
2. Launches QEMU with QMP
3. Waits for `===BTRT_READY===` on serial (with 60s timeout)
4. Extracts BTRT via QMP
5. Parses and reports results
6. Uploads BTRT binary + parsed results as artifacts

### 8. Test Catalog: Migrating Existing Stages

The existing 252 x86 / 184 ARM64 boot stages map directly to BTRT test IDs. The migration is mechanical:

**Current** (xtask string matching):
```rust
BootStage {
    name: "Kernel entry point reached",
    marker: "Kernel entry point reached",
    failure_meaning: "Kernel failed to start",
    check_hint: "Check bootloader and kernel entry",
}
```

**New** (kernel-side BTRT recording):
```rust
// In test catalog (const, shared between kernel and xtask)
const KERNEL_ENTRY: BootTestDef = BootTestDef {
    id: 0,
    name: "kernel_entry",
    category: BootTestCategory::KernelInit,
};

// In kernel main.rs, right after entry
btrt_pass(KERNEL_ENTRY.id, elapsed_us);
```

**ID assignment convention**:
- 0-99: Kernel initialization (GDT, IDT, memory, PCI, drivers)
- 100-199: Subsystem initialization (scheduler, networking, filesystem)
- 200-299: Userspace binary load/exec tests
- 300-499: Userspace test results (signal, socket, fork, etc.)
- 500-511: Reserved for future use

### 9. Error Code Taxonomy

```
0x00    OK          No error
0x01    PANIC       Kernel panic during test
0x02    ASSERT      Assertion failure (expected != actual)
0x03    TIMEOUT     Test did not complete within deadline
0x04    NOT_FOUND   Resource not found (binary, device, file)
0x05    IO_ERROR    I/O operation failed
0x06    PERM        Permission denied
0x07    NOMEM       Out of memory
0x08    NOEXEC      Binary failed to execute
0x09    SIGNAL      Unexpected signal received
0x0A    DEADLOCK    Detected deadlock condition
0x0B    CORRUPT     Data corruption detected
0xFF    UNKNOWN     Unclassified error
```

The `error_detail` u32 carries error-specific context:
- For ASSERT: `(expected_u16 << 16) | actual_u16`
- For SIGNAL: signal number
- For IO_ERROR: errno value
- For NOT_FOUND: hash of resource name
- For TIMEOUT: elapsed milliseconds

## Implementation Plan

### Phase 1: Core Infrastructure

**Files to create:**
- `kernel/src/testing/mod.rs` -- Module root, BTRT global static, init function
- `kernel/src/testing/btrt.rs` -- BootTestResultTable struct, TestResult struct, recording API
- `kernel/src/testing/catalog.rs` -- Test catalog (const array of BootTestDef), shared IDs
- `kernel/src/testing/ktap.rs` -- KTAP serial formatter
- `kernel/src/tracing/providers/boot_test.rs` -- BOOT_TEST_PROVIDER, trace events, counters

**Files to modify:**
- `kernel/src/lib.rs` -- Add `pub mod testing;`
- `kernel/src/tracing/providers/mod.rs` -- Register BOOT_TEST_PROVIDER
- `kernel/src/tracing/mod.rs` -- Re-export boot test provider

### Phase 2: Kernel Integration

**Files to modify:**
- `kernel/src/main.rs` (x86_64) -- Replace `log::info!()` boot markers with `btrt_pass()`/`btrt_fail()` calls
- `kernel/src/main_aarch64.rs` (ARM64) -- Replace `serial_println!()` boot markers with `btrt_pass()`/`btrt_fail()` calls
- Userspace test binaries -- Add BTRT-compatible result reporting (or keep serial markers and have the kernel translate them)

**Key decision**: Userspace tests currently print markers like `"USERSPACE BRK: ALL TESTS PASSED"` to serial. Two options:
- **Option A**: Keep userspace serial markers, kernel monitors serial input and translates to BTRT entries. Complex.
- **Option B**: Userspace tests use a new syscall (e.g., `sys_test_report(test_id, status, error)`) to write directly to the BTRT. Clean but requires a new syscall.
- **Option C**: Kernel parses userspace exit codes. Each test binary exits with 0 (pass) or non-zero (fail), and the kernel's process reaper records the BTRT entry based on the exit code and known PID-to-test mapping. Pragmatic.

**Recommendation**: Option C for v1. The kernel already tracks process exit codes. When a test process exits, the process reaper maps PID -> test_id and writes the BTRT entry. No new syscalls, no serial parsing. For tests that need richer error reporting (beyond exit code), Option B can be added later.

### Phase 3: QMP Client in xtask

**Files to create:**
- `xtask/src/qmp.rs` -- QMP client (connect, capabilities, stop, pmemsave, quit)
- `xtask/src/btrt_parser.rs` -- Binary blob parser, result rendering, KTAP output

**Files to modify:**
- `xtask/src/main.rs` -- Add `boot-test-btrt` subcommand
- `xtask/Cargo.toml` -- Add `serde_json` if not already present

### Phase 4: CI Workflow Update

**Files to modify:**
- `.github/workflows/boot-tests.yml` -- Switch from `arm64-boot-stages` to `boot-test-btrt`
- Add BTRT binary + parsed JSON as workflow artifacts

### Phase 5: Cleanup

**Files to modify:**
- `xtask/src/main.rs` -- Remove old `get_boot_stages()`, `get_arm64_boot_stages()`, `boot_stages()`, `arm64_boot_stages()` functions (~2000 lines of string definitions + polling logic)
- `.github/workflows/boot-stages.yml` -- Remove deprecated workflow

## Verification

### Local Testing
```bash
# Build kernel with BTRT support
cargo build --release --features testing --target aarch64-breenix.json \
  -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem \
  -p kernel --bin kernel-aarch64

# Run with QMP enabled
qemu-system-aarch64 -machine virt -cpu cortex-a72 -m 512M -smp 1 \
  -kernel target/aarch64-breenix/release/kernel-aarch64 \
  -drive file=target/ext2-aarch64.img,format=raw,if=virtio \
  -serial mon:stdio \
  -qmp unix:/tmp/breenix-qmp.sock,server,nowait \
  -nographic

# In another terminal, after seeing ===BTRT_READY===:
echo '{"execute":"qmp_capabilities"}' | socat - UNIX-CONNECT:/tmp/breenix-qmp.sock
echo '{"execute":"pmemsave","arguments":{"val":0x42100000,"size":40960,"filename":"/tmp/btrt.bin"}}' | socat - UNIX-CONNECT:/tmp/breenix-qmp.sock

# Parse results
cargo run -p xtask -- parse-btrt /tmp/btrt.bin
```

### CI Testing
```bash
# Full xtask flow
cargo run -p xtask -- boot-test-btrt --arch arm64
cargo run -p xtask -- boot-test-btrt --arch x86_64
```

### Validation Criteria
- All previously-passing boot stages now appear as PASS in BTRT
- Failed tests include meaningful error_code and error_detail
- KTAP output on serial matches BTRT contents
- QMP extraction produces identical results to serial KTAP
- Total boot-to-result time under 60 seconds (down from 5-8 minutes)
- CI workflow runs successfully on both ubuntu-latest and ubuntu-24.04-arm

## Key Files Reference

| Component | Current Location | Purpose |
|-----------|-----------------|---------|
| x86 boot stages | `xtask/src/main.rs:237-1902` | 252 string marker definitions (TO BE REPLACED) |
| ARM64 boot stages | `xtask/src/main.rs:1903-3063` | 184 string marker definitions (TO BE REPLACED) |
| x86 boot runner | `xtask/src/main.rs:3296-3653` | Serial polling loop (TO BE REPLACED) |
| ARM64 boot runner | `xtask/src/main.rs:3673-4125` | Serial polling loop (TO BE REPLACED) |
| Tracing core | `kernel/src/tracing/core.rs` | TraceEvent (16-byte), global TRACE_BUFFERS |
| Tracing buffer | `kernel/src/tracing/buffer.rs` | Per-CPU ring buffer (1024 entries) |
| Tracing providers | `kernel/src/tracing/providers/` | Existing providers (syscall, sched, irq, process) |
| Tracing counters | `kernel/src/tracing/counter.rs` | Per-CPU atomic counters |
| Tracing output | `kernel/src/tracing/output.rs` | Lock-free serial dump, `trace_dump()` |
| Tracing macros | `kernel/src/tracing/macros.rs` | `trace_event!`, `define_trace_provider!` |
| x86 kernel main | `kernel/src/main.rs` | Boot sequence with `log::info!()` markers |
| ARM64 kernel main | `kernel/src/main_aarch64.rs` | Boot sequence with `serial_println!()` markers |
| ARM64 test loading | `kernel/src/main_aarch64.rs:595-711` | `load_test_binaries_from_ext2()` |
| CI workflow | `.github/workflows/boot-tests.yml` | GitHub Actions (calls xtask arm64-boot-stages) |
| ARM64 QEMU scripts | `docker/qemu/run-aarch64-*.sh` | Native ARM64 test scripts |

## Design Decisions Log

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Exfiltration | QMP `pmemsave` | No serial pollution, atomic snapshot, binary-safe, arch-independent |
| Serial output | Full KTAP alongside blob | Live progress, GitHub Actions readability, debugging fallback |
| Architecture scope | Both x86_64 + ARM64 | Shared catalog, shared BTRT format, arch-specific only at recording sites |
| Entry size | 16 bytes | Matches TraceEvent alignment, cache-line friendly |
| Max tests | 512 slots | Covers current 252 x86 + 184 ARM64 with room for growth |
| Userspace results | Process exit code mapping | No new syscalls needed for v1, kernel reaper maps PID->test_id |
| Error detail | u8 code + u32 detail | Covers 95% of cases without string allocation |
| BTRT address | Printed to serial at init | Simpler than linker script reservation for v1 |
