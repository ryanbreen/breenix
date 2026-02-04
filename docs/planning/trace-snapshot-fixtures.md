# Trace Snapshot Fixture System - Planning Document

## Problem Statement

Currently, Breenix integration tests validate kernel behavior primarily through serial output markers (e.g., searching for "KTHREAD_EXIT: kthread exited cleanly"). This approach has limitations:

1. **Limited observability**: Serial output only captures explicitly logged events, missing the rich behavioral data in trace buffers
2. **No regression detection for timing/ordering**: A change that subtly alters the sequence of kernel events (context switches, syscalls, interrupts) goes undetected if it doesn't affect serial output
3. **Debugging requires reproduction**: When tests fail in CI, developers must manually reproduce and attach GDB to understand what happened
4. **No behavioral baseline**: There's no "known good" reference for what the trace buffer should contain after a successful test run

The tracing framework (`kernel/src/tracing/`) already captures detailed kernel events in per-CPU ring buffers (TraceEvent structures with timestamps, event types, CPU IDs, and payloads). This data is currently only inspected ad-hoc via GDB or post-mortem serial dumps.

**Proposed Solution**: Capture trace buffer snapshots as frozen fixtures after successful test runs. In CI, compare actual trace output against these fixtures to detect behavioral regressions automatically.

---

## Proposed Architecture

### High-Level Flow

```
┌─────────────────┐     ┌─────────────────┐     ┌─────────────────┐
│   Integration   │     │  GDB-based      │     │   Fixture       │
│   Test Runs     │────▶│  Trace Capture  │────▶│   Normalization │
│   (QEMU)        │     │  (at breakpoint)│     │   & Storage     │
└─────────────────┘     └─────────────────┘     └─────────────────┘
                                                         │
                                                         ▼
                                    ┌─────────────────────────────────┐
                                    │   tests/fixtures/traces/        │
                                    │   ├── boot_post.trace.json      │
                                    │   ├── kthread_test.trace.json   │
                                    │   └── syscall_test.trace.json   │
                                    └─────────────────────────────────┘
                                                         │
                         ┌───────────────────────────────┘
                         ▼
┌─────────────────┐     ┌─────────────────┐     ┌─────────────────┐
│   CI Test Run   │────▶│  Capture Actual │────▶│   Compare vs    │
│   (same test)   │     │  Trace Data     │     │   Fixture       │
└─────────────────┘     └─────────────────┘     └─────────────────┘
                                                         │
                                                         ▼
                                                  ┌──────────────┐
                                                  │ PASS / FAIL  │
                                                  │ + diff report│
                                                  └──────────────┘
```

### Components

1. **Trace Capture Module** (`scripts/trace_fixture_capture.py`)
   - Extends existing `scripts/trace_memory_dump.py`
   - Uses GDB to dump `TRACE_BUFFERS` memory at test completion breakpoint
   - Parses raw bytes into structured trace events
   - Also captures counter values from `TRACE_COUNTERS`

2. **Normalizer Module** (`scripts/trace_normalizer.py`)
   - Removes/masks volatile fields (absolute timestamps)
   - Preserves invariant fields (event types, syscall numbers, relative ordering)
   - Computes derived metrics (event counts, timing deltas)

3. **Fixture Storage** (`tests/fixtures/traces/`)
   - JSON files committed alongside tests
   - One fixture per test scenario
   - Includes metadata (kernel version, test parameters)

4. **Comparison Engine** (`scripts/trace_fixture_compare.py`)
   - Loads fixture and actual trace data
   - Performs structural diff
   - Reports mismatches with context

5. **Test Harness Integration**
   - New pytest/cargo test wrapper
   - Optionally runs in "capture mode" to generate fixtures
   - Default runs in "verify mode" to compare against fixtures

---

## Fixture Format

### Why JSON (not binary)?

- **Diffable**: Git diffs show meaningful changes
- **Debuggable**: Developers can inspect fixtures directly
- **Extensible**: Easy to add new fields without breaking existing fixtures
- **Tool-friendly**: Python, jq, and other tools work natively

### Structure

```json
{
  "version": "1.0",
  "metadata": {
    "test_name": "boot_post_test",
    "kernel_commit": "7182bac",
    "capture_date": "2026-02-04T12:00:00Z",
    "architecture": "x86_64",
    "qemu_args": "-smp 1 -m 512"
  },
  "trace_events": {
    "normalized": true,
    "events": [
      {
        "cpu_id": 0,
        "event_type": "0x0102",
        "event_name": "TIMER_TICK",
        "payload": 1,
        "flags": 0,
        "delta_ns": 1000000
      },
      {
        "cpu_id": 0,
        "event_type": "0x0300",
        "event_name": "SYSCALL_ENTRY",
        "payload": 228,
        "flags": 0,
        "delta_ns": 50000
      }
    ],
    "summary": {
      "total_events": 1024,
      "by_type": {
        "TIMER_TICK": 500,
        "SYSCALL_ENTRY": 120,
        "SYSCALL_EXIT": 120,
        "CTX_SWITCH_ENTRY": 50
      }
    }
  },
  "counters": {
    "SYSCALL_TOTAL": 120,
    "IRQ_TOTAL": 500,
    "CTX_SWITCH_TOTAL": 50,
    "TIMER_TICK_TOTAL": 500
  },
  "assertions": {
    "min_timer_ticks": 100,
    "syscall_entry_exit_balance": true,
    "no_unknown_events": true
  }
}
```

### Key Design Decisions

1. **Events stored as list, not per-CPU arrays**: Merged chronologically for easier comparison
2. **`delta_ns` instead of absolute `timestamp`**: Relative timing preserved, absolute values discarded
3. **`summary` section**: Quick sanity checks without parsing every event
4. **`assertions`**: Optional constraints that must hold for fixture validity

---

## Normalization Strategy

### Volatile Fields (to remove/mask)

| Field | Why Volatile | Normalization |
|-------|--------------|---------------|
| `timestamp` | Absolute cycle count varies per run | Convert to `delta_ns` (difference from previous event) |
| Per-CPU write indices | Depends on exact timing | Omit from fixture |
| `dropped` counts | Varies with timing | Include but allow range tolerance |

### Invariant Fields (to preserve exactly)

| Field | Why Invariant |
|-------|---------------|
| `event_type` | Kernel behavior - same code paths = same events |
| `syscall_nr` (in payload) | Specific syscalls executed |
| `payload` for most events | Tied to kernel logic |
| Event ordering within CPU | Deterministic for single-threaded tests |

### Semi-Invariant Fields (allow tolerance)

| Field | Tolerance |
|-------|-----------|
| `delta_ns` | +/- 50% for timer events (QEMU timing varies) |
| Event count | +/- 10% for interrupt-driven events |
| Multi-CPU ordering | Allow event reordering between CPUs |

---

## Capture Workflow

### Manual Fixture Generation

```bash
# Build kernel with tracing enabled
cargo build --release --features testing,external_test_bins --bin qemu-uefi

# Run test with fixture capture
./scripts/trace_fixture_capture.py \
  --test boot_post \
  --breakpoint "kernel::post::POST_COMPLETE" \
  --output tests/fixtures/traces/boot_post.trace.json

# Review and commit
git add tests/fixtures/traces/boot_post.trace.json
git commit -m "Add trace fixture for boot_post test"
```

### GDB Capture Implementation

Building on `scripts/test_tracing_via_gdb.sh`:

```bash
# Start QEMU with GDB
qemu-system-x86_64 ... -gdb tcp::1234 -S &

# Connect GDB and capture
gdb -batch -x - << 'EOF'
target remote localhost:1234
break *POST_COMPLETE_ADDRESS
continue
dump binary memory /tmp/trace_buffers.bin $TRACE_BUFFERS $TRACE_BUFFERS+$TOTAL_SIZE
dump binary memory /tmp/counters.bin $TRACE_COUNTERS $COUNTER_SIZE
quit
EOF

# Parse and normalize
python3 scripts/trace_fixture_capture.py \
  --buffers /tmp/trace_buffers.bin \
  --counters /tmp/counters.bin \
  --normalize \
  --output tests/fixtures/traces/boot_post.trace.json
```

---

## CI Integration

### Test Wrapper Script

```bash
#!/bin/bash
# docker/qemu/run-boot-with-trace-validation.sh

set -e

# Run test and capture trace
./scripts/trace_fixture_capture.py \
  --test boot_post \
  --actual-output /tmp/actual_trace.json

# Compare against fixture
python3 scripts/trace_fixture_compare.py \
  --expected tests/fixtures/traces/boot_post.trace.json \
  --actual /tmp/actual_trace.json \
  --report /tmp/trace_diff.txt

if [ $? -ne 0 ]; then
  echo "TRACE REGRESSION DETECTED"
  cat /tmp/trace_diff.txt
  exit 1
fi

echo "Trace validation passed"
```

### Failure Reporting

When a trace mismatch occurs, the diff report should include:

1. **Summary**: "Expected 120 SYSCALL_ENTRY events, got 118"
2. **First divergence point**: "Events diverge at index 42"
3. **Context**: Show 5 events before and after divergence
4. **Event type breakdown**: Table comparing expected vs actual counts

---

## Update Workflow

When kernel behavior intentionally changes:

```bash
# Option 1: Regenerate single fixture
./scripts/trace_fixture_capture.py \
  --test boot_post \
  --update-fixture

# Option 2: Regenerate all fixtures
./scripts/trace_fixture_capture.py --regenerate-all

# Review changes
git diff tests/fixtures/traces/

# Commit with explanation
git commit -m "Update trace fixtures for new scheduler behavior" \
  -m "The scheduler now yields after 10ms instead of 20ms, doubling timer tick events."
```

---

## Granularity Options

### Full Trace Mode (default)

Capture all events in the ring buffer. Best for:
- Boot sequence tests
- Single-operation tests (one syscall, one context switch)

### Filtered Mode

Capture only specific event types:

```bash
./scripts/trace_fixture_capture.py \
  --test syscall_test \
  --filter-events SYSCALL_ENTRY,SYSCALL_EXIT \
  --output tests/fixtures/traces/syscall_test.trace.json
```

Best for:
- Tests that trigger many irrelevant interrupts
- Focusing on specific subsystem behavior

### Summary Mode

Capture only event counts and counters, not individual events:

```json
{
  "trace_events": {
    "normalized": true,
    "events": [],
    "summary": {
      "total_events": 1024,
      "by_type": { "TIMER_TICK": 500, "SYSCALL_ENTRY": 120 }
    }
  }
}
```

Best for:
- High-level regression detection
- Tests where exact event sequence varies

---

## Implementation Phases

### Phase 1: Core Capture Infrastructure

**Tasks:**
1. Extend `scripts/trace_memory_dump.py` with fixture output format
2. Implement normalization (timestamp to delta_ns conversion)
3. Create `tests/fixtures/traces/` directory structure
4. Document fixture JSON schema

**Deliverables:**
- `scripts/trace_fixture_capture.py` that produces valid fixture JSON
- Single fixture: `tests/fixtures/traces/boot_post.trace.json`

### Phase 2: Comparison Engine

**Tasks:**
1. Implement structural JSON comparison
2. Add tolerance for semi-invariant fields
3. Generate human-readable diff reports
4. Unit tests for comparison logic

**Deliverables:**
- `scripts/trace_fixture_compare.py`
- Test cases for various mismatch scenarios

### Phase 3: Test Harness Integration

**Tasks:**
1. Create wrapper scripts for boot tests
2. Integrate with `docker/qemu/run-boot-parallel.sh`
3. Add GitHub Actions workflow step
4. Document update workflow

**Deliverables:**
- `docker/qemu/run-boot-with-trace-validation.sh`
- CI passes/fails based on trace comparison
- `CLAUDE.md` documentation update

### Phase 4: Multi-Test Expansion

**Tasks:**
1. Generate fixtures for kthread tests
2. Generate fixtures for syscall tests
3. Add filtered capture mode
4. Add summary-only mode

**Deliverables:**
- Fixtures for all major integration tests
- `--filter-events` and `--summary-only` CLI options

### Phase 5: Polish and Documentation

**Tasks:**
1. Error handling and edge cases
2. Performance optimization for large traces
3. Complete documentation
4. Example regression scenario walkthrough

**Deliverables:**
- Production-ready tooling
- `docs/planning/trace-fixture-usage.md`

---

## Open Questions

1. **Breakpoint selection**: How do we determine where to capture the trace?
   - Option A: Named kernel markers (e.g., `POST_COMPLETE`)
   - Option B: Timeout-based (capture after 5 seconds of boot)
   - Option C: Event-triggered (capture after N syscalls)
   - **Recommendation**: Named markers for determinism

2. **Multi-architecture support**: How do fixtures work for ARM64?
   - Option A: Separate fixtures per architecture
   - Option B: Shared fixtures with arch-specific sections
   - **Recommendation**: Separate fixtures initially (ARM64 has different event timings)

3. **Fixture versioning**: What happens when we add new event types?
   - Option A: Fixtures specify schema version, comparator handles upgrades
   - Option B: Regenerate all fixtures when schema changes
   - **Recommendation**: Schema version field with backward compatibility

4. **Counter tolerance**: How much variation is acceptable?
   - Timer interrupts: Highly variable (allow +/- 50%)
   - Syscall counts: Should be exact (0% tolerance)
   - Context switches: Moderate variation (allow +/- 10%)

5. **Storage size**: Will fixtures bloat the repository?
   - Estimate: 1024 events * ~100 bytes/event = ~100KB per fixture
   - With 20 tests: ~2MB total
   - **Acceptable for now; add compression if needed**

---

## Critical Files for Implementation

- `scripts/trace_memory_dump.py` - Foundation for trace parsing; extend with fixture output
- `kernel/src/tracing/core.rs` - TraceEvent and TRACE_BUFFERS definitions; must match parser
- `kernel/src/tracing/counter.rs` - TraceCounter structure for capturing statistics
- `docker/qemu/run-boot-parallel.sh` - Pattern for CI test execution; integrate fixture validation
- `tests/shared_qemu.rs` - Existing test harness; understand checkpoint mechanism for breakpoint selection
