# F32s Exit — Baseline Validation Blocker

Date: 2026-04-20
Branch: `f32s-percpu-resched-staged`
Base commit: `18c2771e` (`F32r: per-CPU resched audit + design + primitive (no consumers) (#331)`)

## What I Built

No production code was changed.

Created this exit document because Phase 1 could not establish the required
green validation harness on the unmodified base branch.

## Original Ask

F32s was intended to convert scheduler wake sites to `resched_cpu(target)` one
site at a time, with full validation after each individual site. The first
required step was to establish a fast validation loop:

- `wait_stress` 60 seconds with zero stalls.
- Quick 60-second Parallels boot reaching bsshd, bounce, render PASS, and
  frames.

Only after that baseline was green should `spawn()`, `spawn_front()`, and I/O
wake sites be converted one at a time.

## Phase 1 Result

Phase 1 did not pass. The requested baseline harness is not green on the
unmodified F32r base commit, so no wake-site conversion was attempted.

| Gate | Command | Result | Evidence |
| --- | --- | --- | --- |
| x86 boot-stages baseline | `cargo run -p xtask -- boot-stages` | FAIL | 201/252 stages passed; first failed stage `[83/252] TCP connect executed`; meaning `sys_connect failed`. |
| wait_stress baseline run 1 | `BREENIX_WAIT_STRESS=1 ./run.sh --parallels --test 150` | FAIL | Wrapper exited 0, but serial reached `[init] wait_stress enabled; starting 60s waitqueue stress` and `[spawn] path='/bin/wait_stress'`; no `WAIT_STRESS_PASS`, no `WAIT_STRESS_PROGRESS`, no `WAIT_STRESS_STALL`. |
| wait_stress baseline run 2 | `BREENIX_WAIT_STRESS=1 ./run.sh --parallels --test 150 --no-build` | FAIL | Wrapper exited 0, but serial stopped after `[init] Breenix init starting (PID 1)`; no `WAIT_STRESS_PASS`, no progress, and no stall marker. |

The `./run.sh --parallels --test` wrapper's zero exit code is not sufficient
for this factory gate; the serial log must contain `WAIT_STRESS_PASS` and no
`WAIT_STRESS_STALL`. Both baseline runs failed that stricter requirement.

## Site-By-Site Conversion Table

No conversion site was touched because Phase 1 failed.

| Site | Status | Commit | Validation |
| --- | --- | --- | --- |
| `spawn()` | Not attempted | n/a | Blocked by failed baseline |
| `spawn_front()` | Not attempted | n/a | Blocked by failed baseline |
| `unblock_for_signal()` / scheduler line near 1509 | Not attempted | n/a | Blocked by failed baseline |
| `unblock_for_child_exit()` / scheduler line near 1604 | Not attempted | n/a | Blocked by failed baseline |
| `wake_io_thread_locked()` / scheduler line near 1769 | Not attempted | n/a | Blocked by failed baseline |
| schedule fallback line near 846 | Not attempted | n/a | Blocked by failed baseline |
| idle gate per-CPU-only switch | Not attempted | n/a | Blocked by failed baseline |
| redundant global removal | Not attempted | n/a | Blocked by failed baseline |
| global `NEED_RESCHED` deletion | Not attempted | n/a | Blocked by failed baseline |

## Before/After CPU Measurement

Not measured. The run stopped before Phase 3 and before any code conversion, so
there is no meaningful before/after CPU comparison for F32s.

## PR URL

No PR was opened. This branch contains only blocker documentation.

## What I Did Not Build

- No `resched_cpu(target)` consumers were added.
- No idle gate behavior was changed.
- No global `NEED_RESCHED` stores or readers were removed.
- No Tier 1 files were edited.
- No serial breadcrumbs were added.

## Known Risks And Gaps

The immediate blocker is that the validation harness required by the F32s plan
does not pass on the unmodified base commit. Converting wake sites now would
make it impossible to attribute later failures to a specific conversion.

The two Parallels stress runs failed differently:

- Run 1 reached the wait_stress spawn but produced no wait_stress output.
- Run 2 stopped earlier, after init startup.

That suggests either a pre-existing scheduler/boot flake at this base, a
Parallels harness issue, or an unrelated regression on `main`. The F32s
conversion should restart only after the wait_stress baseline is proven green.

## How To Verify

From `f32s-percpu-resched-staged` at `18c2771e`, run:

```bash
pkill -9 qemu-system-x86 2>/dev/null; killall -9 qemu-system-x86_64 2>/dev/null; pgrep -l qemu || echo "All QEMU processes killed"
cargo run -p xtask -- boot-stages
BREENIX_WAIT_STRESS=1 ./run.sh --parallels --test 150
rg -n "WAIT_STRESS|WAIT_STRESS_PASS|WAIT_STRESS_STALL|spawn" /tmp/breenix-parallels-serial.log
```

The F32s implementation may proceed only after the wait_stress command produces
`WAIT_STRESS_PASS` with no `WAIT_STRESS_STALL` on the unchanged base.
