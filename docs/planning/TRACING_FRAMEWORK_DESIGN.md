# Breenix Tracing Framework Design

## DTrace-Style Lock-Free Tracing for Kernel Observability

**Status**: Design Document
**Date**: 2026-02-02
**Authors**: Ryan Breen, Claude Code

---

## Table of Contents

1. [Executive Summary](#executive-summary)
2. [Research Findings](#research-findings)
3. [Current State Analysis](#current-state-analysis)
4. [Proposed Architecture](#proposed-architecture)
5. [Core API Design](#core-api-design)
6. [Implementation Roadmap](#implementation-roadmap)
7. [File/Module Structure](#filemodule-structure)
8. [Appendices](#appendices)

---

## 1. Executive Summary

This document proposes a comprehensive, lock-free tracing framework for Breenix inspired by DTrace, Linux ftrace/eBPF, and FreeBSD's DTrace port. The framework enables safe, low-overhead observability in critical kernel paths including interrupt handlers, syscalls, and context switches.

### Key Design Goals

1. **Lock-free operation**: Safe in interrupt handlers, context switches, and syscall paths
2. **Minimal overhead**: Near-zero cost when disabled; minimal impact when enabled
3. **Cross-architecture support**: Works on both x86-64 and ARM64
4. **Provider/probe model**: Subsystems define their own trace points
5. **Atomic counters**: High-performance statistical aggregation
6. **Future /proc exposure**: Designed for eventual procfs visibility

---

## 2. Research Findings

### 2.1 DTrace Architecture

[DTrace](https://en.wikipedia.org/wiki/DTrace), pioneered by Sun Microsystems, provides dynamic tracing without restarts or recompilation.

**Key Principles (from [Oracle DTrace Guide](https://docs.oracle.com/cd/E18752_01/html/819-5488/gcfpv.html)):**

1. **Provider/Probe Model**: Providers are kernel modules that instrument the system and publish probes. Each probe is identified by a 4-tuple: `provider:module:function:name`.

2. **Dynamic Instrumentation**: DTrace works by dynamically patching live running instructions. Static Tracing is also supported where trace points are compiled in.

3. **D Language**: User-programmable actions execute when probes fire. The D language enables filtering and summarization in-kernel before passing to userland - critical for performance.

4. **DIF Virtual Machine**: Actions execute in a simple, safe virtual machine. Misaligned loads and division by zero are caught and handled gracefully.

5. **Destructive Actions**: Most actions are non-destructive (just record state). Destructive actions that modify system state require explicit enablement.

**FreeBSD Implementation Notes (from [FreeBSD DTrace Chapter](https://docs.freebsd.org/en/books/handbook/dtrace/)):**
- Implemented as loadable kernel modules
- Requires CTF (Compact Type Format) for type introspection
- Security: Only root can use DTrace
- Some Solaris providers (cpc, mib) not yet ported

### 2.2 Linux ftrace and Tracepoints

[Linux ftrace](https://docs.kernel.org/trace/ftrace.html) provides function tracing and event tracing infrastructure.

**Ring Buffer Design (from [Lockless Ring Buffer Design](https://docs.kernel.org/trace/ring-buffer-design.html)):**

1. **Per-CPU Buffers**: Separate ring buffer per CPU to avoid cache bouncing and enable lockless writes on the local CPU.

2. **Single Atomic Store**: Each event is written with a single atomic store after index increment.

3. **Writer Nesting**: Writers form a "stack" through interrupt preemption. An interrupted writer must wait for the nested writer to complete.

4. **cmpxchg Operations**: Compare-and-swap used for lockless coordination with two flag bits encoded in page pointers (HEADER, UPDATE).

5. **Two Modes**:
   - **Overwrite mode** (default): Old data overwritten when full
   - **Producer/consumer mode**: New data dropped when full

6. **Page-Based Management**: Three pointers: head_page (reader swap target), tail_page (next write), commit_page (last finished write).

**TRACE_EVENT Macro**: Defines trace events with:
- `name`: Event identifier
- `proto`: Function prototype
- `args`: Arguments passed to probe
- `struct entry`: Fields recorded in ring buffer
- `assign`: Field assignment logic
- `print`: Output format

### 2.3 eBPF Tracing

[eBPF](https://ebpf.io/) is a modern, programmable tracing infrastructure for Linux.

**Key Concepts:**

1. **Event-Driven Programs**: eBPF programs attach to hook points (syscalls, function entry/exit, tracepoints, network events).

2. **Verifier**: Ensures BPF programs are safe - no loops, no backward branches, no crashes.

3. **JIT Compilation**: Generic bytecode compiled to machine-specific instructions.

4. **eBPF Maps**: Share data between kernel and userspace. Types include hash tables, arrays, ring buffers, stack traces.

5. **Kprobes/Kretprobes**: Dynamic attachment to any kernel function entry/exit.

6. **CO-RE (Compile Once - Run Everywhere)**: Programs compiled once can work across kernel versions via BTF type information.

### 2.4 Redox OS Approach

[Redox OS](https://www.redox-os.org/) uses ptrace-based tracing via file handles:
- System call tracing through kernel synchronization
- Single-stepping via x86_64 FLAGS TF bit
- Stop breakpoints for debugger control (PTRACE_STOP_EXEC)

### 2.5 Lock-Free Ring Buffer Patterns

Key insights from [ring buffer implementations](https://kmdreko.github.io/posts/20191003/a-simple-lock-free-ring-buffer/):

1. **Minimize Atomic Operations**: Each atomic operation has overhead; high-quality implementations minimize them.

2. **ABA Problem**: Ring buffers reuse memory, making them susceptible to ABA problems.

3. **Memory Ordering**: Use `Ordering::Release` for writes, `Ordering::Acquire` for reads. `SeqCst` provides strongest guarantees but x86_64 makes most orderings equivalent.

4. **SPSC vs MPMC**: Single-producer single-consumer is simplest; multi-producer requires more complex coordination.

---

## 3. Current State Analysis

### 3.1 Existing Trace Infrastructure

Breenix has two existing trace implementations:

#### `/kernel/src/trace.rs` (Main Tracing Module)

**Capabilities:**
- Lock-free ring buffer (256 entries)
- Single atomic store per event
- Event type encoding: [8-bit type][56-bit payload]
- Global enable/disable
- Predefined event types for context switches, interrupts, syscalls, scheduler, locks, page tables
- Macros: `trace_ctx_switch!`, `trace_irq_entry!`, `trace_syscall_entry!`, `trace_marker!`

**Limitations:**
- Single global buffer (not per-CPU)
- No timestamp support
- No provider/probe abstraction
- No counter support
- No dynamic enabling of individual probes
- dump_trace() uses serial output (locks)

#### `/kernel/src/arch_impl/aarch64/trace.rs` (ARM64 Debug Tracing)

**Capabilities:**
- Ultra-minimal: 256-byte static buffer
- Single-byte markers for GDB inspection
- No locks, no allocations, no serial output
- Atomic index increment for thread safety

**Limitations:**
- ARM64-only
- No structure beyond byte markers
- No timestamps, counters, or event metadata

### 3.2 Related Infrastructure

#### IRQ-Safe Logging (`/kernel/src/irq_log.rs`)
- Per-CPU ring buffers (8 KiB)
- Recursion guard for flushing
- Currently bypassed due to debugging issues

#### Per-CPU Data (`/kernel/src/per_cpu.rs`)
- Cache-aligned (64 bytes) per-CPU structure
- Linux-style preempt_count with bit-field layout
- Accessed via GS segment (x86_64) or TPIDR_EL1 (ARM64)

#### Time Infrastructure (`/kernel/src/time/`)
- TSC-based nanosecond timestamps (x86_64)
- Generic timer (ARM64)
- `get_monotonic_time_ns()` for timestamps

---

## 4. Proposed Architecture

### 4.1 Design Principles

Following DTrace, ftrace, and kernel best practices:

1. **Hierarchy**: `Provider -> Probe -> Action`
2. **Per-CPU Ring Buffers**: Eliminates contention, enables lockless local writes
3. **Static + Dynamic Probes**: Compile-time static probes with runtime enable/disable
4. **In-Kernel Aggregation**: Counters, histograms computed in-kernel
5. **GDB-First Debugging**: Primary inspection via GDB, secondary via serial

### 4.2 High-Level Architecture

```
+-------------------+     +-------------------+     +-------------------+
|   SYSCALL PROVIDER|     |   SCHED PROVIDER  |     |   IRQ PROVIDER    |
|  - sys_entry      |     |  - ctx_switch     |     |  - irq_entry      |
|  - sys_exit       |     |  - schedule       |     |  - irq_exit       |
|  - sys_clock_get  |     |  - preempt        |     |  - timer_tick     |
+--------+----------+     +--------+----------+     +--------+----------+
         |                         |                         |
         v                         v                         v
+------------------------------------------------------------------------+
|                        TRACING CORE                                     |
|                                                                         |
|  +------------------+  +------------------+  +------------------+       |
|  |  Probe Registry  |  |  Event Recorder  |  |  Counter Engine  |       |
|  |  (static array)  |  |  (per-CPU rings) |  |  (atomic ops)    |       |
|  +------------------+  +------------------+  +------------------+       |
|                                                                         |
|  +------------------+  +------------------+                              |
|  |  Timestamp Src   |  |  Enable Bitmap   |                              |
|  |  (TSC/generic)   |  |  (per-provider)  |                              |
|  +------------------+  +------------------+                              |
+------------------------------------------------------------------------+
         |
         v
+------------------------------------------------------------------------+
|                        OUTPUT LAYER                                     |
|                                                                         |
|  +------------------+  +------------------+  +------------------+       |
|  |  GDB Inspection  |  |  Serial Dump     |  |  Future: /proc   |       |
|  |  (x/Nxb symbol)  |  |  (post-mortem)   |  |  (read-only)     |       |
|  +------------------+  +------------------+  +------------------+       |
+------------------------------------------------------------------------+
```

### 4.3 Core Components

#### 4.3.1 Trace Event Structure

```rust
/// A trace event stored in the ring buffer.
/// Optimized for single atomic store (16 bytes, aligned).
#[repr(C, align(16))]
pub struct TraceEvent {
    /// Timestamp in nanoseconds (TSC-based or generic timer)
    pub timestamp: u64,
    /// Event type (8 bits) + flags (8 bits) + payload (48 bits)
    /// Format: [type:8][flags:8][payload:48]
    pub data: u64,
}

impl TraceEvent {
    #[inline(always)]
    pub const fn new(event_type: u8, flags: u8, payload: u48) -> Self {
        Self {
            timestamp: 0, // Filled at write time
            data: ((event_type as u64) << 56)
                | ((flags as u64) << 48)
                | (payload as u64 & 0x0000_FFFF_FFFF_FFFF),
        }
    }
}
```

#### 4.3.2 Per-CPU Ring Buffer

```rust
/// Per-CPU trace ring buffer.
/// Cache-line aligned to prevent false sharing.
#[repr(C, align(64))]
pub struct TraceCpuBuffer {
    /// Ring buffer entries
    entries: [AtomicU128; TRACE_BUFFER_SIZE],
    /// Write index (atomic, wraps with mask)
    write_idx: AtomicUsize,
    /// Read index (only accessed during dump, relaxed ordering)
    read_idx: AtomicUsize,
    /// Events dropped due to overflow (for diagnostics)
    dropped: AtomicU64,
    /// Buffer sequence number for detecting wrap-around
    sequence: AtomicU64,
}

const TRACE_BUFFER_SIZE: usize = 1024; // Power of 2 for efficient masking
const TRACE_BUFFER_MASK: usize = TRACE_BUFFER_SIZE - 1;
```

#### 4.3.3 Provider/Probe Model

```rust
/// A trace provider represents a kernel subsystem that emits events.
pub struct TraceProvider {
    /// Provider name (e.g., "syscall", "sched", "irq")
    pub name: &'static str,
    /// Unique provider ID (compile-time assigned)
    pub id: u8,
    /// Probes offered by this provider
    pub probes: &'static [TraceProbe],
    /// Enable bitmap for this provider's probes
    pub enabled: AtomicU64,
}

/// A trace probe is a specific instrumentation point.
pub struct TraceProbe {
    /// Probe name (e.g., "entry", "exit", "clock_gettime")
    pub name: &'static str,
    /// Probe ID within provider (0-63)
    pub id: u8,
    /// Event type for this probe
    pub event_type: u8,
}
```

#### 4.3.4 Atomic Counters

```rust
/// Per-CPU atomic counter for high-frequency statistics.
/// Designed for contention-free increment in hot paths.
#[repr(C, align(64))]
pub struct TraceCounter {
    /// Per-CPU values (index by cpu_id)
    values: [AtomicU64; MAX_CPUS],
    /// Counter name for reporting
    pub name: &'static str,
    /// Counter ID
    pub id: u16,
}

impl TraceCounter {
    #[inline(always)]
    pub fn increment(&self) {
        let cpu_id = current_cpu_id();
        self.values[cpu_id].fetch_add(1, Ordering::Relaxed);
    }

    /// Get aggregated total (sum across all CPUs)
    pub fn total(&self) -> u64 {
        self.values.iter()
            .map(|v| v.load(Ordering::Relaxed))
            .sum()
    }
}
```

### 4.4 Memory Layout

```
Per-CPU Trace Buffer (embedded in PerCpuData):
+------------------+
| TraceCpuBuffer   |  ~16 KiB per CPU (1024 entries * 16 bytes)
| - entries[1024]  |
| - write_idx      |
| - read_idx       |
| - dropped        |
| - sequence       |
+------------------+

Global Trace State:
+------------------+
| TraceState       |
| - providers[]    |  Static array of registered providers
| - counters[]     |  Static array of counters
| - global_enable  |  Master enable flag
| - per_cpu_ptrs[] |  Pointers to per-CPU buffers
+------------------+
```

Memory budget per CPU: ~16 KiB ring buffer + 64 bytes metadata = ~16.5 KiB

---

## 5. Core API Design

### 5.1 Probe Definition Macros

```rust
/// Define a trace provider with its probes.
///
/// # Example
/// ```rust
/// define_trace_provider! {
///     provider SYSCALL {
///         id: 0x01,
///         probes: [
///             ENTRY    (0x00, "syscall entry"),
///             EXIT     (0x01, "syscall exit"),
///             READ     (0x02, "sys_read called"),
///             WRITE    (0x03, "sys_write called"),
///         ]
///     }
/// }
/// ```
#[macro_export]
macro_rules! define_trace_provider {
    (
        provider $name:ident {
            id: $id:expr,
            probes: [
                $($probe_name:ident ($probe_id:expr, $desc:expr)),* $(,)?
            ]
        }
    ) => {
        pub mod $name {
            use super::*;

            pub const PROVIDER_ID: u8 = $id;

            $(
                pub const $probe_name: u8 = ($id << 4) | $probe_id;
            )*

            pub static PROVIDER: $crate::tracing::TraceProvider =
                $crate::tracing::TraceProvider {
                    name: stringify!($name),
                    id: $id,
                    probes: &[
                        $(
                            $crate::tracing::TraceProbe {
                                name: stringify!($probe_name),
                                id: $probe_id,
                                event_type: ($id << 4) | $probe_id,
                            },
                        )*
                    ],
                    enabled: core::sync::atomic::AtomicU64::new(0),
                };
        }
    };
}
```

### 5.2 Trace Point Macros

```rust
/// Record a trace event if the probe is enabled.
///
/// # Example
/// ```rust
/// trace_event!(SYSCALL::ENTRY, syscall_num as u64);
/// // ... execute syscall ...
/// trace_event!(SYSCALL::EXIT, result as u64);
/// ```
#[macro_export]
macro_rules! trace_event {
    ($probe:expr, $payload:expr) => {
        if $crate::tracing::is_probe_enabled($probe) {
            $crate::tracing::record_event($probe, 0, $payload);
        }
    };
    ($probe:expr, $flags:expr, $payload:expr) => {
        if $crate::tracing::is_probe_enabled($probe) {
            $crate::tracing::record_event($probe, $flags, $payload);
        }
    };
}

/// Trace with two 28-bit values packed into payload.
#[macro_export]
macro_rules! trace_event_2 {
    ($probe:expr, $val1:expr, $val2:expr) => {
        if $crate::tracing::is_probe_enabled($probe) {
            let payload = (($val1 as u64 & 0x0FFF_FFFF) << 28)
                        | ($val2 as u64 & 0x0FFF_FFFF);
            $crate::tracing::record_event($probe, 0, payload);
        }
    };
}
```

### 5.3 Counter Macros

```rust
/// Define a trace counter.
///
/// # Example
/// ```rust
/// define_trace_counter!(SYSCALL_COUNT, "Total syscall invocations");
/// define_trace_counter!(PAGE_FAULT_COUNT, "Total page faults");
/// ```
#[macro_export]
macro_rules! define_trace_counter {
    ($name:ident, $desc:expr) => {
        pub static $name: $crate::tracing::TraceCounter =
            $crate::tracing::TraceCounter::new(stringify!($name), $desc);
    };
}

/// Increment a counter.
#[macro_export]
macro_rules! trace_count {
    ($counter:expr) => {
        $counter.increment();
    };
}
```

### 5.4 Core Functions

```rust
// === Initialization ===

/// Initialize the tracing subsystem.
/// Must be called after per-CPU data is initialized.
pub fn init() {
    // Initialize per-CPU buffers
    // Register built-in providers
    // Set up timestamp source
}

// === Recording ===

/// Check if a probe is enabled (single atomic load).
#[inline(always)]
pub fn is_probe_enabled(event_type: u8) -> bool {
    GLOBAL_TRACE_ENABLED.load(Ordering::Relaxed) != 0
        && probe_is_enabled_internal(event_type)
}

/// Record an event to the local CPU's ring buffer.
/// Must be lock-free and safe in any context.
#[inline(always)]
pub fn record_event(event_type: u8, flags: u8, payload: u64) {
    let cpu_id = current_cpu_id();
    let buffer = get_cpu_buffer(cpu_id);
    let timestamp = get_timestamp_ns();

    let entry = TraceEvent::new(event_type, flags, payload);
    let entry_with_ts = TraceEvent { timestamp, ..entry };

    // Single atomic store after index increment
    let idx = buffer.write_idx.fetch_add(1, Ordering::Relaxed) & TRACE_BUFFER_MASK;

    // SAFETY: u128 atomic store (16 bytes)
    buffer.entries[idx].store(entry_with_ts.as_u128(), Ordering::Release);
}

// === Control ===

/// Enable tracing globally.
pub fn enable() {
    GLOBAL_TRACE_ENABLED.store(1, Ordering::Release);
}

/// Disable tracing globally.
pub fn disable() {
    GLOBAL_TRACE_ENABLED.store(0, Ordering::Release);
}

/// Enable a specific provider's probes.
pub fn enable_provider(provider_id: u8) {
    if let Some(provider) = get_provider(provider_id) {
        provider.enabled.store(u64::MAX, Ordering::Release);
    }
}

/// Enable specific probes within a provider.
pub fn enable_probes(provider_id: u8, probe_mask: u64) {
    if let Some(provider) = get_provider(provider_id) {
        provider.enabled.fetch_or(probe_mask, Ordering::Release);
    }
}

// === Output ===

/// Dump trace buffer contents for a specific CPU.
/// NOT safe in interrupt context - uses serial output.
pub fn dump_cpu_trace(cpu_id: usize) {
    // Disable tracing during dump
    let was_enabled = is_enabled();
    disable();

    let buffer = get_cpu_buffer(cpu_id);
    // ... iterate and print events ...

    if was_enabled { enable(); }
}

/// Get raw buffer pointer for GDB inspection.
pub fn get_buffer_for_gdb(cpu_id: usize) -> (*const TraceEvent, usize) {
    let buffer = get_cpu_buffer(cpu_id);
    (buffer.entries.as_ptr() as *const TraceEvent, TRACE_BUFFER_SIZE)
}
```

### 5.5 Example Usage

```rust
// In kernel/src/syscall/handler.rs
use crate::tracing::{trace_event, trace_count};
use crate::tracing::providers::SYSCALL;

// Near the top of the file
define_trace_counter!(SYSCALL_TOTAL, "Total syscall invocations");

pub fn syscall_handler(num: u64, args: &[u64]) -> isize {
    trace_event!(SYSCALL::ENTRY, num);
    trace_count!(SYSCALL_TOTAL);

    let result = match num {
        SYS_READ => sys_read(args[0], args[1], args[2]),
        SYS_WRITE => sys_write(args[0], args[1], args[2]),
        SYS_CLOCK_GETTIME => {
            trace_event!(SYSCALL::CLOCK_GETTIME, args[0]); // clock_id
            sys_clock_gettime(args[0], args[1])
        }
        // ...
    };

    trace_event!(SYSCALL::EXIT, result as u64);
    result
}
```

```rust
// In kernel/src/interrupts/context_switch.rs
use crate::tracing::trace_event_2;
use crate::tracing::providers::SCHED;

pub fn context_switch(old_thread: &Thread, new_thread: &Thread) {
    trace_event_2!(SCHED::CTX_SWITCH, old_thread.id, new_thread.id);
    // ... perform context switch ...
}
```

---

## 6. Implementation Roadmap

### Phase 1: Core Infrastructure (Week 1-2)

**Goals:**
- Per-CPU ring buffer with single-atomic-store writes
- Timestamp integration with existing TSC/timer infrastructure
- Basic enable/disable control
- GDB-inspectable buffer symbols

**Tasks:**
1. Create `/kernel/src/tracing/mod.rs` module structure
2. Implement `TraceCpuBuffer` with atomic operations
3. Add per-CPU buffer allocation (integrate with PerCpuData)
4. Implement timestamp source abstraction (TSC for x86_64, generic timer for ARM64)
5. Add `#[no_mangle]` symbols for GDB inspection
6. Unit tests for buffer operations

**Deliverables:**
- Working ring buffer with `record_event()` function
- GDB can inspect buffer contents: `x/100xg &TRACE_CPU0_BUFFER`

### Phase 2: Provider/Probe Framework (Week 3)

**Goals:**
- Macro-based provider/probe definition
- Per-probe enable/disable
- Built-in providers for syscall, sched, irq

**Tasks:**
1. Implement `define_trace_provider!` macro
2. Implement probe enable bitmap
3. Create SYSCALL provider with entry/exit probes
4. Create SCHED provider with context_switch/schedule probes
5. Create IRQ provider with entry/exit/timer probes
6. Integrate probes at key kernel points

**Deliverables:**
- Subsystems can define their own providers
- Individual probes can be enabled/disabled at runtime

### Phase 3: Atomic Counters (Week 4)

**Goals:**
- Per-CPU atomic counters for statistics
- Counter aggregation for reporting
- Built-in counters for key metrics

**Tasks:**
1. Implement `TraceCounter` with per-CPU values
2. Implement `define_trace_counter!` macro
3. Add counters: SYSCALL_TOTAL, PAGE_FAULT_TOTAL, CTX_SWITCH_TOTAL, IRQ_TOTAL
4. Add `counters_dump()` function for serial output

**Deliverables:**
- High-frequency counters with no contention
- Aggregated totals available for inspection

### Phase 4: Output and Visualization (Week 5)

**Goals:**
- Post-mortem serial dump
- Structured output format
- Documentation for GDB inspection

**Tasks:**
1. Implement `dump_all_traces()` for serial output
2. Add event type to string mapping for human-readable output
3. Document GDB commands for trace inspection
4. Add trace examples to CLAUDE.md debugging section

**Deliverables:**
- Complete trace dump capability
- Developer documentation

### Phase 5: procfs Integration (Future)

**Goals:**
- `/proc/trace/events` - List available probes
- `/proc/trace/enable` - Control probe state
- `/proc/trace/buffer` - Read trace data
- `/proc/trace/counters` - Read counter values

**Tasks:**
1. Integrate with VFS/procfs infrastructure
2. Implement read-only file handlers
3. Add write handler for enable/disable control

**Deliverables:**
- Runtime tracing control without reboot
- Standard Unix-like tracing interface

---

## 7. File/Module Structure

```
kernel/src/
├── tracing/
│   ├── mod.rs              # Public API, re-exports
│   ├── core.rs             # TraceEvent, record_event, enable/disable
│   ├── buffer.rs           # TraceCpuBuffer, per-CPU allocation
│   ├── provider.rs         # TraceProvider, TraceProbe, registration
│   ├── counter.rs          # TraceCounter, aggregation
│   ├── timestamp.rs        # Timestamp abstraction (TSC/generic)
│   ├── output.rs           # dump_trace, serial formatting
│   ├── macros.rs           # define_trace_provider!, trace_event!, etc.
│   └── providers/
│       ├── mod.rs          # Built-in provider exports
│       ├── syscall.rs      # SYSCALL provider
│       ├── sched.rs        # SCHED provider
│       └── irq.rs          # IRQ provider
└── lib.rs                  # Add: pub mod tracing;
```

### Migration Path for Existing Code

1. **Keep `/kernel/src/trace.rs`** as deprecated compatibility layer
2. **Redirect macros** to new tracing infrastructure
3. **Remove `/kernel/src/arch_impl/aarch64/trace.rs`** when new infrastructure supports ARM64
4. **Update CLAUDE.md** to recommend new tracing macros

---

## 8. Appendices

### A. Memory Ordering Requirements

| Operation | Ordering | Rationale |
|-----------|----------|-----------|
| Write event to buffer | Release | Ensures timestamp written before data visible |
| Read event from buffer | Acquire | Ensures complete event visibility |
| Check probe enabled | Relaxed | Stale value is acceptable (just enables/disables late) |
| Increment counter | Relaxed | Order doesn't matter for statistics |
| Enable/disable global | Release | Propagates to all CPUs |

### B. GDB Inspection Commands

```gdb
# View raw trace buffer for CPU 0
x/1024xg &TRACE_CPU0_BUFFER

# View write index
print TRACE_CPU0_WRITE_IDX

# View specific event (index 42)
x/2xg &TRACE_CPU0_BUFFER + 42*16

# Decode event type from data field
print (TRACE_CPU0_BUFFER[42].data >> 56) & 0xFF

# View all counters
print TRACE_COUNTERS
```

### C. Performance Considerations

1. **Probe Check Cost**: Single atomic load + mask check (~2-5 cycles)
2. **Event Record Cost**: Timestamp read + 2 atomic ops + store (~50-100 cycles)
3. **Counter Increment Cost**: Single atomic add (~10-20 cycles)
4. **Memory Footprint**: ~16.5 KiB per CPU for buffers

### D. Comparison with Existing Approaches

| Feature | Current trace.rs | New Tracing | DTrace | ftrace |
|---------|-----------------|-------------|--------|--------|
| Per-CPU buffers | No | Yes | Yes | Yes |
| Timestamps | No | Yes | Yes | Yes |
| Provider/Probe | No | Yes | Yes | Yes |
| Counters | No | Yes | Yes | Yes |
| Dynamic enable | Global only | Per-probe | Per-probe | Per-event |
| procfs | No | Planned | /dev/dtrace | debugfs |

### E. References

- [DTrace Wikipedia](https://en.wikipedia.org/wiki/DTrace)
- [Oracle DTrace Architecture Guide](https://docs.oracle.com/cd/E18752_01/html/819-5488/gcfpv.html)
- [Linux ftrace Documentation](https://docs.kernel.org/trace/ftrace.html)
- [Linux Lockless Ring Buffer Design](https://docs.kernel.org/trace/ring-buffer-design.html)
- [eBPF Introduction](https://ebpf.io/what-is-ebpf/)
- [FreeBSD DTrace Handbook](https://docs.freebsd.org/en/books/handbook/dtrace/)
- [Lock-Free Ring Buffer in Rust](https://kmdreko.github.io/posts/20191003/a-simple-lock-free-ring-buffer/)
- [Ferrous Systems Lock-Free Ring Buffer](https://ferrous-systems.com/blog/lock-free-ring-buffer/)
- [Redox OS ptrace Implementation](https://www.redox-os.org/news/rsoc-ptrace-2/)

---

## Revision History

| Date | Author | Changes |
|------|--------|---------|
| 2026-02-02 | Claude Code | Initial design document |
