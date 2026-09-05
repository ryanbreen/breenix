# Gate verdict discipline, widened — 2026-09-05

Branch `gates/verdict-discipline-widened`, off `origin/main` at `9b3dd4af`.
Repair commit `5a5c1ce4`. This doc's own commit follows it.

## Background

`docker/qemu/run-x86-prod-profile-boot-test.sh` carries a specific
architecture (this arc touches no `kernel/src/...` file): `set -euo
pipefail` + `set -E` (errtrace), a `report_gate_failure()` function armed on
the `ERR` trap, and a rule — `tests/teardown_structure.rs`'s
`x86_production_profile_gate_verdict_discipline_holds`, backed by
`validate_x86_prod_profile_harness` — that no `exit` statement may appear
anywhere in the script other than the trap's own re-raise
(`exit "$exit_code"`). #801 added two `BREENIX_GATE_TMP` preflight checks to
that script with a bare `exit 1`, which reached before the trap existed and
could end the gate with no verdict line at all — #802. #805 fixed it: the
two checks moved into a `BASE-DIR PREFLIGHT` block immediately after the ERR
trap installs, rejecting with `echo` + bare `false` instead of `exit 1`, so a
rejection is spent through `report_gate_failure`.

#805's PR body disclosed its own scope: #797 put the same `BREENIX_GATE_TMP`
absolute-path guard into eight gate scripts and the AF_UNIX `sun_path` guard
into two, and #805 converted only the two in
`run-x86-prod-profile-boot-test.sh`. "Whether each sibling's preflight should
print its own gate's FAIL line is a judgement about that gate's verdict
model, not a red this branch is carrying." This doc is that judgement, made
uniformly, and the repair that follows from it.

## Census

Grepped `docker/qemu/` and `scripts/` for the verdict-line shapes named in
the task: `"PASS:`, `"FAIL:`, `PASS (`, `FAIL (`, `gate: (PASS|FAIL)`,
`gate run`, `verdict`, and `BREENIX_GATE_TMP`. 36 `.sh` files matched. For
each, the question that matters is not "does it print PASS/FAIL text" —
most of them do, in ad hoc ways — but "does it carry the specific
`report_gate_failure`-via-`ERR`-trap backstop architecture that makes the
#802 shape possible in the first place": `set -e`/`set -E`, a function named
`report_gate_failure`, and a `trap ... ERR` line that arms it. That
architecture is what makes a bare `exit` dangerous (it silently escapes the
trap — verified below) and what makes a uniform "no exit but the trap's own
re-raise" rule meaningful. A script without that architecture has a
different verdict model (an ad hoc echo-then-exit sequence, or an
exit-code-only contract) and forcing the #805 idiom onto it would not be a
parallel fix -- it would be inventing an architecture that script did not have.

Grepping the 36 candidates for `report_gate_failure` + an `ERR` trap that
arms it found exactly **7**, 7 of 7 under `docker/qemu/`, 0 of 7 under
`scripts/`:

```
docker/qemu/run-aarch64-tty-oracle-gate.sh
docker/qemu/run-coreproof-gate.sh
docker/qemu/run-ext2-lock-race-gate.sh
docker/qemu/run-fs-fault-gate.sh
docker/qemu/run-x86-boot-tests.sh
docker/qemu/run-x86-prod-profile-boot-test.sh
docker/qemu/run-x86-tty-oracle-gate.sh
```

The remaining 29 of 36 each use `trap cleanup EXIT` or
`trap "rm -rf $DIR" EXIT` (resource cleanup on any exit, not a verdict
backstop) or carry no trap at all.

### Verified: a bare `exit` does not reach an `ERR` trap

The whole premise of #802/#805 rests on one bash fact, checked directly
rather than assumed:

```bash
set -eE
trap 'echo TRAP_FIRED status=$?' ERR
exit 1
```

produces no output and exits 1 — the trap does not fire. A `return N`
inside a function called as a plain statement, by contrast, **does** fire
the trap with `$?` equal to `N`, checked the same direct way:

```bash
set -Eeuo pipefail
trap 'echo TRAP_FIRED status=$?' ERR
inconclusive() { return 2; }
echo before
inconclusive
echo should_not_print
```

prints `before` then `TRAP_FIRED status=2` and stops — `should_not_print`
does not run, and the outer shell's own exit status (checked with `$?`
right after) is `2`, not `1`. That second fact is what makes the `redden()`
helper below possible: it lets a script reach the trap with a specific,
non-1 exit code without writing a bare `exit` statement.

## Classification

| script | has ERR-trap/verdict-function path? | pre-empting exits before repair? | class |
|---|---|---|---|
| `run-x86-prod-profile-boot-test.sh` | yes | no (fixed by #805) | has-verdict-path-and-clean |
| `run-x86-boot-tests.sh` | yes | yes — 1 preflight (`BREENIX_GATE_TMP`, before the trap check would matter) + 5 in-loop `exit 1` | has-verdict-path-but-preempting-exits |
| `run-x86-tty-oracle-gate.sh` | yes | yes — 1 preflight (`BREENIX_GATE_TMP`, **before its trap was even installed**) + 3 arg-parsing exits + 1 `sun_path` check + 1 missing-ext2-disk check + 10 in-loop exits (16 sites) | has-verdict-path-but-preempting-exits |
| `run-aarch64-tty-oracle-gate.sh` | yes | yes — 3 arg-parsing exits + 14 in-body/in-loop exits (17 sites); no `BREENIX_GATE_TMP` support to begin with | has-verdict-path-but-preempting-exits |
| `run-fs-fault-gate.sh` | yes | yes — 1 preflight (**before its trap was even installed**) + 2 arg-parsing (`exit 2`) + 1 build-warning check + 1 `fail()` helper + 2 `exit 0` success paths (7 sites) | has-verdict-path-but-preempting-exits |
| `run-ext2-lock-race-gate.sh` | yes | yes — 1 preflight (**before its trap was even installed**) + 1 arg-parsing (`exit 64`, F10's own distinct code) + 1 build-warning + 1 `fail()` helper + 5-branch `--park-only` cascade (0/1/2 distinct codes) + 1 final `exit 0` (10 sites) | has-verdict-path-but-preempting-exits |
| `run-coreproof-gate.sh` | yes | yes — 8 of 8 argument/validation `exit 2` sites reached before its trap was installed (the trap installed ~90 lines after the last of them) + 1 `exit 1` + 1 final `exit 0` (10 sites) | has-verdict-path-but-preempting-exits |

The other 29 of the 36 are **no-verdict-path**: they
print PASS/FAIL-shaped text (or a bisect/probe-shaped verdict) through their
own ad hoc means, with no `report_gate_failure`/`ERR`-trap backstop behind
it, so the #802/#805 idiom does not apply to them, and 0 of 29 were
touched. Two recognizable sub-shapes, listed rather than each individually
audited (per the task's own instruction: a script that reports only via
exit code is a different shape):

- **Ad hoc echo-then-exit, no trap backstop** (22 scripts): `run-aarch64-boot-test-native.sh`,
  `run-aarch64-boot-test-strict.sh`, `run-aarch64-full-test.sh`,
  `run-aarch64-kthread-parallel.sh`, `run-aarch64-percpu-stack-custody-gate.sh`,
  `run-aarch64-prod-profile-boot-test.sh` (the **aarch64** production-profile
  gate — a different script from the x86 one this whole arc is about),
  `run-aarch64-refusal-drain-gate.sh`, `run-aarch64-service-sequence-gate.sh`,
  `run-aarch64-stability-test.sh`, `run-aarch64-test-suite.sh`,
  `run-aarch64-testing-profile-boot-test.sh`, `run-boot-parallel.sh`,
  `run-kthread-parallel.sh`, `run-nonblock-eagain-test.sh`, `run-vmware-gate.sh`,
  `run-x86-gate.sh`, `scripts/772-diag-run-arm.sh`, `scripts/check-fs-fault-seams.sh`,
  `scripts/parallels/boot-cycle-test.sh`, `scripts/parallels/launcher-smoke.sh`,
  `scripts/run-arm64-boot-test.sh`, `scripts/test_tracing_via_gdb.sh`,
  `scripts/x86-gate-verdict.sh`. Each of these has its own retry loop,
  explicit per-check echo+return/exit, or classify-at-the-end shape (e.g.
  `run-aarch64-boot-test-native.sh`'s `run_single_test()` returns 1 on
  failure to a retry loop that itself decides `exit 0`/`exit 1` directly,
  with no trap involved). Retrofitting a `report_gate_failure` architecture
  onto any of these is a separate, larger change with its own review, not a
  #802-shaped fix.
- **Exit-code-primary, own text is diagnostic context rather than a
  PASS/FAIL banner** (7 scripts): `run-aarch64-arma609-arm.sh` (a
  classify-and-return helper feeding a caller's own exit code),
  `scripts/772-diag-aggregate.sh` (a stats aggregator, not a verdict),
  `scripts/772-dispatch-boot.sh` (`RESULT tag=... verdict_rc=$VERDICT_RC`,
  forwarding another script's exit code), `scripts/f21-bisect-verdict.sh`
  (git-bisect's own 0/1/125 convention), `scripts/f23-render-verdict.sh` and
  `scripts/f24-render-verdict.sh` (`Returns 0 if rendered content is
  present, 1 otherwise` — an embedded Python image check, no bash PASS/FAIL
  text), `scripts/x86-strand-census.sh` (5 distinct exit classes, own
  precedence rules, consumed by `run-boot-parallel.sh`).

## Repair

Applied the #805 idiom to each of the six `has-verdict-path-but-preempting-exits`
scripts. Two shapes:

**Where the trap was already installed before the pre-empting exit** (the
in-loop `exit 1`s in `run-x86-boot-tests.sh`, the arg-parsing/in-loop exits
in `run-x86-tty-oracle-gate.sh` and `run-aarch64-tty-oracle-gate.sh`, the
`fail()` helpers in `run-fs-fault-gate.sh`/`run-ext2-lock-race-gate.sh`):
the bare `exit N` becomes a bare `false` in place. `set -e` fires the
already-armed trap; `report_gate_failure`'s own `exit "$exit_code"` re-raise
is the one `exit` statement left.

**Where the pre-empting exit ran before the trap existed** (the
`BREENIX_GATE_TMP` preflights in `run-x86-tty-oracle-gate.sh`,
`run-fs-fault-gate.sh`, `run-ext2-lock-race-gate.sh`; the entire
argument-parsing/validation block in `run-coreproof-gate.sh`): the trap
installation (and, for `run-coreproof-gate.sh`, the `cleanup()` `EXIT` trap
and a new `redden()` helper) moved earlier in the script, to immediately
after the variables `report_gate_failure` itself reads (`QEMU_PID`,
`CURRENT_SERIAL`/`OUTPUT_DIR`, already initialized). The check itself then
runs as the first thing under the armed trap — a `BASE-DIR PREFLIGHT` block
matching #805's own naming — rejecting with `echo` + `false`.

**Distinct exit codes** (`run-ext2-lock-race-gate.sh`'s `--park-only` mode:
0 PARK OBSERVED, 1 SPIN/NO PARK, 2 INCONCLUSIVE, review finding F10;
`run-coreproof-gate.sh`'s `exit 2` usage errors) needed a third shape. A
bare `false` yields a fixed status 1, which would collapse F10's three
outcomes onto one code. `redden()` is the fix:

```bash
redden() {
    return "$1"
}
```

Called as `redden 2` (a plain statement, not part of an `if`/`while`
condition or an `&&`/`||` chain), this fires the `ERR` trap with `$?` equal
to `2` — verified above — and `report_gate_failure`'s `exit "$exit_code"`
re-raise carries that code through. No `exit` statement appears at the
call site. `run-ext2-lock-race-gate.sh`'s five-branch `--park-only` cascade
converted from five independent `if` blocks each ending in its own `exit`
to one `if`/`elif` chain: the two FAIL-shaped branches (`SPIN, NO PARK`,
`INCONCLUSIVE`) call `redden 1`/`redden 2`; the success branch (`PARK
OBSERVED`, code 0) takes no exit-shaped action at all and the cascade falls
through — success needs no trap trip, only a normal return to the caller.
The ordinary (non-`--park-only`) verdict logic that used to run
unconditionally after the `PARK_ONLY` block moved into that same `if`'s
`else`, so it stays unreachable in `--park-only` mode -- the same effect
the `PARK_ONLY` block's own five `exit`s had.

`run-coreproof-gate.sh`'s eight validation sites (`unknown argument`,
`unknown component`, `unknown mode`, `unknown window`, `unknown profile`,
and three `<invalid|unknown> ... coverage name` sites) each call
`redden 2`, preserving the exit code the script used for a usage error
before this repair. This
script has no separate `fail()` helper; its one runtime-verdict `false` is
the `FAILED_BOOTS -ne 0` check after the boot loop, and its PASS path
(`echo "ARM64 CORE-PROOF GATE: PASSED"`) now falls off the end the same way
`run-x86-prod-profile-boot-test.sh`'s does, replacing a trailing `exit 0`.

`run-fs-fault-gate.sh`'s two `exit 0` success paths (armed vs. `--disarm`)
merged into one `if`/`else` so both converge without an early exit, matching
`run-x86-prod-profile-boot-test.sh`'s own PASS path — that script has
carried no `exit 0` at any point in this arc; its PASS message is the last thing the script
prints, then it falls off the end with the last command's (an `echo`, status
0) exit code.

## The widened ratchet

`tests/teardown_structure.rs` gains a generalized version of
`validate_x86_prod_profile_harness`'s own no-pre-empting-exit scan, derived
by content rather than a hardcoded file list:

- `shell_scripts_below()`/`gate_and_utility_shell_scripts()`: walks
  `docker/qemu/` and `scripts/` for each `.sh` file.
- `has_report_gate_failure_verdict_trap(script)`: true when the script both
  defines `report_gate_failure() {` and arms it (`trap ... ERR` containing
  that name).
- `report_gate_failure_status_variable(script)`: reads the function's first
  statement (`local <name>=$?`) to find the variable name each script's own
  re-raise uses — 6 of the 7 use `exit_code`, `run-coreproof-gate.sh` uses
  `status` — rather than hardcoding one name.
- `verdict_trap_has_no_preempting_exit(script)`: the rule itself, a direct
  generalization of the original scan — no line may open with the token
  `exit` except the exact re-raise of that script's own status variable. A
  script whose handler does not open with `local <var>=$?` is itself a
  violation, not a silent pass.

Two new tests:

- `gate_scripts_with_verdict_trap_have_no_preempting_exits` — the ratchet:
  census the two directories, filter to the verdict-trap shape, assert the
  no-pre-empting-exit rule on each. Carries a `>= 7` anti-vacuity floor on
  the census size (measured at exactly 7 today; free to grow, per the
  campaign's own [[gate-target-fidelity-528]]-style precedent against
  pinning a closed name list — #549/#551/#527-r1).
- `verdict_trap_no_preempting_exit_rule_is_not_vacuous` — mutation proof:
  the census finds `run-x86-tty-oracle-gate.sh`'s shape; on a scratch copy,
  planting a standalone `exit 1` preflight (mirroring #802's own shape —
  `if [ -z "$BREENIX_GATE_TMP" ]; then` / `exit 1` on its own line / `fi`)
  reddens `verdict_trap_has_no_preempting_exit` and the failure names the
  planted `exit 1` text; a second mutation deletes the `local exit_code=$?`
  opener and confirms that also reddens rather than silently passing.

### Anti-vacuity mutation, run directly

```
$ scripts/run-structure-tests.sh teardown_structure verdict_trap_no_preempting_exit_rule_is_not_vacuous
test verdict_trap_no_preempting_exit_rule_is_not_vacuous ... ok
```

(The mutation's own assertion — that the reddened result names `exit 1` —
is inside the test; a plain `ok` here means both the "must apply" and the
"must redden by name" checks passed. First attempt at writing this
mutation used `if [ -z "$BREENIX_GATE_TMP" ]; then exit 1; fi` **on one
line**, which the scan does not see — first-token-of-line is `if`, not
`exit`, the same blind spot #805's own PR body named for the case-arm form
of the original bug. Corrected to a standalone `exit 1` line before this
doc was written; disclosed here rather than silently fixed, since it is
direct evidence of the scanner's real (and pre-existing, inherited) limit.)

## Structure suites

```
$ scripts/run-structure-tests.sh teardown_structure
test result: ok. 85 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in ~16s
```

(83 before this round; the two new tests above are the delta.)

Full `tests/*_structure.rs` sweep, run individually through
`scripts/run-structure-tests.sh` (the workaround `scripts/run-structure-tests.sh`
itself documents: `cargo test -p breenix --test teardown_structure` cannot
reach these tests in a worktree lacking the forked Rust library the root
crate's build script needs — a build dependency failure unrelated to what
these tests read):

| suite | result |
|---|---|
| aarch64_testing_profile_structure | ok, 2 passed |
| block_request_lifetime_structure | ok, 12 passed |
| context_restore_structure | ok, 97 passed |
| coreproof_component_h_structure | ok, 5 passed |
| coreproof_coverage_structure | ok, 4 passed |
| coreproof_mutation_register_structure | ok, 5 passed |
| coreproof_sites_structure | ok, 4 passed |
| degenerate_transfer_fd_validation_structure | ok, 4 passed |
| dispatch_path_lock_free_structure | ok, 4 passed |
| dispatch_strand_census_structure | ok, 7 passed |
| dma_and_log_sink_structure | ok, 4 passed |
| entry_point_df_structure | ok, 5 passed |
| exec_lock_order_structure | ok, 44 passed |
| exit_tally_structure | ok, 6 passed |
| ext2_lock_structure | ok, 36 passed |
| fork_lock_order_structure | ok, 10 passed |
| green_program_envelope_structure | ok, 14 passed |
| loopback_pump_structure | ok, 72 passed |
| masked_binary_load_structure | ok, 4 passed |
| mmap_floor_structure | ok, 9 passed |
| net_lock_structure | ok, 19 passed |
| poll_tcp_gate_wiring_structure | ok, 3 passed |
| preempt_bracket_structure | ok, 8 passed |
| serial_line_atomicity_structure | ok, 9 passed |
| signal_eintr_predicate_structure | ok, 2 passed |
| strand_handoff_structure | ok, 38 passed |
| syscall_return_register_structure | ok, 6 passed |
| teardown_structure | ok, 85 passed |
| ttbr0_shadow_reconciliation_structure | ok, 32 passed |
| tty_oracle_structure | ok, 14 passed |

**30 of 30 suites green, 564 cases, 0 failed.**

## Run proofs

Each of the 6 repaired scripts gets one default-env run and one simulated
preflight failure. aarch64 gates ran on this Mac; x86 gates ran on beast
(`breenix-x86` Incus container, clone `/root/breenix-verdict` at `5a5c1ce4`,
`BREENIX_GATE_TMP=/root/breenix-verdict-tmp`). Raw output for each of the 12
runs (6 scripts x 2 runs) is saved as its own file under
`docs/planning/green-program/gates/serials/verdict-widened-2026-09-05/`,
named in each run's own "Full output:" line below.

### Environment note

This git worktree has no `rust-fork/` checkout and no prebuilt userspace
artifacts (both gitignored, neither created by a fresh `git worktree add`),
so 4 of the 4 aarch64 gates below (each needing either `boot_tests`'s
embedded userspace ELFs, `include_bytes!` in the kernel's test registry, or
a populated ext2 disk) hit `ERROR: forked Rust library not found` trying to
build userspace from source inside this worktree -- the same failure that
blocks `cargo test` directly, per `scripts/run-structure-tests.sh`'s own
header comment. Read-only artifacts already built from the same tree
(`target/ext2-aarch64.img`, `userspace/programs/aarch64/*.elf`) were copied
in from the main checkout (`/Users/wrb/fun/code/breenix`, `.gitignore`d
build products, not repository content) rather than rebuilt, so these are
genuine boot-and-adjudicate runs on the repaired scripts, exercising the
repaired shell logic end to end, but the userspace/kernel binaries booted
were not built by this run (no userspace or kernel source changed on this
branch, so this substitution does not affect what the gates verify).
Beast's existing reference clone (`/root/breenix/rust-fork`)
has the fork checked out, so both x86 gates on beast built the kernel,
userspace, and ext2 disk from source in `/root/breenix-verdict` itself, no
substitution needed there.

### aarch64 (this Mac)

Host-load rule (<=2 concurrent QEMUs) checked via `pgrep -f '^qemu-system'`
before each boot; each of the 4 boots below ran with 0 or 1 other QEMU
already running (other worktrees' own gates), for a total of 1 or 2 --
confirmed via the same `pgrep` count each time, not assumed.

**`run-aarch64-tty-oracle-gate.sh`** (default: `--boots 1`)

```
$ docker/qemu/run-aarch64-tty-oracle-gate.sh
...
Booting the ARM64 production profile (boot 1/1)...
  boot 1: 14/14 arms PASS, kernel live (bsshd reached)
PASS: aarch64 TTY oracle gate - 1/1 boots, 14 arms green on the shipped production profile
```
exit 0. Full output: `docs/planning/green-program/gates/serials/verdict-widened-2026-09-05/aarch64-tty-oracle-default-pass.txt`.

Simulated preflight failure (unknown argument — this script has no
`BREENIX_GATE_TMP` support to begin with, per the census above, so its own
argument-parsing preflight is the analogous check):

```
$ docker/qemu/run-aarch64-tty-oracle-gate.sh --nonsense-flag
FAIL: unknown argument: --nonsense-flag
aarch64 TTY oracle gate: FAIL (set -e abort at docker/qemu/run-aarch64-tty-oracle-gate.sh:101, exit 1)
  failing command: false
```
exit 1. Full output: `docs/planning/green-program/gates/serials/verdict-widened-2026-09-05/aarch64-tty-oracle-preflight-fail.txt`.

**`run-fs-fault-gate.sh`** (default: aarch64, armed, single boot)

```
$ docker/qemu/run-fs-fault-gate.sh
...
[gate] [FSFAULT:aarch64:COMPLETE:pass=8:fail=0]
  baseline_mount / baseline_read / short_read / eio_data_block / eio_recovery /
  corrupt_inode:arm=size / corrupt_inode:arm=blocks / liveness -- all PASS
fs fault gate (aarch64): PASSED - 8 arms green, kernel live after every injected fault
```
exit 0. Full output: `docs/planning/green-program/gates/serials/verdict-widened-2026-09-05/fs-fault-default-pass.txt`.

Simulated preflight failure (relative `BREENIX_GATE_TMP` — the check moved
ahead of the trap by this round's repair):

```
$ BREENIX_GATE_TMP=relative-not-absolute docker/qemu/run-fs-fault-gate.sh
fs fault gate preflight: BREENIX_GATE_TMP must be an absolute path, got: relative-not-absolute
fs fault gate: FAIL (set -e abort at docker/qemu/run-fs-fault-gate.sh:93, exit 1)
  failing command: false
```
exit 1. Full output: `docs/planning/green-program/gates/serials/verdict-widened-2026-09-05/fs-fault-preflight-fail.txt`.

**`run-ext2-lock-race-gate.sh`** (default: aarch64, single race construction)

```
$ docker/qemu/run-ext2-lock-race-gate.sh
...
[gate] [LOCKRACE:COMPLETE:pass=2:fail=0]
  [LOCKRACE:ROOT:race:verdict=PASS:detail=no-spin-stall:parks=67]
  [LOCKRACE:HOME:race:verdict=PASS:detail=no-spin-stall:parks=63]
[gate] total parks observed: 130
ext2 lock-race gate (aarch64): PASSED - 2 filesystem(s) raced clean (130 total parks), kernel live after
```
exit 0. Full output: `docs/planning/green-program/gates/serials/verdict-widened-2026-09-05/ext2-lock-default-pass.txt`.

Simulated preflight failure (relative `BREENIX_GATE_TMP` — same repair
shape):

```
$ BREENIX_GATE_TMP=relative-not-absolute docker/qemu/run-ext2-lock-race-gate.sh
ext2 lock-race gate preflight: BREENIX_GATE_TMP must be an absolute path, got: relative-not-absolute
ext2 lock-race gate: FAIL (set -e abort at docker/qemu/run-ext2-lock-race-gate.sh:263, exit 1)
  failing command: false
```
exit 1. Full output: `docs/planning/green-program/gates/serials/verdict-widened-2026-09-05/ext2-lock-preflight-fail.txt`.

**`run-coreproof-gate.sh`** (`--seeds 1 --profile max`, to keep the run to
one boot instead of the 2026-08-18 gate-size default of 25/profile x2
profiles)

```
$ docker/qemu/run-coreproof-gate.sh --seeds 1 --profile max
...
=== profile max — 1 boot(s), component A, mode pen, window post_cohort, disarmed 0 ===
  max#1: harness violations=0 iters=54884
  max#1: clean (seed=0x000000000c489b88 iters=54884 sites=12/12)
boots=1 failed=0 violations=0 iters_total=54884 ...
ARM64 CORE-PROOF GATE: PASSED
```
exit 0. Full output: `docs/planning/green-program/gates/serials/verdict-widened-2026-09-05/coreproof-default-pass.txt`.

Simulated preflight failure (`--component Z` — this script has no
`BREENIX_GATE_TMP` support, so its own argument-validation preflight, now
routed through the `redden()` helper the repair added, is the analogous
check; also directly proves `redden N` preserves a non-1 code):

```
$ docker/qemu/run-coreproof-gate.sh --component Z
unknown component: Z

ARM64 CORE-PROOF GATE: FAILED
  at line 136: return "$1" (exit 2)
  started=... ended=...
```
exit **2** (not 1 — `redden 2` preserving the script's own usage-error code
through the trap, exactly as designed). Full output:
`docs/planning/green-program/gates/serials/verdict-widened-2026-09-05/coreproof-preflight-fail.txt`.

### x86 (beast, `breenix-x86` container, clone `/root/breenix-verdict`)

`pgrep -f qemu-system-x86_64` checked <= 2 before each boot (0 running both
times).

**`run-x86-boot-tests.sh`** (`COUNT` defaults to 1)

```
$ BREENIX_GATE_TMP=/root/breenix-verdict-tmp BREENIX_RUST_FORK_LIBRARY=/root/breenix/rust-fork/library ./docker/qemu/run-x86-boot-tests.sh
...
[RECLAIM_DRAIN:nested=1:context_violations=0:selection_capped=3:injected=1:pend_epoch=0:pend_hw=0:pend_shadow=1:pend_selectable=0]
x86 frame-custody gate run 1: PASS
```
The script's own last statement is that `echo`, so it falls off the end with
status 0 — the same PASS-path shape `run-x86-prod-profile-boot-test.sh`
already uses, no `exit 0` needed. `pgrep -fl run-x86-boot-tests.sh` after
that line printed found 0 matches -- the process had exited.
`BREENIX_RUST_FORK_LIBRARY` points at beast's own long-lived reference
clone's fork checkout (`/root/breenix/rust-fork`, a read-only reference:
that clone was read from, and only from); the kernel, userspace, and ext2
disk this run booted were built fresh, from source, inside
`/root/breenix-verdict`. Full output (470 lines, covering each
FRAME_CUSTODY/PT_CUSTODY/oracle line the gate asserts on):
`docs/planning/green-program/gates/serials/verdict-widened-2026-09-05/x86-boot-tests-default-pass.txt`.

**`run-x86-tty-oracle-gate.sh`** (default: `--boots 1`)

```
$ BREENIX_GATE_TMP=/root/breenix-verdict-tmp BREENIX_RUST_FORK_LIBRARY=/root/breenix/rust-fork/library ./docker/qemu/run-x86-tty-oracle-gate.sh
Building the shipped x86_64 production kernel profile...
     Running `target/release/qemu-uefi`
Booting the x86_64 production profile with the TTY oracle (boot 1/1)...
  boot 1: 14/14 arms PASS, kernel live (bsshd reached)
PASS: x86 TTY oracle gate - 1/1 boots, 14 arms green on the shipped production profile
```
exit 0. No `--rebuild-userspace` was needed — the fresh clone's `cargo
build` step (via the kernel's own build script) produced the ext2 image the
gate needs as a side effect, so the run used fresh build output from
`/root/breenix-verdict`, with only the fork library itself read from
beast's reference clone. Full output:
`docs/planning/green-program/gates/serials/verdict-widened-2026-09-05/x86-tty-oracle-default-pass.txt`.

Simulated preflight failures (both run before the default-env boots below
succeeded, so the trap-routed rejection was confirmed independent of
whether a real boot would later succeed):

```
$ env BREENIX_GATE_TMP=relative-not-absolute ./docker/qemu/run-x86-boot-tests.sh
x86 frame-custody gate: FAIL (set -e abort at .../run-x86-boot-tests.sh:93, exit 1)
  failing command: false
x86 frame-custody gate preflight: BREENIX_GATE_TMP must be an absolute path, got: relative-not-absolute
```
exit 1. Full output: `docs/planning/green-program/gates/serials/verdict-widened-2026-09-05/x86-boot-tests-preflight-fail.txt`.

```
$ env BREENIX_GATE_TMP=/root/breenix-verdict-tmp-ggg...ggg(147 chars) ./docker/qemu/run-x86-tty-oracle-gate.sh
FAIL: console socket path exceeds the AF_UNIX sun_path limit of 107 chars: "..." is 144 chars -- shorten BREENIX_GATE_TMP
x86 TTY oracle gate: FAIL (set -e abort at .../run-x86-tty-oracle-gate.sh:180, exit 1)
  failing command: false
```
exit 1. Full output: `docs/planning/green-program/gates/serials/verdict-widened-2026-09-05/x86-tty-oracle-preflight-fail.txt`.

## What is NOT claimed

- **This is not a rebuild demonstration for userspace or kernel source.**
  0 files under `kernel/`, `userspace/`, or `libs/` changed on this branch.
  The 4 aarch64 default-env runs above used a prebuilt ext2 disk and
  userspace ELF set copied in from the main checkout rather than rebuilt in
  this worktree (see "Environment note"); the 2 x86 runs on beast built the
  kernel, userspace, and ext2 disk from source.
- **Not a claim that the 29 no-verdict-path scripts are correct, complete,
  or free of their own defects.** They were read only far enough to
  classify them out of scope for this specific repair; 0 of the 29 were
  modified.
- **Not a claim that `redden()`'s exit-code preservation is exercised by
  the widened Rust ratchet.** `verdict_trap_has_no_preempting_exit` checks
  the absence of a pre-empting `exit` statement; it does not itself execute
  any script or assert what code a `redden N` call actually produces at
  runtime. That is checked separately, at runtime, by the
  `run-coreproof-gate.sh --component Z` simulated-preflight run above
  (`exit 2`, not the `false`-only fallback of `exit 1`) and by the small
  bash reproduction in the "Verified" section above, which shows the
  `redden N` pattern and its observed `TRAP status=2` output directly
  (not committed as a script; the snippet in that section is the
  reproduction).
- **The structure-test scanner's own known blind spot is inherited, not
  fixed.** `verdict_trap_has_no_preempting_exit` — like the original
  `validate_x86_prod_profile_harness` it generalizes — only catches `exit`
  as the first whitespace token of a trimmed line. An `exit 1` embedded
  mid-line (inside a one-line `if ...; then exit 1; fi`, or after a case
  label like `*) ... exit 1 ;;`) is invisible to it, exactly as #805's own
  PR body disclosed for the original rule. This round's own anti-vacuity
  mutation hit that blind spot directly (see the note under "Anti-vacuity
  mutation, run directly") and each of the 6 repaired scripts places
  `exit`/`false`/`redden N` on its own line specifically so
  the scanner *can* see it — but the scanner does not enforce that
  placement itself. Closing that gap is a separate, larger change to the
  scan (parsing case-arm and single-line-if bodies) with its own review,
  not folded into this one.

## claim-lint

```
claim-lint: scripts/claim-lint.py                                              -> exit 0
claim-lint: scripts/claim-lint.py --commit-msg <repair commit message>          -> exit 0
```
