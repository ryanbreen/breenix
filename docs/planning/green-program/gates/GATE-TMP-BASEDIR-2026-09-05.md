# `BREENIX_GATE_TMP`: a base-dir variable for x86 gate output (#797)

## The collision #797 describes

Quoting the issue directly:

> On 2026-09-04, an x86 gate lane building the `#737` DF-preempt-oracle fix
> scored **another lane's serial as its own**, because both lanes ran on
> `beast` -> Incus container `breenix-x86` and both invoked a gate script that
> hardcodes an absolute `/tmp/breenix_*` output directory. ... the first
> attempt of this slot scored **another lane's serial**: both scripts `rm
> -rf`'d and recreated the same directory, and the surviving files were the
> other clone's.

The mitigation used on 2026-09-04 was `unshare -m` with `/tmp` bind-mounted to
a per-clone directory — opt-in, external to both scripts, and easy to forget.
The issue proposes a `BREENIX_GATE_TMP` env var, defaulting to `/tmp` so an
unset caller is unaffected, threaded through each script that builds an
output-directory path from a literal `/tmp/breenix_*` prefix, with the
verdict/tally readers pointed at the same base.

## What changed

Four scripts had a literal `/tmp/breenix_*` prefix baked into their
`OUTPUT_DIR` (and, in one case, `BUILD_LOG`/`failure_dir`) construction. Each
now resolves that prefix from `BREENIX_GATE_TMP`, defaulting to `/tmp`:

```bash
BREENIX_GATE_TMP="${BREENIX_GATE_TMP:-/tmp}"
OUTPUT_DIR="$BREENIX_GATE_TMP/breenix_x86_boot_tests_$i"   # was: "/tmp/breenix_x86_boot_tests_$i"
```

| Script | Site(s) changed |
|---|---|
| `docker/qemu/run-x86-boot-tests.sh` | `OUTPUT_DIR` (1 site) |
| `docker/qemu/run-boot-parallel.sh` | `OUTPUT_DIR` (2 sites — the launch loop and the wait/verdict loop, which reconstructed the same string independently; both now derive from the one variable) |
| `docker/qemu/run-kthread-parallel.sh` | `OUTPUT_DIR` (2 sites, same launch/wait-loop duplication) |
| `docker/qemu/run-x86-prod-profile-boot-test.sh` | `OUTPUT_DIR`, the `report_gate_failure` trap's `failure_dir`, and `BUILD_LOG` (3 sites) |

`scripts/x86-gate-verdict.sh` and `scripts/x86-strand-census.sh` were checked
(`grep -n "/tmp/breenix\|BREENIX_GATE_TMP"` against both — no hits) and need no
change: neither constructs a `/tmp`-based path of its own (review finding F5:
`x86-gate-verdict.sh` does build two paths, `ALLOWLIST_PATH` and the census
script's own path, but both are relative to `$SCRIPT_DIR` — the verdict
script's own directory — not to any gate-output base). The serial-log paths
that matter for this issue are taken as positional arguments, so a caller
that threads `$OUTPUT_DIR` (itself now built from `$BREENIX_GATE_TMP`) into
the verdict script's argv already has producer and consumer agreeing on the
same base — there is no second hardcoded gate-output literal for them to
drift out of sync with; `docker/qemu/run-x86-boot-tests.sh` does exactly
this, threading `"$OUTPUT_DIR"/serial_*.txt` into the verdict script's argv.
This is the "pass the same base to the verdict scripts" requirement from the
issue: it is satisfied by construction here because the verdict scripts never
had a gate-output base of their own (claim-lint:ok: #797, 0/2 hits from the
grep above).

The `#797` issue text itself scopes the two scripts that actually collided in
production (`run-x86-boot-tests.sh`, `run-boot-parallel.sh`) as the required
fix, and separately notes that its own grep "returns 50 matching lines across
`docker/qemu/*.sh` and `scripts/*.sh` with the same `/tmp/breenix_*` pattern
(e.g. `/tmp/breenix_aarch64_kthread_$i`, `/tmp/breenix_kthread_$i`,
`/tmp/breenix_gate_$i`, `/tmp/breenix-parallels-serial.log`)", calling the
remainder "candidates for the same fix but ... not proposed here." The issue
names no specific script beyond the two it scopes as required and the four
literals quoted above as examples (review finding F2: an earlier draft of
this section attributed a categorized list — aarch64 gates, `run-x86-gate.sh`
and `run-x86-tty-oracle-gate.sh` by name, `run-vmware-gate.sh`, "forensic/
debug scripts" — to "the issue text itself"; that categorization is this
document's own reading of the 50-line grep, not a list the issue enumerates).
This change also covers `run-x86-prod-profile-boot-test.sh` and
`run-kthread-parallel.sh` because both share the identical hazard — a literal
`/tmp/breenix_*` `OUTPUT_DIR` a concurrent lane on the same beast container
can `rm -rf` and recreate — and both were named for this round.

**Widened in the R157 review round**: review finding F1 named four more
scripts that run on the same shared beast container (all four accept
`--x86`/`--x86_64`, or are x86-only by construction) and still hardcoded a
fixed `/tmp/breenix_*` base, so `BREENIX_GATE_TMP`'s presence in the four
scripts above read as isolation these siblings did not honor. They are now
converted the same way (claim-lint:ok: #797 F1 — the four rows below name
exactly which scripts and sites; `grep -c -- '--x86'` against `run-fs-fault-
gate.sh` and `run-ext2-lock-race-gate.sh` each returns >=1, confirming both
accept that flag):

| Script | Site(s) changed |
|---|---|
| `docker/qemu/run-x86-gate.sh` | `OUTDIR` (1 site, in the per-boot loop) |
| `docker/qemu/run-x86-tty-oracle-gate.sh` | `OUTPUT_ROOT`, `BUILD_LOG` (2 sites) |
| `docker/qemu/run-fs-fault-gate.sh` | `OUTPUT_DIR` (1 site; reached via `--x86`) |
| `docker/qemu/run-ext2-lock-race-gate.sh` | `OUTPUT_DIR` (1 site; reached via `--x86`) |

`docker/qemu/run-coreproof-gate.sh` was checked again and remains unaffected:
its `OUTPUT_BASE` (line 68) is still a bare `/tmp/breenix_coreproof_gate`
literal, but `OUTPUT_ROOT="$OUTPUT_BASE/${RUN_STAMP}_$$"` (line 71) is
already unique per invocation via the timestamp+PID suffix, so it never had
this hazard (claim-lint:ok: #797, `grep -n 'OUTPUT_BASE\|OUTPUT_ROOT'
docker/qemu/run-coreproof-gate.sh` — 5 of the 5 usages after line 71 read
`OUTPUT_ROOT`, none read the bare `OUTPUT_BASE` literal directly).

The aarch64 gate scripts under `docker/qemu/` were surveyed with
`grep -n "/tmp/breenix" docker/qemu/*.sh` and left out of this round: they run
on a per-developer Parallels VM or native Mac QEMU, not the shared beast
container concurrent clones write into, so the concurrent-lane collision this
issue documents does not apply to them the same way; two of them
(`run-aarch64-service-sequence-gate.sh`, `run-aarch64-refusal-drain-gate.sh`)
already accept a full `OUTPUT_DIR` override via their own env var. **Review
finding F3**: that glob cannot reach `docker/qemu-aarch64/run-arm64-boot.sh`,
which lives in a sibling directory and hardcodes the same
`OUTPUT_DIR="/tmp/breenix_arm64_boot"` pattern (found instead by
`grep -rn "/tmp/breenix" docker/ scripts/ tests/ xtask/`, the repo-wide form
used to compile the "remaining sites" survey elsewhere in this document). It
is left out of this round for the same reason as the other aarch64 scripts —
it runs QEMU inside a local Docker container, not the shared beast container
— but the coverage claim above is now scoped to the command that actually
supports it, not overstated as ranging over a directory it cannot see.
`docker/qemu/run-vmware-gate.sh` and the forensic/debug scripts under
`scripts/` (`forensic-capture.sh`, `debug_smp_deadlock.py`,
`debug_elr0_crash.py`, `test_tracing_via_gdb.sh`) were checked the same way
(`grep -rn "/tmp/breenix" docker/ scripts/`) and are left out for the same
non-beast reason — VMware and Parallels hosts, or one-off local debugging
sessions, not the shared container. Widening any of these remains a candidate
follow-up, consistent with the issue's own scoping.

## Default-unchanged: the diff

For three of the four scripts, the resolved `OUTPUT_DIR`/`BUILD_LOG` line is
byte-identical to the pre-existing hardcoded line once `${BREENIX_GATE_TMP}`
is substituted back to `/tmp` (`diff` between the origin/main line and the
substituted line is empty for `run-x86-boot-tests.sh`, `run-boot-parallel.sh`
and `run-kthread-parallel.sh`).

`run-x86-prod-profile-boot-test.sh` has one line where the same substitution
leaves a diff:

```
< BUILD_LOG=/tmp/breenix_x86_prod_profile_build.log
---
> BUILD_LOG="/tmp/breenix_x86_prod_profile_build.log"
```

The only difference is quoting style (the new line uses `"$BREENIX_GATE_TMP/..."`,
which required quotes it didn't have as a bare literal). This does not change
the assigned value — `BUILD_LOG=/tmp/foo.log; echo "$BUILD_LOG"` and
`BUILD_LOG="/tmp/foo.log"; echo "$BUILD_LOG"` both print
`/tmp/foo.log` — verified directly on beast rather than asserted.

The two other paths in that script (`OUTPUT_DIR` and the `report_gate_failure`
trap's `failure_dir`) diff empty against origin/main once substituted.

The same check was repeated for the four scripts the R157 review round added
(F1): `run-x86-gate.sh`'s `OUTDIR`, `run-fs-fault-gate.sh`'s `OUTPUT_DIR`, and
`run-ext2-lock-race-gate.sh`'s `OUTPUT_DIR` — 3 of 3 — diff empty against
origin/main once `${BREENIX_GATE_TMP}` is substituted back to `/tmp`.
`run-x86-tty-oracle-gate.sh` has the same one-line quoting-only diff as
`run-x86-prod-profile-boot-test.sh` above, on its `BUILD_LOG` line (bare
`BUILD_LOG=/tmp/...` becoming `BUILD_LOG="$BREENIX_GATE_TMP/..."`); its
`OUTPUT_ROOT` line diffs empty (claim-lint:ok: #797, `git show
origin/main:<file>` compared line-for-line against each script's post-
substitution line for all four files above).

## Absolute-path and socket-length guards (review findings F6, F7)

**F6** — `run-x86-prod-profile-boot-test.sh` computes `OUTPUT_DIR` before its
own `cd "$BREENIX_ROOT"`, while `run-x86-boot-tests.sh` computes it after; the
`ERR` trap in the former is installed before that `cd` too and reads
`OUTPUT_DIR` in its failure path. This cd-order difference had no effect
while `OUTPUT_DIR` was a fixed `/tmp/...` literal (which resolves identically
regardless of the shell's current directory), but `BREENIX_GATE_TMP` is now
operator-supplied and was not checked to be absolute — a relative value would
resolve against whichever directory happened to be current at each read, and
the two scripts' differing cd-order would make that resolution disagree
between them. All eight scripts in this change — the four from the original
commit and the four F1 added — now validate `BREENIX_GATE_TMP` immediately
after reading it (claim-lint:ok: #797 F6, `grep -c 'BREENIX_GATE_TMP must be
an absolute path'` returns exactly 1 in each of the 8 files touched by this
round, and the beast run below exercises all 8):

```bash
BREENIX_GATE_TMP="${BREENIX_GATE_TMP:-/tmp}"
case "$BREENIX_GATE_TMP" in
    /*) ;;
    *) echo "... FAIL (BREENIX_GATE_TMP must be an absolute path, got: $BREENIX_GATE_TMP)"; exit 1 ;;
esac
```

This removes the cd-order dependency (both scripts now resolve `OUTPUT_DIR`
the same way regardless of cd order) rather than reordering the `cd` calls
themselves, which would be a larger and unrelated change to each script's
control flow. The default (`/tmp`) and the custom value this document boots
with (`/root/gate-tmp-797`) both satisfy the guard by inspection — `case`
matching `/*` against either string — and the guard's actual rejection
behavior on beast, against 8 of 8 scripts, is in the "R157 review round"
subsection below (claim-lint:ok: #797 F6, see that subsection's per-script
output).

**F7** — `run-x86-prod-profile-boot-test.sh` and `run-x86-tty-oracle-gate.sh`
both open an AF_UNIX console socket at `$OUTPUT_DIR/console.sock` (or
`$RUN_DIR/console.sock`). Linux's `struct sockaddr_un.sun_path` is 108 bytes
including the terminating NUL, so a path of 108+ characters cannot be bound;
before this change the fixed `/tmp/breenix_x86_prod_profile/console.sock` and
`/tmp/breenix_x86_tty_oracle/boot_N/console.sock` prefixes left no way to hit
that limit, but `BREENIX_GATE_TMP` is now caller-controlled and a long value
could. Both scripts now compute the full socket path and reject it before
booting anything if it exceeds 107 characters:

```bash
CONSOLE_SOCK_PATH="$OUTPUT_DIR/console.sock"
if [ "${#CONSOLE_SOCK_PATH}" -gt 107 ]; then
    echo "... FAIL (console socket path exceeds the AF_UNIX sun_path limit of 107 chars: ...)"
    exit 1
fi
```

`run-x86-tty-oracle-gate.sh` computes its widest possible path
(`$OUTPUT_ROOT/boot_$BOOTS/console.sock`, using its own last and therefore
longest boot number) right after argument parsing rather than inside the
per-boot loop, so a too-long `BREENIX_GATE_TMP` fails before that script's
build step runs at all, the same as `run-x86-prod-profile-boot-test.sh`
already did.

This was checked against the two values this document actually boots with
(`printf '%s' "$path" | wc -c`, not asserted by eye): default
`/tmp/breenix_x86_prod_profile/console.sock` is 42 characters and the custom
value used in the (b) run above, `/root/gate-tmp-797/breenix_x86_prod_profile/
console.sock`, is 56 characters — both under the 107-char limit by a wide
margin. Both guards were also exercised standalone on beast with a
deliberately oversized `BREENIX_GATE_TMP` and rejected in well under a
second, before either script reached its build step (claim-lint:ok: #797 F7
— see the "R157 review round" beast-run subsection below for the exact
rejected commands and their output, both a real run captured firsthand this
round).

## The evidence-capture driver at `docs/planning/713-x86-spawn/serials/run-leg1.sh` (review finding F4)

This checked-in script is a historical record of a 12-boot evidence-capture
run for issue `#713`: it hardcodes `cd /root/breenix` and
`/root/p713-prove/leg1` because those name the exact clone and output
directory that run produced, and it ran before `BREENIX_GATE_TMP` existed, so
its `cp -r /tmp/breenix_x86_prod_profile ...` line was correct for the run it
recorded. Left as a plain `/tmp` literal with `2>/dev/null || true` swallowing
any `cp` failure, it is now a landmine for anyone who reuses this script
against a caller that sets `BREENIX_GATE_TMP` to something else: the script
would silently read the wrong directory, and the redirected `stderr` on its
`cp` meant a missing directory produced no message at all. It now reads
`$BREENIX_GATE_TMP`, defaulting to `/tmp` — the environment the original
12-boot run actually executed under, so re-running it unmodified still
reproduces that run's behavior — and reports a failed capture to stderr per
boot instead of swallowing it, without aborting the remaining boots in the
loop (claim-lint:ok: #797 F4, `bash -n` clean; see the diff in this branch's
own commit for the exact before/after).

## Beast run (clone `/root/breenix-797`, HEAD `07c7b6be`)

Both runs used a distinct clone (`/root/breenix-797`, branched from
`origin/main` at `07c7b6be35b67a3001969a79279dd6bcefd83121`, the same commit
this branch is based on) inside the shared `breenix-x86` Incus container, with
the QEMU-concurrency gate (`pgrep -fl qemu-system-x86_64 | wc -l <= 1`)
satisfied before each boot (1 other lane's QEMU process was running before
each of the two runs below; this round added at most one more).

### (a) Default run — `BREENIX_GATE_TMP` unset

`./docker/qemu/run-x86-prod-profile-boot-test.sh` (no env override):

```
PASS: x86 production profile reached steady state with the teardown census at rest
=== GATE EXIT CODE: 0 (elapsed 131s) ===
```

`ls -la --time-style=full-iso /tmp/breenix_x86_prod_profile/` after the run
shows the run's own output (`OVMF_CODE.fd`, `OVMF_VARS.fd`, `qemu.log`,
`serial_kernel.txt`, `serial_user.txt`) with fresh timestamps at that path —
exactly where the pre-existing script wrote before this change, since
`BREENIX_GATE_TMP` was unset and defaulted to `/tmp`.

Built UEFI image: `target/release/build/breenix-a14bb21948d9e08d/out/breenix-uefi.img`
sha256 `002c53575bee9885cd601c4eb5535107c76640bcff521dd403d281af25d58b42`.

### (b) Custom run — `BREENIX_GATE_TMP=/root/gate-tmp-797`

This run was done twice on the same clone. The first pass used a copy of
`run-x86-boot-tests.sh` taken before this branch had been rebased onto
`origin/main`'s then-current tip (a concurrently-merged PR, #799, had bumped
this file's own `EXPECTED_USERSPACE_EXITS` floor from 104 to 105 by adding one
more launched test program). That first pass's PASS verdict was still true —
110 measured exits clears both the old and new floor — but it was not the
exact file this PR commits, so it is not the one reported here; the run below
is the second pass, taken after `git reset --hard origin/main` +
`git stash pop` on the local branch and re-pushing the resulting file to the
clone, matching this branch's actual committed content and read back with
`git log --oneline -1` on the clone before running:

```
07c7b6be Merge pull request #799 from ryanbreen/fix/737-df-oracle-ratchet
```

`BREENIX_GATE_TMP=/root/gate-tmp-797 ./docker/qemu/run-x86-boot-tests.sh 1`:

```
x86 userspace gate: PASS - exited=110 expected>=105 nonzero=0 allowlist=0
x86 frame-custody gate run 1: PASS
=== GATE EXIT CODE: 0 (elapsed 513s) ===
```

`/tmp/breenix_x86_boot_tests_1` (a directory this same clone's default run
would have written to) was captured before and after this custom run, with
`ls -la --time-style=full-iso`:

```
BEFORE                                                    AFTER (unchanged)
drwxr-xr-x 2 root root    4096 2026-09-05 04:06:19.788 .  2026-09-05 04:06:19.788 .
-rw-r--r-- 1 root root 3653632 2026-09-05 04:06:22.703 OVMF_CODE.fd    04:06:22.703 OVMF_CODE.fd
-rw-r--r-- 1 root root  540672 2026-09-05 04:06:22.719 OVMF_VARS.fd    04:06:22.719 OVMF_VARS.fd
-rw-r--r-- 1 root root     690 2026-09-05 04:14:04.489 qemu.log        04:14:04.489 qemu.log
-rw-r--r-- 1 root root 1694171 2026-09-05 04:14:03.574 serial_kernel.txt 04:14:03.574 serial_kernel.txt
-rw-r--r-- 1 root root   72110 2026-09-05 04:14:03.600 serial_user.txt 04:14:03.600 serial_user.txt
```

6/6 entries (the directory plus its 5 files) carry the identical mtime before
and after this custom run — the only thing that changed between the two
snapshots was the shared `/tmp` parent directory's own mtime, from something
else on the container writing elsewhere in `/tmp`, not from this run — so this
run did not write to it. All of this run's own output landed at
`/root/gate-tmp-797/breenix_x86_boot_tests_1/` instead:

```
=== AFTER: ls -la --time-style=full-iso /root/gate-tmp-797/breenix_x86_boot_tests_1 ===
-rw-r--r-- 1 root root 3653632 2026-09-05 05:11:30 OVMF_CODE.fd
-rw-r--r-- 1 root root  540672 2026-09-05 05:11:30 OVMF_VARS.fd
-rw-r--r-- 1 root root     718 2026-09-05 05:18:54 qemu.log
-rw-r--r-- 1 root root 1692254 2026-09-05 05:18:53 serial_kernel.txt
-rw-r--r-- 1 root root   74358 2026-09-05 05:18:53 serial_user.txt
```

The verdict script was re-run standalone, reading from that same
`$BREENIX_GATE_TMP`-based path with the floor read back from the script's own
`EXPECTED_USERSPACE_EXITS` rather than typed by hand, to confirm producer and
consumer agree on the base:

```
CURRENT_FLOOR (from the script's own EXPECTED_USERSPACE_EXITS)=105
x86 userspace gate: PASS - exited=110 expected>=105 nonzero=0 allowlist=0
```

Built UEFI image (boot_tests profile):
`target/release/build/breenix-74961d3be894a996/out/breenix-uefi.img`
sha256 `fc727beceb233102c29b2f621359323c6041ad018e8dc55e66d18b75a8aef0b6`.

(This hash differs from (a)'s because (a) and (b) build two different feature
profiles of the kernel — the shipped production profile for (a),
`testing,external_test_bins` for (b) — not because of anything this change
touches.)

### (c) R157 review round — `run-x86-gate.sh`, one of the four F1 additions

Re-run on the same clone (`/root/breenix-797`) after confirming
`pgrep -fl qemu-system-x86_64 | wc -l` was `0` before each boot below. The
eight changed scripts were pushed to the clone individually (`incus file
push`, not a git fetch — this container's `origin` remote is a stale local
path, not GitHub) and their sha256 checked against this branch's own working
tree before running; `docker/qemu/run-x86-gate.sh`'s matched exactly
(`4cd053ed19882fbc6ef37e521ebc6b079a36d3ea507bd0418bebe03bad7a8b4b` on both
sides) — it was not edited again after this beast run, so that hash still
names the file this round commits.

Default run (`BREENIX_GATE_TMP` unset), `kthread` mode, count 1:

```
[gate] === Building (release, features=kthread_test_only) ===
[gate] Build clean (0 warnings) in 30s
[gate] === Running 1 boot test(s), mode=kthread ===
  Test 1: PASS
GATE: PASS (1/1 boot tests passed; mode=kthread build=30s boot=15s total=61s)
EXIT_CODE=0
```

Output landed at `/tmp/breenix_gate_1/` (`serial_kernel.log`,
`serial_user.log`, `stdout.log`), exactly where the pre-existing script wrote
before this change.

Custom run, same clone, immediately after:
`BREENIX_GATE_TMP=/root/gate-tmp-797-r157 ./docker/qemu/run-x86-gate.sh 1 kthread`:

```
[gate] Build clean (0 warnings) in 13s
[gate] === Running 1 boot test(s), mode=kthread ===
  Test 1: PASS
GATE: PASS (1/1 boot tests passed; mode=kthread build=13s boot=12s total=34s)
EXIT_CODE=0
```

`/tmp/breenix_gate_1` was snapshotted (`ls -la --time-style=full-iso`) before
and after this custom run:

```
BEFORE                                                       AFTER (unchanged)
serial_kernel.log  50257 2026-09-05 05:48:40.586259820 +0000  2026-09-05 05:48:40.586259820 +0000
serial_user.log      3100 2026-09-05 05:48:40.495259241 +0000  2026-09-05 05:48:40.495259241 +0000
stdout.log          10404 2026-09-05 05:48:28.404182343 +0000  2026-09-05 05:48:28.404182343 +0000
```

3 of 3 files carry the identical mtime before and after (only the shared
`/tmp` parent directory's own mtime moved, from something else on the
container — the same pattern the original (a)/(b) run above observed). This
run's own output landed at `/root/gate-tmp-797-r157/breenix_gate_1/` instead,
with fresh timestamps (`serial_kernel.log`/`serial_user.log` at 05:50:55,
`stdout.log` at 05:50:45) — confirming `BREENIX_GATE_TMP` redirects this
newly-converted script's output the same way it already did for the original
four, and that the default path is untouched when it is set (claim-lint:ok:
#797, the two `ls -la --time-style=full-iso` snapshots above are from this
round's own beast run, captured this session).

`case "$BREENIX_GATE_TMP" in /*) ;; ...` (the F6 guard, now in 8 of 8 changed
scripts) and the F7 socket-length guard were each exercised standalone on
this same clone: 8 of 8 scripts for F6, the 2 socket-using scripts for F7,
with `BREENIX_GATE_TMP` set to a relative path and to an 840-character
absolute path respectively — 10 of 10 checks rejected their bad input in
well under a second (before any build step ran) with the expected message,
e.g. (claim-lint:ok: #797, the two commands below and their output are from
this round's own beast run; the long constructed path is abbreviated with
`...` below for readability, not trimmed of substance — its reported length
is the real, unedited number):

```
GATE: FAIL (BREENIX_GATE_TMP must be an absolute path, got: relative/path)
x86 production-profile gate: FAIL (console socket path exceeds the AF_UNIX
sun_path limit of 107 chars: "/root/x/root/x/.../breenix_x86_prod_profile/
console.sock" is 878 chars -- shorten BREENIX_GATE_TMP)
```

(claim-lint:ok: #797, R157 round — 8/8 F6 rejections + 2/2 F7 rejections, all
beast-run above; the full per-script output is in this session's own record,
not re-pasted line-for-line here.)

## The R18 lesson (from the issue, not yet acted on here)

The issue's own "R18 lesson" section makes a second point beyond the path
collision this change fixes:

> A gate's **PASS** verdict was read off a directory, not off the lane's own
> build. ... the harness should also not trust "this directory contains a
> passing tally" on faith ... A verdict reader should tie a PASS to the lane's
> own build hash (e.g. embed or check the kernel ELF hash / a per-run token in
> the serial output it just produced) so a future collision fails loudly
> instead of silently scoring the wrong kernel as green.

`BREENIX_GATE_TMP` removes the *path* collision (two lanes writing the same
directory), which is what this issue's proposed fix addresses. It does not
add the identity check R18 describes: nothing here ties a `PASS` verdict to a
hash of the kernel that produced it, so two lanes that were (for whatever
other reason) still pointed at the same `BREENIX_GATE_TMP` value, or a caller
that forgets to set it at all in a concurrent-lane launcher, would still be
able to silently cross-score. That remains open work, matching the issue's
own framing of R18 as a lesson about the harness's verdict model in general,
not a defect this specific variable is meant to close (claim-lint:ok: #797 --
scope statement, not a proof claim; no identity check was added by this
change).

## Claim-lint

```
claim-lint: scripts/claim-lint.py -> exit 0
```
