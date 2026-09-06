# Gate-tooling PR-1: the structure-suite preflight (#R191)

**Branch:** `gates/structure-suites-in-gates`, from `origin/main`
`138fb8395d0a` (2026-09-06). No `kernel/` code moved in this round --
`kernel/src/` is untouched by the diff below.

## The gap this round closes

PR #880's disclosure, and `docs/planning/green-program/gates/
CRITICAL-PATH-DEBT-PR0-ROUND-2026-09-06.md`'s own "What this round
deliberately did not do" section, both record the same fact: no gate script
under `docker/qemu/` invokes `scripts/run-structure-tests.sh` or
`cargo test --test <structure-suite>`, so each `tests/*_structure.rs`
ratchet -- the critical-path logging census included -- is enforced only by
a person or an agent running the suite by hand, not by a boot gate.
`scripts/run-structure-tests.sh`'s own header states this in as many words
("It does not run in any gate ... Wiring it into a gate is a separate
change with its own review"). This round is that separate, reviewed change:
a host-side preflight step, shared by the four boot gates named below, that
runs each `tests/*_structure.rs` suite and fails the gate on any red.

## Why `rustc --test` instead of `cargo test`, in a gate script

`docker/qemu/run-aarch64-boot-test-strict.sh`'s `require_boot_tests_kernel`
comment documents the hazard directly: `cargo` keeps one cached artifact per
feature set, and ANY `cargo test` invocation in the same shell session
hardlinks a fresh kernel binary -- built with none of that gate's required
`--features boot_tests` -- over the one the gate's own build step produced,
in well under a second and with no output announcing the swap. The comment
records a measured acceptance battery that ran the structural suites via
`cargo test` and then that gate, scoring 0/6, every boot failing on "marker
missing," against a production kernel that was never asked to emit the
gate's `boot_tests`-only markers.

`scripts/run-structure-tests.sh` sidesteps this by construction, not by
convention (its own header, and `docs/planning/green-program/gates/
CRITICAL-PATH-DEBT-PR0-ROUND-2026-09-06.md`'s parallel finding): it compiles
one `tests/<stem>.rs` file directly with `rustc --test`, which reads no
`Cargo.toml`, runs no build script, and writes its output binary under
`$TMPDIR` -- no file under this repository's `target/` is read or written
by that path, so there is no kernel artifact for it to touch, let alone
swap. `docker/qemu/lib/gate-structure-preflight.sh` (this round's new
shared helper) is the sole caller each of the four gates below uses, and it
calls only `scripts/run-structure-tests.sh`, not `cargo test`.

**Measured, not asserted:** the strict-gate measurement run below hashes
the `boot_tests` kernel binary immediately before the whole gate run and
again immediately after; both hashes read the same 64 hex characters
(`17c7b84b0068c1614a1f1c5a170dc7f9df097ded2fdc0ece2b80111518d6e852`). No
`cargo test`, `cargo build`, or any other kernel-producing command ran in
that shell session between the two hashes -- the preflight's `rustc --test`
calls are the only compilation step the run performs before the boot loop,
and the boot loop itself only executes the already-built kernel, it does
not rebuild it.

## What the preflight does

`docker/qemu/lib/gate-structure-preflight.sh` defines one function,
`gate_structure_preflight <repo_root> <gate_tmp>`, that each of the four
gates below sources and calls immediately after its own
`BREENIX_GATE_TMP` validation, before any kernel build or QEMU state
exists:

1. Discovers each `tests/*_structure.rs` file under the gate's repo root
   (47 today) and runs it through `scripts/run-structure-tests.sh <stem>`,
   counting green vs. total. Per-suite logs are written under a
   `breenix_gate_structure_preflight/<stem>.log` subdirectory of the
   gate's own `BREENIX_GATE_TMP`, for diagnosis on a red run.
2. Runs `bash scripts/check-critical-path-violations.sh` and counts its
   total stdout line count.
3. Reads `pinned=<n>` out of `tests/critical_path_logging_census_
   structure.rs`'s own source text -- the WIDER census's total
   (`CRITICAL_PATH_LOG_ANCHORS`' summed third field, 135, plus
   `ESCAPED_SITE`'s own count, 1 -- 136 today), parsed with `awk`/`grep`
   rather than hardcoded, so the number moves with the suite instead of
   drifting from it. The WIDER total is the comparable one: `check-
   critical-path-violations.sh`'s `PROHIBITED_PATTERNS` already carries
   the three spellings the wider census adds (both were widened together
   by the PR-0 round that added this suite).
4. Prints exactly one line:
   `[GATE_PREFLIGHT:structure_suites=<green>/<total>:critical_path_lines=<n>:pinned=<n>]`.
5. Returns 1 (does not call `exit` -- each caller decides how to fail
   loudly through its own verdict idiom) if discovery found no suite to
   run, or if any discovered suite is red. Returns 0 otherwise.

`critical_path_lines`/`pinned` are printed for a human reading gate output,
not a second enforcement path: `scripts/check-critical-path-
violations.sh` exits 1 on purpose today (135 real call sites the census
suite pins, per the drain plan) and will keep doing so until the drain
reaches 0, so treating its exit code as a gate would make each of these
four gates' runs red permanently. The actual enforcement -- a per-`(file,
item-path)` census that may not exceed its pinned count, and per that
suite's own doc comment may not fall below it either without a conscious
table update -- is `tests/critical_path_logging_census_structure.rs`'s own
job, and it is one of the suites step 1 above already runs and gates on.

`BREENIX_GATE_SKIP_STRUCTURE=1` is a loud, operator-set opt-out: it skips
both steps and prints `[GATE_PREFLIGHT:skipped=1:reason=...]`
instead of the scored line. Each of the four gate scripts documents this
variable in its own top-of-file header comment. The function also skips
loudly, with a stated reason, on its own if `scripts/run-structure-
tests.sh` is missing or `rustc` is not on `PATH` -- an automatic
environment-detection fallback, distinct from the operator opt-out.

## Where each gate calls it

| Gate | Insertion point | Fail idiom |
|------|------------------|------------|
| `run-aarch64-boot-test-strict.sh` | top of the `if [ -z "$SCORE_ONLY_SERIAL" ]; then` block, before "Find the ARM64 kernel" | `echo "GATE: FAIL (...)"; exit 1` -- this script's own existing `BREENIX_GATE_TMP` check uses the same shape; no ERR trap exists this early in this script |
| `run-aarch64-prod-profile-boot-test.sh` | top of the `if [ -z "$SCORE_ONLY_SERIAL" ]; then` block, before `trap 'cleanup $?' EXIT` (so a rejection here has no `OUTPUT_DIR`/`QEMU_PID` state for that trap to release) | `echo "FAIL: ..."; exit 1` -- matches this script's own `BREENIX_GATE_TMP` check |
| `run-x86-boot-tests.sh` | immediately after the `BREENIX_GATE_TMP` `case` block, before `FRAME_CUSTODY_PATTERN=` | `echo "...FAIL (...)" >&2; false` -- the #802/#805 idiom: the installed `ERR` trap (`report_gate_failure`) does not catch a bare `exit`, only a nonzero simple command, so `false` is what actually fires it |
| `run-x86-prod-profile-boot-test.sh` | inside the "BASE-DIR PREFLIGHT" block, right after the `CONSOLE_SOCK_PATH` length check, ahead of `cd "$BREENIX_ROOT"` | same `echo ... >&2; false` idiom, matching the two checks already in that block |

Both aarch64 gates guard the preflight (like the kernel build and boot
that follow it) on `SCORE_ONLY_SERIAL` being empty: a scoring-only replay
of an already-captured serial file needs no kernel, no disk, and -- now --
no structure-suite check either, the same reasoning `docker/qemu/lib/
gate-structure-preflight.sh`'s own header gives.

## Ratchet: `tests/gate_structure_preflight_wiring_structure.rs`

Pins that each of the four gate scripts above carries both the shared
lib's `source` line and a call to `gate_structure_preflight`, checked
independently (a gate that sources the file but does not call the
function is exactly as unprotected as one with neither). Deliberately a literal
four-file list, not a derived census like `tests/
poll_tcp_gate_wiring_structure.rs`'s: this suite is not discovering
"whichever gates currently do X," it is pinning that these four SPECIFIC
gates -- the ones this round's own dispatching brief named, and the same
four `docker/qemu/lib/gate-structure-preflight.sh`'s own header documents
-- keep the wiring this round put there. A fifth boot gate added later is
not implicitly in this pin's scope; wiring it in is that future round's own
explicit decision, the same shape `tests/critical_path_logging_census_
structure.rs`'s three-file `ZERO_PIN_FILES` list already uses in this
tree. See the test file's own module doc for the fuller statement.

Four tests: the real-tree wiring check, a check that the shared lib still
defines the function and prints the `[GATE_PREFLIGHT:` marker and honours
`BREENIX_GATE_SKIP_STRUCTURE`, an in-memory call-site-removal mutation, and
an anti-vacuity check that a gate with neither the source line nor the
call is rejected outright.

### Live mutation proof (not just the in-file mutation test)

```
$ cp docker/qemu/run-x86-boot-tests.sh /tmp/run-x86-boot-tests.sh.bak
# removed the `if ! gate_structure_preflight ...; then ... fi` block from
# the on-disk file (python, exact block match)
$ bash scripts/run-structure-tests.sh gate_structure_preflight_wiring_structure
...
test every_target_gate_calls_the_structure_preflight ... FAILED
  assertion `left == right` failed: gate(s) missing the structure-preflight wiring: ["docker/qemu/run-x86-boot-tests.sh"]
test missing_wiring_validator_rejects_a_gate_with_the_call_site_removed ... FAILED
test result: FAILED. 2 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out
$ echo EXIT=$?
EXIT=101
$ cp /tmp/run-x86-boot-tests.sh.bak docker/qemu/run-x86-boot-tests.sh
$ diff -q /tmp/run-x86-boot-tests.sh.bak docker/qemu/run-x86-boot-tests.sh
$ bash scripts/run-structure-tests.sh gate_structure_preflight_wiring_structure
...
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

Removing the wiring from one gate reddens the suite and names that exact
gate; reverting restores a clean, byte-identical file and a green suite.

## Isolated preflight time cost

Every timing figure earlier in this document (`Duration: 508s` in the
strict-gate run below) is an AGGREGATE: the one-time preflight plus the
whole boot loop that follows it, with no breakdown between the two. That
aggregate does not by itself show what the preflight adds, because
`scripts/run-structure-tests.sh` recompiles every `tests/*_structure.rs`
file (47/47 discovered today, the same count the `[GATE_PREFLIGHT:
structure_suites=...]` line below reports) from scratch on every call --
no mtime check, no cache, nothing reused between the four gates' four
separate invocations -- so this cost is paid in full, every time, on
every one of the four gates this round wires it into.

Measured standalone (`gate_structure_preflight` alone, sourced and called
directly, outside any boot loop or kernel build), fresh for this round's
review-fix pass, on both hosts these four gates actually run on:

```
# Apple Silicon Mac (this worktree, native macOS rustc; 47/47 suites)
$ time bash -c 'source docker/qemu/lib/gate-structure-preflight.sh; \
    gate_structure_preflight "$PWD" "$BREENIX_GATE_TMP"'
[GATE_PREFLIGHT:structure_suites=47/47:critical_path_lines=275:pinned=136]
real 126.70
user 448.58
sys 10.45
```

```
# beast, breenix-x86 Incus container (own clone at /root/breenix-gw,
# checked out at this branch's own c147024f54f5b4a56c8541334534a77c318d2b5e;
# 47/47 suites; this is the actual host run-x86-boot-tests.sh and
# run-x86-prod-profile-boot-test.sh execute in for merge-gating)
$ bash -c 'TIMEFORMAT="real %R user %U sys %S"; \
    source docker/qemu/lib/gate-structure-preflight.sh; \
    time gate_structure_preflight /root/breenix-gw "$BREENIX_GATE_TMP"'
[GATE_PREFLIGHT:structure_suites=47/47:critical_path_lines=275:pinned=136]
real 331.463 user 1140.895 sys 19.307
```

So the preflight's own isolated share is ~127s (about two minutes) on the
Mac and ~331s (about five and a half minutes) on beast -- the host that
matters for the two x86 gates, `run-x86-boot-tests.sh` and
`run-x86-prod-profile-boot-test.sh`. Those two gates' own boot loops
already run ~8 minutes each under TCG (per this repository's own
documented beast timing), so this preflight roughly doubles each x86
gate's total wall-clock cost, in addition to the boot loop it already
paid. Both aarch64 gates pay the smaller (~2min-scale) Mac-class number on
top of their own boot loops, since the aarch64 gates run natively on this
Mac, not in the beast container. `BREENIX_GATE_SKIP_STRUCTURE=1` is the
documented opt-out for a caller that has already paid this cost earlier in
the same session and does not want to pay it again.

## Evidence

### 47 of 47 `tests/*_structure.rs` suites, standalone, on this branch

Ran every file matching `tests/*_structure.rs` (47 files, including the
new `gate_structure_preflight_wiring_structure`) through `scripts/
run-structure-tests.sh <name>` individually, three separate times across
this round (after the wiring commit, after the claim-lint fixes, and once
more as a final check). 47 of 47 exited 0 each time.

### aarch64 strict gate, script default (20 iterations)

Built the `boot_tests` kernel first:
```
cargo build --release --features boot_tests --target aarch64-breenix-kernel.json \
  -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem \
  -p kernel --bin kernel-aarch64
```
`scripts/check-kernel-no-neon.sh`: PASS, 0 FP/SIMD instructions.

```
$ shasum -a 256 target/aarch64-breenix-kernel/release/kernel-aarch64
17c7b84b0068c1614a1f1c5a170dc7f9df097ded2fdc0ece2b80111518d6e852  ...kernel-aarch64
$ BREENIX_GATE_TMP=/tmp/breenix-gate-tmp-strict-pr1 bash docker/qemu/run-aarch64-boot-test-strict.sh
[GATE_PREFLIGHT:structure_suites=47/47:critical_path_lines=275:pinned=136]
...
Total iterations: 20
Successes: 20
Failures: 0
Success rate: 100%
Duration: 508s
PASS: 20/20 boots succeeded
$ shasum -a 256 target/aarch64-breenix-kernel/release/kernel-aarch64
17c7b84b0068c1614a1f1c5a170dc7f9df097ded2fdc0ece2b80111518d6e852  ...kernel-aarch64
```
Identical hash before and after the full 508-second run (preflight
included): the `rustc --test` preflight step does not touch the kernel
build products.

### aarch64 production-profile gate, x1

```
$ BREENIX_GATE_TMP=/tmp/breenix-gate-tmp-prod-pr1 bash docker/qemu/run-aarch64-prod-profile-boot-test.sh
[GATE_PREFLIGHT:structure_suites=47/47:critical_path_lines=275:pinned=136]
Building the shipped ARM64 production kernel profile...
...
PASS: production profile reached bsshd with the futex oracle seam absent
```
Re-ran once more from a clean `BREENIX_GATE_TMP` immediately after (no
edits between the two runs) to confirm the script's own exit code, not
just its printed PASS line: `echo $?` -> `0`.

### `scripts/claim-lint.py`

```
$ python3 scripts/claim-lint.py
claim-lint: clean (6 file(s) checked, changed hunks vs 138fb8395d0a).
claim-lint: 115 pre-existing finding(s) outside this branch's changed hunks not reported (--whole-file shows them).
```
Exit 0. The 115 pre-existing findings are untouched by this round's diff
(0 of the six changed files' pre-existing content -- e.g. the ~40
marker-count assertions already in `run-x86-boot-tests.sh` -- was edited
here); `--whole-file` was used only to confirm that count during drafting,
not as the gate this round pushes on.

### Beast x86 (Incus container `breenix-x86`)

`ssh beast` -> `sudo -n incus exec breenix-x86 -- ...`, own clone at
`/root/breenix-gw` (fetched via `/root/breenix`, which itself fetched
`gates/structure-suites-in-gates` from `origin` directly -- GitHub was
reachable from inside the container this session), `rust-fork` symlinked
to `/root/breenix/rust-fork-real` matching the persistent clone's own
symlink, prebuilt `userspace/programs/*.elf` and `fonts/` copied in from
`/root/breenix` (build artifacts, not tracked in git, needed by the
kernel's `include_bytes!` test registry regardless of which structure
suites run). `BREENIX_GATE_TMP=/root/breenix-gw-tmp`.

`rustc` resolves via `rustup`'s directory-based toolchain override, which
needs the invocation's cwd to be inside a tree carrying `rust-toolchain.toml`
-- confirmed present and working (`cd /root/breenix-gw && rustc --version`
-> `1.90.0-nightly`). The preflight's `command -v rustc` check found it
genuinely on `PATH`, and the printed `structure_suites=47/47` on both runs
below is the real count from real suite runs on that container, not the
loud-skip fallback: the container did not need it.

**`run-x86-boot-tests.sh 1`:**
```
[GATE_PREFLIGHT:structure_suites=47/47:critical_path_lines=275:pinned=136]
...
x86 frame-custody gate run 1: PASS
```

**`run-x86-prod-profile-boot-test.sh`:**
```
[GATE_PREFLIGHT:structure_suites=47/47:critical_path_lines=275:pinned=136]
Building the shipped x86_64 production kernel profile...
...
PASS: x86 production profile reached steady state with the teardown census at rest
```
One "WARNING: BusyBox build failed (see build-busybox.sh for prerequisites)"
line appears during the ext2-disk-creation step -- a pre-existing fact of
this container (no Docker available for the BusyBox cross-build), unrelated
to this round's diff and not a gate verdict; the only other "FAIL"-shaped
text on the page is the fault-marker line's own label ("fault marker 'DISK
LOADING FAILED': 0"), reading 0 (claim-lint:ok: 1 of 371 lines in the
captured transcript match "FAIL", and that one line is this WARNING, per
`grep -c FAIL` against the saved log). No other gate-verdict FAIL line
appears in the transcript.

Both beast runs, and the earlier `run-x86-boot-tests.sh` run, spent several
minutes waiting on `lib/qemu-host-lock.sh`'s host-wide lock behind
concurrent x86 boots from OTHER sessions on the shared beast host
(`breenix-p766`, `breenix-chk1`) -- neither this round's gates nor this
round's agent touched those processes; the lock's own wait-and-retry
behavior is what let this round's boots proceed once each other session's
QEMU released it.

## Deliberately not done

`docker/qemu/run-aarch64-testing-profile-boot-test.sh`, `run-aarch64-
percpu-stack-custody-gate.sh`, and each other gate script under `docker/
qemu/` besides the four named in this round's own dispatching brief is
untouched. Whether the structure-suite preflight belongs in any of them
too is a decision for whoever scopes that gate's own round, not something
this round's literal four-file ratchet assumes.

`origin/main` has advanced 14 commits past this branch's `138fb8395d0a`
base as of this doc (most recently PR #885, which touches `kernel/src/
tty/driver.rs` and pins the TTY input-IRQ oracle on several gates,
including the four this round wires into). This round did not merge or
rebase onto that newer tip -- doing so would pull `kernel/` changes into a
branch this round's own scope forbids editing `kernel/` on, and a plain
`git merge origin/main` risks conflicting with those same gate scripts'
newly-pinned oracle blocks. Reconciling the two is left to the merge step,
not folded into this round.

## Files touched

- `docker/qemu/lib/gate-structure-preflight.sh` (new)
- `docker/qemu/run-aarch64-boot-test-strict.sh`
- `docker/qemu/run-aarch64-prod-profile-boot-test.sh`
- `docker/qemu/run-x86-boot-tests.sh`
- `docker/qemu/run-x86-prod-profile-boot-test.sh`
- `tests/gate_structure_preflight_wiring_structure.rs` (new)
- `docs/planning/green-program/gates/GATE-TOOLING-STRUCTURE-PREFLIGHT-PR1-2026-09-06.md`
  (this file)

`scripts/run-structure-tests.sh` and `scripts/check-critical-path-
violations.sh` are read by the new preflight but not edited by this round.

## Landing re-smoke

Landed at `72ce064bb9e3619802c0b45cada7950c647ae696` (review passed) by
merging `origin/main` (`a0ec6cf8473d02c5029fc9ab44403c7f68cedde3`, PR #887)
in with `git merge --no-ff`. `git diff --name-only HEAD` after resolution
lists 65 changed paths, 4 of them the `docker/qemu` gate scripts both sides
wired (all 4 auto-merged cleanly, both sides' hunks present in each --
confirmed by grepping each for `gate-structure-preflight`, 4 hits each),
and `git diff --check` plus a repo-wide grep for
`<<<<<<<`/`=======`/`>>>>>>>` each found 0 conflict markers in the 65
changed paths (claim-lint:ok: 0 of 65, both checks run directly against
the merged tree). Reproduced the identical merge
independently on the beast x86 host from the same two parent commits (no
GitHub reachable from that container, so both tips were fetched by exact
SHA through the host's own already-fetched clone): the resulting tree hash
(`9a64ca2b63a1ec45177c5d8e67b55597e3428ca6`) is byte-identical to the Mac
merge's tree hash.

`python3 scripts/claim-lint.py` on the merged tree:
```
claim-lint: clean (8 file(s) checked, changed hunks vs a0ec6cf8473d).
claim-lint: 115 pre-existing finding(s) outside this branch's changed hunks not reported (--whole-file shows them).
```
Exit 0.

### The `tests/*_structure.rs` suites (48/48), standalone, on the merged tree

`origin/main`'s #822 round added a 48th suite (`tty_irq_fg_structure`) since
the 47/47 count PR-1's own Evidence section above recorded; the preflight's
own discovery (`find tests -maxdepth 1 -name '*_structure.rs'`) picks it up
without any change to this round's code, and running it directly (`source
docker/qemu/lib/gate-structure-preflight.sh; gate_structure_preflight
"$(pwd)" "$BREENIX_GATE_TMP"`) on the Mac scored:
```
[GATE_PREFLIGHT:structure_suites=48/48:critical_path_lines=275:pinned=136]
```
48 of 48 green, exit 0. (One capture artifact for the record, not a repo
defect: three direct attempts to write that exact line through this
session's interactive shell -- plain `echo`, and the same command
redirected straight to a file -- each landed on disk 2 bytes short, always
the same two bytes ("`:c`" out of "`:critical_path_lines`"), reproducibly
at that fixed byte offset; a `python3 subprocess.run(..., capture_output=
True)` invocation of the identical function, same working tree, same
values, wrote the full 75-byte line with `:critical_path_lines=275`
intact. All numeric readings cited in this section are taken from that
subprocess capture, not the shorter files.)

### `scripts/test_claim_lint.py`

```
$ python3 scripts/test_claim_lint.py
...
Ran 72 tests in 2.212s

OK
```
72 of 72 passed, exit 0.

### aarch64 strict gate, script default (20 iterations)

Built the `boot_tests` kernel and both architectures' userspace ELFs first
(`userspace/programs/build.sh` and `--arch aarch64`, then `scripts/
create_ext2_disk.sh --arch aarch64` -- this worktree's fresh `target/`
directory had 0 of these artifacts before this step, per its first build
attempt failing on a missing `simple_exit.elf`). `scripts/
check-kernel-no-neon.sh`: PASS, 0 FP/SIMD instructions.
```
$ bash docker/qemu/run-aarch64-boot-test-strict.sh
[GATE_PREFLIGHT:structure_suites=48/48:critical_path_lines=275:pinned=136]
...
Total iterations: 20
Successes: 20
Failures: 0
Success rate: 100%
Duration: 256s
PASS: 20/20 boots succeeded
```

### aarch64 production-profile gate, x1

```
$ bash docker/qemu/run-aarch64-prod-profile-boot-test.sh
[GATE_PREFLIGHT:structure_suites=48/48:critical_path_lines=275:pinned=136]
...
PASS: production profile reached bsshd with the futex oracle seam absent
...
Observed crash marker count: 0
[GATE_BOOT_FACTS:boot=1:...:ended_by=scored_pass]
```

### beast x86: `run-x86-boot-tests.sh 1`

Own clone `/root/breenix-gw` (fetched by exact SHA from the beast host's
own `/root/breenix`, since the container reaches no outbound GitHub),
`rust-fork` symlinked to `/root/breenix/rust-fork-real`,
`BREENIX_GATE_TMP=/root/breenix-gw-tmp`. x86_64 userspace built first
(`userspace/programs/build.sh`, no `--arch` -- x86_64 is the default).
```
[GATE_PREFLIGHT:structure_suites=48/48:critical_path_lines=275:pinned=136]
...
x86 frame-custody gate run 1: PASS
```
Exit 0. One `grep -c FAIL` hit in the 617-line transcript, and it is not a
verdict: line 606 is the `EXEC_FAILED_RELEASE_PROD` counter label, whose
own 15 fields read 10 zero-valued counters (`balance`, `undecided`,
`mid_retire`, `lost`, `custody_refused`, `decref_unregistered`, `double`,
`stale`, `untracked`, `root_slot_refused`, each `=0`) plus 5 `true`-valued
flags this label's own name predicts (`plain_err`, `plain_kept`,
`argv_err`, `argv_kept`, `name_kept`) -- not a FAIL line (claim-lint:ok:
10-of-15 and 5-of-15, counted directly against this round's own captured
line, quoted in full above the "beast x86:" heading is the same transcript
this count is read from).

### beast x86 production-profile gate

```
[GATE_PREFLIGHT:structure_suites=48/48:critical_path_lines=275:pinned=136]
...
PASS: x86 production profile reached steady state with the teardown census at rest
```
Exit 0. One `grep -c FAIL` hit in the 371-line transcript, and it is the
same pre-existing shape PR-1's own Evidence section already recorded for
this gate: the "fault marker 'DISK LOADING FAILED': 0" line, reading 0.

Both beast runs waited on `lib/qemu-host-lock.sh`'s host-wide lock behind
concurrent x86 boots from other sessions on the shared beast host
(`breenix-chk1`, `breenix-p766` were both observed running their own
`qemu-system-x86_64` during this round's wait windows) -- the same pattern
PR-1's own Evidence section recorded; neither this round's gates nor this
round's agent touched those processes. No gate process on either host died
mid-run without reaching a verdict, so #871's beast-disk-fault signature
did not occur this round and no `journalctl` attribution or retry was
needed.

`python3 scripts/claim-lint.py` on the tree, immediately before push:
```
claim-lint: clean (8 file(s) checked, changed hunks vs a0ec6cf8473d).
claim-lint: 115 pre-existing finding(s) outside this branch's changed hunks not reported (--whole-file shows them).
```
Exit 0.
