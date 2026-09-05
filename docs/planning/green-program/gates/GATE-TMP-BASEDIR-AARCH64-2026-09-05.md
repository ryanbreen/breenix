# `BREENIX_GATE_TMP` for the aarch64 gate scripts (#825)

## The collision #825 describes

`run-aarch64-boot-test-strict.sh` and `run-aarch64-prod-profile-boot-test.sh`
each wrote their per-run serial, ext2-writable copy and (for the strict
gate) failure-preservation directory to a fixed `/tmp/breenix_aarch64_*`
path. Two runs of the same gate on one host -- two worktrees, two agents --
therefore wrote and `rm -rf`'d the same files. #825's own report reads an
observed 18/20 false red produced exactly this way: a second agent's strict
run on a different worktree was writing the same `/tmp/breenix_aarch64_strict_N`
paths while the first agent's poll loop was scoring them.

Five sibling x86 gate scripts already carried the fix for the identical
shape (#797, `docs/planning/green-program/gates/GATE-TMP-BASEDIR-2026-09-05.md`):
`BREENIX_GATE_TMP`, defaulting to `/tmp` (so an unset caller is
byte-identical to before), validated absolute. The aarch64 family, and a
few `scripts/` utilities with the same hazard, did not.

## What changed

Both scripts #825 names, plus the eleven more scripts under `docker/qemu/`
and `scripts/` a mechanical scan (below) found carrying the identical shape --
a variable's last literal assignment before an `rm -rf "$VAR"` line begins
with `/tmp/breenix`, carries no `$$` PID disambiguator, and the script
does not reference `BREENIX_GATE_TMP` anywhere -- now read `BREENIX_GATE_TMP` with
the same absolute-path guard PR #801 gave the x86 scripts:

| Script | Site(s) changed |
|---|---|
| `docker/qemu/run-aarch64-boot-test-strict.sh` | `run_single_test`'s `OUTPUT_DIR`, `report_failure`'s `failure_dir`, the closing message (3 sites) |
| `docker/qemu/run-aarch64-prod-profile-boot-test.sh` | `OUTPUT_DIR`, `cleanup`'s `failure_dir` (2 sites) |
| `docker/qemu/run-aarch64-boot-test-native.sh` | `run_single_test`'s `OUTPUT_DIR`, the closing `tail` (2 sites) |
| `docker/qemu/run-aarch64-kthread-parallel.sh` | `OUTPUT_DIR` (launch loop + wait/verdict loop, reconstructed independently, same duplication PR #801 found in the x86 twin), the closing message (3 sites) |
| `docker/qemu/run-aarch64-tty-oracle-gate.sh` | `OUTPUT_ROOT` (1 site; twin of the already-fixed `run-x86-tty-oracle-gate.sh`) |
| `docker/qemu/run-aarch64-percpu-stack-custody-gate.sh` | `OUTPUT_DIR` (1 site) |
| `docker/qemu/run-aarch64-refusal-drain-gate.sh` | `OUTPUT_DIR`, migrated from its own bespoke `BREENIX_REFUSAL_DRAIN_OUTPUT_DIR`-only default onto the shared convention, keeping that variable as a higher-priority override for any existing caller (1 site) |
| `docker/qemu/run-aarch64-full-test.sh` | `OUTPUT_DIR` (1 site) |
| `docker/qemu/run-aarch64-stability-test.sh` | `OUTPUT_DIR` (1 site) |
| `docker/qemu/run-aarch64-test-suite.sh` | `RESULTS_DIR` (1 site) |
| `docker/qemu/run-aarch64-test.sh` | `OUTPUT_DIR` (1 site; Docker-wrapped, but the `rm -rf`/`mkdir`/bind-mount runs on the host before `docker run`) |
| `docker/qemu/run-aarch64-userspace.sh` | `OUTPUT_DIR` (1 site; same Docker-wrapped shape, and shares the literal `/tmp/breenix_aarch64_1` path with `run-aarch64-test.sh` -- even two *different* scripts running at once collided) |
| `scripts/test_tracing_via_gdb.sh` | `OUTPUT_DIR`'s default, with the script's pre-existing `--out` flag kept as a higher-priority override (1 site) |

Thirteen scripts, five commits (`75505b52`, `d7cfebf3`, `79bd2c19`,
`6d937ead`, `f155ad23` on this branch).

## How the extra eleven were found

The two scripts #825 names were fixed first, by hand. A grep of
`docker/qemu/*aarch64*.sh` and `scripts/*.sh` for `/tmp/breenix` then found
roughly forty files mentioning the literal in some form -- most of them Parallels
VM tooling, one-off debug/bisect scripts, or scripts already immune (three
more: `run-aarch64-service-sequence-gate.sh`, `run-aarch64-arma609-arm.sh`
and `run-aarch64-userspace-test.sh` build their `RUN_STAMP`/`OUTPUT_DIR`
from a timestamp plus `$$`, so two concurrent runs cannot land on the same
path; `run-aarch64-interactive.sh` does not `rm -rf` its one `/tmp/breenix`
reference). A mechanical script-text scan narrowed that list to the
exact shape: the last literal `VAR="..."` assignment before each
`rm -rf "$VAR"` line, starting with `/tmp/breenix`, with no `$$` and no
`BREENIX_GATE_TMP` anywhere in the file. That scan named seven more files
by hand-reading the code first (`run-aarch64-boot-test-native.sh`,
`run-aarch64-kthread-parallel.sh`, `run-aarch64-tty-oracle-gate.sh`,
`run-aarch64-percpu-stack-custody-gate.sh`,
`run-aarch64-refusal-drain-gate.sh`, `run-aarch64-full-test.sh`,
`run-aarch64-stability-test.sh`), then, re-run against the fixed tree, four
more the hand pass had not yet reached (`run-aarch64-test-suite.sh`,
`run-aarch64-test.sh`, `run-aarch64-userspace.sh`,
`scripts/test_tracing_via_gdb.sh`). Re-run again after those four were
fixed, the scan reports zero remaining hits.
claim-lint:ok: #825, this same scan's own output is what named each of the
eleven files in the table above; the zero-hit re-run is that scan invoked
again after every file it named was fixed.

## What was found but not fixed here

- **`run-aarch64-test.sh` and `run-aarch64-userspace.sh` both end with
  `docker kill $(docker ps -q --filter ancestor=breenix-qemu-aarch64)`.**
  `docker ps --filter ancestor=` matches on the image, not the container
  id, so this kills *any* container currently running from that image, not
  only the one the current invocation started -- a second, independent
  collision hazard `BREENIX_GATE_TMP` does not address (it separates the
  two runs' files; this line still terminates the other run's process
  regardless of where its output lands). Filed as
  [#829](https://github.com/ryanbreen/breenix/issues/829); not fixed here
  because the fix is a different shape (capture and kill this invocation's
  own container id) than a base-directory variable, and both scripts are
  Docker-wrapped legacy tools superseded by the native-QEMU scripts
  CLAUDE.md's Test Scripts section documents.
- **`run-aarch64-test-suite.sh` mutates `kernel/src/main_aarch64.rs` in
  place per test binary it runs.** Two concurrent invocations would stomp
  each other's kernel-source edits regardless of where `RESULTS_DIR`
  points. `BREENIX_GATE_TMP` does not touch this; it is disclosed, not
  fixed, in this branch.
- **The R18 lesson from #797's own issue still applies unchanged here**:
  `BREENIX_GATE_TMP` separates the *paths* two concurrent runs write to.
  It does not tie a gate's PASS verdict to the kernel binary that produced
  it, so two lanes pointed at the *same* `BREENIX_GATE_TMP` value (or a
  launcher that forgets to set it) can still silently cross-score. That
  remains open work this variable was not built to close.

## The ratchet (`tests/teardown_structure.rs`)

`gate_scripts_route_per_run_output_under_breenix_gate_tmp` extends the
file's existing `gate_and_utility_shell_scripts()` census (the same helper
`gate_scripts_with_verdict_trap_have_no_preempting_exits` already walks).
For each script under `docker/qemu/` and `scripts/`, `fixed_tmp_rm_rf_violation`
walks the script line by line, tracking each literal `VAR="..."`
assignment (an optional leading `local ` stripped) and checking each
`rm -rf "$VAR"` line against the most recent one -- not the script's last
assignment of that name overall, so a value reassigned *after* the
`rm -rf` (several of these scripts reassign `OUTPUT_DIR` inside a
per-iteration loop) cannot hide the value actually deleted. A hit is a
value starting with `/tmp/breenix`, carrying no `$$`, in a script that does
not mention `BREENIX_GATE_TMP` anywhere in its text — a script that has adopted
the convention anywhere is not, by this rule's own definition, missing it.
An anti-vacuity floor separately pins the count of scripts that already
carry `BREENIX_GATE_TMP` at 22 (measured on this branch: 9 from #797's
original PR, 13 from this one).

`fixed_tmp_rm_rf_violation_rule_is_not_vacuous` proves the predicate on
four legs: it names the fixed path in reconstructed pre-fix text; it
clears the fixed shape; it clears the `$$`-disambiguated shape the three
untouched sibling scripts already use safely; and, the anti-vacuity
mutation, it reverts the whole added `BREENIX_GATE_TMP` block plus the
`OUTPUT_DIR` line in the *real* `run-aarch64-stability-test.sh` text (not
a synthetic string) and confirms the predicate names the same variable and
path a fresh read of the file would show.

### Mutation record

```
cmd:  cargo test --test teardown_structure -- gate_scripts_route_per_run_output_under_breenix_gate_tmp
      (run with docker/qemu/run-aarch64-stability-test.sh's added
       BREENIX_GATE_TMP block + absolute-path guard, and its OUTPUT_DIR
       line's $BREENIX_GATE_TMP reference, reverted to the pre-fix text)
exit: 1 (FAILED)
assertion: "only 21 script(s) under docker/qemu/ or scripts/ carry
            BREENIX_GATE_TMP support; expected at least 22"
```

The mutation reddens on the anti-vacuity floor assertion (22 -> 21) rather
than the violation-list assertion further down, since the floor check runs
first in the test body and Rust's `assert!` stops at the first failure.
The violation-list path -- the one that would name the exact offending
`(variable, literal path)` pair -- is what
`fixed_tmp_rm_rf_violation_rule_is_not_vacuous`'s own final leg exercises
directly, on the same mutated text, asserting the predicate returns
`Some(("OUTPUT_DIR", "/tmp/breenix_aarch64_stability"))`. The mutation was
applied to a scratch copy and the real file was restored (`git status`
byte-clean) before this doc was written; the mutation is captured here as
a record, not a change this branch carries.

## Boot runs (2026-09-05)

Kernel: built at this branch's head (`f155ad23`) with
`cargo build --release --features boot_tests --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`,
passing `scripts/check-kernel-no-neon.sh` (0 FP/SIMD instructions). QEMU
concurrency was 0 before each launch below.

### (a) Two strict-gate runs, launched 5s apart, different `BREENIX_GATE_TMP`

```
BREENIX_GATE_TMP=<scratch>/gt825-tmp-a ./docker/qemu/run-aarch64-boot-test-strict.sh 1   # launched first
BREENIX_GATE_TMP=<scratch>/gt825-tmp-b ./docker/qemu/run-aarch64-boot-test-strict.sh 1   # launched 5s later
```

Run A: `PASS: 1/1 boots succeeded`, duration 13s.
Run B: `PASS: 1/1 boots succeeded`, duration 10s.

Both trees, listed after both runs exited:

```
=== tree a (gt825-tmp-a) ===
breenix_aarch64_strict_1/serial.txt            44874 bytes
breenix_aarch64_strict_1/ext2-writable.img  268435456 bytes

=== tree b (gt825-tmp-b) ===
breenix_aarch64_strict_1/serial.txt            42523 bytes
breenix_aarch64_strict_1/ext2-writable.img  268435456 bytes
```

Each `serial.txt` is a distinct size carrying that run's own boot content
(tree A's tail shows the scheduler-strand and TTBR0-ASID census lines from
its own boot; tree B's tail shows a different in-flight `CLONEVM_EXEC_TEST`
sequence) — two independent captures, not one run's file read back twice.
Neither tree contains any file from the other's base directory.

claim-lint:ok: #825, the two byte counts and two tail excerpts above are
this proof's own artifact; no separate file was preserved beyond the
scratch directories the runs wrote to directly.

### (b) One prod-profile run, default `BREENIX_GATE_TMP` (unset)

```
./docker/qemu/run-aarch64-prod-profile-boot-test.sh
```

`PASS: production profile reached bsshd with the futex oracle seam absent`,
exit 0. Output landed at the pre-existing `/tmp/breenix_aarch64_prod_profile/serial.txt`
(22329 bytes) — the same path an unset `BREENIX_GATE_TMP` resolved to
before this branch, confirmed by reading the file at that literal path
after the run, not asserted by eye.

## Structural suites and claim-lint

```
cargo test --test <name>   for each of the 30 tests/*_structure.rs files
-> 30/30 green, 0 failed (teardown_structure.rs: 88 passed, including the
   two new tests above)

python3 scripts/test_claim_lint.py
-> exit 0

claim-lint: scripts/claim-lint.py -> exit 0
claim-lint: scripts/claim-lint.py --files docs/planning/green-program/gates/GATE-TMP-BASEDIR-AARCH64-2026-09-05.md -> exit 0
claim-lint: scripts/claim-lint.py --commit-msg <msg> -> exit 0   (x6, one per commit)
```

## What is NOT claimed

- The two run trees in the "Boot runs" section above show file separation on
  1/1 boots each (run A, run B), not a 20-iteration merge-gate soak.
  claim-lint:ok: #825, the byte counts and tail excerpts named there are
  this same doc's own artifact.
- The 5-second launch gap approximates the collision window #825's report
  describes; the exact instant both QEMU processes were simultaneously
  alive was not independently timestamped -- run A completed in 13s and
  run B launched at t=5s, so the two processes' live windows overlapped for
  some fraction of that gap, not a measured one. What the file listing
  above records is that the two runs' files stayed apart, which is the
  property `BREENIX_GATE_TMP` is built to provide regardless of the
  processes' overlap.
  claim-lint:ok: #825, scope statement -- process-overlap timing was not
  separately measured; the file-separation result is the listing above.
- The `docker kill` hazard in `run-aarch64-test.sh` / `run-aarch64-userspace.sh`
  and the kernel-source mutation in `run-aarch64-test-suite.sh` are
  disclosed above, not fixed by this branch.
- The R18 identity-check gap (a PASS verdict is not tied to the kernel hash
  that produced it) is unchanged by this branch, exactly as #797's own doc
  disclosed for the x86 fix.
- This branch does not add the lock #826 is expected to add; "launch only
  when the concurrent-QEMU count is <= 2" was this round's own operating
  discipline, not a mechanism this branch ships.
