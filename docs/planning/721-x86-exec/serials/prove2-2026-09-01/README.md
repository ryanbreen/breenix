# #721 prove-slot round 2 -- leg 3/4 (B2 runtime coverage + beast gates)

Branch `fix/721-x86-exec` @ `1ec898f1` (fix2-721's `aa5f0fd8` plus this
round's own commit repairing a stale gate literal -- see below).

## B2 runtime coverage -- the x86 CLONE_VM sibling refusal, observed live

`docker/qemu/run-x86-boot-tests.sh` run 1 of 5 (beast, `breenix-x86` Incus
VM), serial confirms the refusal fires on **both** x86 exec bodies:

```
kernel::process::manager: exec_process: rejecting exec for PID 86 while
CLONE_VM sibling PID 87 thread 168 still holds inherited CR3 0x49a0000
kernel::process::manager: exec_process_with_argv: rejecting exec for PID 88
while CLONE_VM sibling PID 89 thread 170 still holds inherited CR3 0x49a0000
```

`[EXEC_DETACH_ORACLE:x86:bodies=2:...:sibling_refused=2:...]` -- the guard
added to x86 `exec_process` in fix2-721 (B2) is genuinely exercised at
runtime, not just compiled.

## Gate-literal bug found and fixed this round (commit `1ec898f1`)

Discovering the above also surfaced that `docker/qemu/run-x86-boot-tests.sh`'s
`EXEC_DETACH_ORACLE_LITERAL` and `tests/teardown_structure.rs`'s matching
`EXEC_DETACH_ORACLE_VECTOR`/harness census (two sites) still hardcoded the
**pre-B2** value `sibling_refused=0` -- fix2-721 correctly updated the
in-kernel verdict check (`teardown.rs:3705`, `sibling_refused != 2`) but
missed these two other copies of the same literal. Both agreed with each
other (both stale) so the host census gave a false green, and the beast gate
script's 900s poll loop could never match a string that no longer appears.
Fixed: both literals updated to `sibling_refused=2`, matching the measured
live value above. Re-verified green: `teardown_structure` 79/81 (same 2
pre-existing unrelated failures), `exec_lock_order_structure` 42/42,
`tty_oracle_structure` 16/16.

## `run-x86-boot-tests.sh` x5 (post-fix, head `1ec898f1`)

4/5 PASS. Run 5 hit a pre-existing, previously-documented, previously-measured
signature -- **not a regression from this branch**.

- Run 1: PASS (`x86 frame-custody gate run 1: PASS`), including the sibling
  refusal confirmed above.
- Run 2: PASS.
- Run 3: PASS.
- Run 4: PASS.
- Run 5: FAILED at `run-x86-boot-tests.sh:454` (`test "$passed" = true`) --
  the 900-second poll loop expired before every required marker was observed.

### Run 5 is issue #716, recurring (exact signature match)

Verified directly against the preserved serials in this directory
(`run5-716-serial_kernel.txt`, 18,673 lines; `run5-716-serial_user.txt`, 953
lines):

- `TEST_TALLY: exited=107 nonzero=0 failed=[]` -- present, clean.
- `TEST RUNNER: All tests passed` -- present.
- `TOMBSTONE_QUIESCE:` -- **0 occurrences**.
- `RECLAIM_DRAIN:` -- **0 occurrences**.
- `KSTACK_QUIESCE_LEAK:` -- **0 occurrences**.
- `TOMBSTONE_CENSUS:` (the sibling marker that normally does print alongside
  those three) -- **0 occurrences**.
- `panic`/`PANIC` -- **0 occurrences** in either file.
- After "All tests passed", the serial shows nothing but idle-loop churn
  (`Next thread from queue: 1, cpu: 0` / `Idle thread 1 is alone, continuing`)
  for the remainder of the capture.

This is character-for-character the signature documented and measured in
`docs/planning/green-program/x86-prod/serials/prove-673-leg3-boot4-667-recurrence.md`
and `docs/planning/green-program/tty/EVIDENCE-x86-confirm-2026-08-31.md`,
filed as **#716** (a recurrence of the retracted #667): the post-test
settle-census stall where `TOMBSTONE_QUIESCE`/`RECLAIM_DRAIN`/
`KSTACK_QUIESCE_LEAK` never emit after all userspace tests pass, with a
previously-measured baseline rate of ~6.7% on unmodified `origin/main` and
~10% on a different branch (both small-sample). This round's own rate,
1/5 = 20%, is on the high end but within noise for n=5.

### Branch causation excluded on mechanism, not diff-absence alone

`git diff origin/main...HEAD --stat` shows this branch touches
`kernel/src/task/scheduler.rs` and `kernel/src/task/thread.rs` -- two of the
files #716's own writeup warns a diff-only argument cannot dismiss (they are
exactly the scheduler-adjacent files #673's diff touched when #716 was
originally mis-excluded that way). So this round inspected the actual diff
rather than stopping at file-list absence: every change in both files is
confined to the exec-scheduler-commit path --
`ExecSchedCommit`/`EXEC_SCHED_COMMITS`/`SCHED_AFTER_PM_VIOLATIONS`/
`EXEC_COMMIT_UNPINNED`/`EXEC_COMMIT_MISSING_THREAD` losing their
`#[cfg(target_arch = "aarch64")]` gates so the type compiles for x86 too, a
field rename (`new_ttbr0` -> `new_page_table_root`), the `ret_zero_pc_oracle_exec`
cfg gaining an explicit `target_arch = "aarch64"` clause, and
`Thread::clear_inline_schedule_state` losing its aarch64-only gate (its body
is unchanged; it is just reachable, and a no-op in practice, on x86 now). None
of this touches `idle_loop()`, the tombstone quiesce/reclaim-drain settle
mechanism, or anything on the boot's steady-state idle path -- the code
`ExecSchedCommit::apply` and `clear_inline_schedule_state` run only during an
`exec()` syscall, never during idle-loop settle. Mechanism review agrees with
the diff-absence signal here; both point away from this branch.

**Attribution: run 5 is ATTRIBUTED to pre-existing #716, not counted as
UNATTRIBUTED.**
