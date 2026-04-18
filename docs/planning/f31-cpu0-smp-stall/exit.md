# F31 Exit - CPU0 WFI Stall Under SMP

## Outcome

PARTIAL / BLOCKED.

Phase 1 completed and disproved the initial hypothesis that PR #321 / F29 alone
was the exact breaker. The first bad commit in the requested sequence was
current main `c8d6d75c`; its predecessor `d6aebfa1` was good. That points to
PR #324 / F28 as the regression boundary.

No kernel fix from this factory passed the required 120s Parallels validation,
so no fix commit or PR was opened.

## What Changed

- `docs/planning/f31-cpu0-smp-stall/phase1-bisect.md`: committed the requested
  five-commit bisect table and raw evidence pointers.
- `docs/planning/f31-cpu0-smp-stall/diagnosis.md`: recorded the F28 diagnosis,
  failed fix attempts, and blocker status.
- `docs/planning/f31-cpu0-smp-stall/prompt.md`: points to the frozen factory
  prompt.
- No kernel code changes are retained.

## Bisect Table

| Commit | Contents | Evidence | Verdict |
| --- | --- | --- | --- |
| `c8d6d75c` | current main, PRs #321-#324 | `tick_count=[142,8419,...]`, `PPI27_pending=1`, `cpu0_breadcrumb=107`, AHCI TIMEOUT | Bad |
| `d6aebfa1` | pre-F28, has F27+F29+F30 | boot completed; bsshd listened; no AHCI timeout | Good |
| `25083e23` | pre-F30, has F27+F29 | bounce started; CPU0 ticks reached 30000 | Good |
| `e2fd72e2` | pre-F27, has F29 only | 8 CPUs online; boot completed; bsshd listened; no AHCI timeout | Good |
| `ab351efe` | pre-F29, F26 baseline | bounce started; CPU0 ticks reached 85000 | Good |

## Root Cause

F28 / PR #324's graphics wait changes are the observed regression boundary, but
the exact mechanism was not fixed in this run. The bad signature is CPU0 in the
known WFI/PPI27 failure mode while other CPUs continue taking timer ticks; AHCI
then times out while init tries to start bsshd and bounce.

## Failed Fix Attempts

- Restored pre-F28 compositor waiter publication timing while keeping the
  post-block re-check: still reproduced AHCI timeout and CPU0 stall.
- Rolled back F28 `kernel/src/syscall/graphics.rs`: avoided the AHCI timeout but
  failed the full gate because bounce never started and VirGL timed out.
- Gated Parallels back to single CPU with F28 graphics intact: stalled before
  telnetd completed.
- Added `dsb sy` to shared ARM64 `halt_with_interrupts()`: still reproduced the
  SMP AHCI timeout.
- Combined F28 graphics rollback with Parallels single-CPU gate: stalled before
  bwm creation.

## Validation

Passed:

```bash
cargo build --release --target aarch64-breenix.json \
  -Z build-std=core,alloc \
  -Z build-std-features=compiler-builtins-mem \
  -p kernel --bin kernel-aarch64
```

Failed:

```bash
./run.sh --parallels --test 120
```

No 120s run satisfied all required conditions.

## PR URL

None.
