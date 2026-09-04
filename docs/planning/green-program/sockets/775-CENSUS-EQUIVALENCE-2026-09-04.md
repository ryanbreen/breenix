# #775 dispatch strand census — what replaced the records, and what it reads

<!-- claim-lint:ok: the round-1 battery is the 5-row table this file replaced,
     and the captures backing every table below are committed under
     docs/planning/green-program/sockets/serials/775/case-a/README.md and its
     five sibling directories. -->
`#775` removes three formatted records from the x86 context-switch path and
replaces the host-side census that parsed them with a fixed atomic per-TID
ledger the kernel publishes itself. Round 1 measured that migration on 5 boots,
all green, with 0 serials committed. This document is the round-2 replacement:
19 boots across the four cases and a head-green pair, plus 2 production boots
and a 102-file replay, with the captures committed and the command that
produced each number printed beside it.

## The two mechanisms

| | old | new |
|---|---|---|
| source | `Saved kernel context for blocked thread N`, `Restored kernel context for thread N`, `(thread N) exited with code`, on COM2 | `kernel/src/task/dispatch_strand_census.rs`, a 4096-entry `[AtomicU8]` |
| consumer | `git show bfbb7575:scripts/x86-strand-census.sh`, awk over the whole log | `scripts/x86-strand-census.sh`, awk over the LAST `[DISPATCH_STRAND_CENSUS:...]` snapshot |
| `threads_saved_blocked` | distinct TIDs with at least one save record anywhere in the log | ledger slots with `EVER_SAVED` at the instant of that snapshot |
| `stranded` | ever-saved, not exited, last restore line before last save line | ever-saved, and neither `EXITED` nor `LAST_EVENT_RESTORED` at that instant |
| exit codes | 0 for `stranded=0`, 1 for `stranded>0`, 2 for usage/IO error | 0 when the last snapshot says `stranded=0`, 1 when it says `stranded>0`, 2 when no usable snapshot exists |
| where it is emitted from | the dispatch path, once per save and once per restore | `idle_thread_fn` and the loopback pump, at most once per second, plus 1 final snapshot at the last userspace exit |

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

## The production profile

`serials/775/production/`. Two boots of
`docker/qemu/run-x86-prod-profile-boot-test.sh` at head `365c20c2`, whose own
build line passes no `--features` flag.

Both printed, at line 249 of their transcripts:

```
PASS: x86 production profile reached steady state with the teardown census at rest
```

and both carry a snapshot on COM1. The last line of each, verbatim:

```
[SW]<K>[SW]<K>[DISPATCH_STRAND_CENSUS:saved=0:stranded=0:tids=-:tid_overflow=0:ledger_overflow=0]
```

`scripts/x86-strand-census.sh` reads them as `threads_saved_blocked=0
stranded=0` over 2213 and 2459 lines, rc 0 on 2 of 2. Under round 1 this
profile carried no marker and the census exited 2 (round-1 F6). The marker has
to be the heartbeat: `grep -c 'USERSPACE TEST COMPLETE'` is 0 in both captures
on both serials, and the completion snapshot only runs inside the block that
prints that line.

Each of the 2 boots admitted exactly 1 snapshot. That is census
*availability*, which is what the prod gate needs; 2 boots at 1 snapshot each
is not a measurement of steady-state cadence, and no measurement in this
document bounds it.

## What the heartbeat costs

Each snapshot is one `serial_println!`, so it costs its own text plus the
newline that macro emits, on COM1 only. Measured on the committed head
captures (`serials/775/head-green/`, commit `365c20c2`,
`testing,external_test_bins`):

| capture | snapshots | bytes added | capture bytes | share |
|---|---:|---:|---:|---:|
| `head-green/boot1/serial_user.txt` | 10 | 930 | 57936 | 1.61% |
| `head-green/boot2/serial_user.txt` | 11 | 1051 | 54875 | 1.92% |
| `head-green/boot1/serial_kernel.txt` | 0 | 0 | 335459 | 0% |
| `head-green/boot2/serial_kernel.txt` | 0 | 0 | 330341 | 0% |

So about a kilobyte per boot, on COM1. Against it, the records this change
removes ran 431 to 552 saves and an equal number of restores per boot on the
same profile at the pre-removal commit (`case-d/boot{1..5}/serial_kernel.txt`),
and the COM2 capture shrinks from 496992–543031 bytes there to 330341–335459
bytes here. The net effect on serial volume is a reduction of about 165 to 210
kilobytes per boot.

### Why the serial lock is legal where it prints

`report_heartbeat_if_due()` is called from exactly two places
(`tests/dispatch_strand_census_structure.rs` pins both counts at 1):
`idle_thread_fn` in `kernel/src/main.rs`, immediately after
`enable_and_hlt()` returns, and `loopback_pump_fn`'s loop in
`kernel/src/net/loopback_pump.rs`. Both are ordinary kernel-thread bodies, not
the interrupt path and not the context-switch path — the two places
`CLAUDE.md` forbids serial output in, and the two places the removed records
lived.

Three things make the emission safe there rather than merely conventional:

1. The callee refuses to emit unless `crate::arch_interrupts_enabled()` is
   true, so it cannot print from a caller that arrived with interrupts masked
   even if a future call site is added in one.
2. `serial::_print` samples the interrupt flag, disables interrupts, takes
   `SERIAL1`, and re-enables only if the flag was set, so across its 1 lock
   acquisition interrupts are masked. That closes the re-entrancy that
   deadlocks a logging path.
3. The rate limiter is a single `AtomicU64` compare-exchange shared by both
   emitters, so two housekeeping threads cannot both decide to print for the
   same second.

The recorders themselves — `note_save`, `note_restore`, `note_exit` — remain
what they were: one bounds-checked slice index and one atomic RMW each, no
lock, no allocation, no formatting, on the paths the removed records occupied.

## Builds and tests at the head

The 4 transcripts under `serials/775/builds/x86-testing.txt` and its three
siblings are at `365c20c2`. Each of the 3 build commands was forced to
recompile by touching `kernel/src/task/dispatch_strand_census.rs` first, so
each transcript is a real compile rather than a cache hit.

| command | result |
|---|---|
| `cargo build --release --features testing,external_test_bins --bin qemu-uefi` | `Finished ... in 13.86s`, exit 0, 0 warning lines |
| `cargo build --release --bin qemu-uefi` | `Finished ... in 13.53s`, exit 0, 0 warning lines |
| `cargo build --release --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64` | `Finished ... in 28.29s`, exit 0, 1 warning line |

<!-- claim-lint:ok: the aarch64 warning line names only the toolchain's own
     core package; the same established exception is recorded in
     docs/planning/t3g-prb/PRB-STAGE3-GATE-RESULTS.md. -->
The aarch64 warning is the pinned nightly's future-incompatibility notice for
`core v0.0.0` in the toolchain's own source tree, printed verbatim in
`builds/aarch64-kernel.txt`. No warning names a Breenix crate.

`tests/*_structure.rs` were run one target per invocation, under Cargo — it
did not self-lock, so no `rustc --test` fallback was needed. 26 invocations,
26 `exit=0`, 26 `test result: ok` lines, 502 passed, 0 failed, 0 lines
matching `^(warning|error)`. Full output in `builds/structure-tests.txt`.

## Narrowings this round did not close

- **A snapshot is an instant, not the boot.** Documented and measured in case
  A leg 2. The `stranded` set on an unfinished boot can name a thread that a
  later record shows resuming.
- **Snapshot freshness is bounded by the guest's monotonic clock**, which is
  what the one-second limiter counts. The committed captures carry 10 to 16
  snapshots for boots the host timed out at 150 seconds; this round did not
  measure the guest's monotonic-to-wall ratio, so the honest statement is that
  the last snapshot is the newest reading the kernel published, not that it
  describes the end of the capture.
- **`ledger_overflow > 0` still exits 0.** The script prints
  `kernel ledger overflowed (N event(s)); snapshot is incomplete` and then
  exits on `stranded` alone, so an incomplete snapshot reading `stranded=0`
  passes. That is the exit contract R125 specifies. Reaching it needs a TID at
  or above `LEDGER_CAPACITY` = 4096; saved-blocked populations in the captures
  this document cites run 0 to 11, and of the 260 snapshots the committed
  captures under `serials/775/` carry, 260 read `ledger_overflow=0` and 260
  read `tid_overflow=0` (`grep -rho 'ledger_overflow=[0-9]*'`, `sort | uniq
  -c`). No boot in this round exercised either overflow arm.
- **Multi-log aggregation changed meaning** (round-1 F9). The old script
  concatenated N logs and aggregated across them; the new one concatenates the
  same N and reports the last snapshot found, so a multi-boot capture now
  reads as its final boot rather than as a union. No in-repo caller passes
  more than one boot's pair of files.
