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
change: both take serial-log paths as positional arguments rather than
constructing any path themselves, so a caller that threads `$OUTPUT_DIR`
(itself now built from `$BREENIX_GATE_TMP`) into the verdict script's argv
already has producer and consumer agreeing on the same base — there is no
second hardcoded literal for them to drift out of sync with. This is the
"pass the same base to the verdict scripts" requirement from the issue: it is
satisfied by construction here because the verdict scripts never had a base of
their own (claim-lint:ok: #797, 0/2 hits from the grep above).

The `#797` issue text itself scopes the two scripts that actually collided in
production (`run-x86-boot-tests.sh`, `run-boot-parallel.sh`) as the required
fix and names the remaining `/tmp/breenix_*` sites across the tree (aarch64
gates, single-shot x86 gates like `run-x86-gate.sh` and
`run-x86-tty-oracle-gate.sh`, `run-vmware-gate.sh`, forensic/debug scripts)
as candidates for the same treatment, not proposed there. This change also
covers `run-x86-prod-profile-boot-test.sh` and `run-kthread-parallel.sh`
because both share the identical hazard — a literal `/tmp/breenix_*`
`OUTPUT_DIR` a concurrent lane on the same beast container can `rm -rf` and
recreate — and both were named for this round. The aarch64 gate scripts were
surveyed (`grep -n "/tmp/breenix" docker/qemu/*.sh`) but left out of this
round: they run on a per-developer Parallels VM or native Mac QEMU, not the
shared beast container two concurrent clones write into, so the concurrent-
lane collision this issue documents does not apply to them the same way; two
of them (`run-aarch64-service-sequence-gate.sh`,
`run-aarch64-refusal-drain-gate.sh`) already accept a full `OUTPUT_DIR`
override via their own env var. Widening the aarch64 scripts to the same
`BREENIX_GATE_TMP` convention remains a candidate follow-up, consistent with
the issue's own scoping.

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
