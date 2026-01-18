# Kernel Threads (kthreads) Implementation Plan

## Overview

Implement Linux-style kernel threads for Breenix to enable:
1. Background kernel work that can sleep and block
2. Deferred work processing (workqueues)
3. Softirq processing under load (ksoftirqd)
4. Memory reclamation (kswapd)
5. Per-CPU kernel threads for scalability

## Why Kthreads Are Necessary

### Current Limitations

Without kthreads, Breenix cannot properly handle:

| Scenario | Problem | Impact |
|----------|---------|--------|
| Memory pressure | No background reclamation | Deadlock when alloc needs to wait |
| Network load | Synchronous packet processing | Interrupt blocked, packets dropped |
| Deferred work | All work must complete in IRQ | Stack overflow, missed deadlines |
| Softirq storms | No way to defer to thread context | Userspace starvation |

### What Kthreads Enable

Kernel threads can:
- **Sleep**: Block on mutexes, wait for events
- **Allocate memory**: Use blocking allocation (equivalent to GFP_KERNEL)
- **Be preempted**: Run alongside other threads fairly
- **Run indefinitely**: No cycle budget like interrupt handlers

## Current Breenix State

### What Already Exists ✓

```
kernel/src/task/
├── thread.rs      # Thread structure with states, priorities
├── scheduler.rs   # Round-robin scheduler, blocking primitives
├── spawn.rs       # spawn_thread() for kernel threads
└── mod.rs

kernel/src/per_cpu.rs   # Softirq infrastructure (pending bitmap)
kernel/src/interrupts/  # irq_enter/exit, do_softirq hooks
```

- **Thread-based scheduler** (not process-based)
- **`spawn_thread(name, entry_fn)`** creates kernel threads
- **Softirq bitmap** and `raise_softirq()` / `do_softirq()` infrastructure
- **Blocking primitives** for pause/waitpid
- **Per-CPU data** via GS-relative addressing

### What's Missing ✗

1. **Thread lifecycle management**: No way to stop/join threads
2. **Workqueue system**: No deferred work queuing
3. **Softirq handlers**: Infrastructure exists, handlers are stubs
4. **Named thread identification**: No `[kswapd]` style names in process list
5. **Per-CPU thread spawning**: No CPU affinity for kthreads
6. **Wakeup mechanisms**: No way for external events to wake sleeping kthreads

## Architecture

### Kthread Structure

```rust
/// Kernel thread control block
pub struct Kthread {
    /// Thread ID (same as regular thread)
    tid: u64,

    /// Thread name for debugging (e.g., "kswapd", "kworker/0:0")
    name: String,

    /// Stop flag - thread should check this and exit
    should_stop: AtomicBool,

    /// Completion to signal when thread has exited
    exited: Completion,

    /// CPU affinity (None = any CPU, Some(n) = bound to CPU n)
    cpu_affinity: Option<usize>,

    /// Thread function's private data
    data: *mut c_void,
}
```

### Kthread States

```
                    ┌──────────────────────────────────────┐
                    │                                      │
                    ▼                                      │
┌─────────┐    ┌─────────┐    ┌─────────┐    ┌──────────┐ │
│ Created │───▶│ Running │───▶│ Blocked │───▶│ Stopping │─┘
└─────────┘    └─────────┘    └─────────┘    └──────────┘
     │              │              │               │
     │              │              │               ▼
     │              │              │         ┌──────────┐
     │              └──────────────┴────────▶│  Exited  │
     │                                       └──────────┘
     │                                             ▲
     └─────────────────────────────────────────────┘
                    (creation failed)
```

### Workqueue Architecture

```
                         ┌─────────────────────────────┐
                         │     schedule_work(item)     │
                         └─────────────┬───────────────┘
                                       │
                                       ▼
              ┌────────────────────────────────────────────┐
              │              Workqueue                      │
              │  ┌─────────────────────────────────────┐   │
              │  │ work_item → work_item → work_item   │   │
              │  └─────────────────────────────────────┘   │
              └────────────────────┬───────────────────────┘
                                   │
           ┌───────────────────────┼───────────────────────┐
           │                       │                       │
           ▼                       ▼                       ▼
    ┌─────────────┐         ┌─────────────┐         ┌─────────────┐
    │ kworker/0:0 │         │ kworker/1:0 │         │ kworker/2:0 │
    │   (CPU 0)   │         │   (CPU 1)   │         │   (CPU 2)   │
    └─────────────┘         └─────────────┘         └─────────────┘
```

## Implementation Phases

### Phase 1: Core Kthread Infrastructure

**Goal**: Basic kthread lifecycle (create, run, stop, exit)

**Location**: `kernel/src/task/kthread.rs`

#### API

```rust
/// Create a kernel thread (doesn't start it)
pub fn kthread_create<F>(func: F, name: &str) -> Result<KthreadHandle, KthreadError>
where
    F: FnOnce() + Send + 'static;

/// Create and immediately start a kernel thread
pub fn kthread_run<F>(func: F, name: &str) -> Result<KthreadHandle, KthreadError>
where
    F: FnOnce() + Send + 'static;

/// Signal thread to stop (non-blocking)
pub fn kthread_stop(handle: &KthreadHandle) -> Result<(), KthreadError>;

/// Wait for thread to exit
pub fn kthread_join(handle: KthreadHandle) -> Result<i32, KthreadError>;

/// Check if current thread should stop (called by kthread function)
pub fn kthread_should_stop() -> bool;

/// Park current thread until unparked
pub fn kthread_park();

/// Unpark a parked thread
pub fn kthread_unpark(handle: &KthreadHandle);
```

#### Thread Function Pattern

```rust
fn my_kthread_fn() {
    log::info!("[my_kthread] started");

    while !kthread_should_stop() {
        // Check for work
        if let Some(work) = get_pending_work() {
            process_work(work);
        } else {
            // No work - park until woken
            kthread_park();
        }
    }

    log::info!("[my_kthread] stopping");
}
```

#### Deliverables

- [ ] `Kthread` struct with stop flag and completion
- [ ] `kthread_create()` / `kthread_run()`
- [ ] `kthread_should_stop()` / `kthread_stop()`
- [ ] `kthread_park()` / `kthread_unpark()` for sleep/wake
- [ ] Integration with existing scheduler
- [ ] Boot stage tests for kthread lifecycle

#### Test Cases

```
KTHREAD_CREATE: kthread created
KTHREAD_RUN: kthread running
KTHREAD_STOP: kthread received stop signal
KTHREAD_EXIT: kthread exited cleanly
KTHREAD_JOIN: kthread join completed
```

---

### Phase 2: Workqueue System

**Goal**: Deferred work execution via kworker threads

**Location**: `kernel/src/task/workqueue.rs`

#### API

```rust
/// A unit of deferred work
pub struct WorkItem {
    func: Box<dyn FnOnce() + Send>,
    name: &'static str,  // For debugging
}

/// Global system workqueue
pub static SYSTEM_WQ: Workqueue;

/// Schedule work on system workqueue
pub fn schedule_work(item: WorkItem);

/// Schedule work with delay
pub fn schedule_delayed_work(item: WorkItem, delay: Duration);

/// Create a custom workqueue
pub fn create_workqueue(name: &str, flags: WorkqueueFlags) -> Workqueue;

/// Queue work on specific workqueue
pub fn queue_work(wq: &Workqueue, item: WorkItem);

/// Flush all pending work (wait for completion)
pub fn flush_workqueue(wq: &Workqueue);
```

#### Workqueue Flags

```rust
bitflags! {
    pub struct WorkqueueFlags: u32 {
        /// One worker per CPU (default)
        const WQ_PERCPU = 0x01;
        /// Single worker, not bound to CPU
        const WQ_UNBOUND = 0x02;
        /// High priority workers
        const WQ_HIGHPRI = 0x04;
        /// Memory reclaim safe
        const WQ_MEM_RECLAIM = 0x08;
    }
}
```

#### Deliverables

- [ ] `WorkItem` structure
- [ ] Per-CPU kworker threads
- [ ] `schedule_work()` / `queue_work()`
- [ ] Work stealing between idle workers (optional)
- [ ] `flush_workqueue()` for synchronization
- [ ] Boot stage tests

#### Test Cases

```
WORKQUEUE_INIT: workqueue system initialized
KWORKER_SPAWN: kworker/0:0 spawned
WORK_SCHEDULED: work item queued
WORK_EXECUTED: work item executed by kworker
WORK_FLUSH: workqueue flushed successfully
```

---

### Phase 3: Softirq Threads (ksoftirqd)

**Goal**: Move softirq processing to thread context under load

**Location**: `kernel/src/task/softirqd.rs`

#### Design

Currently, softirqs are processed in `irq_exit()`. Under heavy load, this starves userspace. `ksoftirqd` processes softirqs when:
- Too many softirqs in one `do_softirq()` call (> 10 iterations)
- Softirqs were raised during softirq processing

#### Softirq Types to Implement

```rust
pub enum SoftirqType {
    HiTasklet = 0,      // High priority tasklets
    Timer = 1,          // Timer callbacks
    NetTx = 2,          // Network transmit
    NetRx = 3,          // Network receive
    Block = 4,          // Block device completion
    Tasklet = 5,        // Normal tasklets
    Sched = 6,          // Scheduler
    Rcu = 7,            // Read-copy-update
}
```

#### Deliverables

- [ ] Per-CPU `ksoftirqd` threads
- [ ] Softirq handler registration
- [ ] `raise_softirq()` to set pending bits
- [ ] `do_softirq()` with iteration limit
- [ ] Wakeup ksoftirqd when limit exceeded
- [ ] NET_RX handler for network packets
- [ ] Boot stage tests

#### Test Cases

```
KSOFTIRQD_SPAWN: ksoftirqd/0 spawned
SOFTIRQ_RAISE: softirq raised
SOFTIRQ_PROCESS: softirq processed in irq_exit
SOFTIRQ_DEFER: softirq deferred to ksoftirqd
NET_RX_SOFTIRQ: network packet processed via softirq
```

---

### Phase 4: Memory Kthreads

**Goal**: Background memory management

**Location**: `kernel/src/mm/kswapd.rs`, `kernel/src/mm/kcompactd.rs`

#### kswapd Design

```rust
/// Memory zone watermarks
pub struct Watermarks {
    /// Below this: wake kswapd
    low: usize,
    /// Target for kswapd reclamation
    high: usize,
    /// Below this: direct reclaim (caller blocks)
    min: usize,
}

/// Page reclamation policy
fn kswapd_main() {
    loop {
        // Sleep until woken by page allocator
        kthread_park();

        if kthread_should_stop() {
            break;
        }

        // Reclaim pages until high watermark reached
        while free_pages() < watermarks.high {
            // Scan LRU lists
            // Evict pages (write to swap if dirty)
            // Add to free list
            reclaim_pages(BATCH_SIZE);
        }
    }
}
```

#### Deliverables

- [ ] Memory watermarks (low, high, min)
- [ ] `kswapd` per memory zone
- [ ] LRU page lists (active, inactive)
- [ ] Page reclamation algorithm
- [ ] Wakeup from page allocator
- [ ] Boot stage tests

#### Test Cases

```
KSWAPD_SPAWN: kswapd spawned
WATERMARK_CHECK: watermarks configured
MEMORY_PRESSURE: low watermark reached
KSWAPD_WAKE: kswapd woken for reclaim
PAGE_RECLAIM: pages reclaimed successfully
WATERMARK_RESTORE: high watermark restored
```

---

## File Structure

```
kernel/src/task/
├── mod.rs              # Add kthread, workqueue exports
├── thread.rs           # Existing thread structure
├── scheduler.rs        # Existing scheduler
├── spawn.rs            # Existing spawn_thread
├── kthread.rs          # NEW: kthread lifecycle
├── workqueue.rs        # NEW: workqueue system
└── softirqd.rs         # NEW: ksoftirqd threads

kernel/src/mm/
├── mod.rs              # Add kswapd export
├── frame_allocator.rs  # Add watermark checks
└── kswapd.rs           # NEW: memory reclamation thread
```

## Boot Stage Integration

Add to `xtask/src/main.rs`:

```rust
// Phase 1: Core kthread
("KTHREAD_CREATE: kthread created", "Kthread creation"),
("KTHREAD_RUN: kthread running", "Kthread running"),
("KTHREAD_STOP: kthread received stop signal", "Kthread stop"),
("KTHREAD_EXIT: kthread exited cleanly", "Kthread exit"),

// Phase 2: Workqueue
("WORKQUEUE_INIT: workqueue system initialized", "Workqueue init"),
("KWORKER_SPAWN: kworker spawned", "Kworker spawned"),
("WORK_EXECUTED: work item executed", "Work executed"),

// Phase 3: Softirqd
("KSOFTIRQD_SPAWN: ksoftirqd spawned", "Ksoftirqd spawned"),
("SOFTIRQ_PROCESS: softirq processed", "Softirq processed"),

// Phase 4: Memory
("KSWAPD_SPAWN: kswapd spawned", "Kswapd spawned"),
```

## Dependencies

### Required Before Starting

- Existing thread/scheduler infrastructure ✓
- Softirq bitmap in per_cpu.rs ✓
- Basic memory allocator ✓

### Parallel Work Possible

- Network stack improvements (can use workqueue when ready)
- Block device I/O completion (can use workqueue when ready)
- Timer subsystem improvements (can use softirq when ready)

## Testing Strategy

### Unit Tests

Each phase should have isolated tests:
- Kthread lifecycle without other threads interfering
- Workqueue with controlled work items
- Softirq with synthetic interrupt load

### Integration Tests

- Run full kernel with kthreads, verify no regressions
- Memory pressure test with kswapd
- Network load test with ksoftirqd

### Stress Tests

- Spawn 100+ kthreads, verify scheduler handles it
- Queue 1000+ work items, verify all complete
- Trigger softirq storms, verify userspace not starved

## Performance Considerations

### Kthread Overhead

- Each kthread needs ~8KB kernel stack
- Context switch cost same as regular threads
- Idle kthreads consume no CPU (parked)

### Workqueue Tradeoffs

| Approach | Latency | Throughput | Memory |
|----------|---------|------------|--------|
| Per-CPU workers | Low | High | Higher |
| Single worker | Higher | Lower | Lower |
| Work stealing | Medium | High | Higher |

**Recommendation**: Start with per-CPU workers (matches Linux default)

### Softirq Budget

- Limit `do_softirq()` to 10 iterations
- After limit, wake ksoftirqd and return
- Prevents softirq storms from blocking userspace

## References

### Linux Source

- `kernel/kthread.c` - kthread implementation
- `kernel/workqueue.c` - workqueue system
- `kernel/softirq.c` - softirq and ksoftirqd
- `mm/vmscan.c` - kswapd implementation

### Documentation

- [Linux Workqueue Documentation](https://docs.kernel.org/core-api/workqueue.html)
- [Per-CPU Kthreads](https://docs.kernel.org/admin-guide/kernel-per-CPU-kthreads.html)
- [Page Frame Reclamation](https://www.kernel.org/doc/gorman/html/understand/understand013.html)

### FreeBSD Comparison

- `sys/kern/kern_kthread.c` - kthread_add/kthread_exit
- `sys/kern/kern_synch.c` - sleep/wakeup primitives
