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
| `round3/r3-head-green/` | idle loop + pump | 104 / 0 | 208 / 1342 / 27017 | `saved=11 stranded=0`, rc 0, GATE PASS |
| `round3/r3-idle-cadence/` | idle loop only | 98 / 0 | 14 / 993 / 1190 | `saved=11 stranded=0`, rc 0, GATE PASS |
| `round3/r3-idle-strand/` | idle loop only, + mutation E | 90 / 0 | 1001 / 1025 / 2004 | `saved=11 stranded=3`, rc 1, GATE FAIL |

<!-- claim-lint:ok: each row is one committed capture under
     docs/planning/green-program/sockets/serials/775/round3/ with its gate
     transcript beside it. -->
The N-to-0 column is finding N8 closed: the snapshot is on the kernel-log
channel the removed records used, not on the interactive user console. The
27017 ms gap in the first row sits between seq=1 at 550 ms and seq=2 at
27567 ms -- boot and early userspace, where neither emitter ran.

`r3-idle-cadence` is the no-loopback-emission condition. Its
`pump-heartbeat-disabled.patch` removes the single call in
`kernel/src/net/loopback_pump.rs`, so 98 of its 98 snapshots come from the idle
loop. Its first snapshot is at ms=43183, because the CPU does not idle while
the userspace test programs are running; from there to the end of the capture
at ms=139552 — 96 seconds — the largest gap between consecutive snapshots is
1190 ms against a 1000 ms limiter. Under the round-2 wiring this capture would
have carried 0 idle-driven snapshots.

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

`report_heartbeat_if_due()` is called from exactly two places
(`tests/dispatch_strand_census_structure.rs` pins both counts at 1):
`idle_loop()` in `kernel/src/interrupts/context_switch.rs`, at the top of its
loop body beside the reclaim and IRQ-log-flush housekeeping already there, and
`loopback_pump_fn`'s loop in `kernel/src/net/loopback_pump.rs`. Neither is the
interrupt path or the context-switch path — the two places `CLAUDE.md` forbids
serial output in, and the two places the removed records lived. `idle_loop` is
a plain function the dispatch path IRETQs into; it is not itself dispatch code.

Three things make the emission safe there rather than merely conventional:

1. The callee refuses to emit unless `crate::arch_interrupts_enabled()` is
   true, so it cannot print from a caller that arrived with interrupts masked
   even if a future call site is added in one.
2. `serial::_log_print` samples the interrupt flag, disables interrupts, takes
   `SERIAL2`, and re-enables only if the flag was set, so across its 1 lock
   acquisition interrupts are masked. That closes the re-entrancy that
   deadlocks a logging path.
3. The rate limiter is a single `AtomicU64` compare-exchange shared by both
   emitters, so two housekeeping threads cannot both decide to print for the
   same second.

The recorders themselves — `note_save`, `note_restore`, `note_exit` — remain
what they were: one bounds-checked slice index and one atomic RMW each, no
lock, no allocation, no formatting, on the paths the removed records occupied.

## Builds and tests at the head

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
- **Cadence depends on the CPU idling, and staleness is unbounded.** The
  limiter caps emission at one per second; no mechanism sets a floor. Measured:
  with the pump's heartbeat disabled, the idle loop alone held a maximum gap of
  1190 ms across 98 snapshots on the test profile
  (`round3/r3-idle-cadence/`), and 2004 ms across 90 on the wedged one
  (`round3/r3-idle-strand/`) — but on the shipped profile, which barely idles,
  boot 1 of `round3/r3-production/` published 2 snapshots 8323 ms apart and
  then 0 more for the rest of the boot. A wedge that spins a CPU rather than
  idling would do the same. The capture carries no end-of-boot timestamp, so
  how stale the newest snapshot was when the boot ended is not derivable from
  the log; what the census prints instead is the observed gaps.
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
  2 debug) in `tests/dispatch_strand_census_structure.rs`. The 5 boots of the
  shipped arm are `round3/r3-production/gate-{1..5}.txt`.
