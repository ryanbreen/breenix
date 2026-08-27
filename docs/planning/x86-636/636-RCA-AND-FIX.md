# #636 — RCA and fix: the loopback enqueue now schedules its own delivery

Branch `fix/636-loopback-fin-delivery`, two commits on `main` @ `7fd74a91`.

## Outcome up front

The experiment's mechanism class was right and its inference was one step short.
Loopback delivery on x86 had **no owned path** — but not because `kloopbackd` was
slow or asleep. It was because **every kick the enqueue could give is
edge-triggered**: `Scheduler::unblock()` acts on a `Blocked -> Ready` transition
and returns `AlreadyRunnable` — a silent no-op — for a pump that is runnable but
not running. Measured on a failing boot, not argued:

```
loopback: ... drain_contended=0 drain_take_abandoned=0 drain_completed=55
          max_residency_ticks=595 slow_deliveries=2
          pump_tid=3 pump_passes=4 pump_rearms=0 pump_rearm_from_sched=0
          pump_wakes=32 pump_wake_rejected=0 pump_wake_already_awake=29
```

Thirty-two wakes, **twenty-nine dropped as AlreadyRunnable**, zero re-arms from
`schedule()` (which is `unblock()`-based and therefore blind in exactly the same
state), and **four pump passes in the whole boot**. Contention is ruled out by
the same line: `drain_contended=0`, `drain_take_abandoned=0`.

The fix gives delivery an owner that needs no scheduling decision at all: the
enqueue raises the NetRx softirq and the NetRx handler drains the queue.
`irq_exit()` runs pending softirqs on the way out of the next interrupt whenever
the interrupted context was preemptible, so a queued loopback packet is
delivered within a timer tick of being queued. `kloopbackd` remains the
thread-context backstop; both kicks fire.

**No test bound was touched.** `EOF_WAKE_BOUND_MS`, `DATA_WAKE_BOUND_MS`,
`LOAD_SPIN_MS`, `WATCHDOG_AT_MS` and every gate threshold are byte-identical to
main. No Tier-1 file was modified.

## How the RCA got there (and what it corrected)

The first act was to build the instrument the kernel did not have. `#636`'s
quantity is delivery latency and nothing in the kernel could state it: the
loopback queue held packets with no notion of when they were queued. Commit 1
stamps every queued packet and reports queue-to-delivery residency, a
max-residency high-water mark, a slow-delivery count, and the draining context
(pump / syscall / idle / softirq).

Two premises died on contact with it.

1. **The x86 gate battery I first ran proved nothing** — `run-boot-parallel.sh`
   does not build, and the image on beast was three hours stale, so the "25-boot
   instrumented run" was main's old binary. Rebuilt (`--rebuild` always) and
   re-ran. Stated because the first table looked plausible and was worthless.

2. **The "~10.2 s backstop" and "the drain guard is stuck" theories are both
   dead.** `drain_contended=0` and `drain_take_abandoned=0` in a failing boot
   kill the contention story outright. The backstop note's `execv()` correlation
   is consistent with the serials but is not the cause; `execv` is simply a long
   syscall whose *exit* reaches a drain site.

3. **Residency is measured in TICKS, not milliseconds.** `crate::time::get_monotonic_time()`
   returns the raw tick counter, and on x86_64 `PIT_HZ = 200`, so one tick is
   five milliseconds there (aarch64 targets 1000 Hz, where the identity holds).
   The instrument therefore reports `residency_ticks` and refuses to carry a unit
   the kernel does not keep. See "Defect found and NOT fixed" below.

## Evidence

Every serial is preserved on beast under `/root/r636/<ARM>/serials/<ID>/` with a
per-boot `provenance.txt` (arm, batch, slot, tree, HEAD, kernel-log size, date),
plus `census.txt` (the loopback census + LOOPBACK_WAKE_TEST lines + tally) and a
600 KB kernel-log tail. Driver: `/root/run636.sh`, builder `/root/build636.sh`
(full rebuild: userspace ELFs, kernel, UEFI image, test_binaries.img, ext2.img).

### Before (main @ 7fd74a91, instrumented, 20 boots, 5-way)

| | |
|---|---|
| #636 hits (`reader_exit_15`) | 1/20, `eof wait_ms=9623` |
| max loopback residency | **256–595 ticks (1.3–3.0 s) in 12 of 20 boots**; the rest under the 50-tick report floor |
| delivering context of every slow delivery | `source=pump` |
| pump passes in the failing boot | **4** |
| pump wakes dropped as AlreadyRunnable | **29 of 32** |

### After (fix, 40 boots, 5-way)

| | |
|---|---|
| #636 hits | **0/40** |
| eof waits | 788–2080 ms (main: fast mode to 3403 ms, slow mode 6346–10383 ms) |
| max loopback residency | **below the report floor in all 40 boots** — no delivery exceeded 50 ticks |
| verdicts | 39 PASS, 1 FAIL attributed below |

Against the experiment's 180-sample main baseline (15/180 = 8.3%), 0/40 gives
Fisher p = 0.08 on the rate alone; against mutation (a) below, 0/40 vs 3/20 gives p = 0.028. The load-bearing proof is not that p-value —
it is the residency collapse, which is a direct measurement of the quantity the
fix targets, from 1.3–3.0 s in most boots to below 250 ms in all of them.

### Every non-#636 failure, attributed (UNATTRIBUTED = 0)

`FIX-b01-boot2` — the exact filed **#608** signature: `sys_read:
fd=140737454784552, buf_ptr=0x1, count=0` repeating (6976 copies inside the
600 KB tail alone), `USERSPACE TEST COMPLETE` absent, its own loopback test
having already passed (`eof wait_ms=977`). 1/40 here against the experiment's
4/180 (2.2%) measurement. Not #636, not a regression.

The mutation arms carried two more, both attributed: `MUT2-b01-boot2` is the same
#608 signature (6976 garbage-fd reads, `USERSPACE TEST COMPLETE` absent), and
`MUT2-b01-boot3`, which my harvest table shows as `UNKNOWN`, in fact **passed** —
the gate log reads `Test 3: PASS ... nonzero=0` and its tally is clean; the
`UNKNOWN` is my harvester's per-slot verdict grep colliding with an interleaved
kernel log line, a defect in the harness rather than in the boot.

### A measurement that did not go the way the story wants

The residency instrument stayed **below its report floor in the failing MUT2
boots too**: no loopback packet in any of them waited more than 50 ticks. So the
8–9 second `eof` wait is *not* one packet's queue residency, and the fix does not
work by shortening one packet's wait. What the before-picture shows is a kernel
in which loopback delivery routinely took 1.3–3.0 s (main's instrumented arm:
12 of 20 boots) and the pump had effectively stopped running (4 passes, 29 of 32
kicks dropped); the whole cohort's socket programs share that queue, and the
test's own chain — reader writes the ready pipe, load child wakes and releases
the peer, peer exits and emits the FIN, reader wakes — runs through their
progress. Giving delivery an owner collapses the max residency below 250 ms and
removes the slow mode; the step from "delivery is owned" to "this particular
9-second chain does not happen" is an inference consistent with every
measurement here, not something these batteries prove hop by hop. Said plainly
so the next round does not inherit it as a proven claim.

## The fix, in three parts

1. **The enqueue raises the NetRx softirq** (`kick_loopback_delivery()`), and the
   NetRx handler drains the loopback queue. This is the owned, timely path. The
   existing `wake_loopback_pump()` call stays: neither kick replaces the other.
2. **The empty-queue exit of `drain_loopback_rounds` now flushes TCP's deferred
   TX queue.** It did not, so a segment parked there (TCP parks what it cannot
   send during RX processing) had no owner at all until some *other* loopback
   packet happened to arrive and carry it out. Same family of defect, found while
   reading the drain.
3. **Every exit that leaves packets queued re-raises the delivery softirq**
   (`rearm_if_work_remains()`): the contended exit and the exhausted-round-budget
   exit both used to return "work remains" with nobody coming back for it.

## Mutations, proven singly

Each mutation is exactly one deleted line on top of the fix, applied on beast as
an uncommitted edit, with a full rebuild before its arm and the tree restored
afterwards (`git status` clean apart from the pre-existing untracked
`rust-fork-real/`).

**(a) Delete the loopback drain from the NetRx softirq handler** (`MUT2`, 20
boots): **the slow mode returns — 3/20 (15%)**, `eof wait_ms=` 8057, 9436, 8850,
against the fix's 0/40. Fisher exact 0/40 vs 3/20: **p = 0.028**. This is the
load-bearing line.

**(b) Delete the enqueue's own softirq raise** (`MUT`, 20 boots): **no effect
measured — 20/20 PASS, every wait 1044–2069 ms.** Reported as the null it is.
The enqueue's raise is redundant *in this cohort*, because this profile runs
e1000 traffic continuously and NetRx is raised often enough on its own that the
handler's drain finds the packet anyway. It is kept because the redundancy is a
fact about this cohort and not about the design: a pure-loopback workload raises
no device interrupts at all, and that is exactly the shape of the aarch64
registry tests (`loopback_recv_wake_when_idle`, `loopback_recv_wake_under_load`),
which drive `tcp_send` with no device traffic behind it. Removing it would make
delivery depend on unrelated hardware activity.

**(c) Each new ratchet reddens under its own single deletion.** Three validators,
  six tests, in `tests/loopback_pump_structure.rs`:
  - `loopback_enqueue_owns_its_delivery` + two rejection tests (drop the kick at
    the enqueue; drop the drain from the NetRx handler).
  - `loopback_drain_exits_own_their_leftovers` + two rejection tests (drop the
    flush from the empty-queue exit; drop the kick from `rearm_if_work_remains`).
  The validators are **census-shaped**: they count the functions that push onto
  `LOOPBACK_QUEUE` and require every one to kick, and they count the `return`
  statements in `drain_loopback_rounds` and require every one to be discharged.
  A new enqueue site or a new early return reddens them; no literal list of
  today's sites appears anywhere in them.

## Local loops

- Host structural suites: **381 tests, 0 failed** across
  `block_request_lifetime_structure, context_restore_structure,
  dma_and_log_sink_structure, exec_lock_order_structure, exit_tally_structure,
  kernel_no_neon_guard, loopback_pump_structure, net_lock_structure,
  percpu_stack_custody, serial_line_atomicity_structure,
  signal_eintr_predicate_structure, strand_handoff_structure,
  teardown_structure, x86_gate_verdict_test`.
- aarch64 kernel builds clean (zero warnings) in both the plain and the
  `--features boot_tests` profiles, soft-float target `aarch64-breenix-kernel.json`
  only.
- x86 kernel builds clean on beast (zero warning/error lines in the full
  rebuild log).
- aarch64 boot test: PASS on the first attempt. Strict aarch64 gate: **20/20**.
  (A first run failed on `init-group refusal oracle counter marker missing` —
  attributed to my own wrong build profile: that marker only exists in
  `--features boot_tests`. Rebuilt in the right profile and it passes.)

## aarch64

`kernel/src/net/` is shared, so the aarch64 kernel changed and is covered by the
boot batteries above. The change is arch-neutral by construction: `raise_softirq`
and the NetRx handler exist on both arches, and aarch64's own owned paths
(`kloopbackd` plus the three `drain_loopback_from_idle()` idle backstops in
`main_aarch64.rs`) are untouched. aarch64 shares the edge-triggered-kick defect
in the source, but it is much better defended there — three idle drain sites
against x86's one — and its deterministic registry tests
(`loopback_recv_wake_when_idle`, `loopback_recv_wake_under_load`) were already
green. The softirq path is strictly additional cover for it.

## Defect found and NOT fixed — needs an operator ruling

`crate::time::get_monotonic_time()` is documented as, and used everywhere as,
**milliseconds**, and returns the raw tick counter. On x86_64 `PIT_HZ = 200`
(`kernel/src/time/timer.rs:14`), so one tick is 5 ms and the function
**under-reports elapsed time by 5x on x86**. `kernel/src/time_test.rs` pins the
wrong premise in its own doc comment ("At 1000 Hz PIT, ticks == milliseconds")
and asserts the identity that makes the bug invisible. aarch64 targets 1000 Hz,
where the identity is correct, so this is x86-only.

I did not fold the correction into this round. Every "_ms" consumer of that
function on x86 currently gets a real-time value 5x longer than it asks for, and
making it honest makes them all 5x shorter *in real time* at once — including the
teardown grace timestamps in `tracing/providers/teardown.rs` and several
in-kernel registry test deadlines. That is a consumer audit and its own battery,
not a rider on a #636 fix, and getting it wrong would destabilise gates this
round has no business touching.

This is disclosed rather than fixed, which the campaign's own law does not let an
agent decide unilaterally — so it is put to the operator as a ruling, with the
evidence above, and a ledger `blocker` entry filed. My new observable is immune
to it: it reports ticks and names the per-arch tick period.
