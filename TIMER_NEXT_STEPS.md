# Breenix Timer System - Next Steps

**Date**: January 21, 2025  
**Author**: Ryan Breen & Claude Code  
**Purpose**: Actionable implementation roadmap based on expert timer system recommendations

## Executive Summary

The basic timer system is now functional with 1ms resolution. This document outlines the next implementation steps based on expert guidance, organized as discrete, one-day story-point tasks suitable for Jira tracking.

## Architecture Decisions

### 1. Timer Hardware Strategy

| Phase | Hardware | Use Case | Resolution |
|-------|----------|----------|------------|
| **Current** | PIT @ 1kHz | Basic scheduling, ms sleeps | 1ms |
| **Next** | PIT + RTC | Wall clock time | 1ms + 1s |
| **Future** | TSC-deadline LAPIC | µs-level timing | <1µs |
| **Optional** | HPET fallback | Old hardware without invariant TSC | ~100ns |

**Decision**: Stick with PIT for now, plan TSC-deadline upgrade after core features work.

### 2. System Call Evolution

Keep existing `sys_get_time` returning milliseconds (u64) for backward compatibility.

Add new POSIX-style calls:
```rust
sys_clock_gettime(clock_id: u32, timespec: *mut Timespec) -> i32
sys_nanosleep(req: *const Timespec, rem: *mut Timespec) -> i32
```

### 3. Time Synchronization Strategy

1. **Boot**: Read RTC once → `boot_wall_time`
2. **Runtime**: `wall_time = boot_wall_time + monotonic_ms`
3. **Drift correction**: Gradual slew algorithm (preserves monotonic ordering)
4. **Future**: NTP client replaces RTC as time authority

## Implementation Tasks

### Task 1: RTC Driver & Wall-Clock API 🕐

**Story Points**: 1 day  
**Dependencies**: None

**Deliverables**:
```rust
// kernel/src/time/rtc.rs
pub fn init();
pub fn read_datetime() -> DateTime;

// kernel/src/time/mod.rs
pub fn get_real_time() -> DateTime;  // boot_wall_time + monotonic
```

**Implementation Details**:
- CMOS RTC ports: 0x70 (index), 0x71 (data)
- Read registers 0x00-0x09 (seconds through year)
- BCD decode: `(bcd >> 4) * 10 + (bcd & 0xF)`
- Double-read validation for consistency
- Cache `boot_wall_time` during init

**Acceptance Criteria**:
- [ ] RTC driver reads valid date/time
- [ ] `get_real_time()` returns wall clock time
- [ ] Unit test: BCD conversion
- [ ] Integration test: time advances between reads

### Task 2: POSIX clock_gettime Syscall 🕑

**Story Points**: 1 day  
**Dependencies**: Task 1 (for CLOCK_REALTIME)

**Deliverables**:
```rust
// Syscall numbers
const SYS_CLOCK_GETTIME: u64 = 228;  // Linux x86_64 number

// Clock IDs
const CLOCK_REALTIME: u32 = 0;
const CLOCK_MONOTONIC: u32 = 1;

// Implementation
fn sys_clock_gettime(clock_id: u32, timespec: *mut Timespec) -> SyscallResult;
```

**Implementation Details**:
- `CLOCK_MONOTONIC`: Convert `get_monotonic_time()` to timespec
- `CLOCK_REALTIME`: Convert `get_real_time()` to timespec
- Use existing `copy_to_user` for safe memory access
- Return -EINVAL for unknown clock_id

**Acceptance Criteria**:
- [ ] Syscall handler registered
- [ ] Both clock types return valid timespecs
- [ ] Userspace test program validates monotonic ordering
- [ ] No time regression in consecutive calls

### Task 3: Timer Wheel & Sleep Implementation 🕒

**Story Points**: 1 day  
**Dependencies**: None

**Deliverables**:
```rust
// Kernel API
pub fn sleep_ms(ms: u64);
pub fn sleep_until(deadline: u64);

// Syscall
fn sys_nanosleep(req: *const Timespec, rem: *mut Timespec) -> SyscallResult;
```

**Implementation Details**:
```rust
// Timer wheel structure
struct TimerWheel {
    // Binary heap of (deadline_ms, thread_id)
    timers: BinaryHeap<(u64, u64)>,
}

// In timer interrupt:
while let Some(&(deadline, thread_id)) = timers.peek() {
    if deadline <= current_ms {
        wake_thread(thread_id);
        timers.pop();
    } else {
        break;
    }
}
```

**Acceptance Criteria**:
- [ ] Threads can sleep for specified duration
- [ ] Multiple threads can sleep concurrently
- [ ] Sleep accuracy within ±2ms
- [ ] Userspace test: 10Hz periodic task

### Task 4: TSC-Deadline Fast Path (Optional) 🕓

**Story Points**: 1 day  
**Dependencies**: Tasks 1-3 complete

**Deliverables**:
```rust
// Feature detection
pub fn has_invariant_tsc() -> bool;
pub fn has_tsc_deadline() -> bool;

// Fast sleep for < 1ms delays
pub fn sleep_us(us: u64);
```

**Implementation Details**:
- Check CPUID.80000007H:EDX[8] for invariant TSC
- Check CPUID.01H:ECX[24] for TSC-deadline
- MSR_IA32_TSC_DEADLINE (0x6E0) programming
- Fallback to PIT if unavailable

**Acceptance Criteria**:
- [ ] Feature detection works on real hardware
- [ ] Sub-millisecond sleeps possible
- [ ] Graceful fallback on old CPUs
- [ ] Benchmark: µs-level accuracy

### Task 5: Virtualization Optimizations 🕔

**Story Points**: 1 day  
**Dependencies**: Basic timer system working

**Deliverables**:
- KVM-clock detection and usage
- Paravirt timer configuration
- Guest time sync protocol

**Implementation Details**:
- Check CPUID for KVM signature
- Use kvmclock for CLOCK_REALTIME when available
- Prefer TSC-deadline even under virtualization
- Add virtio-time support (future)

## Testing Strategy

### Unit Tests
```rust
#[test]
fn test_monotonic_never_decreases() {
    let t1 = get_monotonic_time();
    busy_wait_ms(10);
    let t2 = get_monotonic_time();
    assert!(t2 > t1);
}

#[test]
fn test_rtc_bcd_decode() {
    assert_eq!(bcd_to_binary(0x59), 59);
    assert_eq!(bcd_to_binary(0x12), 12);
}
```

### Integration Tests
```rust
// Userspace test program
fn periodic_task_test() {
    for i in 0..10 {
        let start = clock_gettime(CLOCK_MONOTONIC);
        println!("Tick {}: {}ms", i, start);
        nanosleep(100_000_000); // 100ms
    }
}
```

### Performance Benchmarks
- PIT overhead: ~1-2µs per tick
- TSC-deadline overhead: ~100ns per timer
- Sleep accuracy: ±1ms for PIT, ±1µs for TSC

## Migration Path

1. **Phase 1** (Current): Basic PIT timer ✅
2. **Phase 2**: Add RTC + wall clock
3. **Phase 3**: Timer wheel + sleep syscalls
4. **Phase 4**: POSIX time APIs
5. **Phase 5**: TSC-deadline optimization
6. **Phase 6**: Tickless kernel (distant future)

## Risk Mitigation

| Risk | Mitigation |
|------|------------|
| RTC unreliable in VMs | Use KVM-clock when available |
| TSC not invariant | Feature detection + HPET fallback |
| Sleep accuracy poor | Document limitations, offer high-res alternative |
| Time goes backwards | Monotonic clock + gradual slew for corrections |

## Summary

The timer system evolution follows a pragmatic path:
1. Start simple (PIT) ✅
2. Add essential features (RTC, sleep)
3. Optimize later (TSC-deadline)
4. Support virtualization properly

Each task is independently implementable and testable, suitable for single-developer sprints.

## Next Action

Start with **Task 1: RTC Driver** as it has no dependencies and enables wall-clock time for the shell prompt and logging.