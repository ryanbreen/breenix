# Breenix Tracing Framework Implementation

This document details the DTrace-style lock-free tracing framework implemented for Breenix kernel observability.

## Overview

The tracing framework provides comprehensive kernel instrumentation with:

- **Lock-free operation**: Safe in interrupt handlers, context switches, and syscall paths
- **Minimal overhead**: Near-zero cost when disabled (~5 instructions); minimal impact when enabled
- **Cross-architecture support**: Works on both x86-64 and ARM64
- **Per-CPU ring buffers**: Eliminates contention, enables lockless local writes
- **Provider/probe model**: Subsystems define their own trace points
- **Atomic counters**: Per-CPU counters with cache-line alignment to prevent false sharing
- **GDB-first debugging**: Primary inspection via GDB, secondary via serial output
- **procfs integration**: Expose tracing via `/proc/trace/*`

## Architecture

```
+-------------------+     +-------------------+     +-------------------+
|  SYSCALL PROVIDER |     |   SCHED PROVIDER  |     |   IRQ PROVIDER    |
|  - sys_entry      |     |  - ctx_switch     |     |  - irq_entry      |
|  - sys_exit       |     |  - schedule       |     |  - timer_tick     |
+--------+----------+     +--------+----------+     +--------+----------+
         |                         |                         |
         v                         v                         v
+------------------------------------------------------------------------+
|                        TRACING CORE                                    |
|  Per-CPU ring buffers | Atomic enable bitmaps | Lock-free recording   |
+------------------------------------------------------------------------+
         |
         v
+------------------------------------------------------------------------+
|                        OUTPUT / VISUALIZATION                          |
|  Serial dump | GDB helpers | Event formatting | Panic handler         |
+------------------------------------------------------------------------+
         |
         v
+------------------------------------------------------------------------+
|                        /proc/trace INTERFACE                           |
|  events | enable | buffer | counters | providers                      |
+------------------------------------------------------------------------+
```

## Implementation Phases

### Phase 1: Core Infrastructure

**Files Created:**

| File | Lines | Description |
|------|-------|-------------|
| `kernel/src/tracing/mod.rs` | ~130 | Module exports and public API |
| `kernel/src/tracing/core.rs` | ~360 | TraceEvent, global state, record functions |
| `kernel/src/tracing/buffer.rs` | ~250 | Per-CPU lock-free ring buffers |
| `kernel/src/tracing/timestamp.rs` | ~160 | Architecture-specific timestamps |

**Key Components:**

#### TraceEvent Structure (16 bytes)
```rust
#[repr(C, align(16))]
pub struct TraceEvent {
    pub timestamp: u64,    // CPU cycles (TSC/CNTVCT)
    pub event_type: u16,   // Event identifier
    pub cpu_id: u8,        // CPU that generated event
    pub flags: u8,         // Event-specific flags
    pub payload: u32,      // Event-specific data
}
```

#### Per-CPU Ring Buffers
- 1024 entries per CPU (16 KiB per buffer)
- Lock-free writes using atomic fetch_add
- Overwrite mode (old events silently replaced when full)
- 64-byte cache-line aligned to prevent false sharing

#### Timestamp Sources
- **x86-64**: RDTSC (Time Stamp Counter) - ~3 cycles overhead
- **ARM64**: CNTVCT_EL0 (Virtual Counter) - ~3 cycles overhead

#### GDB-Inspectable Symbols
```gdb
print TRACE_ENABLED           # Global enable flag
x/1024xg &TRACE_BUFFERS       # View raw buffers
print TRACE_CPU0_WRITE_IDX    # CPU 0 write index
```

---

### Phase 2: Provider/Probe Framework

**Files Created:**

| File | Lines | Description |
|------|-------|-------------|
| `kernel/src/tracing/provider.rs` | ~300 | TraceProvider, TraceProbe, registration |
| `kernel/src/tracing/macros.rs` | ~280 | trace_event!, define_trace_provider! |
| `kernel/src/tracing/providers/mod.rs` | ~60 | Built-in provider exports |
| `kernel/src/tracing/providers/syscall.rs` | ~100 | Syscall tracing provider |
| `kernel/src/tracing/providers/sched.rs` | ~150 | Scheduler tracing provider |
| `kernel/src/tracing/providers/irq.rs` | ~120 | IRQ tracing provider |

**Built-in Providers:**

| Provider | ID | Events |
|----------|-----|--------|
| SCHED_PROVIDER | 0x00 | CTX_SWITCH_ENTRY, CTX_SWITCH_EXIT, SCHED_PICK, SCHED_RESCHED |
| IRQ_PROVIDER | 0x01 | IRQ_ENTRY, IRQ_EXIT, TIMER_TICK |
| SYSCALL_PROVIDER | 0x03 | SYSCALL_ENTRY, SYSCALL_EXIT |

**Macro Usage:**
```rust
// Record a trace event (only if tracing enabled)
trace_event!(SYSCALL_PROVIDER, SYSCALL_ENTRY, syscall_num as u32);

// Record with two 16-bit values packed into payload
trace_event_2!(SCHED_PROVIDER, CTX_SWITCH_ENTRY, old_tid, new_tid);
```

**Integration Points:**

| File | Integration |
|------|-------------|
| `kernel/src/syscall/handler.rs` | Syscall entry/exit traces |
| `kernel/src/interrupts/context_switch.rs` | Context switch traces |
| `kernel/src/interrupts/timer.rs` | Timer tick traces |

---

### Phase 3: Atomic Counters

**Files Created:**

| File | Lines | Description |
|------|-------|-------------|
| `kernel/src/tracing/counter.rs` | ~440 | TraceCounter with per-CPU storage |
| `kernel/src/tracing/providers/counters.rs` | ~140 | Built-in counters |

**Counter Design:**
```rust
#[repr(C, align(64))]  // Cache-line aligned
pub struct CpuCounterSlot {
    pub value: AtomicU64,
    _padding: [u8; 56],  // Prevent false sharing
}

pub struct TraceCounter {
    pub name: &'static str,
    pub description: &'static str,
    pub per_cpu: [CpuCounterSlot; MAX_CPUS],
}
```

**Built-in Counters:**

| Counter | Description |
|---------|-------------|
| `SYSCALL_TOTAL` | Total syscall invocations |
| `IRQ_TOTAL` | Total interrupt invocations |
| `CTX_SWITCH_TOTAL` | Total context switches |
| `TIMER_TICK_TOTAL` | Total timer tick interrupts |

**Macro Usage:**
```rust
// Define a counter
define_trace_counter!(MY_COUNTER, "Description");

// Increment by 1
trace_count!(MY_COUNTER);

// Increment by N
trace_count_add!(MY_COUNTER, 10);

// Query aggregate value
let total = MY_COUNTER.aggregate();
```

---

### Phase 4: Output and Visualization

**Files Created:**

| File | Lines | Description |
|------|-------|-------------|
| `kernel/src/tracing/output.rs` | ~850 | Serial dump, GDB helpers, formatting |

**Buffer Dump Functions:**
- `dump_buffer(cpu_id)` - Dump single CPU's ring buffer
- `dump_all_buffers()` - Dump all CPU trace buffers
- `dump_latest_events(n)` - Dump N most recent events (sorted by timestamp)

**Counter/Provider Dumps:**
- `dump_counters()` - Print all counter values with per-CPU breakdown
- `dump_providers()` - Print all providers and enable state
- `dump_event_summary()` - Print event counts by category

**Event Helpers:**
- `event_type_name(type)` - Convert event type to string (e.g., "SYSCALL_ENTRY")
- `payload_description(type)` - What the payload field means

**GDB Callable Functions:**
```gdb
call trace_dump()           # Complete dump
call trace_dump_latest(20)  # Last 20 events
call trace_dump_cpu(0)      # CPU 0's buffer
call trace_dump_counters()  # Counter values only
```

**Serial Output Format:**
```
[TRACE] CPU0 idx=42 ts=1234567890 type=0x0300 SYSCALL_ENTRY syscall_nr=1
[TRACE] CPU0 idx=43 ts=1234567900 type=0x0301 SYSCALL_EXIT result=0
[COUNTER] SYSCALL_TOTAL: 12345 (cpu0=12300, cpu1=45)
[PROVIDER] syscall (id=0x3): enabled=0xffffffffffffffff
```

**Panic Handler Integration:**
```rust
// Safe to call from panic handlers (lock-free serial output)
pub fn dump_on_panic() {
    disable();  // Stop new events
    dump_latest_events(32);
    dump_counters();
    dump_providers();
}
```

---

### Phase 5: /proc Integration

**Files Created:**

| File | Lines | Description |
|------|-------|-------------|
| `kernel/src/fs/procfs/mod.rs` | ~400 | procfs core: initialization, entry types, content generators, inode allocation |
| `kernel/src/fs/procfs/trace.rs` | ~240 | Trace-specific /proc/trace/* content generators |

**/proc/trace Entries:**

| Entry | Description | Format |
|-------|-------------|--------|
| `/proc/trace/enable` | Tracing enable state | `0` or `1` |
| `/proc/trace/events` | Available trace points | `0xNNNN NAME - description` |
| `/proc/trace/buffer` | Trace buffer contents | `cpu idx timestamp type name payload` |
| `/proc/trace/counters` | Counter values | `name: total (cpuN=value, ...)` |
| `/proc/trace/providers` | Registered providers | `name: id=0xNN enabled=0xNNNN` |

**Standard procfs Entries:**

| Entry | Description |
|-------|-------------|
| `/proc/cpuinfo` | CPU vendor, model, features |
| `/proc/meminfo` | Memory statistics |
| `/proc/uptime` | System uptime |
| `/proc/version` | Kernel version |
| `/proc/self` | Symlink to current process |
| `/proc/[pid]/status` | Process status |
| `/proc/[pid]/cmdline` | Command line |
| `/proc/[pid]/stat` | Process statistics |
| `/proc/[pid]/maps` | Memory mappings |

---

## Usage Guide

### Enabling Tracing

```rust
use kernel::tracing;
use kernel::tracing::providers::{SYSCALL_PROVIDER, SCHED_PROVIDER, IRQ_PROVIDER};

// Enable tracing globally
tracing::enable();

// Enable specific providers
SYSCALL_PROVIDER.enable_all();
SCHED_PROVIDER.enable_all();
IRQ_PROVIDER.enable_all();

// Or enable just specific probes
SCHED_PROVIDER.enable_probe(0);  // Only context switches
```

### Recording Custom Events

```rust
use kernel::tracing::{trace_event, define_trace_provider, define_trace_probes};

// Define a custom provider
define_trace_provider!(MY_PROVIDER, 0x10, "my_subsystem");
define_trace_probes!(MY_PROVIDER,
    MY_EVENT_A = 0,  // 0x1000
    MY_EVENT_B = 1,  // 0x1001
);

// Record events
trace_event!(MY_PROVIDER, MY_EVENT_A, some_value as u32);
```

### Defining Custom Counters

```rust
use kernel::tracing::{define_trace_counter, trace_count, trace_count_add};

define_trace_counter!(MY_OPS_TOTAL, "Total operations performed");

fn my_operation() {
    trace_count!(MY_OPS_TOTAL);
    // ... operation code ...
}

fn batch_operation(count: u64) {
    trace_count_add!(MY_OPS_TOTAL, count);
}
```

### GDB Debugging Session

```gdb
# Check if tracing is enabled
print TRACE_ENABLED

# View CPU 0's buffer (raw)
x/100xg &TRACE_BUFFERS

# Check counter values
print SYSCALL_TOTAL.per_cpu[0].value
print CTX_SWITCH_TOTAL.per_cpu[0].value

# Dump all trace data to serial
call trace_dump()

# Dump last 20 events
call trace_dump_latest(20)

# View provider state
print SYSCALL_PROVIDER.enabled
```

### Reading from /proc

```bash
# Check if tracing is enabled
cat /proc/trace/enable

# List available events
cat /proc/trace/events

# Read trace buffer
cat /proc/trace/buffer

# Read counter values
cat /proc/trace/counters

# List providers
cat /proc/trace/providers
```

---

## Design Decisions

### Lock-Free Operation
All tracing operations use atomic instructions (fetch_add, load, store) with relaxed ordering. This ensures safety in:
- Interrupt handlers (can't hold locks)
- Context switch code (might switch while holding lock)
- Syscall hot paths (performance critical)

### Per-CPU Buffers
Each CPU has its own ring buffer, eliminating cross-CPU contention. Events are written locally and can be merged by timestamp during analysis.

### 64-Byte Cache Alignment
Counter slots and buffer metadata are aligned to 64-byte cache lines to prevent false sharing between CPUs.

### Relaxed Atomic Ordering
Counters use `Ordering::Relaxed` because:
- Exact counts aren't critical (approximate is fine)
- Provides maximum performance (~1 cycle for increment)
- No cross-CPU synchronization needed

### Raw Timestamps
Events store raw CPU cycles (TSC/CNTVCT) without conversion. This:
- Minimizes recording overhead
- Preserves full precision
- Allows offline conversion with known frequency

### Event Type Encoding
Event types use 16-bit values: `(provider_id << 8) | probe_id`. This allows:
- Up to 256 providers
- Up to 256 probes per provider
- Fast category filtering (mask upper byte)

---

## Performance Characteristics

| Operation | Overhead (cycles) | Notes |
|-----------|-------------------|-------|
| Trace disabled check | ~5 | 2 atomic loads + branch |
| Event recording | ~20-30 | timestamp + atomic write |
| Counter increment | ~3 | Single atomic fetch_add |
| Buffer full wrap | 0 extra | Modulo via bitmask |

Memory footprint:
- Per-CPU buffer: 16 KiB (1024 events × 16 bytes)
- 8 CPUs total: 128 KiB for buffers
- Counter storage: ~512 bytes per counter (8 CPUs × 64 bytes)

---

## Files Summary

### New Files (kernel/src/tracing/)

| File | Purpose |
|------|---------|
| `mod.rs` | Module exports, documentation |
| `core.rs` | TraceEvent, global state, record_event() |
| `buffer.rs` | TraceCpuBuffer ring buffer |
| `timestamp.rs` | RDTSC/CNTVCT timestamp sources |
| `provider.rs` | TraceProvider, TraceProbe types |
| `macros.rs` | trace_event!, define_trace_counter!, etc. |
| `counter.rs` | TraceCounter with per-CPU storage |
| `output.rs` | Serial dump, GDB helpers, formatting |
| `providers/mod.rs` | Built-in provider exports |
| `providers/syscall.rs` | Syscall provider |
| `providers/sched.rs` | Scheduler provider |
| `providers/irq.rs` | IRQ provider |
| `providers/counters.rs` | Built-in counters |

### New Files (kernel/src/fs/procfs/)

| File | Purpose |
|------|---------|
| `mod.rs` | procfs core: initialization, entry types, content generators, inode allocation |
| `trace.rs` | /proc/trace/* content generators |

### Modified Files

| File | Changes |
|------|---------|
| `kernel/src/lib.rs` | Added `pub mod tracing;` |
| `kernel/src/main.rs` | Added `tracing::init()` call |
| `kernel/src/fs/mod.rs` | Added `pub mod procfs;` |
| `kernel/src/syscall/handler.rs` | Added trace points |
| `kernel/src/interrupts/context_switch.rs` | Added trace points |
| `kernel/src/interrupts/timer.rs` | Added trace points |

---

## Future Enhancements

1. **Write support for /proc/trace/enable** - Enable/disable via `echo 1 > /proc/trace/enable`
2. **Binary trace format** - For high-performance trace capture
3. **Trace filtering** - Filter by event type, CPU, timestamp range
4. **Userspace trace tools** - `btrace`, `bstat` utilities
5. **eBPF-style programmability** - Custom trace programs
6. **Network export** - Stream traces over network for remote analysis
