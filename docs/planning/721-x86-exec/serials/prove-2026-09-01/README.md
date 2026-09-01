# #721 prove-slot evidence — 2026-09-01

Independently reproduced (prove slot, not the implementer) at head commit
`7e2484cef33d7cd74ceebd61720cb4fcdd3aa3fc` on `fix/721-x86-exec`
(`origin/main` base `b5b56c77f2af0722ef1137ca7ded8d4d8be7ece8`).

## Leg 1 — extended x86 prod gate x10 + TTY oracle gate x3 (beast)

`docker/qemu/run-x86-prod-profile-boot-test.sh` x10: **10/10 exit=0**
(`prod-x10-summary.txt`). Every exec pin green on every run — representative
excerpt from run 10 in `prod-x10-run10-excerpt.txt`: all four positive
`EXEC_SMOKE` markers =1, all three negative markers =0, `EXEC_LOCK_ORDER
first commit`=1, all three violation counters =0.

`docker/qemu/run-x86-tty-oracle-gate.sh` x3: **3/3 exit=0**
(`tty-x3-summary.txt`), each boot `13/13 arms PASS`
(`tty-x3-run3-excerpt.txt`). **Correction to the task's framing**: arm 14
(`cloexec_exec`) is NOT running 14/14. Per `impl-notes.md`'s "Arm 14 could
not actually be re-admitted" section, round 3 (commit `40d3ead8`) re-admitted
it, hit a real x86 production-profile fork() ENOSYS gap, and was reverted
(commit `7e2484ce`, the current HEAD) with the finding filed as
[#745](https://github.com/ryanbreen/breenix/issues/745). The gate as shipped
on this branch scores 13 arms by design; that is what was proven here.

## Leg 2 — anti-vacuity: restore the ENOSYS arm

Mutated `kernel/src/syscall/handlers.rs`'s `sys_execv_with_frame` production
(`#[cfg(not(feature = "testing"))]`) arm on beast back to a bare
`SyscallResult::Err(38)` (warning-clean via explicit `let _ =` references, so
the runtime assertions redden, not the build's own warnings gate — see
`anti-vacuity-red-excerpt.txt`).

Rebuilt, ran the prod-profile gate: **RED**, exactly as required —
`anti-vacuity-red-excerpt.txt`:
```
x86 production-profile gate: FAIL (set -e abort at docker/qemu/run-x86-prod-profile-boot-test.sh:904, exit 1)
  failing command: test "$(marker_count "$EXEC_SMOKE_TARGET_ENTER_LITERAL")" -eq 1
  exec smoke target enter argc=2 (#721): 0
  exec smoke exec failed (must be absent, #721): 1
  exec lock order first commit (#721 K7): 0
```
`EXEC_SMOKE:EXEC_FAILED` fired instead of `TARGET_ENTER`, and the commit
receipt never fired (`first commit: 0`) — the exec bailed before ever
reaching the scheduler commit, exactly as expected.

`git checkout --` restored the file (`git status`/`git diff --stat` both
empty after), rebuilt (zero warnings), reran: **GREEN** again —
`anti-vacuity-restore-green-excerpt.txt`, all pins back to the leg-1 shape.

K1/K3/K4/K7/K13's condition-specific mutations are the `negative_*` tests
already built into `tests/exec_lock_order_structure.rs` (host leg, below) —
42/42 pass, including every named negative test the precheck's binding
conditions require.

## Leg 3 — run-x86-boot-tests.sh x5 + run-x86-gate.sh 1 full (beast)

`run-x86-boot-tests.sh` x5: **5/5 exit=0**, zero known-signature flakes
observed this run (`boot-tests-x5-summary.txt`,
`boot-tests-x5-verdicts.txt`) — no #716/#700/#692/#702/900s-poll-ceiling
occurrences to attribute; UNATTRIBUTED=0 trivially (no failures at all).

`run-x86-gate.sh 1 full`: **PASS**
(`full-gate-x1-verdict.txt`: `GATE: PASS (1/1 boot tests passed; mode=full
build=16s boot=150s total=174s)`).

## Leg 4 — local aarch64 (QEMU quiet-wait)

`run-aarch64-full-test.sh --rebuild --boot-tests-only`: **PASS, 109/109
tests** (`aarch64-boot-tests-only-verdict.txt`).

`run-aarch64-service-sequence-gate.sh --profile both --boots 25 --rebuild`:
**49/50 GREEN (98.0%)**, UNATTRIBUTED=0 both profiles
(`aarch64-service-sequence-gate-census.txt`). The single non-GREEN boot
(max profile, boot 24/25) is the gate's own pre-adjudicated
[#690](https://github.com/ryanbreen/breenix/issues/690) signature
(`clonevm_exec_test` second-stage stall, ~1-in-30 rate, independently
confirmed as a pre-existing OPEN issue via `gh issue view 690`) — the gate
script's own comment labels it "pre-existing, ATTRIBUTED". Not touched by
this diff (aarch64's own exec path is untouched; #721 is the x86 production
exec wiring). cortex-a72 profile: 25/25 GREEN (100%).

## Leg 5 — host structural suites (individually)

- `cargo test --test exec_lock_order_structure`: **42/42 passed**
  (`host-exec_lock_order_structure.txt`).
- `cargo test --test tty_oracle_structure`: **16/16 passed**
  (`host-tty_oracle_structure.txt`).
- `cargo test --test teardown_structure`: **79/81 passed, 2 pre-existing
  failures** (`host-teardown_structure.txt`):
  `v3_structural_closures_are_exact` and
  `deliberately_broken_variants_fail_the_ratchet`. Independently verified
  pre-existing and unrelated to this diff: reproduced the identical two
  failures on an unmodified `origin/main` @ `b5b56c77f2af0722ef1137ca7ded8d4d8be7ece8`
  checkout (separate worktree, same command) — both concern
  `kernel/src/proof/driver_h.rs` (thread-state census) and
  `kernel/src/syscall/futex.rs` (vacuity checks), neither touched anywhere
  in this branch's diff (`git diff --stat` against the same base lists 15
  files, none of them `driver_h.rs` or `futex.rs`).

## Overall

x86: leg 1 (13/13 = 10/10, 13/13 = 3/3), leg 2 (red-then-green clean), leg 3
(5/5 + 1/1 full) all green; no unattributed failures.
aarch64: boot-tests-only 109/109; service-sequence 49/50 (1x pre-adjudicated
#690, UNATTRIBUTED=0).
Host: 42+16 = 58/58 new/touched-surface structural tests green;
teardown_structure's 2 pre-existing failures independently confirmed
unrelated.
