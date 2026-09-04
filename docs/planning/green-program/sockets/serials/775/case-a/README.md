# Case A — both censuses read from one boot, at the pre-removal commit

Case A is the only place the old and the new census can be compared on the
same boot: `29344251` still compiles the three dispatch records the old script
parses, and `5b419714` overlays this round's heartbeat on top of it without
touching them. `git diff --quiet 5b419714 365c20c2` is true for
`kernel/src/task/dispatch_strand_census.rs`, `kernel/src/task/mod.rs`,
`kernel/src/main.rs`, `kernel/src/net/loopback_pump.rs`,
`kernel/src/syscall/handlers.rs`, `scripts/x86-strand-census.sh` and
`scripts/x86-gate-verdict.sh` — 7 of 7 paths — so the overlay runs this
branch's census code, not an approximation of it.

Two legs live here.

| leg | mutation | boots | strands observed |
|---|---|---:|---:|
| `historical-wedge/` | `bare-hlt-wedge.patch`, byte-identical to the one `../case-b-post-removal-mutation/` was booted with | 7 | 0 |
| `deterministic-strand/` | `mutation-E.patch`, 14 lines in `wake_expired_timers` | 3 | 3 |

## Leg 1 — the historical wedge did not reproduce here

`historical-wedge/` is `07fa248b` = `5b419714` plus the same
`interrupts::disable()` + `hlt()` patch that wedged the post-removal head in
`../case-b-post-removal-mutation/` (`diff` of the two committed
`bare-hlt-wedge.patch` files is empty). It was run as
`run-x86-gate.sh 3 full` followed by `run-x86-gate.sh 4 full`.

| boot | old saved | new saved | old stranded | new stranded | old rc | new rc | gate verdict |
|---:|---:|---:|---:|---:|---:|---:|---|
| 1 | 11 | 11 | 0 | 0 | 0 | 0 | PASS |
| 2 | 11 | 11 | 0 | 0 | 0 | 0 | PASS |
| 3 | 11 | 11 | 0 | 0 | 0 | 0 | PASS |
| 4 | 11 | 11 | 0 | 0 | 0 | 0 | PASS |
| 5 | 11 | 11 | 0 | 0 | 0 | 0 | PASS |
| 6 | 11 | 11 | 0 | 0 | 0 | 0 | PASS |
| 7 | 11 | 11 | 0 | 0 | 0 | 0 | FAIL — `failing process is not allowlisted: clock_gettime_test` |

7 of 7 agree on `threads_saved_blocked`, `stranded`, `lines=` and rc. What
they agree on is the green reading: this wedge fired on 0 of these 7 boots.

That is a disclosure, not a result. The same mutation at the same commit
*did* wedge once during the round-2 implementation run, before this slot took
over: a gate transcript on the beast VM at 07:30 UTC records head `07fa248b`,
`STRAND_CENSUS: threads_saved_blocked=10 stranded=5 lines=4714` and five named
threads. That boot's serials were overwritten by later runs and are not
committed, so nothing here rests on it. Counting it, the wedge fired on 1 of 8
boots at this commit. The same patch fired on the post-removal head on the
boot committed under `../case-b-post-removal-mutation/`, and passed green on
two earlier post-removal attempts at a sibling scratch commit. It is a race,
and it is not a usable oracle for a paired comparison.

## Leg 2 — a mutation that strands by construction

`deterministic-strand/` replaces the race with something that cannot not fire.
`mutation-E.patch` adds 14 lines to `Scheduler::wake_expired_timers` in
`kernel/src/task/scheduler.rs`: after the existing stale-entry check, an x86
arm drops the wake for any thread whose `blocked_in_syscall` flag is set. Such
a thread has had its kernel context saved, is not on the ready queue, and now
has no waker — the definition of the fault both censuses exist to detect,
produced on purpose rather than waited for.

3 boots, `run-x86-gate.sh 3 full`, 3 of 3 `GATE: FAIL` on the strand census:

| boot | old saved | new saved | old stranded | new stranded | old rc | new rc |
|---:|---:|---:|---|---|---:|---:|
| 1 | 11 | 10 | 24, 26, 36 | 21, 24, 25, 26, 28, 36 | 1 | 1 |
| 2 | 11 | 10 | 24, 26, 36 | 21, 24, 25, 26, 28, 36 | 1 | 1 |
| 3 | 11 | 10 | 24, 26, 36 | 21, 24, 25, 26, 27, 36 | 1 | 1 |

Both censuses fail the boot on 3 of 3, and the new one names its threads. They
do not report the same numbers, and the next section attributes each of the 4
differences to a record in the capture.

### Where the difference comes from

`deterministic-strand/divergence-analysis.txt` carries, for each of the 3
boots, both census outputs, the snapshots COM1 received in order, and a
per-TID record history read from `deterministic-strand/boot1/serial_kernel.txt`
and its two siblings alone, so its line numbers are one timeline. Boot 1's
table:

```
tid    saves    restores first_save  last_save   last_rest   exit_line
21     9        9        2065        4193        4197        4369
24     5        4        2276        3396        3388        -1
25     148      148      2457        4626        4654        4663
26     1        0        2237        2237        -1          -1
28     111      111      2070        4063        4067        4145
36     1        0        2188        2188        -1          -1
38     1        1        4602        4602        4624        4653
```

- Threads 24, 26 and 36 are the ones mutation E parked for good: 26 and 36
  have 1 save and 0 restores, 24's last save at line 3396 follows its last
  restore at 3388, and none of the three has an exit line. Both censuses name
  all three, on 3 of 3 boots.
- Threads 21, 25 and 28 are the three the new census adds. Each has an equal
  save and restore count, a restore after its last save, and an exit line
  (4369, 4663, 4145). They were parked at the instant of the last snapshot and
  resumed after it.
- Thread 38 is why old `saved=11` and new `saved=10`: its first save is at
  line 4602 of a 4669-line capture, and 0 of the 16 snapshots were taken after
  it, so the ledger carried 10 ever-saved slots rather than 11.

The snapshot list shows the same thing from the other side. Boot 1 received 16
snapshots; the last 10 are byte-identical at `saved=10 stranded=6`, so the
reading is settled rather than sampled at an arbitrary instant — settled at a
state that precedes COM2 lines 4063 onward.

### What that means, stated plainly

The old census is a whole-log aggregate: a thread it names was never restored
anywhere in the capture. The new census is the ledger's state at the last
snapshot: a thread it names had no restore *yet*. On a boot that keeps running
after the snapshot, the second is a superset of the first — 3 of 3 boots here,
with every extra thread accounted for above.

The direction of the difference matters for what the gate does with it:

- On these 3 boots both mechanisms fail the boot and the new one's named set
  contains the old one's, so the diagnosis a reader gets is a superset of the
  truth, not a different answer.
- The over-report is only reachable on a boot that does not finish. On the 15
  unmutated boots this round captured on this profile — 7 in
  `historical-wedge/`, 6 in `../case-d/`, 2 in `../head-green/` — the new
  census read `stranded=0` on 15 of 15, agreeing with the old one on the 13 of
  those where the old one can still read anything.
- It is bounded by snapshot freshness, and freshness is bounded by the guest's
  monotonic clock: the limiter admits one snapshot per second of monotonic
  time, and these captures carry 10 to 16 of them for boots the host timed out
  at 150 seconds. This round did not measure the guest's monotonic-to-wall
  ratio, so the honest statement is that the last snapshot is the newest
  reading the kernel published, not that it is the state at the end of the
  capture.

Round 1's design produced no line at all on a boot like this. This one
produces a settled reading that fails the gate and names threads, three of
which are exactly the threads that were stranded for good.

## Files

`historical-wedge/`: `boot{1..7}/` each with `serial_user.txt`,
`serial_kernel.txt`, `old-census.txt`, `new-census.txt`, `verdict.txt`;
`gate-boots1-3.txt` and `gate-boots4-7.txt`; `commits.txt`;
`heartbeat-overlay.patch` (`29344251` → `5b419714`); `bare-hlt-wedge.patch`
(`5b419714` → `07fa248b`).

`deterministic-strand/`: `boot{1..3}/` with the same five files each;
`gate.txt`; `commits.txt`; `mutation-E.patch`; `divergence-analysis.txt`.

Both censuses were run as `<script> serial_user.txt serial_kernel.txt`, the
old one from `git show bfbb7575:scripts/x86-strand-census.sh`, and each
census file carries its exit code as its last line.
