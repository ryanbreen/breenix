# #775 dispatch strand census — what replaced the records, and what it reads

<!-- claim-lint:ok: the round-1 battery is the 5-row table this file replaced,
     and the captures backing every table below are committed under
     docs/planning/green-program/sockets/serials/775/case-a/README.md and its
     five sibling directories. -->
`#775` removes formatted records from the x86 context-switch path and replaces
the host-side census that parsed them with a fixed atomic per-TID ledger the
kernel publishes itself. Round 1 measured that migration on 5 boots, all green,
with 0 serials committed. Round 2 replaced that with 19 boots across four
cases, a head-green pair, 2 production boots and a 102-file replay.

Round 3 (this revision) changes what the kernel emits and where, and re-measures
it: the heartbeat moves to the idle loop x86 actually runs (finding N1), the
snapshot moves to COM2 (N8), and each snapshot gains `seq`, `tick` and `ms` so
the consumer can select the newest one rather than the last one in argument
order (F9) and report how stale its reading is (N14). The round-3 captures are
under `serials/775/round3/`; the round-2 tables below are unchanged except
where a number did not survive re-derivation, which is marked where it sits.

<!-- claim-lint:ok: the captures are the 21 round-2 boots and the 4 round-3
     boots committed under docs/planning/green-program/sockets/serials/775. -->
Each number in this document is derived from a capture committed under
`docs/planning/green-program/sockets/serials/775/`, with the command beside it.

## The two mechanisms

| | old | new |
|---|---|---|
| source | `Saved kernel context for blocked thread N`, `Restored kernel context for thread N`, `(thread N) exited with code`, on COM2 | `kernel/src/task/dispatch_strand_census.rs`, a 4096-entry `[AtomicU8]` |
| consumer | `git show bfbb7575:scripts/x86-strand-census.sh`, awk over the whole log | `scripts/x86-strand-census.sh`, awk over the highest-`seq` `[DISPATCH_STRAND_CENSUS:...]` snapshot |
| `threads_saved_blocked` | distinct TIDs with at least one save record anywhere in the log | ledger slots with `EVER_SAVED` at the instant of that snapshot |
| `stranded` | ever-saved, not exited, last restore line before last save line | ever-saved, and neither `EXITED` nor `LAST_EVENT_RESTORED` at that instant |
| exit codes | 0 for `stranded=0`, 1 for `stranded>0`, 2 for usage/IO error | 0 when the newest valid snapshot says `stranded=0`, 1 when it says `stranded>0`, 2 when there is no valid snapshot or the inputs mix two boots, 3 when the ledger overflowed so the snapshot is incomplete |
| which snapshot decides | n/a (whole-log aggregate) | the one with the highest `seq`, so the reading does not depend on argument order |
| where it is emitted from | the dispatch path, once per save and once per restore | `context_switch.rs::idle_loop()` and the loopback pump, at most once per second, plus 1 final snapshot at the last userspace exit |
| which serial channel | COM2, the kernel log | COM2 (round 3, finding N8; round 2 used COM1) |

The predicate is the same relation. What changed is *when an answer exists*
and *what instant it describes*, and that is what the four cases below
measure.

## Case A — both censuses read from one boot

`serials/775/case-a/`. The comparison needs a commit that carries both
mechanisms. `29344251` still compiles the three records; the scratch commit
`5b419714` overlays this round's heartbeat on it. `git diff --quiet 5b419714
365c20c2` holds for 7 of 7 paths that carry the census — the module,
`kernel/src/task/mod.rs`, `kernel/src/main.rs`,
`kernel/src/net/loopback_pump.rs`, `kernel/src/syscall/handlers.rs`,
`scripts/x86-strand-census.sh`, `scripts/x86-gate-verdict.sh` — so the overlay
runs this branch's code rather than an approximation of it.

```bash
git show bfbb7575:scripts/x86-strand-census.sh > /root/775c/old-census.sh
./docker/qemu/run-x86-gate.sh 3 full        # then 4 full
/root/775c/old-census.sh boot/serial_user.txt boot/serial_kernel.txt
./scripts/x86-strand-census.sh boot/serial_user.txt boot/serial_kernel.txt
```

### Leg 1 — `historical-wedge/`, the committed bare-hlt wedge, 7 boots

`07fa248b` = `5b419714` + `bare-hlt-wedge.patch`, the same patch (byte for
byte, `diff` of the two committed copies is empty) that wedged the
post-removal head under case B.

| boot | old saved | new saved | old stranded | new stranded | old rc | new rc | gate |
|---:|---:|---:|---:|---:|---:|---:|---|
| 1 | 11 | 11 | 0 | 0 | 0 | 0 | PASS |
| 2 | 11 | 11 | 0 | 0 | 0 | 0 | PASS |
| 3 | 11 | 11 | 0 | 0 | 0 | 0 | PASS |
| 4 | 11 | 11 | 0 | 0 | 0 | 0 | PASS |
| 5 | 11 | 11 | 0 | 0 | 0 | 0 | PASS |
| 6 | 11 | 11 | 0 | 0 | 0 | 0 | PASS |
| 7 | 11 | 11 | 0 | 0 | 0 | 0 | FAIL, `clock_gettime_test` not allowlisted |

7 of 7 agree on every field including `lines=`. The wedge fired on 0 of these
7 boots, so what they agree on is the green reading. It fired once earlier in
the round-2 run at the same commit (a beast gate transcript records
`saved=10 stranded=5 lines=4714`); those serials were overwritten and are not
committed, so nothing here rests on them. The wedge is a race, and case A does
not use it as its oracle.

### Leg 2 — `deterministic-strand/`, a mutation that cannot not fire, 3 boots

`mutation-E.patch` adds 14 lines to `Scheduler::wake_expired_timers`: an x86
arm that drops the timer wake for any thread whose `blocked_in_syscall` flag
is set. Such a thread has a saved kernel context, is off the ready queue, and
has no waker left.

| boot | old saved | new saved | old stranded | new stranded | old rc | new rc | gate |
|---:|---:|---:|---|---|---:|---:|---|
| 1 | 11 | 10 | 24, 26, 36 | 21, 24, 25, 26, 28, 36 | 1 | 1 | FAIL, strand |
| 2 | 11 | 10 | 24, 26, 36 | 21, 24, 25, 26, 28, 36 | 1 | 1 | FAIL, strand |
| 3 | 11 | 10 | 24, 26, 36 | 21, 24, 25, 26, 27, 36 | 1 | 1 | FAIL, strand |

Both mechanisms fail the boot on 3 of 3 and the new one names its threads.
They do not report the same numbers, and
`deterministic-strand/divergence-analysis.txt` attributes each difference to
a record. Boot 1, from `serial_kernel.txt` alone so the line numbers are one
timeline:

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

Threads 24, 26 and 36 are the ones the mutation parked for good, and both
censuses name all three on 3 of 3 boots. Threads 21, 25 and 28 each have a
restore after their last save and an exit line; they were parked when the last
snapshot was taken and resumed after it. Thread 38's first save is at line
4602 of 4669, which accounts for old `saved=11` against new `saved=10`.

**The one semantic difference this round found, stated plainly.** The old
census is a whole-log aggregate; the new one is the ledger's state at the last
snapshot. On a boot that keeps running after that snapshot, the new
`stranded` set is a superset of the old one — 3 of 3 boots here, every extra
member accounted for above. The over-report is only reachable on a boot that
does not finish, and it never inverts a verdict: on 3 of 3 the named set
contains the genuinely stranded threads. On the 15 unmutated boots this round
captured on this profile (7 in `case-a/historical-wedge/`, 6 in `case-d/`, 2
in `head-green/`) the new census read `stranded=0` on 15 of 15.

## Case B — the post-removal head, wedged

`serials/775/case-b-post-removal-mutation/`. `482a2e86` = `365c20c2` +
`bare-hlt-wedge.patch` (`git diff --stat` reports 1 file changed, 12
insertions).

| fact | value |
|---|---|
| `USERSPACE TEST COMPLETE` | 0 in `serial_user.txt`, 0 in `serial_kernel.txt` |
| `TEST_TALLY:` | 0 in both |
| census snapshots on COM1 | 12 of 12 from the heartbeat, 0 from the completion site |
| last snapshot | `saved=10 stranded=5 tids=21,24,25,27,36 tid_overflow=0 ledger_overflow=0` |
| `scripts/x86-strand-census.sh` | 5 named threads + `STRAND_CENSUS: threads_saved_blocked=10 stranded=5 lines=4829`, rc 1 |
| `scripts/x86-gate-verdict.sh` | rc 1, failing the boot on the strand arm (quoted verbatim in `serials/775/case-b-post-removal-mutation/verdict.txt`) |

<!-- claim-lint:ok: the two greps in the table above are the evidence -- 0
     USERSPACE TEST COMPLETE and 0 TEST_TALLY in 2 of 2 captures for this
     boot, which is the block the completion snapshot sits inside. -->
This is the case round-1 F1 said the mechanism could not reach and F8 said was
untested: a boot that does not reach the last userspace exit, so the
completion snapshot did not run on it. Both outputs replay byte-identically
from the committed captures.

The snapshots' `stranded` values in order are 0, 4, 5, 6, 4, 5, 5, 5, 5, 5, 5,
5 — which is why the consumer judges the last one and not any of them, and why
it must not be an anchored whole-line match: 12 of the 12 markers sit mid-line
after other COM1 output.

## Case C — the no-record serials, three ways

`serials/775/case-c/`. Population: the committed files matching
`docs/planning/green-program/*/serials/*.txt` that carry neither a census
marker nor a `Saved kernel context` record — 102 of the 121 files that match
the glob, including the specimen round-1 F2 named.

```bash
grep -L "DISPATCH_STRAND_CENSUS" docs/planning/green-program/*/serials/*.txt \
  | xargs grep -L "Saved kernel context"
```

| pair | census rc=0 | census rc=2 | verdicts naming a strand |
|---|---:|---:|---:|
| main, `bfbb7575` | 102 | 0 | 0 of 102 |
| round 1, `66d68849` census + the same unchanged verdict script | 0 | 102 | 102 of 102 |
| this head | 0 | 102 | 0 of 102 |

Main's and the head's verdict *sentences* agree on 102 of 102 and differ on 0:
101 `FAIL - USERSPACE TEST COMPLETE was absent; boot did not finish` and 1
`PASS` (`tracing/serials/x86-r4-strand-excerpt.txt`, an excerpt that does
carry the completion markers). The head keeps main's rc=2-vs-rc=0 difference
honestly — it has no marker to read on these inputs and says so — while
restoring main's verdict through the rc=2 fallthrough. That is the
`#702`-vs-strand distinction `docker/qemu/run-x86-gate.sh:207-219` requires.

The reviewer-named specimen, verbatim:

```
### main (bfbb7575) census
STRAND_CENSUS: threads_saved_blocked=0 stranded=0 lines=304          rc=0
### round-1 (66d68849) census
strand census: expected exactly one DISPATCH_STRAND_CENSUS line, found 0   rc=2
### this head census
strand census: no DISPATCH_STRAND_CENSUS line found                  rc=2

### main verdict
x86 userspace gate: FAIL - USERSPACE TEST COMPLETE was absent; boot did not finish
### round-1 verdict
x86 userspace gate: FAIL - a thread was saved blocked in a kernel wait and never restored (see the strand census above)
### this head verdict
x86 userspace gate: census unavailable; continuing with ordered first-cause checks
x86 userspace gate: FAIL - USERSPACE TEST COMPLETE was absent; boot did not finish
```

## Case D — green boots with the heartbeat present

`serials/775/case-d/`. 6 boots of `5b419714`, `testing,external_test_bins`,
through `run-x86-gate.sh 4 full` then `1 full` then `1 full`.

| boot | old saved | new saved | old stranded | new stranded | old rc | new rc | gate |
|---:|---:|---:|---:|---:|---:|---:|---|
| 1 | 11 | 11 | 0 | 0 | 0 | 0 | PASS |
| 2 | 11 | 11 | 0 | 0 | 0 | 0 | PASS |
| 3 | 11 | 11 | 0 | 0 | 0 | 0 | FAIL, `loopback_wake_test_child` not allowlisted |
| 4 | 11 | 11 | 0 | 0 | 0 | 0 | PASS |
| 5 | 11 | 11 | 0 | 0 | 0 | 0 | PASS |
| 6 | 11 | 11 | 0 | 0 | 0 | 0 | PASS |

6 of 6 agree on `threads_saved_blocked`, `stranded`, `lines=` and rc; 5 of the
6 are gate-green. Boot 3's failure is a userspace test flake downstream of the
census, of the same class that hit `case-a/historical-wedge` boot 7 at a
different commit naming a different program.

## Round 3 — the idle loop, measured

<!-- claim-lint:ok: N1's repro counted 0 of 215 round-2 census markers preceded
     by an idle-dispatch breadcrumb; the boot-level consequence is the
     r3-idle-cadence capture in this same table. -->
`serials/775/round3/`, 3 gate boots at this head, one QEMU at a time. Round-2
finding N1 was that `report_heartbeat_if_due()` was called from `main.rs`'s
`idle_thread_fn`, whose body is not dispatched on x86: `kernel_main_continue`
stores it as `init_task`'s entry point and immediately marks that thread
Running on the boot context, and once any thread reaches Ring 3
`is_ring3_confirmed()` latches and `context_switch.rs` rewrites idle's frame to
its own `idle_loop()`. The hook now sits at the TOP of `idle_loop`'s body —
not after its `enable_and_hlt()`, because `setup_idle_return` restarts the
function on every idle dispatch, so the code after the halt runs only when the
halt returns without the timer handler switching away.

| capture | emitters live | snapshots (COM2 / COM1) | gaps min/mean/max ms | census |
|---|---|---:|---|---|
| `round3/r3-head-green/` | idle loop + pump + completion site | 104 / 0 | 208 / 1342 / 27017 | `saved=11 stranded=0`, rc 0, GATE PASS |
| `round3/r3-idle-cadence/` | idle loop + completion site | 98 / 0 | 14 / 993 / 1190 | `saved=11 stranded=0`, rc 0, GATE PASS |
| `round3/r3-idle-strand/` | idle loop only, + mutation E | 90 / 0 | 1001 / 1025 / 2004 | `saved=11 stranded=3`, rc 1, GATE FAIL |


The gap columns are over ALL snapshots, which is what the census script prints.
Two of the three rows include one completion-site snapshot that the 1000 ms
limiter does not govern, and dropping it changes the row: rows 1 and 2 become
866 / 1355 / 27017 over 103 snapshots and 1000 / 1004 / 1204 over 97. Row 3
carries no completion marker, so its 90 really are all idle-driven (round-4
review finding R3-1).

<!-- claim-lint:ok: each row is one committed capture under
     docs/planning/green-program/sockets/serials/775/round3/ with its gate
     transcript beside it. -->
The N-to-0 column is finding N8 closed: the snapshot is on the kernel-log
channel the removed records used, not on the interactive user console. The
27017 ms gap in the first row sits between seq=1 at 550 ms and seq=2 at
27567 ms -- boot and early userspace, where neither emitter ran.

`r3-idle-cadence` is the no-loopback-emission condition. Its
`pump-heartbeat-disabled.patch` removes the single call in
`kernel/src/net/loopback_pump.rs`, so **97 of its 98 snapshots come from the
idle loop**. The 98th is the completion-site snapshot: `seq=8` at ms=50381,
which `kernel/src/syscall/handlers.rs` emits directly under the runner's
final pass line, outside the limiter. Round 3 published
"98 of its 98" here and the same sentence in the round-3 README; both were
false, and round 4 corrects both (review finding R3-1).
<!-- claim-lint:ok: the 98th snapshot is seq=8 in the committed capture; the
     97-snapshot recomputation is the command block below. -->

The correction moves the headline cadence number with it, because the published
1190 ms was the gap from heartbeat `seq=7` to that non-heartbeat `seq=8`, and
the 14 ms minimum printed in the same row was `seq=8` to `seq=9`. Over the 97
idle-driven snapshots alone the gaps are **min 1000 / mean 1004 / max 1204 ms**
against a 1000 ms limiter. The first is at ms=43183, because the CPU does not
idle while the userspace test programs are running; the last is at ms=139552,
so the cadence held for 96 seconds. Under the round-2 wiring this capture would
have carried 0 idle-driven snapshots.

```
grep -o 'DISPATCH_STRAND_CENSUS:seq=[0-9]*:tick=[0-9]*:ms=[0-9]*' \
  serials/775/round3/r3-idle-cadence/serial_kernel.txt |
  awk -F'[=:]' '{seq=$3+0; ms=$7+0; if (seq==8) next;
    if (p!="") {g=ms-p; if (g>mx) mx=g; if (mn==""||g<mn) mn=g; s+=g; n++}
    p=ms; c++} END {printf "count=%d gaps=%d min=%d mean=%.0f max=%d\n",
    c, n, mn, s/n, mx}'
count=97 gaps=96 min=1000 mean=1004 max=1204
```
<!-- claim-lint:ok: the five numbers above are the output of the command
     printed with them, run on the committed capture. -->

`r3-idle-strand` adds `case-a/deterministic-strand/mutation-E.patch` on top, so
the strand is by construction and the pump still cannot emit. 90 of its 90
snapshots read `stranded>0`; the newest one, at ms=139014, names threads 24
(`loopback_wake_test`), 26 (`futex_handoff_oracle`) and 36
(`loopback_wake_test_child_22_main`), and the gate fails on the strand arm with
the round-3 wording:

```
x86 userspace gate: FAIL - a thread was saved blocked in a kernel wait and was
still not restored at the latest census snapshot (see the strand census above)
```

That pair is what the N1 fix is for: a wedged boot with no loopback emission
still published the ledger 90 times in 91 seconds and still named the threads.

## Round 4 — the third emission context, and what it cost to get there

Round 3 shipped two cadence contexts, and the round-3 review measured what they
are worth on the profile that ships: `docker/qemu/run-x86-prod-profile-boot-test.sh`
published a post-init snapshot on 4 of 6 boots and, on the other 2, nothing but
the loopback pump's pre-init `seq=1` (finding R3-5). Both contexts depend on the
rest of the kernel giving them a reason to run.
<!-- claim-lint:ok: 4 of 6 and 2 of 6 are the review's own counts, reproduced
     at 6 of 6 by the lost-wake arm committed here. -->

| context | where | when it runs | what stops it |
|---|---|---|---|
| idle loop | top of `idle_loop()`'s body, `kernel/src/interrupts/context_switch.rs` | on an idle dispatch, i.e. when the scheduler's queues are empty | a CPU that does not idle: the shipped profile takes 1 idle dispatch in a two-minute boot |
| `kloopbackd` | top of `loopback_pump_fn()`'s loop, `kernel/src/net/loopback_pump.rs` | on a pump pass, i.e. when loopback traffic wakes the thread | a profile without loopback traffic: the shipped profile has 1 pump pass in a two-minute boot |
| `kstrandd` | `census_thread_fn()`, `kernel/src/task/dispatch_strand_census.rs` | when the scheduler DISPATCHES it after its ~1 s timer wake: no other subsystem has to give it a reason to run, but the CPU still has to reach it | anything that stops it being dispatched -- a wedge, a lost wake, or a CPU busy enough that the wake-to-dispatch latency #766 measures runs to seconds (19939 ms and 17888 ms on the two round-4 gate captures, measured below) |
<!-- claim-lint:ok: the 3 rows are the 3 call sites
     tests/dispatch_strand_census_structure.rs pins at a count of 1 each. -->

<!-- claim-lint:ok: the 3 contexts share the 1 LAST_HEARTBEAT_NS
     compare-exchange in kernel/src/task/dispatch_strand_census.rs. -->
All three go through `report_heartbeat_if_due()`, so they share one `AtomicU64`
compare-exchange and cannot together exceed one snapshot per second. The
completion site in `kernel/src/syscall/handlers.rs` is a fourth emitter and is
NOT one of the three: it calls `report_snapshot()` directly, once, outside the
limiter, immediately after `USERSPACE TEST COMPLETE`.

### The lost wake `kstrandd` found

The first six production boots with `kstrandd` present published fewer
snapshots than round 3 did, not more: 1 or 2 markers per boot, 0 of them
from `kstrandd`, and 3 of the 6 carried only the pre-init pump snapshot. The
captures are `serials/775/round4/kstrandd-lost-wake/boot{1..6}/`. `kstrandd`
was created (`Added thread 4 'kstrandd' to scheduler`), dispatched once, and
not dispatched again across a boot carrying 3158 `[SW]` context switches.
<!-- claim-lint:ok: 6 of 6 lost-wake boots carry 1 or 2 markers; boot1's
     3158 is `grep -o '\[SW\]' serial_user.txt | wc -l`. -->

The cause is not in this branch's code. `Scheduler::block_current_for_timer`
set `thread.blocked_in_syscall = true` unconditionally, and that flag means
"parked inside a syscall of an owning process". Its x86 consumer acts on
exactly that reading: with the flag set and `from_userspace` false,
`kernel/src/interrupts/context_switch.rs` saves through
`save_kernel_context_with_guard`, which writes the registers into
`process.main_thread`. A pure kernel thread has no process, so the lookup
returns `None` and the registers are not written. The thread departs the CPU
with no saved context, and `note_save` does not fire either -- which is why
`kstrandd` did not appear in the census's own ledger while it was missing.

The rest of the family already draws the distinction: `block_current()` leaves
the flag clear, `block_current_in_syscall()` sets it, and the docstring on the
pair says why. `block_current_for_timer` was one of 2 producers asserting it
unconditionally, and x86 is where the missing conjunct bites. Both producers now
set the flag from `thread.owner_pid.is_some()`; the second one is the subject of
the boot-replay section below.

Round 4 wrote that "6 of 6 aarch64 consumers conjoin `thread.owner_pid.is_some()`
before acting on the flag". No census produces that, and round-5 finding R4-8
asked for the one that does. Here it is, over the whole kernel:

```
grep -rn 'blocked_in_syscall' kernel/src --include='*.rs'
```

202 lines. Bucketed by the shape of the line — comment or doc (`//`, `*`),
field declaration (`blocked_in_syscall: bool`), write (`blocked_in_syscall =`),
occurrence only inside a string literal, field copy into a struct literal, a
call to the `clear_blocked_in_syscall_current()` helper, or an expression that
BRANCHES on the flag:

| bucket | lines |
|---|---:|
| comment or doc | 51 |
| field declaration | 2 |
| write | 78 |
| string literal only | 7 |
| field copy | 19 |
| helper call | 7 |
| **branches on the flag** | **30** |
| other read | 8 |
| **total** | **202** |

Of the **30** branch sites, **14** also require an owning process in the same
`if`/`while`/`match` expression and **16** do not.

Round 5 published those 30 as bare `file:line` pins, and **2 of the 30 were
already stale at the commit that published them** — round-6 finding F2.
`task/scheduler.rs:3597` is a blank line at that head and `:4936` is a closing
brace; the real sites are `:3594` and `:4933`. The numbers were right at
`adcf82d8` and became wrong at `645bab38`, which removed 3 lines above them,
while the text carrying them landed 2 commits later at `7f319a1c` without being
re-derived. Same shape as the `#549`/`#551` lesson this document cites twice as
the reason not to pin literals.

So the census is anchored by NAME below — the enclosing function and the branch
expression itself — with **30 of the 30 line numbers re-derived by command at
write time**, not just the two that were wrong. Derived at HEAD
**`4c2ffc2b`**; round 6 makes no kernel edit, so `git diff 4c2ffc2b HEAD --
kernel/` is empty and the table holds at the round-6 head as well.
<!-- claim-lint:ok: the table below is the output of that re-derivation run,
     which checks 30 of 30 rows against
     docs/planning/green-program/sockets/775-CENSUS-EQUIVALENCE-2026-09-04.md
     rather than trusting any round-5 pin. -->

| # | file | line | function | branch expression | conjoins `owner_pid` |
|--:|---|--:|---|---|:-:|
| 1 | `arch_impl/aarch64/context_switch.rs` | 989 | `trace_ctx_publish` | `if thread.owner_pid.is_none() \|\| !thread.blocked_in_syscall {` | yes |
| 2 | `arch_impl/aarch64/context_switch.rs` | 1218 | `trace_eret_resume` | `if thread.owner_pid.is_none() \|\| !thread.blocked_in_syscall {` | yes |
| 3 | `arch_impl/aarch64/context_switch.rs` | 1308 | `trace_schedule_resume` | `if thread.owner_pid.is_none() \|\| !thread.blocked_in_syscall {` | yes |
| 4 | `arch_impl/aarch64/context_switch.rs` | 1364 | `trace_kernel_resume_irq` | `if thread.owner_pid.is_none() \|\| !thread.blocked_in_syscall {` | yes |
| 5 | `arch_impl/aarch64/context_switch.rs` | 1404 | `trace_resched_tail` | `if thread.owner_pid.is_none() \|\| !thread.blocked_in_syscall \|\| thread.id() != expected_tid {` | yes |
| 6 | `arch_impl/aarch64/context_switch.rs` | 3851 | `save_userspace_context_inline` | `if thread.owner_pid.is_some() && thread.blocked_in_syscall {` | yes |
| 7 | `arch_impl/aarch64/context_switch.rs` | 3873 | `save_userspace_context_inline` | `&& thread.blocked_in_syscall` | yes |
| 8 | `arch_impl/aarch64/context_switch.rs` | 3959 | `save_kernel_context_inline` | `&& thread.blocked_in_syscall` | yes |
| 9 | `arch_impl/aarch64/context_switch.rs` | 4029 | `save_kernel_context_inline` | `if thread.owner_pid.is_some() && thread.blocked_in_syscall {` | yes |
| 10 | `arch_impl/aarch64/context_switch.rs` | 4051 | `save_kernel_context_inline` | `&& thread.blocked_in_syscall` | yes |
| 11 | `arch_impl/aarch64/context_switch.rs` | 4175 | `restore_kernel_context_inline` | `if thread.owner_pid.is_some() && thread.blocked_in_syscall {` | yes |
| 12 | `arch_impl/aarch64/context_switch.rs` | 5820 | `check_need_resched_and_switch_arm64` | `if thread.owner_pid.is_some() && thread.blocked_in_syscall {` | yes |
| 13 | `arch_impl/aarch64/context_switch.rs` | 6400 | `inline_schedule_trampoline` | `if thread.owner_pid.is_some() && thread.blocked_in_syscall {` | yes |
| 14 | `arch_impl/aarch64/timer_interrupt.rs` | 440 | `trace_kernel_resume_timer_irq` | `if thread.owner_pid.is_none() \|\| !thread.blocked_in_syscall {` | yes |
| 15 | `arch_impl/aarch64/context_switch.rs` | 3691 | `inline_ret_dispatch_info_if_ready` | `&& (thread.blocked_in_syscall \|\| thread.context.elr_el1 >= KERNEL_VIRT_BASE);` | no |
| 16 | `arch_impl/aarch64/context_switch.rs` | 4164 | `restore_kernel_context_inline` | `raw_uart_char(if thread.blocked_in_syscall {` | no |
| 17 | `arch_impl/aarch64/context_switch.rs` | 4847 | `dispatch_thread_locked` | `} else if is_kernel \|\| blocked_in_syscall \|\| is_in_kernel_mode {` | no |
| 18 | `arch_impl/aarch64/context_switch.rs` | 4856 | `dispatch_thread_locked` | `if (blocked_in_syscall \|\| is_in_kernel_mode) && !is_kernel {` | no |
| 19 | `arch_impl/aarch64/timer_interrupt.rs` | 1122 | `dump_lockup_state` | `if t.blocked_in_syscall {` | no |
| 20 | `arch_impl/aarch64/exception.rs` | 1566 | `handle_sync_exception` | `raw_uart_char(if thread.blocked_in_syscall {` | no |
| 21 | `arch_impl/aarch64/exception.rs` | 1584 | `handle_sync_exception` | `raw_uart_char(if thread.blocked_in_syscall {` | no |
| 22 | `interrupts/context_switch.rs` | 460 | `check_need_resched_and_switch` | `} else if !from_userspace && (blocked_in_syscall \|\| old_thread_is_user) {` | no |
| 23 | `interrupts/context_switch.rs` | 860 | `switch_to_thread` | `} else if blocked_in_syscall` | no |
| 24 | `task/scheduler.rs` | 2848 | `block_current_departure_gate` | `} else if self.get_thread(tid).map(\|thread\| thread.blocked_in_syscall)` | no |
| 25 | `task/scheduler.rs` | 2863 | `block_current_departure_gate` | `} else if self.get_thread(tid).map(\|thread\| thread.blocked_in_syscall) != Some(true) {` | no |
| 26 | `task/scheduler.rs` | 3594 | `wake_io_thread_locked` | `thread.state == ThreadState::Ready && thread.blocked_in_syscall` | no |
| 27 | `task/scheduler.rs` | 4933 | `get_process_cpu_ticks` | `&& !t.blocked_in_syscall` | no |
| 28 | `syscall/handlers.rs` | 3737 | `complete_wait` | `if thread.blocked_in_syscall {` | no |
| 29 | `syscall/wait.rs` | 423 | `complete_wait` | `if thread.blocked_in_syscall {` | no |
| 30 | `socket/udp.rs` | 378 | `test_blocking_recvfrom_blocks_and_wakes` | `.map(\|thread\| thread.state == ThreadState::Blocked && thread.blocked_in_syscall)` | no |

30 rows, 14 conjoined, 16 bare. 30 of 30 were checked at write time: the line
still carries `blocked_in_syscall`, is not a comment, and is not a write. The
grep behind the two corrected pins, `grep -n 'blocked_in_syscall'
kernel/src/task/scheduler.rs`, restricted to that file's 4 branch rows:

```
2848:        } else if self.get_thread(tid).map(|thread| thread.blocked_in_syscall)
2863:            } else if self.get_thread(tid).map(|thread| thread.blocked_in_syscall) != Some(true) {
3594:                    thread.state == ThreadState::Ready && thread.blocked_in_syscall
4933:                                && !t.blocked_in_syscall
```

So the conjunct is the NORM on aarch64, not a rule anywhere, and 4 of the 7
bare aarch64 sites are diagnostic-only (`raw_uart_char`/`raw_serial_str`
breadcrumbs at :4164, :1122, :1566, :1584) — a distinction the bucketing above
does not make, and this sentence does rather than leave it implied. Guarding the
two producers closes the class where it was reachable; it does not make the
flag's meaning universal at the consumers, and the x86 site the lost wake ran
through (`interrupts/context_switch.rs:460`) is still one of the 16.
<!-- claim-lint:ok: the 202/30/14/16 counts above are the output of the
     bucketing run at this head over that grep; the classifier's rules are
     stated in the paragraph that introduces the table, and all 30 anchors
     were re-derived by command at the round-6 head (finding F2). -->

This is also the diagnosis of the round-2 attempt at a dedicated census
kthread, which page-faulted and was abandoned without a root cause. It slept
through the same primitive, so it was dispatched from a context that had not
been written.
<!-- claim-lint:ok: the before-and-after boots are the two committed sets under
     serials/775/round4/; the consumer census is the 30-row table above, 14
     sites conjoining owner_pid.is_some() and 16 not. This comment carried
     round 4's retracted "6 aarch64 sites against 1 x86 site" wording until
     round 6; the sentence it attested was replaced in round 5 and the
     attestation was missed. -->

### The zero-feature production profile, before and after

Six boots each side, one QEMU at a time, on the beast `breenix-x86` container.
Both sets are committed: `serials/775/round4/kstrandd-lost-wake/` is `kstrandd`
present with the lost wake, `serials/775/round4/production/` is `kstrandd`
working. Both arms are 6 of 6 `PASS: x86 production profile reached steady
state with the teardown census at rest`.

| boot | markers, lost-wake arm | markers, fixed arm | newest snapshot, fixed arm |
|---:|---:|---:|---|
| 1 | 2 | 54 | `seq=54:tick=11747:ms=62044:saved=13:stranded=7` |
| 2 | 2 | 37 | `seq=37:tick=7812:ms=42459:saved=12:stranded=7` |
| 3 | 1 | 54 | `seq=54:tick=11473:ms=60728:saved=12:stranded=7` |
| 4 | 1 | 55 | `seq=55:tick=11680:ms=61616:saved=12:stranded=7` |
| 5 | 2 | 53 | `seq=53:tick=10863:ms=62189:saved=12:stranded=7` |
| 6 | 1 | 40 | `seq=40:tick=8739:ms=47100:saved=12:stranded=7` |

6 of 6 boots on the fixed arm carry a post-init snapshot; on the lost-wake arm
3 of 6 carry only the pump's pre-init `seq=1`, which is R3-5 reproduced. On
6 of 6 fixed-arm boots `seq=1` is the pump with `saved=0` and `seq=2` is
`kstrandd` at ms 1798-1978. The two arms are two populations of 6, not a
paired diff: the fixed arm was re-run at the final head so that the second
producer's fix is in it as well.

The two readings side by side are the point of the whole exercise:

```
# scripts/x86-strand-census.sh SERIALS/serial_kernel.txt SERIALS/serial_user.txt

# serials/775/round4/production/boot1  (kstrandd running)
strand census: thread 5 (init) saved blocked and not restored as of the latest snapshot (seq 54, tick 11747)
... 6 more named threads: 15 bsshd, 17 telnetd, 19 bterm, 20 blog, 22 bcheck, 23 blogd ...
strand census: latest snapshot seq=54 tick=11747 at 62044 ms; 54 valid snapshot(s), previous 1056 ms earlier, largest gap 2719 ms
strand census: age at the completion marker: not measurable -- this capture carries no USERSPACE TEST COMPLETE, so it has no kernel timestamp for a known late point; newest snapshot seq=54 at 62044 ms
STRAND_CENSUS: threads_saved_blocked=13 stranded=7 lines=2283     rc=1

# serials/775/round4/kstrandd-lost-wake/boot3  (kstrandd never dispatched again)
strand census: latest snapshot seq=1 tick=5 at 1001 ms; 1 valid snapshot(s), no earlier snapshot to measure cadence against
strand census: age at the completion marker: not measurable -- this capture carries no USERSPACE TEST COMPLETE, so it has no kernel timestamp for a known late point; newest snapshot seq=1 at 1001 ms
STRAND_CENSUS: threads_saved_blocked=0 stranded=0 lines=2217      rc=0
```
<!-- claim-lint:ok: both blocks are that command's output, re-run at this head
     on the two committed captures. Only the 6 repeated thread lines are
     elided, and they are named on the elision line; round 4's version of this
     block silently dropped the age line (round-5 finding R4-14). The marker
     counts are `grep -c DISPATCH_STRAND_CENSUS`. -->

The blind boot reports `stranded=0` and exits 0. That is what an unobservable
census costs, and it is why R3-5 was a blocking finding rather than a
limitation.

The rc=1 on the working boot is the round-3 caveat unchanged and still correct:
on this profile the seven named threads are parked in syscalls, not stranded,
and no in-repo consumer reads a production capture as a verdict -- the
production gate does not call `scripts/x86-gate-verdict.sh`. What the fixed arm
buys is that the reading is now CURRENT: on 6 of 6 boots the newest snapshot is
about a second old at the end of the capture instead of tens of seconds.

### The test-profile gate, and the defect `kstrandd` surfaced on the way

Adding `kstrandd` reddened the x86 TEST-profile gate on 2 of 2 boots with a
kernel page fault at `0x1e`, on the boot thread's own kernel stack, after the
capture showed the boot thread re-executing a test it had already finished. The
round-3 head is 2 of 2 `PASS` on the same host, so the regression was this
branch's. The three arms and the diagnosis are at
`serials/775/round4/boot-replay/`.

The producer was the sibling of the one above:
`Scheduler::block_current_for_io_publish` set `blocked_in_syscall = true`
unconditionally, and the boot thread reaches it during init --
`Completion::wait_timeout_uninterruptible()` backs the ext2 root read. On a
thread with no process the flag makes each later context save a silent no-op,
so the next boot-time idle restore replays the last save that landed.
`kstrandd` is what surfaced it: it forces that boot-time restore branch tens of
times inside the pre-userspace init phase, where the round-3 head took it
rarely.

With both producers guarded the gate is green again:

| capture | verdict | snapshots | age at the completion marker |
|---|---|---:|---|
| `round4/gate-green/boot1/` | `PASS - exited=22 expected>=10 nonzero=0 allowlist=0` | 118 | 1137 ms |
| `round4/gate-green/boot2/` | `PASS - exited=22 expected>=10 nonzero=0 allowlist=0` | 115 | 842 ms |
<!-- claim-lint:ok: the 2 rows are the 2 committed gate transcripts under
     serials/775/round4/gate-green/. -->

#### The hole those two captures carry, and what does not fill it

Round 4 called these "the first captures where the census cadence covers the
whole boot". That is false, and each boot's own `gate.txt` prints the
refutation on line 8 (round-5 finding R4-1). Both boots publish at a 1 s
cadence to `seq=5` and then go about twenty seconds without a snapshot:

| capture | hole | from | to | its own `gate.txt:8` |
|---|---:|---|---|---|
| `round4/gate-green/boot1/` | 19939 ms | `seq=5` at 4789 ms | `seq=6` at 24728 ms | `largest gap 19939 ms` |
| `round4/gate-green/boot2/` | 17888 ms | `seq=5` at 4840 ms | `seq=6` at 22728 ms | `largest gap 17888 ms` |

```
grep -o 'DISPATCH_STRAND_CENSUS:seq=[0-9]*:tick=[0-9]*:ms=[0-9]*' \
  serials/775/round4/gate-green/boot1/serial_kernel.txt |
awk -F'[=:]' '{seq=$3;ms=$7; if(p!=""){g=ms-p; if(g>1500)
  printf "gap %d ms: seq %d (%d ms) -> seq %d (%d ms)\n", g, ps, p, seq, ms}
  p=ms; ps=seq}'
```

`kstrandd` is alive across the hole. It is `Added thread 4 'kstrandd' to
scheduler` at `serial_kernel.txt:487` on both boots -- before the first snapshot
line, 520 -- and the string `kstrandd` occurs exactly once in each capture, so
0 lines record it exiting. After the hole it resumes: over `seq>=6` on boot 1,
112 gaps, min 12 ms, mean 1022 ms, max 1806 ms.

What the hole spans is the userspace-process creation burst. The first line
inside it on boot 1 is line 682:

```
[ INFO] kernel: RING3_SMOKE: creating hello_time userspace process (early)
```

and the `seq=6` snapshot that ends the hole is line 2337, with line 2336
immediately before it:

```
[ INFO] kernel::syscall::handlers: sys_execv: Loading program '/usr/local/test/bin/clonevm_exec_test'
[ INFO] kernel::syscall::handlers: sys_execv: argc=2
[DEBUG] kernel::syscall::handlers: sys_execv: argv[0] = 'clonevm_exec_test'
[DEBUG] kernel::syscall::handlers: sys_execv: argv[1] = '--second-stage'
[DISPATCH_STRAND_CENSUS:seq=6:tick=4632:ms=24728:saved=1:stranded=1:tids=27:tid_overflow=0:ledger_overflow=0]
```

(lines 2333 to 2337, quoted verbatim). Round 5 wrote that the line immediately
before `seq=6` was the `sys_execv: Loading program` one; that line is 2333,
four lines earlier, and 2336 is the `argv[1]` line — round-6 finding F5, an
un-re-run quote in a subsection whose other numbers re-derive exactly.

The other two contexts do not fill it, and the capture is what says so rather
than an argument about them: the three emitters share ONE rate limiter, so a
hole is an absence of snapshots from the three of them together, not from
`kstrandd` alone.
Neither of the other two published in that window, which is what being
demand-driven predicts -- `idle_loop` runs only on an idle dispatch and the CPU
had runnable work across the burst, and `loopback_pump_fn` runs only when
loopback traffic wakes it.

So what `kstrandd` bought is narrower than round 4 wrote, and it is still the
thing R3-5 was about: its cadence needs no other SUBSYSTEM to act first. It
does need the CPU. A snapshot is published only once the kthread is DISPATCHED
after its timer wake, and that wake-to-dispatch latency is the quantity #766
exists for -- `693-RCA-2026-09-02.md` §7 item 2: "the deadline is honoured to
within a tick, but dispatch after it is bounded only by a full 50 ms-per-thread
round robin", measured there at p90 2592 ms and max 10318 ms over 324
re-derivable trials. Under this burst it is about twice that maximum.
<!-- claim-lint:ok: each number in this subsection was produced by running the
     command beside it on the two committed captures at this head. -->

### The age at the completion marker, asserted

Round-3 finding N14 was that the staleness of the newest snapshot is stated but
not bounded. A capture carries no end-of-boot timestamp, so staleness at the
END of a boot is still not derivable from it -- but a capture that reaches
`USERSPACE TEST COMPLETE` does carry a kernel timestamp for a known late
instant, because the completion site emits a snapshot there.

`scripts/x86-strand-census.sh` now prints, on each capture it reads, the age of
the newest CADENCE snapshot at that instant, and exits 4 when a `stranded=0`
reading came from a snapshot staler than its bound.
`scripts/x86-gate-verdict.sh` turns exit 4 into a gate FAIL. A capture with no
completion marker prints that the age is not measurable rather than inventing a
reference.

**A RED READING IS NEVER MASKED — the precedence is 1 > 3 > 4 > 2, and the code
takes the exits in that order (round-6 finding F1).** Round 5 fixed R4-5 by
exiting 2 on a capture that carries the completion marker with no valid
snapshot after it, but put that arm AHEAD of the strand verdict. On the
round-5 head, the reviewer's own fixture — `round4/gate-green/boot1` with every
snapshot from `seq=29` up deleted, whose newest remaining snapshot reads
`saved=11:stranded=2:tids=25,37` — printed `census incomplete at completion
marker` and exited 2, and `x86-gate-verdict.sh` scored the boot **PASS**. A red
strand had become a green gate. Each of the three non-red conditions —
overflowed ledger, stale age, truncated capture — says the reading is WORSE
than it looks, never better, so none may downgrade a named strand to "census
unavailable". `stranded>0` now exits 1 first, whatever else is true of the
capture; the truncated-at-the-marker rc=2 is the LAST arm and is reached only
on a reading that is clean, unoverflowed and not stale. The two UNREADABLE rc=2
classes (no valid snapshot at all, snapshots from more than one boot) still
short-circuit ahead of everything, because no verdict can be computed from
those inputs at all. On that same fixture at the round-6 head: <!-- claim-lint:ok: 2 of the 2 truncation arms are covered by tests in tests/x86_gate_verdict_test.rs and the code order by host_consumers_have_no_removed_record_dependency in tests/dispatch_strand_census_structure.rs; the mutation table below records what each of 6 mutations reddens. -->

```
strand census: thread 25 (loopback_wake_test) saved blocked and not restored as of the latest snapshot (seq 28, tick 10003)
strand census: thread 37 (loopback_wake_test_child_22_main) saved blocked and not restored as of the latest snapshot (seq 28, tick 10003)
strand census: latest snapshot seq=28 tick=10003 at 49903 ms; 28 valid snapshot(s), previous 1001 ms earlier, largest gap 19939 ms
strand census: age at the completion marker: not measurable -- this capture carries the USERSPACE TEST COMPLETE marker but no valid snapshot follows it in that capture, so it is TRUNCATED and the newest reading (seq=28 at 49903 ms) carries no freshness evidence
STRAND_CENSUS: threads_saved_blocked=11 stranded=2 lines=3879
```

(the whole output: 5 lines of 5, unelided)

rc=1, and through `EXPECTED_EXITS=10 scripts/x86-gate-verdict.sh` beside that
boot's `serial_user.txt`: `x86 userspace gate: FAIL - a thread was saved blocked
in a kernel wait and was still not restored at the latest census snapshot`. The
SAME truncation with the snapshots rewritten to `stranded=0:tids=-` still
exits 2 with `census incomplete at completion marker`, and the gate reports
`census unavailable; continuing` and then `PASS` — the R4-5 behaviour, kept.
<!-- claim-lint:ok: both runs were made at the round-6 head against
     docs/planning/green-program/sockets/serials/775/round4/gate-green/boot1/
     and the 2 outcomes are pinned by 2 tests in
     tests/x86_gate_verdict_test.rs. -->

Three further properties of that arm were round-5 findings and are now the
tool's behaviour:

* **The bound is derived, not chosen (R4-2), and it has exactly ONE copy
  (round-6 finding F4).** Round 4 set it at 5000 ms with no derivation, on an
  n=2 margin reading. It is now #766's measured MAXIMUM wake-to-dispatch
  overrun plus margin: `693-RCA-2026-09-02.md` lines 109-110 measure
  `write_ms - token_ms - delay_ms` over 324 re-derivable trials at min 84 ms,
  p50 426.5 ms, p90 2592 ms, max 10318 ms, and §7 item 2 of that document names
  the mechanism ("dispatch after it is bounded only by a full 50 ms-per-thread
  round robin"). The census cadence rides on exactly that latency, so the bound
  is a statement about the known distribution rather than about two boots. **It
  tightens when #766 lands** — which is exactly why round 5's fix was
  incomplete: it moved the ratchet off the VALUE and left four more
  hand-maintained copies of it, including the sentence the operator reads on a
  gate FAIL, which no test pinned at all. Setting the census bound to 300 there
  printed `bound 300 ms` on one line and the old, unchanged number on the next,
  with 0 of the 20 tests red. The value now lives in one place, the
  `stale_limit_ms` assignment in `scripts/x86-strand-census.sh`;
  `scripts/x86-gate-verdict.sh` reads it back out of the census's own
  `STRAND_CENSUS: STALE ... bound_ms=` line, and the tests derive it from the
  assignment. `grep -rn 15000 scripts tests` at this head returns exactly one
  line, the assignment itself. In the docs the number survives as OUTPUT and as
  history, not as a restatement to keep in step: 2 pasted census age lines in
  the table below, 2 rows of the mutation table naming a mutation that was
  performed, the sentence quoting this grep, and — under
  `docs/planning/green-program/sockets/serials/775/` — 325 committed capture
  files, which are bytes a tool printed. The structure test therefore scans
  `scripts/` and `tests/`, the hand-maintained surfaces, and that scope is
  stated here rather than left implied. Under the same 300 ms mutation, the two
  lines now agree:

  ```
  strand census: age at the completion marker: 1137 ms (newest cadence snapshot seq=28 at 49903 ms, completion snapshot seq=29 at 51040 ms, bound 300 ms)
  x86 userspace gate: FAIL - the strand census read stranded=0 from a snapshot that was already more than 300 ms stale at the completion marker, so the clean reading is stale rather than clean (see the age line above)
  ```

  What holds the value now that no consumer restates it is the DERIVATION,
  checked rather than cited: `the_staleness_bound_has_exactly_one_copy_in_the_repository`
  in `tests/dispatch_strand_census_structure.rs` reads #766's measured maximum
  out of `693-RCA-2026-09-02.md` and fails if the bound drops below it, and it
  fails again if any file under `scripts/` or `tests/` that mentions the census
  carries a second copy of the number. That consumer set is a census, not a
  written list, so a consumer added tomorrow is covered the day it is written.
* **A capture with the marker but no snapshot after it is INCOMPLETE, not
  markerless (R4-5) — but it may not mask a red reading (F1).** Round 4's arm
  set its completion snapshot only from a valid snapshot scanned after the
  marker, so a truncated capture printed "this capture carries no USERSPACE
  TEST COMPLETE" -- a false sentence -- and then skipped the bound, which is the
  one shape the bound exists for. A CLEAN truncated capture now exits 2 with
  `census incomplete at completion marker`; a RED one exits 1 and names its
  threads, with the truncation reported on the age line rather than deciding
  the verdict. The marker is detected independently of the snapshot parse, so
  the two facts -- the marker's presence and a following snapshot's presence --
  are reported separately instead of one standing in for the other.
  <!-- claim-lint:ok: the two arms are pinned by
       a_red_strand_in_a_truncated_capture_is_still_reported_red and
       a_clean_truncated_capture_is_incomplete_at_the_completion_marker in
       tests/x86_gate_verdict_test.rs, and the code order by
       host_consumers_have_no_removed_record_dependency; the mutation table
       below is what each reddens. -->
* **The age line does not depend on argument order (R4-6).** It was computed
  from a flag streamed over `cat -- "$@"`, in a script whose header publishes
  "Argument ORDER DOES NOT MATTER". The captures are now awk OPERANDS and the
  marker and its snapshot are located by position within ONE capture. Run on
  `round4/gate-green/boot1/` in both orders, the whole output is byte-identical,
  `lines=4903` included -- which also closes the round-3 R3-12 residual, where
  `lines=` read 4902 one way and 4903 the other.

On the three committed round-3 gate captures:

| capture | age line |
|---|---|
| `round3/r3-head-green/` | `793 ms (newest cadence snapshot seq=18 at 53611 ms, completion snapshot seq=19 at 54404 ms, bound 15000 ms)` |
| `round3/r3-idle-cadence/` | `1190 ms (newest cadence snapshot seq=7 at 49191 ms, completion snapshot seq=8 at 50381 ms, bound 15000 ms)` |
| `round3/r3-idle-strand/` | `not measurable -- this capture carries no USERSPACE TEST COMPLETE` |

The arm is pinned by `tests/x86_gate_verdict_test.rs`:
`a_fresh_census_at_the_completion_marker_passes_and_prints_the_age` and
`a_stale_clean_census_at_the_completion_marker_is_not_a_pass` use the same
fixture shape and the same `stranded=0`, differing only in the cadence gap, and
both take the bound from the census rather than restating it (F4). The
truncation arms are a PAIR on the same bytes --
`a_red_strand_in_a_truncated_capture_is_still_reported_red` and
`a_clean_truncated_capture_is_incomplete_at_the_completion_marker` both build
the truncated shape by deleting `seq>=29` from the committed
`round4/gate-green/boot1/` capture and differ only in `stranded`/`tids`, so what
they hold is the PRECEDENCE and not the existence of either arm. The first is
round 5's `a_marker_with_no_snapshot_after_it_is_incomplete_rather_than_markerless`
inverted: that test asserted `Some(2)` on the red fixture, which is the outcome
F1 called a regression. `the_age_line_does_not_depend_on_argument_order` reads
the same capture both ways. 21 tests, 0 failed, up from round 5's 20.

Six mutations were run at the round-6 head, each applied to a pristine file,
scored across BOTH `tests/x86_gate_verdict_test.rs` and
`tests/dispatch_strand_census_structure.rs`, then reverted byte-identically.
This is exactly what each reddens:

| mutation | tests reddened |
|---|---|
| truncated-at-marker arm moved AHEAD of the strand arm (round 5's order) | 2: `a_red_strand_in_a_truncated_capture_is_still_reported_red`, `host_consumers_have_no_removed_record_dependency` |
| truncated-at-marker arm replaced by `if (0)` | 2: `a_clean_truncated_capture_is_incomplete_at_the_completion_marker`, `host_consumers_have_no_removed_record_dependency` |
| awk operands reverted to `cat -- "$@"` streaming | 1: `the_age_line_does_not_depend_on_argument_order` |
| `stale_limit_ms` 15000 -> 300 | 5: `accepts_a_complete_green_cohort_at_the_expected_floor`, `highest_seq_snapshot_wins_even_when_two_share_a_physical_line`, `rejects_a_fault_killed_test_and_names_the_process`, `rejects_a_tally_below_the_expected_exit_floor`, `the_staleness_bound_has_exactly_one_copy_in_the_repository` |
| rc=4 sentence restates the bound instead of reading it back | 2: `host_consumers_have_no_removed_record_dependency`, `the_staleness_bound_has_exactly_one_copy_in_the_repository` |
| a second copy of the bound added to a census consumer | 1: `the_staleness_bound_has_exactly_one_copy_in_the_repository` |

The first two rows are the anti-vacuity pair F1 asks for: swapping the two arms'
order reddens the red-fixture test and NOT the clean-fixture one, and silencing
the truncation arm reddens the clean-fixture test and NOT the red-fixture one.
Each also reddens the structure test that pins the code order, which is the
point of pinning it by position.

The `15000 -> 300` row is worth reading carefully. The three age tests no longer
flip on it, because they now derive the bound — that is what F4 asked for, and
it means **no test pins the bound's VALUE any more**. What reddens instead is
the derivation check (the bound would fall below #766's measured 10318 ms
maximum) plus four gate-level tests whose green fixtures become stale under a
300 ms bound. A bound change that stays above the measured maximum is a
one-line change that no test opposes, by design; the header and
`693-RCA-2026-09-02.md` are what justify the number.
<!-- claim-lint:ok: the 6 mutations were applied and reverted at the round-6
     head; each row's reddened-test list is the `cargo test` output of both
     suites under that mutation, and the baseline before and after was 0
     failures. -->

### The 30 surviving records, by function

Round-3 finding R3-9: the record ratchet in
`tests/dispatch_strand_census_structure.rs` said a change there "forces the
equivalence document's surviving-record table to be updated", and this document
had no such table -- only the parenthetical `30 records; 11 error, 9 trace,
8 info, 2 debug`. The ratchet's comment now says what it enforces, and the
breakdown it referred to is here. It is by FUNCTION, not by line: line pins in
this branch's documents have gone stale twice already (findings F18 and R3-2).

| function in `kernel/src/interrupts/context_switch.rs` | debug | error | info | trace | total |
|---|---:|---:|---:|---:|---:|
| `note_dispatch_guard_unavailable` | 0 | 1 | 0 | 1 | 2 |
| `restore_userspace_thread_context` | 1 | 3 | 1 | 3 | 8 |
| `save_current_thread_context_with_guard` | 0 | 3 | 0 | 1 | 4 |
| `save_kthread_context` | 0 | 0 | 0 | 1 | 1 |
| `setup_first_userspace_entry` | 0 | 0 | 3 | 0 | 3 |
| `setup_kernel_thread_return` | 0 | 1 | 0 | 0 | 1 |
| `switch_to_thread` | 1 | 3 | 4 | 3 | 11 |
| **total** | **2** | **11** | **8** | **9** | **30** |

```
awk '
  /^(pub )?fn /{f=$0; sub(/\(.*/,"",f); sub(/^.*fn /,"",f); fn=f}
  /log::(trace|debug|info|warn|error)!/{lvl=$0; sub(/.*log::/,"",lvl);
    sub(/!.*/,"",lvl); count[fn"|"lvl]++; grand++}
  END{for (k in count) print k, count[k]; print "GRAND", grand}
' kernel/src/interrupts/context_switch.rs | sort
```
<!-- claim-lint:ok: every cell above is a line of that command's output, run at
     this head; GRAND is 30 and matches the ratchet's own count. -->

19 of the 30 are non-`error`, and they sit inside the dispatch functions
`CLAUDE.md` forbids serial output in. That is disclosed, not defended: the
measurement below is why the whole class is not removed, and the 22 boots
behind that measurement are now committed at
`serials/775/round3/case-d-broad-removal/` (finding R3-10).

## The production profile

`serials/775/production/` (round 2) and `serials/775/round3/r3-production/`
(round 3). Boots of `docker/qemu/run-x86-prod-profile-boot-test.sh`, whose own
build line passes no `--features` flag at all.

**Round 2, 2 boots at `365c20c2`.** Both printed, at line 249 of their
transcripts, `PASS: x86 production profile reached steady state with the
teardown census at rest`, and both carried exactly 1 census marker, on COM1:

```
[SW]<K>[SW]<K>[DISPATCH_STRAND_CENSUS:saved=0:stranded=0:tids=-:tid_overflow=0:ledger_overflow=0]
```

That is the last census-CARRYING line, not the last line of the capture: it is
line 32 of a 152-line `serial_user.txt` (151 in boot 2), and about 120 lines of
console output follow it (round-3 finding N12). It also precedes init: `Added
thread 4 'init' to scheduler` is at line 519 of the same boot's COM2 capture.
So `saved=0` on those boots was structural, and round 2's explanation for the
single snapshot — that the boots had not run long enough — was wrong; each held
steady state for at least 60 seconds (`boot1/gate.txt`: `console prompt count
over 60s: 1 -> 2`). The real reason is round-3 finding N1: the only reachable
emitter at that commit was the loopback pump, which blocks itself absent
loopback traffic, and this profile carries 0 loopback packets.

**Round 3, 5 boots at this head.** 5 of 5 printed the same PASS line and 5 of 5
reported `1 -> 2`. Boot 1 carries 2 markers, both on COM2:

```
[DISPATCH_STRAND_CENSUS:seq=1:tick=4:ms=1033:saved=0:stranded=0:tids=-:tid_overflow=0:ledger_overflow=0]
[DISPATCH_STRAND_CENSUS:seq=2:tick=1370:ms=9356:saved=4:stranded=2:tids=4,10:tid_overflow=0:ledger_overflow=0]
```

`seq=1` is the pump's first pass again; `seq=2` is the idle loop, and it reads
`saved=4` — real ledger state, taken after init exists, rather than round 2's
pre-init reading. That is the N1 fix visible in the shipped kernel.

What this profile does not give is a cadence, and the reason is measurable: the
shipped kernel barely idles. Boot 1 has 1 `<I>` idle dispatch across 2947 `[SW]`
context switches, so the newest snapshot is at 9356 ms of a boot that then ran
for two more minutes. `scripts/x86-strand-census.sh` on that capture therefore
exits 1, naming thread 4 (`init`) and thread 10 (`exec_smoke`) — both parked in
a syscall at that instant, neither stranded. No in-repo consumer reads it that
way: the production gate does not call `scripts/x86-gate-verdict.sh`, and the 3
callers that do run on the test profile. The cadence measurement is
`round3/r3-idle-cadence/`, below, not this directory.

## What the heartbeat costs

Each snapshot is one line. Round 3 moves it from COM1 to COM2 (finding N8), so
both sides of the trade are now on the same channel. Measured on the committed
round-3 head capture (`serials/775/round3/r3-head-green/`,
`testing,external_test_bins`, 150 s boot):

| capture | snapshots | bytes added | capture bytes | share |
|---|---:|---:|---:|---:|
| `round3/r3-head-green/serial_kernel.txt` | 104 | 11808 | 342907 | 3.44% |
| `round3/r3-head-green/serial_user.txt` | 0 | 0 | 55458 | 0% |

Round 2's figures, for comparison, were 10 and 11 snapshots at 930 and 1051
bytes on COM1 and 0 on COM2 (`head-green/`); the count went up because the
heartbeat now runs from a context that actually executes.

Against that, the records this change removes ran 431 to 631 saves per boot on
the same profile at the pre-removal commit, over the 6 committed captures in
`case-d/boot{1..6}/serial_kernel.txt`:

| boot | saves | restores | COM2 bytes |
|---:|---:|---:|---:|
| 1 | 552 | 552 | 543031 |
| 2 | 478 | 478 | 508320 |
| 3 | 480 | 478 | 513649 |
| 4 | 431 | 431 | 496992 |
| 5 | 449 | 449 | 503034 |
| 6 | 631 | 631 | 572581 |

<!-- claim-lint:ok: the six rows above are the six committed case-d captures;
     the command is `grep -c` over each serial_kernel.txt. -->
Saves and restores are equal on 5 of the 6 — round 2 said "an equal number of
restores", which boot 3 (480 saves, 478 restores) falsifies — and round 2's
quoted ranges were computed over `boot{1..5}` while the directory holds six
(round-3 finding N9). The COM2 capture shrinks from 496992–572581 bytes there
to 342907 bytes here, a reduction of 154085 B (150.5 KiB / 154.1 kB) to
229674 B (224.3 KiB / 229.7 kB) per boot, net of the 11808 bytes the snapshots
add back.

### Why the serial lock is legal where it prints

`report_heartbeat_if_due()` is called from exactly three places
(`tests/dispatch_strand_census_structure.rs` pins the 3 counts at 1 each):
`idle_loop()` in `kernel/src/interrupts/context_switch.rs`, at the top of its
loop body beside the reclaim and IRQ-log-flush housekeeping already there;
`loopback_pump_fn`'s loop in `kernel/src/net/loopback_pump.rs`; and
`census_thread_fn` — the `kstrandd` kthread — in
`kernel/src/task/dispatch_strand_census.rs`. 0 of the 3 is the interrupt path
or the context-switch path — the two places `CLAUDE.md` forbids serial output in, and
the two places the removed records lived. `idle_loop` is a plain function the
dispatch path IRETQs into; it is not itself dispatch code. `kstrandd` is an
ordinary kernel thread that has just returned from a halt.
<!-- claim-lint:ok: 3 of 3 call sites are pinned at a count of 1 each by
     tests/dispatch_strand_census_structure.rs. -->

Three things make the emission safe there rather than merely conventional:

1. The callee refuses to emit unless `crate::arch_interrupts_enabled()` is
   true, so it cannot print from a caller that arrived with interrupts masked
   even if a future call site is added in one.
2. `serial::_log_print` samples the interrupt flag, disables interrupts, takes
   `SERIAL2`, and re-enables only if the flag was set, so across its 1 lock
   acquisition interrupts are masked. That closes the re-entrancy that
   deadlocks a logging path.
3. The rate limiter is a single `AtomicU64` compare-exchange shared by all
   three emitters, so two of them cannot both decide to print for the same
   second. Adding `kstrandd` therefore cannot raise the serial volume above
   the ceiling the other two already had.
   claim-lint:ok: the 3 emitters share the 1 `LAST_HEARTBEAT_NS`
   compare-exchange in kernel/src/task/dispatch_strand_census.rs.

The recorders themselves — `note_save`, `note_restore`, `note_exit` — remain
what they were: one bounds-checked slice index and one atomic RMW each, no
lock, no allocation, no formatting, on the paths the removed records occupied.

## Round 5 — the two producers gated on aarch64

Round-5 finding R4-10: `block_current_for_timer` and `block_current_for_io_publish`
are arch-SHARED. `block_current_for_io_publish` is reached through the public
`block_current_for_io` / `block_current_for_io_with_timeout`. Outside
`scheduler.rs` those two have 4 call sites --
`kernel/src/task/waitqueue.rs:76` and `:120`,
`kernel/src/task/completion.rs:323` and `kernel/src/net/tcp.rs:1497` -- which
is the waitqueue sleep path plus the `Completion` device-I/O path the AHCI and
virtio-blk drivers reach it through, on both arches. Round 4's aarch64 evidence
for the two producers was one 5-line COMPILE transcript and 0 serial captures.
<!-- claim-lint:ok: the 4 call sites are `grep -rn block_current_for_io
     kernel/src --include='*.rs'` at this head, minus scheduler.rs and minus
     the 8 comment lines. -->

Round 5 booted them. Both aarch64 gates were run on the ARM Mac against the
kernel source of commit `645bab38` -- a diff of `kernel/` between that commit
and this head is empty -- at most 2 QEMUs at a time:

| gate | command | boots | result |
|---|---|---:|---|
| strict | `docker/qemu/run-aarch64-boot-test-strict.sh 10`, after `cargo build --release --features boot_tests --target aarch64-breenix-kernel.json ...` | 10 | `Successes: 10  Failures: 0  Success rate: 100%  Duration: 118s`, `PASS: 10/10 boots succeeded`, exit 0 |
| production profile | `docker/qemu/run-aarch64-prod-profile-boot-test.sh`, run 5 times | 5 | exit 0 on 5 of 5, each `PASS: production profile reached bsshd with the futex oracle seam absent` |

**0 reds, so 0 to attribute.** Over the 15 committed captures,
`grep -ilE 'KERNEL PANIC|panic!|DATA_ABORT|INSTRUCTION_ABORT|Unhandled sync
exception|soft lockup'` matches 0 of 15 files, and 10 of 10 strict boots carry
`[EXEC_SMOKE:TARGET_OK]` once and `TESTS_COMPLETE` twice. 0 of the
pre-adjudicated aarch64 signatures (#555, #576, #626, #586, #609) appear.

Round 5 supported that last clause with a sentence that was false, and
round-6 finding F3 caught it: "the only `softirq` and `timer_delay` strings in
the 15 are that suite's own `[TEST:...:PASS]` and `[TEST:...:START]` markers".
Excluding `[TEST:` lines, the 15 captures carry **16 more** in 2 shapes —
`NET: pre-primed NetRx softirq for bootstrap callback re-enable` in 15 of 15
files, and one `[timer_delay] attempt=1 verdict=in-band elapsed_ms=10 ...`
record in `strict/boot1/serial.txt` alone. Neither is a #555 or #536
signature: the first is a boot-time NET initialisation record and the second is
the `timer_delay` test's own measurement, verdict `in-band`, i.e. a pass. The
attribution survives; the exhaustive census claim carrying it did not, and a
one-line grep falsified it. `serials/775/round5/README.md` carries the counts.

Captures are at `serials/775/round5/aarch64/strict/gate.txt` with a
`boot{1..10}/serial.txt` beside it, and
`serials/775/round5/aarch64/prod-profile/boot{1..5}/`, a `gate.txt` and a
`serial.txt` each. `serials/775/round5/README.md` records the head, the
commands and the red attribution.

### By-catch: a census that exits 0 without running was scored as clean

Not a review finding -- this round produced the state by accident and then
closed it. An apostrophe inside a comment in `x86-strand-census.sh`'s
single-quoted awk program terminates the program string; what is left prints
0 lines and exits 0. `scripts/x86-gate-verdict.sh` read that exit 0 as a clean
census and carried on. 6 of the 19 verdict tests stayed green against the
broken tool, `accepts_a_complete_green_cohort_at_the_expected_floor` among
them, so a gate run would have scored a boot with no census reading at all as
a pass.

The verdict script now requires a `STRAND_CENSUS:` summary line whenever the
census exits 0, and fails the gate with "the tool did not run to completion"
when it is absent. `a_census_that_exits_zero_without_a_summary_line_is_not_a_pass`
in `tests/x86_gate_verdict_test.rs` runs a copy of the verdict script beside a
stub census that exits 0 silently; deleting the arm, re-run at the round-6
head, reddens that 1 test and 0 others across both suites. 21 tests, 0 failed.

The x86 side of R4-2 is also there. The archived round-3-head control whose
gate transcript motivated the derived bound had no committed serials, so the
same control was re-run at `3495c3f3` on the same beast container and both
boots are committed in full, 3 files each. The first is
`docs/planning/green-program/sockets/serials/775/round5/x86/control-round3-head/boot1/gate.txt`,
with the two serial captures beside it, and `boot2` is its sibling:
`GATE: PASS` on 2 of 2, ages 435 ms and 805 ms, both under the bound the census
printed with them.

## Round 6: what it changed, and what it did not build

Round 6 answers findings F1 through F8 and the round-4 leftover R4-12. It edits
**no kernel source**: `git diff --stat -- kernel/` at the round-6 head is empty,
so the build transcripts recorded below are still transcripts of these bytes
and no boot was re-run to support this round. The 4 files it touches are
`scripts/x86-strand-census.sh`, `scripts/x86-gate-verdict.sh`,
`tests/x86_gate_verdict_test.rs` and `tests/dispatch_strand_census_structure.rs`,
plus this document and two serial READMEs.

Host tests at the round-6 head, one target per invocation:

| suite | result |
|---|---|
| `tests/*_structure.rs`, 26 targets | 26 exit 0, **506 passed**, 0 failed, 0 lines matching `^(warning\|error)` |
| `tests/x86_gate_verdict_test.rs` | **21 passed**, 0 failed, exit 0 |

506 is round 5's 505 plus
`the_staleness_bound_has_exactly_one_copy_in_the_repository`; 21 is round 5's
20 with `a_marker_with_no_snapshot_after_it_is_incomplete_rather_than_markerless`
replaced by the pair `a_red_strand_in_a_truncated_capture_is_still_reported_red`
and `a_clean_truncated_capture_is_incomplete_at_the_completion_marker`.

**R4-12, closed.** Round 5 corrected issue #783's "4/9 red" in a comment but
left the title itself saying it. The title now reads `... (production-profile
gate 5/9 red)`; `gh issue view 783 --json title` returns that string. One edit,
no other field touched.

**F8, a record-keeping defect, superseded rather than rewritten.** Commit
`7f319a1c`'s message records its claim-lint invocation as
`--files <the 5 files>` — a placeholder where the command should be. The
invocation was real and exited 0, and re-running the lint confirms it, but the
message is pushed history and is not rewritten. The verbatim command is in this
round's notes and in the PR body, and the round-6 commits carry real, named
invocations.
<!-- claim-lint:ok: the per-commit claim-lint lines are listed in the round-6
     notes and reproduced in the PR body; `git log --format=%B` over the
     round-6 commits is what produced that list. -->

## Builds and tests at the head

`serials/775/round5/builds/` holds this round's transcripts and
`serials/775/round4/builds/` the previous round's; `serials/775/round3/builds/`
and `serials/775/builds/` hold the two before that. Each build command was
forced to recompile by touching `kernel/src/task/dispatch_strand_census.rs`
first, so each transcript is a real compile rather than a cache hit.

| command | round-5 result | round-4 result |
|---|---|---|
| `cargo build --release --features testing,external_test_bins --bin qemu-uefi` | `Finished ... in 16.32s`, exit 0, 0 lines matching `^(warning\|error)` | `Finished ... in 13.53s`, exit 0, 0 such lines |
| `cargo build --release --bin qemu-uefi` | `Finished ... in 16.07s`, exit 0, 0 such lines | `Finished ... in 12.58s`, exit 0, 0 such lines |
| `cargo build --release --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64` | `Finished ... in 6.78s`, exit 0, 1 such line | `Finished ... in 6.34s`, exit 0, 1 such line |
| the same aarch64 command with `--features boot_tests` (the strict gate's input) | `Finished ... in 7.58s`, exit 0, 1 such line | not run |

Round-5 transcripts are in `serials/775/round5/builds/`, and that README
records which commit each ran at and why the head's build inputs are the same
bytes. Round-5 test totals: 26 structure targets, 505 passed, 0 failed, 0
warning/error lines; `tests/x86_gate_verdict_test.rs` 20 passed, 0 failed.

The two x86 builds ran on the beast `breenix-x86` container; the aarch64 builds
ran on the ARM Mac. In both rounds `tests/*_structure.rs` were run one target
per invocation, 26 targets each time. Round 5:
`round5/builds/structure-tests-targets.txt`, 26 exit 0, 505 passed, 0 failed,
0 lines matching `^(warning|error)`; `round5/builds/verdict-test.txt`, 20
passed, 0 failed. Round 4: `round4/builds/structure-tests-26-targets.txt` with
the same 26/505/0, and `round4/builds/verdict-test.txt` at 17 passed. The three
new verdict tests are the truncated-capture arm (R4-5), the argument-order arm
(R4-6) and the silent-census arm.

<!-- claim-lint:ok: the aarch64 warning line names only the toolchain's own
     core package; the same established exception is recorded in
     docs/planning/t3g-prb/PRB-STAGE3-GATE-RESULTS.md. -->
The aarch64 warning is the pinned nightly's future-incompatibility notice for
`core v0.0.0` in the toolchain's own source tree, printed verbatim in
`round5/builds/aarch64-kernel.txt` and in `round4/builds/aarch64-kernel.txt`.
It names 0 Breenix crates.

## Round-3's builds, for comparison

`serials/775/round3/builds/` holds the round-3 transcripts; `serials/775/builds/`
holds round 2's, at `365c20c2`. Each of the 3 build commands was forced to
recompile by touching `kernel/src/task/dispatch_strand_census.rs` first, so
each transcript is a real compile rather than a cache hit.

| command | round-3 result |
|---|---|
| `cargo build --release --features testing,external_test_bins --bin qemu-uefi` | `Finished ... in 15.04s`, exit 0, 0 lines matching `^(warning\|error)` |
| `cargo build --release --bin qemu-uefi` | `Finished ... in 15.37s`, exit 0, 0 such lines |
| `cargo build --release --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64` | `Finished ... in 29.97s`, exit 0, 1 such line |

`tests/*_structure.rs` were run one target per invocation under Cargo:
26 targets, 26 exit 0, 505 passed, 0 failed, 0 lines matching
`^(warning|error)` across the 26
(`docs/planning/green-program/sockets/serials/775/round3/builds/structure-tests-26-targets.txt`). `tests/x86_gate_verdict_test.rs` ran
separately: 14 passed, 0 failed (`round3/builds/verdict-test.txt`).

<!-- claim-lint:ok: the aarch64 warning line names only the toolchain's own
     core package; the same established exception is recorded in
     docs/planning/t3g-prb/PRB-STAGE3-GATE-RESULTS.md. -->
The aarch64 warning is the pinned nightly's future-incompatibility notice for
`core v0.0.0` in the toolchain's own source tree, printed verbatim in
`builds/aarch64-kernel.txt`. No warning names a Breenix crate.

Round 2 ran the same suite at `365c20c2` and recorded 26 invocations, 26
`exit=0`, 502 passed, 0 failed (`builds/structure-tests.txt`); the round-3
count is 505 because this round adds 3 test functions.

## The round-2 captures no longer replay under the round-3 script

The marker shape changed this round (it gained `seq`, `tick` and `ms`), so the
round-2 captures under `serials/775/case-a/`, `case-b-post-removal-mutation/`,
`case-d/`, `head-green/` and `production/` carry markers the current
`scripts/x86-strand-census.sh` rejects as malformed. Running it on
`head-green/boot1` prints `no valid DISPATCH_STRAND_CENSUS snapshot found (10
malformed marker(s), first: [DISPATCH_STRAND_CENSUS:saved=0:...])` and exits 2.

That is the honest behaviour -- those bytes were produced by a kernel that
emitted a different marker -- but it means the round-2 tables in this document
replay only against the script as it was at `51d7468f`
(`git show 51d7468f:scripts/x86-strand-census.sh`), and the round-3 tables
replay against the current one. The committed `new-census.txt` files under the
round-2 directories are the outputs of the script of their day.

## Narrowings still open, and what round 3 closed

Closed this round: the multi-log reading is now order-independent and rejects
two boots (F9/N5); each marker is validated and a malformed one is counted
rather than discarding the reading (N6); an overflowed ledger exits 3 and can
no longer be reported as clean (F21); the emitted sentence says "not restored
as of the latest snapshot" rather than the overclaim round 2 printed (N4); the snapshot is
back on COM2 (N8); the idle-side emitter runs (N1).

Still open:

- **A snapshot is an instant, not the boot.** Measured in case A leg 2, and
  again in `round3/r3-production/`, where the newest snapshot names `init` and
  `exec_smoke` as not-restored at 9356 ms of a boot that ran for two more
  minutes. The predicate cannot tell a thread parked in a syscall from one
  stranded in it; only the passage of time can, and only if a later snapshot
  exists.
- **Cadence no longer depends on the CPU idling, but it is not periodic;
  staleness is bounded only at the completion marker.** Round 4 added
  `kstrandd`, which sleeps on the scheduler's timer heap, so a cadence exists on
  a profile that never idles and carries no loopback traffic: 6 of 6
  zero-feature production boots publish **37 to 55** snapshots each
  (`round4/production/`: 54, 37, 54, 55, 53, 40), against 1 or 2 on the six
  lost-wake boots. Those are **two distinct populations of 6, not a paired
  diff** -- the fixed arm was re-run at a later head so the second producer's
  fix is in it too. What is now ASSERTED is the age of the newest cadence
  snapshot at the completion marker, bounded at the census's own
  `stale_limit_ms` with a gate FAIL above it. What is NOT bounded: staleness at the END of a boot (the capture carries
  no end-of-boot timestamp, so on a profile with no completion marker — the
  production one — the census says the age is not measurable and prints the
  observed gaps instead), and the cadence in the MIDDLE of a boot, which the
  two gate captures show going 19939 ms and 17888 ms without a snapshot. A
  wedge that stops `kstrandd` running at all would still go unbounded on the
  production profile.
  <!-- claim-lint:ok: the six counts are `grep -c DISPATCH_STRAND_CENSUS` over
       round4/production/boot{1..6}/serial_kernel.txt, run at this head; round
       4 published the range as "39 to 54" and the arms as "the same six
       boots" (round-5 finding R4-3). -->

- **What the limiter does and does not promise.** It caps emission at one per
  second; no mechanism sets a floor. Measured on the round-3 captures, with
  the pump's heartbeat disabled: the idle loop alone held a maximum gap of
  1204 ms across its 97 idle-driven snapshots on the test profile
  (`round3/r3-idle-cadence/`; round 3 published 1190 ms across 98, which
  counted the completion-site snapshot the limiter does not govern — round-4
  finding R3-1), and 2004 ms across 90 on the wedged one
  (`round3/r3-idle-strand/`). On the shipped profile at the round-3 head, boot
  1 of `round3/r3-production/` published 2 snapshots 8323 ms apart and then 0
  more for the rest of the boot; `kstrandd` is what closed that, and
  `round4/production/` is the same profile publishing throughout. A wedge that
  keeps `kstrandd` off the CPU would still stop the cadence.
- **`ledger_overflow > 0` is now rc=3, and rc=3 is fall-through, not FAIL.**
  `scripts/x86-gate-verdict.sh` reports `STRAND CENSUS INCOMPLETE ... NO usable
  strand evidence in either direction` and continues to the ordered first-cause
  checks, exactly as it does for rc=2. A boot whose ledger overflowed is
  therefore judged by the other checks alone. Reaching the arm needs a TID at
  or above `LEDGER_CAPACITY` = 4096; saved-blocked populations in the captures
  this document cites run 0 to 11. Of the 215 snapshots carried by the round-2
  capture files (`find serials/775 -name 'serial_*.txt' | xargs grep -oh
  DISPATCH_STRAND_CENSUS | wc -l`), 215 read `ledger_overflow=0` and 215 read
  `tid_overflow=0`; of the 294 in the round-3 captures, 294 read both as 0.
  Round 2 quoted 260 for that universal, which came from `grep -rho` over the
  whole directory and so counted the READMEs, gate transcripts and census
  outputs that echo the markers as well (round-3 finding N10).
- **`drain_loopback_from_idle()` in `main.rs`'s `idle_thread_fn` is dead** for
  the same reason the heartbeat was: that body is not dispatched. #775 does not
  move it, because doing so changes loopback behaviour and is unrelated to the
  census.
- **The broad record removal is not shipped.** Removing the 22 non-`error` log
  records from `kernel/src/interrupts/context_switch.rs` — the whole class, not
  the 3 finding F15 names — was built and measured, and reddened the x86
  production-profile gate:

  | arm | boots | pass | fail | failure signatures |
  |---|---:|---:|---:|---|
  | `51d7468f`, the pre-round-3 head | 8 | 6 | 2 | `bsshd started != 1` (:1016) x2 |
  | census on COM2 + idle hook, 33 of 33 records kept | 5 | 5 | 0 | — |
  | + only the 3 records F15 names removed (this head) | 5 | 5 | 0 | — |
  | + the 22 non-error records removed | 9 | 4 | 5 | prompt count 3 or 0 (:942/:952/:953) x4, `bsshd started != 1` x1 |

  The prompt-count signature appears on 4 of 9 boots of the broad arm and on 0
  of 13 boots of the other two, so this branch ships the narrow removal and
  pins the surviving set as a census (30 records; 11 error, 9 trace, 8 info,
  2 debug) in `tests/dispatch_strand_census_structure.rs`, broken down by
  function in the round-4 table above. The 5 boots of the shipped arm are
  `round3/r3-production/gate-{1..5}.txt`; round-3 finding R3-10 was that the
  other 22 had no committed evidence, and they are now at
  `round3/case-d-broad-removal/`, 22 summary rows plus the 4
  prompt-signature boots in full. The regression is filed as #783, with the
  signature, the A/B recipe and the committed specimen paths: the logging was
  load-bearing for something, and #775 does not know what.
