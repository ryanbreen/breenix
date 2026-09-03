# #697 census-pin gate evidence, 2026-09-02

<!-- claim-lint:ok: every N-of-M count and evidence path in this document
     names a file in this same directory (or a grep line number inside one),
     cited in the paragraph that makes the claim. -->
This is the prove-slot record (2 of 2 unpatched-main boots red, 5 of 5 branch
boots green, 1 of 1 mutation boot red, 2 of 2 revert-verification boots
attributed -- full breakdown below) for branch `fix/697-census-pin`, head
`907f096a` (author commit
already on the branch when this round started; this document adds no code
change, only gate evidence). Scope: `docker/qemu/run-x86-boot-tests.sh`'s
`PRODUCTION_REAPED_ROWS` pin, which was a frozen literal (`4`) until commit
63e5f8e0 (PR #765, the #707 test `tcp_cloexec_exec_test`) added a
RING3_SMOKE-roster process that forks a peer and `waitpid()`s it, moving the
true reaped-row count to 5 without moving the pin. The fix at `907f096a`
derives the pin from the RING3_SMOKE roster in `kernel/src/main.rs` instead
(#697 item 2, shape (b) of the fix notes). This round runs the unpatched-main
bytes, the fixed branch, and a deliberate mutation of the derivation, each on
beast (`breenix-x86` Incus VM), and preserves every boot's serials under this
directory. No kernel source changed in this round.

## Method

Two fresh scratch clones on beast (`/root/breenix-697-prove-main` at
`509802e5`, `/root/breenix-697-prove-branch` at `907f096a`), both cloned from
`https://github.com/ryanbreen/breenix.git`, neither derived from the host's
own `/root/breenix` checkout. `docker/qemu/run-x86-boot-tests.sh`'s own
`set -e` + ERR-trap design aborts the whole script on the first failing
assertion inside its `for i in 1..COUNT` loop, so a single invocation with
`COUNT=N` cannot yield N independently-classified boots once any boot in the
sequence reds -- confirmed directly: the first `run-x86-boot-tests.sh 2`
invocation against unpatched main stopped after boot 1's `:548` assertion
(`main-unpatched/boot1-gate.txt:426`) and left `/tmp/breenix_x86_boot_tests_2`
untouched (its mtime stayed at a stale prior run, `2026-09-02 21:23:35`,
while boot 1's directory carried the fresh `01:48:41` timestamp -- both
checked live with `stat` before re-running boot 2 on its own, see
`main-unpatched/boot2-gate.txt`). Each boot below is therefore its own
separate
`run-x86-boot-tests.sh 1` invocation, each boot's `serial_kernel.txt` /
`serial_user.txt` / `qemu.txt` / `-gate.txt` copied out to its own subdirectory
here before the next boot starts.

## Unpatched main, `509802e5` -- `main-unpatched/`

2 of 2 boots red, both at the exact assertion #697 names:

| boot | resident | removed | resident+removed-2 | old literal | result |
|---|---|---|---|---|---|
| 1 | 0 | 7 | 5 | 4 | FAIL `run-x86-boot-tests.sh:548` |
| 2 | 1 | 6 | 5 | 4 | FAIL `run-x86-boot-tests.sh:548` |

Both boots fail the identical line and command --
`main-unpatched/boot1-gate.txt:426-427` and
`main-unpatched/boot2-gate.txt:407-408`:
```
x86 frame-custody gate run 1: FAIL (set -e abort at ./docker/qemu/run-x86-boot-tests.sh:548, exit 1)
  failing command: test "$(( CENSUS_RESIDENT + CENSUS_REMOVED - TOMBSTONE_FIXTURE_REMOVALS ))" -eq "$PRODUCTION_REAPED_ROWS"
```
with the census lines at `main-unpatched/boot1/serial_user.txt:1006`
(`[TOMBSTONE_CENSUS:resident=0:removed=7:...]`) and
`main-unpatched/boot2/serial_user.txt:997`
(`[TOMBSTONE_CENSUS:resident=1:removed=6:...]`). Both boots' true reaped-row
count is 5 (`resident + removed - TOMBSTONE_FIXTURE_REMOVALS`, invariant
across the split per the script's own comment), against the old frozen
literal `PRODUCTION_REAPED_ROWS=4` -- neither split can pass that pin,
matching #697's own claim that this is deterministic, not a rate. No #692 or
#731 signature appears in either boot's log; both reds are the census
assertion alone.

## Branch `fix/697-census-pin`, `907f096a` -- `branch/`

5 of 5 boots PASS:

| boot | resident | removed | resident+removed-2 | result | log:line |
|---|---|---|---|---|---|
| 1 | 1 | 6 | 5 | PASS | `branch/boot1-gate.txt:566` |
| 2 | 1 | 6 | 5 | PASS | `branch/boot2-gate.txt:427` |
| 3 | 0 | 7 | 5 | PASS | `branch/boot3-gate.txt:427` |
| 4 | 0 | 7 | 5 | PASS | `branch/boot4-gate.txt:427` |
| 5 | 0 | 7 | 5 | PASS | `branch/boot5-gate.txt:427` |

The script does not echo `PRODUCTION_REAPED_ROWS` itself, so its derived
value is read off the arithmetic instead: `resident + removed - 2` equals 5
on 5 of 5 boots above, and the `:548`-shaped assertion (shifted to line ~607
by the fix's added comment lines) passed on that same 5 of 5, so the
derivation resolved to 5 regardless of which side of the reap/retire race
the sample landed on. That matches the fix notes'
independent derivation, re-checked directly against this worktree before the
round started: `grep -cE 'match (process::)?fork\(\) \{'` against each of
the 15 RING3_SMOKE roster files (`kernel/src/main.rs`, the block bounded by
the "canonical list of test binaries" comment and the following
`without_interrupts(|| {`) finds 4 sites in
`userspace/programs/src/loopback_wake_test.rs`, 1 in
`userspace/programs/src/tcp_cloexec_exec_test.rs`, and 0 in the other 13,
summing to 5. Neither #692 (`reader_exit_15`) nor #731 fired in this 5-boot
sample; a 5-boot sample is not evidence either intermittent is fixed, only
that they did not land in this run (`#692` was reproduced by-catch in this
same round -- see the mutation-revert section below).

## Mutation -- `mutation/`

Per the fix's own scope discipline: redden the derivation by claiming one
more forking roster process than actually launches, matching
`mutation/mutation-diff.txt` and `mutation/apply-mutation.sh` (both
committed here verbatim). Applied against the `907f096a` clone with
`git checkout --` confirmed clean immediately beforehand:

```diff
+PRODUCTION_REAPED_ROWS=$(( PRODUCTION_REAPED_ROWS + 1 ))  # MUTATION-697: redden the pin (prove slot)
 readonly PRODUCTION_REAPED_ROWS
```

**Fired, 1 boot** (`mutation/fired/`): derived value becomes 6, the boot's
own census (`mutation/fired/serial_user.txt:1010`,
`resident=0:removed=7`, sum 5) does not move, and the assertion fails --
`mutation/fired-gate.txt:407-408`:
```
x86 frame-custody gate run 1: FAIL (set -e abort at ./docker/qemu/run-x86-boot-tests.sh:608, exit 1)
  failing command: test "$(( CENSUS_RESIDENT + CENSUS_REMOVED - TOMBSTONE_FIXTURE_REMOVALS ))" -eq "$PRODUCTION_REAPED_ROWS"
```
Same assertion, same failing command, as the unpatched-main reds above --
confirming the mutation reddens the exact check the fix repairs, not some
unrelated line.

**Reverted**: `git checkout -- docker/qemu/run-x86-boot-tests.sh`
(`mutation/revert-mutation.sh`, committed here verbatim) followed
immediately by `git status --porcelain` and `git diff --stat` on that one
file, both empty -- run live on beast, not merely asserted.

**Revert verification, 2 boots** (`mutation/revert-attempt1-692intermittent/`,
`mutation/revert-clean/`): the first post-revert boot hit the pre-existing
`#692` intermittent --
`mutation/revert-attempt1-692intermittent-gate.txt:407-408` and
`mutation/revert-attempt1-692intermittent/serial_user.txt:996`:
```
x86 frame-custody gate run 1: FAIL (set -e abort at ./docker/qemu/run-x86-boot-tests.sh:513, exit 1)
  failing command: test "$passed" = true
[TEST:userspace:loopback_recv_wake:FAIL:reader_exit_15]
```
This is `#697`'s own named `#692` signature (`reader_exit_15`), not the
census assertion -- the poll loop broke on the userspace-test `:FAIL` marker
before `passed` was ever set true, and the boot did not reach line 608 at
all (see `mutation/revert-attempt1-692intermittent-gate.txt`, which has no
line 608 in it), so the revert is not implicated in this red. Preserved rather than
discarded per this round's own "a red is a result" instruction, then
re-run once more: `mutation/revert-clean-gate.txt:427` is a plain
`x86 frame-custody gate run 1: PASS`, and `git status --porcelain` /
`git diff --stat` on `docker/qemu/run-x86-boot-tests.sh` were both empty
immediately before this boot too (same revert, checked again).

## Unattributed count: 0

10 boots run this round (2 unpatched-main + 5 branch + 3 mutation-phase), and
all 10 of 10 reds and passes below are attributed to a named cause: 2 of 10
to the #697 census-pin defect (unpatched main `main-unpatched/boot1-gate.txt`,
`main-unpatched/boot2-gate.txt`, matching the exact `:548` assertion and
failing command that also fires at `:608` on the mutated branch,
`mutation/fired-gate.txt`), 1 of 10 to the deliberate mutation itself
(expected, not a defect), 1 of 10 to the pre-existing `#692` `reader_exit_15`
intermittent (`mutation/revert-attempt1-692intermittent-gate.txt`), and the
remaining 6 of 10 (5 of 5 branch boots plus 1 post-revert boot,
`mutation/revert-clean-gate.txt`) are plain PASS.
