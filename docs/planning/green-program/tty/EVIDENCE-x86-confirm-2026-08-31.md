# TTY — x86 evidence port, confirmation pass, 2026-08-31

Green program, TTY-x86 evidence arc. Confirmation slot. Companion to
`EVIDENCE-2026-08-30.md` (the aarch64 arc-4 record this port extends). Branch
`feat/green-tty-x86`, tested at `16d6ff5b97984b62e5fb37b8f32fa75b25a67aa3`, on
`main @ 09ae3f44cfcfc1be11a04a5739c44bb4f3d3b007`.

This is the in-repo evidence record for an independent confirmation pass over the
implement slot's work (`ff08f344` + `16d6ff5b`, see that slot's `impl-notes.md`,
not committed in-repo). The full narrative dossier is
`scratchpad/gttyx86/dossier.md` in the confirming session's own scratchpad (not
committed — session-scoped); this file is the durable, in-repo summary. Serials
referenced below are alongside this file in `serials/`, prefixed `x86-` (the
aarch64-side serials from arc 4 are `aarch64-*` in the same directory).

## Summary verdict

Four of five confirmation legs are fully green and reproduce the implement slot's
claims exactly (10 total x86 production-profile boots across two gate scripts, one
shared-kernel-code mutation reddening identically to its aarch64 counterpart and
reverting clean, aarch64 fully unregressed at 3+1 boots). **One leg found a real,
attributed, blocking defect**: `tests/teardown_structure.rs`'s
`x86_production_profile_gate_verdict_discipline_holds` test — a pre-existing,
project-wide ratchet this port did not touch — reddens because the port's new
`TTY_ORACLE_LITERAL` pin in `run-x86-prod-profile-boot-test.sh` uses
`-ge 1` instead of the exact-count `-eq` convention that ratchet requires and every
other pin in that file follows. Not fixed in this confirmation pass (read-only
verification of landed bytes); the fix is a one-line change
(`-ge 1` → `-eq 2`, the count is structurally always exactly 2 on this port's design).

## Arm disposition (14 total, 13 on x86)

All 14 arms use only setsid/ioctl/PTY/termios/fork primitives except
`cloexec_exec` (arm 14), which requires a real `exec()` return-never and is excluded
on x86 pending `#721` (x86 `exec()` ENOSYS in the zero-feature production build).
The exclusion is visible, not silent: `#[cfg(target_arch = "aarch64")]` on both the
call site and the arm's two arm-local imports, `ARM_COUNT` split 14/13, and the x86
gate additionally asserts the arm emits **zero** verdict lines (pass or fail), not
merely that it's absent from the expected-PASS list.

## Leg results

1. **`run-x86-prod-profile-boot-test.sh` x5** (no `--boots` flag; five separate
   invocations) — all `PASS`, TTY canary `complete=2:fail=0` every boot, every
   `#673`/`#713`-era pin green throughout.
   **`run-x86-tty-oracle-gate.sh --boots 5 --rebuild-userspace`** — 13/13 arms PASS
   all 5 boots, `[TTY_ORACLE:COMPLETE:pass=13:fail=0]` every boot, zero
   `cloexec_exec` verdict lines.
2. **Anti-vacuity mutation** — `kernel/src/syscall/pty.rs`'s M1a fix (master-side
   `O_NONBLOCK` carry-through, zero `target_arch` cfg in the file, genuinely shared
   code) reverted to `let status_flags = 0;`. Reddened the dedicated x86 gate with
   the **exact same signature** as aarch64's own M1a result:
   `FAIL:nonblock_open:master_nonblock_dropped:fl=0x0`. Reverted: `git diff --stat`
   empty, rebuilt, green again (1/1 boots, 13/13 arms).
3. **aarch64 regression** — `run-aarch64-tty-oracle-gate.sh --boots 3
   --rebuild-userspace`: 3/3 boots, 14/14 arms PASS, unchanged from the arc-4
   baseline. `run-aarch64-prod-profile-boot-test.sh --rebuild-userspace`: PASS, TTY
   oracle marker count 2, failure count 0. Service-sequence 25x2 not run: `git diff
   --name-only 09ae3f44..16d6ff5b` touches zero `kernel/` paths, so there is no
   shared kernel code change for that gate to re-prove.
4. **Host structural suites** — `tty_oracle_structure.rs`: 16/16 green, including
   the x86-side mutation battery (4 planted drifts, all correctly caught).
   `teardown_structure.rs`: **80 PASS, 1 FAIL**
   (`x86_production_profile_gate_verdict_discipline_holds`, see summary above — the
   file itself is untouched by this port's diff, confirming the ratchet was already
   green on `main` and reddens solely because of the port's own new `-ge 1` pin).
5. **`run-x86-boot-tests.sh` 10+ boots** — two full attempts, neither a clean
   single-invocation run (the script aborts the whole batch on its first red).
   **Attempt 1**: boot 1 PASS, boot 2 FAILED. **Attempt 2**: boots 1-8 PASS, boot 9
   FAILED. 9 PASS / 2 FAIL across 11 total boot attempts. Both failures share the
   identical shape: the script's internal 900-second poll loop expired before
   observing every required success marker present in one simultaneous tick, but
   every marker — including the two terminal ones, `TEST_TALLY:` and `🏁 TEST
   RUNNER: All tests passed 🏁` — is present in that same boot's own serial when
   inspected afterward, immediately preceded by a long run of live DNS/HTTP
   `sys_recvfrom` traffic. Neither failing boot's serial contains any crash/panic
   marker; both show clean idle-loop steady state.
   `grep -ci "tty_oracle\|\[init\]"` against all four serial files (both arches of
   both failing boots) returns **zero** matches every time, structurally proving
   neither failure can involve the TTY-x86 port's code: `run_tty_oracle()` (and
   everything else this port added) is reachable only through the production-init
   cfg block in `kernel/src/main.rs`, which is compiled *out* whenever `feature =
   "testing"` is set — and this gate always builds with `testing` set.
   Attempt 1's failure coincided with beast under severe, unrelated contention
   (load average 20.8/21.0/23.5 on an 8-core host at the time, including a chronic
   966%-CPU process running since Aug 27) — the same failure *class* `#725`
   pre-adjudicated as host-contention. **Attempt 2's failure occurred under
   confirmed-low, uncontended load (1.0-1.2 throughout)**, ruling out host CPU
   contention as the sole or necessary cause and showing this is a distinct,
   recurring, load-independent flake in the registry's own DNS/HTTP-leg timing
   margin against the fixed 900s budget. Filed as a new issue,
   **[`#731`](https://github.com/ryanbreen/breenix/issues/731)** — not fixed here
   (pre-existing in `main`'s own gate, unrelated to this port, filing is this
   confirm slot's appropriate action per its read-only charter).

## Honest limits

- The Leg 4 red is real and blocking a clean "confirm GREEN" declaration; not fixed
  in this pass by design (read-only confirmation of landed bytes at `16d6ff5b`).
- Leg 5's two failures are unrelated-to-this-port with certainty (structural
  code-unreachability proof, confirmed empirically on both occurrences), but the
  root cause of the flake itself (a pre-existing, `main`-inherited registry-timing
  margin, filed as `#731`) is not diagnosed further than "DNS/HTTP-leg timing vs.
  the fixed 900s budget, load-independent" in this pass.
- `#705`'s issue-body `@path` documentation defect (a prior session's `gh issue
  edit` mistake) is still present, unfixed; flagged again, not this slot's to fix.
