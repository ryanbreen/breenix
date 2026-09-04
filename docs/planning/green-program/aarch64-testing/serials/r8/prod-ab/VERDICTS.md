# R7-003 attribution: the production gate, branch head against `main`

Review finding R7-003 was a single production-profile failure in the reviewer's
own 3-boot smoke at `7d0ac41f` — `FAIL: Poll TCP oracle marker missing` — against
a documented 3/3. Round 8 was told to attribute it by running the same gate on
`main` and on the head, alternating, two concurrent.

## Method

`docker/qemu/run-aarch64-prod-profile-boot-test.sh`, 8 invocations per arm, in 8
rounds of two: one `main` boot and one head boot started together and waited on
together, so both arms see the same host load. Each invocation builds its own
no-`--features` kernel in its own worktree and boots once with a 120 s window.

* head arm: `fix/562-761-aarch64-testing-profile` @ `7d0ac41f` plus round 8's
  own commits, in this worktree.
* `main` arm: a separate worktree at `d6b7a186`, built from scratch (its own
  userspace + ext2 fixture).
* Both arms ran the branch copy of the gate script. That script and
  `d6b7a186`'s were byte-identical before round 8; round 8's only change to it
  makes the output and failure directories overridable so two arms can run at
  once (`BREENIX_PROD_PROFILE_OUTPUT_DIR`, `BREENIX_PROD_PROFILE_FAILURE_DIR`).
  The kernel is the variable.

Runner: `run.sh` (committed beside this file). Raw per-invocation stdout:
`logs/` (16 files). All 16 boots' serials, pass or fail, are in `serials/`.
claim-lint:ok: 16 of 16 boots, 16 logs and 16 serials.

## Result

| round | `main` @ d6b7a186 | head |
|---|---|---|
| 1 | PASS | PASS |
| 2 | PASS | FAIL: seam-absent timeout marker count must be exactly one |
| 3 | PASS | PASS |
| 4 | PASS | FAIL: seam-absent timeout marker count must be exactly one |
| 5 | PASS | FAIL (verbatim gate text): bsshd did not reach its listening state |
| 6 | PASS | FAIL (verbatim gate text): bsshd did not reach its listening state |
| 7 | PASS | FAIL: seam-absent timeout marker count must be exactly one |
| 8 | PASS | PASS |

**`main` 8/8. Head 3/8.** `exitcodes.txt` holds the 16 exit statuses.

claim-lint:ok: 8 of 8 and 3 of 8, `exitcodes.txt` and the 16 logs in `logs/`.

## The R7-003 signature itself

`FAIL: Poll TCP oracle marker missing` appeared **0 times on either arm** in
these 16 boots. It did reproduce on the head in the same session's R13 smoke —
`../smoke/prod-boot2-run-log.txt`, one of 3 boots — and it appears once in the
round-6 25-boot branch battery (boot 21). It appears in 0 of the 8 `main` boots
here and in 0 of the 8 `main` boots of round 6's baseline.

So R7-003 is not a pre-existing flake of the gate: it is one low-rate member of
the branch's production-profile degradation, which this A/B measures directly.

claim-lint:ok: 0 of 16 A/B boots, 1 of 3 smoke boots, 1 of 25 round-6 branch
boots, 0 of 16 `main` boots across both rounds.

## What the failures look like

0 of the 5 head failures here carries a crash marker, an
`EXT2_LOCK_SPIN_STALL` line, or a `!!! SOFT LOCKUP DETECTED !!!` dump: `grep -ac`
returns 0 for each pattern on each of them, one of which is
`docs/planning/green-program/aarch64-testing/serials/r8/prod-ab/serials/head_2-serial.txt`. The dominant shape is that
guest output **stops entirely** a moment after an init `[spawn]` line or a child
exit, while the gate waits out its remaining ~113 s:

| serial | last guest uptime | last line |
|---|---|---|
| `head_2-serial.txt` | 553 ms | `[syscall] exit(0) pid=4 name=block_eintr_oracle_child_4` |
| `head_4-serial.txt` | (before the first heartbeat) | `[spawn] path='/bin/heartbeat'` |
| `head_6-serial.txt` | 8528 ms | `CLONEVM_EXEC_TEST: child exited` |
| `head_7-serial.txt` | 580 ms | `[spawn] path='/bin/block_eintr_oracle'` |
| `head_5-serial.txt` | 119319 ms | heartbeats continued for the whole window; the service chain stopped advancing at `clonevm_exec_test` |

The same shape holds for the smoke's two failures (`../smoke/`): boot 1 stops at
1734 ms after `[spawn] path='/bin/block_eintr_oracle'`, boot 2 at 6657 ms right
after `[spawn] path='/bin/poll_tcp_oracle'` — the R7-003 signature is that
second one, i.e. the oracle it names is simply the service the boot happened to
be spawning when everything stopped.

Two things that follow from the bytes, and one that does not:

* **It is not #728.** The ext2 spin-stall printer is not feature-gated, and 0 of
  the 19 serials in this directory and the smoke's contain a single
  `EXT2_LOCK_SPIN_STALL` line.
* **The aarch64 soft-lockup watchdog fired in 0 of the 5**, although it
  is compiled into this profile (`kernel/src/arch_impl/aarch64/timer_interrupt.rs`).
  Its metric is context-switch and syscall progress, so either progress
  continued among threads with no output, or the timer stopped.
  claim-lint:ok: 0 of 5 failing serials contain `SOFT LOCKUP DETECTED`.
* **What actually wedges is not established here.** This directory measures the
  degradation and its shape; it does not name the defect.

claim-lint:ok: 0 `EXT2_LOCK_SPIN_STALL` and 0 `SOFT LOCKUP DETECTED` lines in
19 of 19 serials (`serials/` here plus `../smoke/prod-boot*-serial.txt`).
