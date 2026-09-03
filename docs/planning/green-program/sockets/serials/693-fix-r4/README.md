# Round 4 battery notes -- `fix/693-poll-wake-loss` @ `f7633147`

Ran the 4 batteries specified for this round (2 boot counts, 1 mutation +
revert, 1 structure-test pair, 1 family run), in an isolated worktree
(`/Users/wrb/fun/code/breenix/.claude/worktrees/wf_c7e84095-70f-3`, detached
at `f7633147`, checked out from `origin/fix/693-poll-wake-loss`). Fresh setup
per the round-3 prove notes' own record of the steps: `rust-fork` symlink
created, aarch64 userspace built (`userspace/programs/build.sh --arch
aarch64`, exit 0, 146 binaries), `target/ext2-aarch64.img` built
(`scripts/create_ext2_disk.sh --arch aarch64`, exit 0).
<!-- claim-lint:ok: the 4 batteries are §1-§4 below, each with its own
     command and count. -->

## 1. `run-aarch64-percpu-stack-custody-gate.sh`, 3 clean boots

3 of 3 exit 0, `ARM64 PERCPU STACK CUSTODY GATE: PASSED`. Counts, `grep -c`
on each committed serial under
`docs/planning/green-program/sockets/serials/693-fix-r4/`:

| boot | `[POLL_TCP_TIMEOUT]` | `[POLL_TCP_READY_LOST]` | `[POLL_TCP_ORACLE:FAIL` |
|---|---|---|---|
| 1 (`aarch64-percpu-gate-clean-boot1-serial-20260903.txt`) | 2 | 0 | 0 |
| 2 (`aarch64-percpu-gate-clean-boot2-serial-20260903.txt`) | 1 | 0 | 0 |
| 3 (`aarch64-percpu-gate-clean-boot3-serial-20260903.txt`) | 2 | 0 | 0 |

Boot 3's serial reaches the oracle's own terminal verdict --
`[POLL_TCP_ORACLE:PASS:stages=4:idle_ms=138:late_ms=83:park_ms=82:forced_ms=155:forced_late_by_ms=350]`
-- which matters for §2 below: on a correct kernel the Race-mode trial
(`late_ms=83`) resolves in 83 ms, not anywhere near its 5-second budget.

## 2. Mutation 693-K, 2 boots -- 0 of 2 matched the round's stated expectation

Applied by hand at `kernel/src/syscall/handlers.rs:4156`
(`ready_count = scan_fds(&mut pollfds, &fd_snapshots);`, the loop's only
post-wake re-scan), matching the round-3 FIX doc's own description
(`docs/planning/green-program/sockets/693-FIX-2026-09-02.md:415-417`). Built
clean (the only diagnostic is the expected `unused_mut` warning on
`ready_count`, confirming the mutation compiled into the tested binary), 2
boots run.

**Both boots: exit 0, `ARM64 PERCPU STACK CUSTODY GATE: PASSED`, 0 of 2
`[POLL_TCP_READY_LOST]`, 0 of 2 `[POLL_TCP_ORACLE:FAIL`.** The task's stated
expectation ("the gate must FAIL on `[POLL_TCP_READY_LOST]` with 0 oracle
FAILs") held on 0 of the 2 attempts run
(`aarch64-percpu-gate-mutK-boot1-serial-20260903.txt`,
`aarch64-percpu-gate-mutK-boot2-serial-20260903.txt`, both under
`docs/planning/green-program/sockets/serials/693-fix-r4/`). A red is a
result; this one came back green when a red was expected, which is itself
the reportable fact -- recorded here rather than re-run until it matched the
prediction.

**Why, traced to source rather than guessed.** Both mutated serials end at
the identical point, mid-oracle: the stage-3 (Race-mode) peer's own
`[POLL_TCP_ORACLE:PEER:branch=sent:...]` line, its `exit(0)` reap, and one
more heartbeat -- 680 of 680 lines in each of the 2 files, with 0 of 2
containing a second `[POLL_TCP_TIMEOUT]`, a `LOSTWAKE_PROBE`, or a
`LATE_PUBLISH` line, all three of which appear by the equivalent point in
each of the 3 clean-boot serials in §1. `stage_late`'s parent poll
(`userspace/programs/src/poll_tcp_oracle.rs:1202`, `LATE_TIMEOUT_MS =
5_000` at `:186`) is the trial the peer line belongs to (`late_peer` forks
from both `stage_late` and `stage_forced_late` via the shared
`run_late_trial`, but `run()` at `:1238-1244` calls `stage_late` first, and
only `LATE_TIMEOUT_MS`, not the 150 ms `FORCED_TIMEOUT_MS`, is 5000). Under
the mutation, that poll's `ready_count` is never updated after the initial
pre-loop scan, so it cannot observe the peer's mid-window publish and has to
run its own 5-second timeout out fully before the kernel prints anything.

The gate script's own polling loop
(`docker/qemu/run-aarch64-percpu-stack-custody-gate.sh:146-150`) breaks --
and its `trap cleanup EXIT` handler (`:58-60`) then `kill`s QEMU -- once its
8 required conditions (the 4 leg markers, `BOOT_TESTS:PASS`, `ALIEN`,
`BLOCK_EINTR_ORACLE`, and `POLL_TCP_ORACLE_LINE`) are 8 of 8 non-empty.
`POLL_TCP_ORACLE_LITERAL` (`:33`) is the bare prefix `'[POLL_TCP_ORACLE:'`,
satisfied by the very first line the oracle program ever prints (here, the
stage-3 peer's own `PEER:branch=sent` line) -- not by the oracle's terminal
`PASS`/`FAIL` verdict. The other 7 of those 8 conditions are already true by
the time the oracle starts (boot-test-suite completion, at line ~590-600 of
the clean-boot serials, precedes the oracle's first output at line ~676-679
in the same files), so this gate's own patience window past that point is
bounded by roughly one `sleep 1` cycle (the loop's per-iteration check
interval, `:154`), not by the oracle's completion.

That ~1-2 s window is short against the ~5 s the mutated `stage_late` trial
needs to legitimately give up and let the kernel print
`[POLL_TCP_READY_LOST]`. Boot 3 in §1 is the direct comparator: on a correct
kernel the same trial resolves in 83 ms, comfortably inside the harness's
patience window, which is why the 3 of 3 clean boots in §1 pass (1 of 3
reaching the oracle's full `PASS` line, as boot 3 did; 2 of 3 only as far as
its first output, as boots 1-2 did, matching the shape both mutated boots
show) -- not because the `[POLL_TCP_READY_LOST]` check ran and found
nothing, but because on this specific mutation this gate's own timing gives
that check no opportunity to run to the point the marker would appear. This
does not reopen F1: the wiring itself (grep literals present, R96
census-clean per §3) is correct and would catch the marker if it appeared,
and F1's own round-3 repro used the service-sequence gate's specimen, not
this gate's. It is a separate, pre-existing characteristic of this one
gate's presence-only polling loop that this round's battery surfaces for the
first time against this specific mutation on this specific gate; whether it
is shared by any of the other 7 gate scripts the R96 census in §3 covers was
not investigated here, out of scope for this round's ask.

Reverted the mutation (`git diff kernel/src/syscall/handlers.rs` against
head is empty afterward, confirmed via `git status --short
kernel/src/syscall/handlers.rs` producing 0 lines of output); 1 confirmation
boot: exit 0, PASSED, `TIMEOUT=2 READY_LOST=0 ORACLE_FAIL=0`
(`aarch64-percpu-gate-revert-clean-boot-serial-20260903.txt`).

## 3. The R96 structure ratchet -- GREEN at head, RED on removal

At head: `cargo test --test poll_tcp_gate_wiring_structure -- --nocapture`
-- 3 of 3 tests GREEN, `R96 census: 8 script(s) assert
"POLL_TCP_ORACLE:FAIL"`.

Negative check, in a scratch copy (tracked files only, via `git ls-files`
piped to `rsync --files-from`, 113 MB, plus the `rust-fork` symlink and a
fresh `cargo test` build -- `CARGO_MANIFEST_DIR` is a compile-time constant,
so the branch worktree's already-compiled test binary cannot be pointed at a
different tree; the same test had to be rebuilt inside the copy): removed
the literal `POLL_TCP_READY_LOST` from
`docker/qemu/run-aarch64-refusal-drain-gate.sh` in the copy only (its
`POLL_TCP_ORACLE:FAIL` occurrence, and the census count of 8, both left
untouched in that same file). Re-ran the same test inside the copy:
**1 of 3 tests RED**, naming the file exactly --

```
1 of 8 script(s) asserting the oracle's "POLL_TCP_ORACLE:FAIL" verdict do
not also require the kernel's "POLL_TCP_READY_LOST" marker (R93):
docker/qemu/run-aarch64-refusal-drain-gate.sh.
```

Scratch copy deleted afterward (`rm -rf`); `git status --short` run in the
branch worktree immediately after returned 0 of 0 lines referencing
anything under `docker/qemu/` or `scripts/`, confirming the branch
worktree's own `run-aarch64-refusal-drain-gate.sh` matches the committed
bytes at `f7633147` throughout this step.

## 4. Full `tests/*_structure.rs` family, once

25 of 25 files, `cargo test --test <name> ...` for all 25 names in one
invocation, exit 0. 499 of 499 tests passed, 0 failed, summed from each
file's own `test result:` line (12+97+5+4+5+4+4+4+42+6+36+10+14+63+4+9+19+3+8+9+2+38+6+81+14
= 499).

## Claim-lint

```
claim-lint: python3 scripts/claim-lint.py                                -> exit 0
claim-lint: python3 scripts/claim-lint.py --files <this file>            -> exit 0
```
