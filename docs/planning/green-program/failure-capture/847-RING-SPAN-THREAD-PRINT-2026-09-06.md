# #847: the timer tick publishes the ring-span numbers; a boot test prints them

**Branch:** `fix/847-ring-span-thread-print`, base `ac1dc914`, with
`origin/main` merged in at `01cc56f1` after the gate evidence below was taken.
That merge brings in #854 (`scripts/breenix_runner.py`, `scripts/ci/ring3_check.sh`)
and a docs file; `grep` for either script in
`run-aarch64-boot-test-strict.sh`, `run-aarch64-prod-profile-boot-test.sh` and
`run-x86-boot-tests.sh` returns 0 hits, so no gate that produced the evidence
below runs either of them, and the merge touches 0 of the 5 files this round
changes. The structural suites and `claim-lint` were re-run at the merge commit
(§8).
**Ruling:** R188 -- the timer tick must not print.
**Lane:** #847 fix-forward (R157 small-PR ratchet). This is a main-reddening
regression from PR #852, which pinned the `[RING_SPAN:...]` marker on two
gates; the cap exception for it is recorded on the lane ledger.

## 1. The defect

`ring_span_self_check::report()` (`kernel/src/tracing/providers/irq.rs`,
`#[cfg(feature = "boot_tests")]`) wrote the marker

```
[RING_SPAN:cpu=0:span_ms=<N>:writes=<W>:dropped=<D>:ticks_total=<T>:tick_events=<E>]
```

with the lock-free `raw_serial_*` writers, from inside `trace_timer_tick` --
the function both timer interrupt handlers call directly on each tick. That was
lock-free by design: taking the standard logger's lock in an ISR risks the
documented interrupt-vs-logger-lock deadlock (a timer interrupt firing on the
CPU that already holds the lock).

On a `-smp 4` aarch64 boot the cost of that lock-freedom is visible. Another
CPU's serial line can be in flight on the same shared UART at the same instant,
and the two writers' bytes interleave. #847 quotes two specimens caught during
the PR-2 fix round. A third, preserved with this round at
`serials/847/red-main-interleaved-boot17.txt`, is the cleanest:

```
[TEST:memory:guard_page_exists:START]
[TEST:memory:guard_p[age_exists:PASS]
RING_SPAN:cpu=0:span_ms=1491:writes=503:dropped=0:ticks_total=3983:tick_events=62]
```

The marker's opening `[` landed inside the other CPU's `guard_page_exists` line.
Every digit of the measurement survived intact -- the ratio it carries,
3983/62 = 64.2, clears the gate's floor of 10 with six times the margin -- and
the gate still rejected the boot, because the line no longer matches the pinned
regex. That boot is otherwise a clean pass: `[TESTS_COMPLETE:111/111]`,
`[BOOT_TESTS:PASS]`, `[PIN_GUARD_ORACLE:...:verdict=PASS]`,
`[CENSUS_WIDEN_ORACLE:...:PASS]`. It was 1 of 20 boots in the #850 landing
battery of 2026-09-06T01:35Z; the issue's own smaller samples measured 2 of 6,
2 of 3 and 3 of 8, which is where its "~10%" comes from.

## 2. Why the tick must not print

The lock-free writer is the right call for the context and the wrong tool for
this job, and the two facts are independent:

- **The ISR cannot take the logger's lock.** That has not changed, and this
  round does not change it. Anything that prints from a timer tick has to be
  lock-free, and anything lock-free on a shared UART can interleave with a
  concurrent writer.
- **This particular marker does not have to be printed from the tick.** Only
  the MEASUREMENT is tick-bound: it reads CPU 0's ring at a checkpoint defined
  in ticks (`CHECK_AT_MS = 1000`, crossed by exactly one tick), and the ring's
  contents at that instant are what `span_ms`, `writes`, `dropped` and
  `tick_events` describe. Serializing six integers to a UART is not tick-bound
  at all.

So the fix separates them. The tick keeps the measurement and gives up the
print; a thread-context boot test takes the print and gains the locked writer.

### What was not chosen

- **A UART reservation token** (a CAS-based, bounded-spin claim each unlocked
  writer would check). #847 lists it as a direction. It introduces a new
  lock-free arbitration primitive on the shared serial path, which the other
  68 unlocked writers in the census would each have to adopt for it to be worth
  anything -- a much larger change than this one defect warrants, on a path
  where a new spin has a wide blast radius.
- **Documenting the false FAIL as acceptable.** It fails safe (it rejects good
  boots, it does not accept bad ones), but a gate that rejects ~1 boot in 10
  for a reason unrelated to what it measures is exactly the gate noise the
  green-gate arc's UNATTRIBUTED rule exists to stop.

## 3. What moved

### 3.1 The publisher (`kernel/src/tracing/providers/irq.rs`)

`report()` became `publish()`. The arithmetic in it is unchanged line for line:
same ring pointer idiom, same `TIMER_TICK`-typed filter, same single traversal
computing `oldest`/`newest`/`tick_events`, same `timestamp_to_nanos` conversion,
same `TIMER_TICK_TOTAL.aggregate()` read. What replaced its twelve `raw_serial_*`
calls -- six `raw_serial_str`, five `raw_serial_dec` and one
`raw_serial_newline` -- is seven atomic stores:

```rust
CPU.store(MEASURED_CPU, Ordering::Relaxed);
SPAN_MS.store(span_ms, Ordering::Relaxed);
WRITES.store(writes, Ordering::Relaxed);
DROPPED.store(dropped, Ordering::Relaxed);
TICKS_TOTAL.store(ticks_total, Ordering::Relaxed);
TICK_EVENTS.store(tick_events, Ordering::Relaxed);
READY.store(true, Ordering::Release);
```

`observe()` is untouched: the same relaxed-load early return on each tick, the
same `CHECKED.swap(true, AcqRel)` one-shot latch, now calling `publish()`.

The tick path is therefore strictly cheaper than before -- it lost the marker
line's byte-at-a-time UART MMIO writes (69 bytes of fixed text, five decimal
fields and a CRLF: 85 bytes on the wire for each of the two well-formed markers
under `serials/847/`) and gained six relaxed stores and one release store, once
per boot -- and it still takes no lock, makes no heap allocation, does no string
formatting and performs no I/O.

`cpu` is published rather than baked into the marker text, so the printer has no
independent idea of which ring produced the numbers beside it. It is
`MEASURED_CPU = 0` today, which is the same ring `report()` read, so the emitted
text is unchanged.

### 3.2 The ordering argument

There is one publisher and it runs at most once per boot: `observe()` returns
early unless `CHECKED.swap(true, AcqRel)` returns `false`, so a single tick on
a single CPU reaches `publish()`, and no second call can rewrite a slot
afterwards.

claim-lint:ok: #847 -- an argument about the `swap` in `observe()`, checkable
against the ten lines of that function, not a measured rate.

The six value stores are `Relaxed`, and `READY.store(true, Release)` follows
them. A reader that observes `READY == true` with an `Acquire` load has taken
the matching acquire edge and therefore observes those six stores. A reader that
observes `false` returns without touching a slot -- both accessors (`is_ready`,
`claim`) check `READY` first -- so a half-written set is not reachable through
this module's own API. That is the ordering contract in full: one
release/acquire pair, and no further synchronization, because the publisher
runs at most once.

claim-lint:ok: #847 -- 2 of 2 accessors in the module check `READY` before
reading a slot, and 1 of 1 writer sits behind the `CHECKED` latch; both are
countable in the file.

`claim()` additionally latches the PRINT with `CLAIMED.swap(true, AcqRel)`, so
the report is handed to at most one caller. That is what holds the marker to one
line if both call sites in §3.3 are ever live in the same build.

### 3.3 The printer (`kernel/src/test_framework/registry.rs`)

A new boot test, `ring_span_report`, registered in `TIMER_TESTS`
(`Arch::Any`, `TestStage::EarlyBoot`, `timeout_ms: 5000`):

1. `wait_for_ring_span_ready(RING_SPAN_READY_TIMEOUT_MS)`;
2. on success, `claim()` the report and emit it with `serial_println!`;
3. `TestResult::Pass`, or `Fail("ring-span self-check did not publish before
   its deadline")`.

`serial_println!` is the locked, interrupt-masked writer this framework's own
`[TEST:...]` markers go through (`kernel/src/test_framework/executor.rs`), so
the ring-span marker and those lines now serialize against each other on the
shared UART instead of racing.

**The deadline: 3000 ms of guest monotonic time**, three times the 1000 ms
publication checkpoint. The wait is bounded a second way as well, because a wait
that cannot end would hang the boot rather than fail it: `wait_for_ring_span_ready`
returns `false` immediately if interrupts are masked at the call site. Only a
timer tick can publish the measurement, and only a timer tick advances the clock
the deadline is read from, so with interrupts masked neither the flag nor the
deadline could ever move.

The wait spins (`core::hint::spin_loop()`) rather than yielding. Interrupts are
enabled, so the publishing tick still runs and the waiting thread is still
preemptible; not yielding keeps the x86 call site clear of #567, which says a
test that schedules in that boot window can poison the boot thread's resume
context.

**Two call sites, one line.** On aarch64 the registry executor dispatches the
test (`main_aarch64.rs`'s `run_all_tests()`). x86 does not dispatch the staged
registry executor at all -- that is behind `x86_staged_registry`, off by default
because of #567 -- so `run_x86_ring_span_gate()` calls the same test function
from `kernel/src/main.rs`, in the shape `run_x86_loopback_gates` already uses. It
is placed AFTER the x86 boot-test gate block, not inside it: the gates in that
block pin absolute frame, page-table and kernel-stack counts, and a bounded spin
between two of them would open a preemption window they do not expect.
Interrupts are still enabled at that point (`interrupts::disable()` comes below,
after the oracles), which is what lets the publishing tick run.

### 3.4 What did NOT move

- **The emitted line's shape.** Byte-for-byte the same fields in the same order.
- **The ring-span predicate.** Both before and after this round it is evaluated
  in the gate scripts, from the marker's fields:
  `docker/qemu/run-aarch64-boot-test-strict.sh` (`span_ms > 0` as a liveness
  sanity check, `ticks_total >= tick_events * RING_SPAN_RATIO_FLOOR` as the
  verdict) and `docker/qemu/run-x86-boot-tests.sh` (the same ratio). Neither
  script is touched by this round; the marker pin stays.
- **The two prod-profile absence checks**
  (`run-aarch64-prod-profile-boot-test.sh`, `run-x86-prod-profile-boot-test.sh`).
  The print site moved from one `boot_tests`-only module to another
  (`test_framework::registry` is `#[cfg(feature = "boot_tests")]` in full), so a
  zero-feature kernel has no call site for it. Measured: 0 `RING_SPAN` lines in
  the prod-profile boot in section 7.

## 4. Census delta

`tests/serial_line_atomicity_structure.rs`'s `UNLOCKED_MULTI_BYTE_WRITE_ANCHORS`
loses one anchor:

| | anchors | call sites |
|---|---|---|
| before | 70 | 717 |
| after | 69 | 711 |

The removed entry is
`("kernel/src/tracing/providers/irq.rs", "#[cfg(feature=boot_tests)] mod ring_span_self_check::fn report", 6)`
-- its 6 `raw_serial_str` calls. Its justification ("fires at most once per boot,
from inside `trace_timer_tick`, where the logger's lock is unavailable") was
sound about the lock and wrong about the consequence, and the comment left in
its place says so, rather than leaving a silent hole where a reader scanning the
file would expect an entry.

**Not claimed:** that the other 69 anchors are safe from the same interleaving.
They are the same accepted trade-off #847 describes -- code that must not take
the logger's lock. What changed is one writer that did not have to make that
trade at all.

## 5. Oracle: red on main -> green at HEAD

### 5.1 RED, on main's code, on the real gate

`serials/847/red-main-interleaved-boot17.txt` was captured by
`docker/qemu/run-aarch64-boot-test-strict.sh` (script default, cortex-a72,
`-smp 4`) at 2026-09-06T01:35Z, boot 17 of a 20-boot battery during the #850
landing. `kernel/src/tracing/providers/irq.rs` has not changed since `9ec2120f`,
which is an ancestor of this branch's base `ac1dc914`, so the kernel that
produced it carries main's `ring_span_self_check` unmodified.

The corrupted line is quoted in §1. Scored deterministically through the strict
gate's own no-boot replay mode, at this branch's base:

```
$ BREENIX_STRICT_SCORE_ONLY=docs/planning/green-program/failure-capture/serials/847/red-main-interleaved-boot17.txt \
    ./docker/qemu/run-aarch64-boot-test-strict.sh
SCORE: FAIL - Ring-span self-check marker missing (.../red-main-interleaved-boot17.txt)
exit=1
```

The three serials committed under `serials/847/` are stored with the repository's
LF line endings, the same normalization the existing `serials/pr2/` captures
carry; the committed blob of the red specimen scores `FAIL - Ring-span
self-check marker missing` (exit 1) and the committed blob of the green one
scores `PASS` (exit 0), so the normalization does not change what either
demonstrates.

Its `GATE_BOOT_FACTS` sibling
(`docs/planning/green-program/failure-capture/serials/847/red-main-interleaved-boot17.facts.txt`)
records `ended_by=poll_exhausted`, which is the same fact from the driver's
side: the gate's wait loop polls for the full success condition, that condition
did not become true within the window, and the scoring pass then named the
check that was failing.

### 5.2 GREEN, at HEAD

`./docker/qemu/run-aarch64-boot-test-strict.sh` at branch HEAD (script default,
20 boots, cortex-a72, `-smp 4`, native HVF), kernel rebuilt from the committed
source immediately before the run, `scripts/check-kernel-no-neon.sh` PASS on
that binary:

```
Total iterations: 20
Successes: 19
Failures: 1
Success rate: 95%
Duration: 422s
Failed iterations: 16
```

**RING_SPAN reds: 0 of 20.** Every boot in the run, the failing one included,
emitted a well-formed marker -- `grep -ahoE` for the gate's exact pinned regex
across the 20 serials returns 20 lines:

```
[RING_SPAN:cpu=0:span_ms=1512:writes=527:dropped=0:ticks_total=3987:tick_events=62]
```
(boot 1; the 20 lines read `span_ms` 1484-1568, `tick_events` 62 in each,
`ticks_total` 3899-4002, `dropped` 0 in each -- ratios 62.9 to 64.5 against the
gate's floor of 10.)

**The one failure is attributed, by exact signature, to open issue #836.**
Boot 16:

```
[FCNTL_PM_CONTENTION_ORACLE:aarch64:arm_wait_us=15:armed=0:acquired=1:holder_cpu=2:pm_busy_probe=0:calls=0:eagain=0:first_errno=18446744073709551615:first_wait_us=0:hold_safety=1:hold_done=1:joined=1:FAIL:hold_safety_release]
[TEST:syscall:fcntl_pm_contention_oracle:FAIL:fcntl contention oracle's peer released the process-manager lock on its safety deadline; the arming rendezvous did not complete]
```

#836 is that marker's `hold_safety_release` arm with the same field pattern
(`armed=0`, tiny `arm_wait_us`, `hold_safety=1`, `hold_done=1`, `joined=1`),
whose RCA is the driver taking its pin after spawning the holder. It is not on
any path this round touches, and it is temporally disjoint from this round's
addition: that test is `TestStage::ProcessContext`, and in boot 16's own serial
it starts 156 lines after `[TEST:timer:ring_span_report:PASS]` and 141 lines
after `[STAGE:sched:ADVANCE]`. The same flake appeared in PR-2's own round
(that round doc, section 6.2). Serial preserved in the tree at
`docs/planning/green-program/failure-capture/serials/847/attributed-836-boot16.txt`
(its `GATE_BOOT_FACTS` sibling alongside it); this round's new occurrence is
recorded on #836.

Host load during the run, from the gate's own `GATE_BOOT_FACTS` fields:
`qemu_at_start=0` on 20 of 20 boots, `load_at_start` 1.90 to 5.37.

An earlier 20-boot run of the same gate, on a binary built from these same
kernel edits before a comment-only rewording pass, scored 20/20 with 20
well-formed markers (`847-gate/strict-run1.log`, duration 405s). It is reported
here as the second data point it is, not merged into the run above.

### 5.3 Anti-vacuity: the gate still fails when the marker is absent

The green run's own boot-1 serial (preserved at
`docs/planning/green-program/failure-capture/serials/847/green-head-boot1.txt`),
replayed through the strict gate's no-boot scoring mode, and then replayed again
with its one `RING_SPAN` line removed and the rest of the serial left intact:

```
$ BREENIX_STRICT_SCORE_ONLY=/tmp/847-green-serial.txt ./docker/qemu/run-aarch64-boot-test-strict.sh
SCORE: PASS - /tmp/847-green-serial.txt
exit=0

$ grep -av RING_SPAN /tmp/847-green-serial.txt > /tmp/847-green-no-ringspan.txt
$ BREENIX_STRICT_SCORE_ONLY=/tmp/847-green-no-ringspan.txt ./docker/qemu/run-aarch64-boot-test-strict.sh
SCORE: FAIL - Ring-span self-check marker missing (/tmp/847-green-no-ringspan.txt)
exit=1
```

So the green above is a marker the gate actually reads, not a check that has
gone quiet.

### 5.4 What the wait cost, measured from the serial

`wait_for_ring_span_ready` spins, so it is worth showing what that spin did to
the parallel EarlyBoot cohort around it. In boot 1 the timer subsystem's kthread
reaches the test and prints `[TEST:timer:ring_span_report:START]`; the marker
follows 32 lines later, and those 32 lines are four other subsystems running to
completion --  `virtio_blk_invalid_sector`, `virtio_blk_uninitialized_read`,
`[SUBSYSTEM:scheduler:early:COMPLETE:4/4]`, `[SUBSYSTEM:filesystem:early:COMPLETE:10/10]`,
`[SUBSYSTEM:process:early:COMPLETE:6/6]` and six `memory` tests. The spinning
thread did not stall them.

The marker lands between `[TEST:memory:user_stack_alignment:PASS]` and
`[TEST:memory:kernel_stack_base:START]` -- the same point in the cohort where
main's tick-printed marker lands (the `gate-tmp-head-strict` red specimen
interleaves at exactly that pair of lines). The publication instant did not
move; only the writer did.

## 6. Mutations

Each leg was applied to the real file on disk, run, and reverted; `diff`
against a pre-mutation copy confirmed each revert byte-identical.

| leg | change | cmd | exit | assertion |
|---|---|---|---|---|
| M1: move the print back into the tick | insert `crate::tracing::output::raw_serial_str("[RING_SPAN:cpu=0:span_ms=");` beside `READY.store(true, Ordering::Release)` in `irq.rs` | `scripts/run-structure-tests.sh ring_span_report_site_structure` | **101** | 2 of 6 tests fail: `the_tick_provider_reaches_no_unlocked_serial_writer` ("the ring_span_self_check module must not write to serial ... (#847)") and `the_marker_text_is_not_in_the_tick_provider` ("the irq trace provider must not carry the `[RING_SPAN:cpu=...` marker text") |
| M1b: the same mutation, against the census | as M1 | `scripts/run-structure-tests.sh serial_line_atomicity_structure` | **101** | 2 of 9 fail -- the re-added unlocked writer has no census anchor, so the census catches the same regression from the other side |
| M2: in-memory `#[should_panic]` legs | the same insertion, applied to a source string the test holds | `scripts/run-structure-tests.sh ring_span_report_site_structure` | 0 | `moving_the_print_back_into_the_tick_would_be_caught` and `moving_the_marker_text_back_into_the_provider_would_be_caught` both pass, i.e. both assertion bodies do panic on the mutated source |
| M3: PR-2's sampling guard, deleted on disk | remove ` && tick_count & (TICK_SAMPLE - 1) == 0` from `trace_timer_tick` | `scripts/run-structure-tests.sh trace_ring_depth_structure` | **101** | 3 of 4 fail, the same shape PR-2's own M3 recorded: `the_ring_event_is_recorded_only_inside_the_sampling_guard` panics with "must gate its ring write on `tick_count & (TICK_SAMPLE - 1)`", `timer_tick_total_still_increments_unconditionally` on the same missing substring, and the anti-vacuity leg on its own fixture-assumption check (the double-mutation artifact PR-2 documented) |
| M4: marker absent from an otherwise-green serial | `grep -av RING_SPAN` over this round's own green boot-1 serial | `BREENIX_STRICT_SCORE_ONLY=... ./docker/qemu/run-aarch64-boot-test-strict.sh` | **1** | `SCORE: FAIL - Ring-span self-check marker missing`; the unstripped serial scores `SCORE: PASS`, exit 0 |
| baseline (anti-vacuity for M3) | unmutated tree | `scripts/run-structure-tests.sh trace_ring_depth_structure` | 0 | 4 passed, 0 failed -- so M3's red is the mutation, not a suite that was already red |

PR-2's own kernel-rebuild mutation leg (`TICK_SAMPLE = 1`, rebuild, real boots,
gate FAIL on the sampling ratio) is not re-run here: this round does not touch
`TICK_SAMPLE`, the guard, or the ratio the gate scores, and M3 above re-runs
that leg's ratchet half against the shipped file. **Not claimed:** that the
`TICK_SAMPLE = 1` kernel-rebuild leg was re-executed in this round.

## 7. Gates run

| Gate | Arch/profile | Marker | Command | Result |
|---|---|---|---|---|
| `run-aarch64-boot-test-strict.sh` | aarch64, `boot_tests` | pinned (regex unchanged) | script default (20 boots) at HEAD | **19/20**, 0 RING_SPAN reds, 20/20 well-formed markers; the 1 failure is #836 (§5.2) |
| `run-aarch64-prod-profile-boot-test.sh` | aarch64, no features | absent (`RING_SPAN_COUNT -eq 0`) | `./docker/qemu/run-aarch64-prod-profile-boot-test.sh` | **PASS**, exit 0; `grep -ac RING_SPAN` on the serial = 0 |
| `run-x86-boot-tests.sh` | x86_64, `boot_tests` | pinned (regex unchanged) | beast, `./docker/qemu/run-x86-boot-tests.sh 1` | **PASS**, exit 0. `x86 frame-custody gate run 1: PASS`; `x86 userspace gate: PASS - exited=110 expected>=105 nonzero=0 allowlist=0`; marker `[RING_SPAN:cpu=0:span_ms=2722:writes=31:dropped=0:ticks_total=200:tick_events=12]` (ratio 16.67 against the floor of 10), preceded by `[TEST:timer:ring_span_report:START]` / `:PASS]` -- the x86 direct call site emitting through the same registry function |

aarch64 builds, both from the committed source in a private worktree:

```
cargo build --release --features boot_tests --target aarch64-breenix-kernel.json \
    -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem \
    -p kernel --bin kernel-aarch64
-> Finished, 0 kernel warnings
scripts/check-kernel-no-neon.sh
-> PASS: 0 FP/SIMD load/store instructions in kernel .text (allowlisted & suppressed: 0)
```

x86 build, on beast (`/root/breenix-p847`, `BREENIX_GATE_TMP=/root/breenix-p847-tmp`):

```
cargo build --release --features boot_tests,testing,external_test_bins --bin qemu-uefi
-> build exit=0, warning/error lines: 0
./scripts/check-x86-dispatch-no-alloc.sh
-> PASS: 0 allocating call targets in 3 in-scope symbol(s), 19 edge(s) checked. (exit 0)
```

The x86 gate ran with 3 unrelated `qemu-system-x86_64` peers already on the host
at start (the container is shared); it passed under that load.

## 8. Suites and lints

```
tests/*_structure.rs, each via scripts/run-structure-tests.sh   -> 37/37 green
  including tests/ring_span_report_site_structure.rs            -> 6 passed, 0 failed
  including tests/serial_line_atomicity_structure.rs            -> 9 passed, 0 failed
  including tests/trace_ring_depth_structure.rs                 -> 4 passed, 0 failed
python3 scripts/test_claim_lint.py                              -> Ran 72 tests, OK (exit 0)
scripts/check-x86-dispatch-no-alloc.sh (beast, on the linked x86 ELF) -> PASS, exit 0
scripts/check-critical-path-violations.sh                       -> exit 1, byte-identical to origin/main (see §9.3)
```

Claim-discipline receipts for this round:

```
claim-lint: scripts/claim-lint.py                                    -> exit 0
claim-lint: scripts/claim-lint.py --commit-msg <msg1>                -> exit 0
claim-lint: scripts/claim-lint.py --commit-msg <msg2>                -> exit 0
claim-lint: scripts/claim-lint.py --commit-msg <msg3>                -> exit 0
```

## 9. What is NOT claimed

1. **Not claimed:** that the remaining 69 unlocked multi-byte serial writers in
   the census cannot interleave the same way. They can; they are the same
   accepted trade-off. This round removed one writer from that set because that
   one did not need to be in it (§4).
2. **Not claimed:** that #847's failure mode is shown absent by the gate run in
   §5.2. A ~1-in-10 event is not excluded by 20 boots on its own; what the run
   shows is 20 of 20 well-formed markers out of the locked writer, and the
   argument that this writer cannot interleave is structural (§2, §3.2) rather
   than statistical.
3. **Not claimed:** that `scripts/check-critical-path-violations.sh` passes.
   It exits 1 on this branch and exits 1 with byte-identical output on
   `origin/main` (`diff` of the two runs is empty); the violations it lists are
   `serial_println!` marker call sites in `kernel/src/task/scheduler.rs`, which
   this round does not touch. It did not pass before this round either.
4. **Not claimed:** any cycle-level measurement of the tick path. §3.1 argues
   from instruction shape -- twelve `raw_serial_*` calls (85 bytes of
   byte-at-a-time UART MMIO writes) removed, seven atomic stores added, once
   per boot -- not from a counter.
5. **Not claimed:** that the 3000 ms deadline is portable to a host or
   acceleration mode where the boot reaches the registry executor more than 3 s
   of guest time before the 1000 ms publication checkpoint. It is three times
   the checkpoint; a boot that misses it fails the test loudly rather than
   emitting a silent absence.
