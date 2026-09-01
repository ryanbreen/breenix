# #728 x86 recapture round — host-load-corrected, still not captured

Branch `fix/728-ext2-lock-discipline`, evidence gathered against
`1744d98e` (code bytes identical to `85d08733`; `1744d98e` only adds the
prior round's docs). This round's premise, from the dispatching task:
the beast host had been crippled for days by an orphaned `ugrep` pinning
~10 cores; the coordinator killed it before this round started, and the
task was to re-run the x86 capture on the now-quieter host.

## 1 of 1 host-quiet confirmation performed, with an honest caveat

`uptime` sampled repeatedly across the ~30-minute capture window on the
physical beast host (40 cores):

| time (UTC) | load avg (1m, 5m, 15m) |
|---|---|
| 21:40:28 | 10.80, 18.62, 20.18 |
| 21:40:45–21:40:55 | 9.79–10.21, ~18, ~20 |
| 22:01:45 | 6.00, 11.99, 16.20 |
| 22:06:02 | 2.28, 6.60, 12.94 |
| 22:11:00 | **24.21**, 14.96, 14.50 |
| 22:12:13 (inside the guest) | 0.28, 0.38, 1.02 |

The orphaned `ugrep` itself was confirmed present but **idle**: two
`ugrep` processes (PIDs 2555202/2563091, started Aug19) still exist but
show 0% CPU and only 38s of accumulated CPU time each — consistent with
the coordinator's kill having actually stopped the runaway behavior, not
merely masked it. No single process at ~900%+ CPU was found this round
(the profile the prior investigation attributed to the orphan). The
one load spike observed (22:11:00, 24.21) was checked live via
`ps aux --sort=-%cpu`: the top entries were three `bfs` file-search
processes (~10-12% CPU each) and a PR-review `claude` subprocess
(~7%), i.e. ordinary multi-tenant beast usage, not a recurrence of the
crippling process — reported here, not fought. **Honest caveat**: load
is bursty on shared infrastructure; this round's host was quiet on
average and did not have the crippling week-long process, but it was
not silent the entire window.

The `breenix-x86` Incus VM's own internal load stayed at or near zero
throughout (0.15–0.28); the previous round's own committed evidence
reports the same shape for the guest
(`docs/planning/green-program/nic-bus/serials/728-prove-round2/README.md:69,72`:
"physical host load average 16-25 throughout both attempts ... [a]
single non-breenix tenant process ... ~870-960% CPU") — the contention
when present is at the physical host, not the guest.

## Leg A (RED) and Leg B (GREEN) — still NOT captured, and now better explained

Stale processes from an earlier round were found already running (left
running past that round's own final snapshot per
`728-prove-round2/README.md:64` — see "Stale state found and cleared"
below) and killed first, then GREEN/RED were **relaunched fresh** at
`85d08733`/reverted-`85d08733` bytes on the
confirmed-quiet host, using the documented single-hunk revert for RED
(`kernel/src/fs/ext2/mod.rs`'s x86 `ext2_lock_can_sleep()` arm regains
the `interrupts_enabled()` conjunct dc4cb536 removed — diff below,
harness-only `_red`-suffixed output dir, `docker/qemu/run-ext2-lock-race-gate.sh`
otherwise untouched):

```diff
--- a/kernel/src/fs/ext2/mod.rs
+++ b/kernel/src/fs/ext2/mod.rs
@@ -1769,6 +1769,9 @@ fn ext2_lock_can_sleep() -> bool {
         if crate::per_cpu::in_interrupt() {
             return false;
         }
+        if !<crate::arch_impl::x86_64::X86Cpu as crate::arch_impl::traits::CpuOps>::interrupts_enabled() {
+            return false;
+        }
         crate::per_cpu::preempt_count() == 1
     }
 }
```

Both runs (`X86_BOOT_TIMEOUT=3600 X86_POLL_BOUND=1800`, `--no-build`
against already-fresh binaries) reached the leg cleanly — GREEN spawns
`lockrace_holder`/`lockrace_contender` at kernel-log line 14670, RED at
14674 — and both then exhibit the same shape the previous round's own
committed evidence recorded
(`docs/planning/green-program/nic-bus/serials/728-prove-round2/README.md:54-63`:
both attempts "reached the leg ... and were actively scheduling ... not
wedged, just extremely slow" with "zero `LOCKRACE` output"):
active thread scheduling (repeated `Switching from thread 1194/1195 to
1/1194/1195`), **zero** `EXT2_LOCK_SPIN_STALL`, **zero** `LOCKRACE`
lines, no soft-lockup, no panic, confirmed by direct grep against the
final captured files (`x86-oracle/green-serial-kernel.txt`,
`x86-oracle/red-serial-kernel.txt`):

```
$ grep -c LOCKRACE green-serial-kernel.txt red-serial-kernel.txt
0
0
$ grep -c EXT2_LOCK_SPIN_STALL red-serial-kernel.txt
0
```

**What's new and more precise this round**: a directly-measured
post-spawn line-advance rate, sampled repeatedly on the confirmed-quiet
host, not inferred from a single before/after delta:

| window (UTC) | GREEN lines | RED lines | interval | GREEN rate | RED rate |
|---|---|---|---|---|---|
| 21:53:28 | 14673 | (pre-leg) | — | — | — |
| 21:56:47 | 14689 | 14679 | 199s | 12.4s/line | — |
| 21:57:35 | 14693 | 14683 | 48s | 12.0s/line | 12.0s/line |
| 22:01:39 | 14713 | 14703 | 244s | 12.2s/line | 12.2s/line |
| 22:06:08 | 14733 | 14723 | 269s | 13.5s/line | 13.5s/line |
| 22:11:06 | 14757 | 14747 | 298s | 12.4s/line | 12.4s/line |
| 22:12:13 (final, at kill) | 14759 | 14749 | — | — | — |

The rate is **flat at ~12–13.5s/line across the whole window**,
including through the 22:11:00 load spike to 24.21 and the
subsequent drop back to near-zero — the line-advance pace did not
visibly track the host load fluctuation either direction. Total
post-spawn advance in ~26–27 minutes of continuous observation: 89
lines (GREEN), 75 lines (RED). This is compatible with the previous
round's own measurement (~10–13s/line, made under confirmed *heavy*
contention) rather than materially faster — **the pace does not appear
to be dominantly host-contention-driven**, contradicting the leading
hypothesis carried by the last two rounds. A plausible alternative
(not chased further — out of this round's scope): double-nested
virtualization (physical host → Incus/QEMU VM `breenix-x86` →
this round's own TCG-emulated `-smp 1` test QEMU) may not deliver the
guest's own timer interrupts at real-wall-clock cadence regardless of
how busy the host is, which would make the leg's internal multi-second
completion/retry waits take far longer in real time than their nominal
duration independent of contention. This is a hypothesis for whoever
picks this up next, not a proven mechanism (no mutation was performed
to test it).

At this rate, reaching a shape comparable to aarch64's own completed
race (682 total lines in its green capture, run at a similar
verbosity) would require on the order of hours of continuous real time
per leg — outside what this round's session window could sustain. The
runs were killed (by me, as the task requires) after ~26–27 minutes of
observation with zero markers, honestly reported as **NOT CAPTURED**,
same disposition as the three prior attempts, not weakened to a
pass and not silently discarded.

**Leg C (below) is the evidence that does speak to whether the fix's
park path is reachable/effective on real x86 syscall entry** — the
in-kernel oracle harness (Legs A/B) remains the open gap.

## Leg C — historical repro config, `run-boot-parallel.sh`, `-smp 1`

Dedicated clone `/root/breenix-728-prove-hist` on `breenix-x86`, clean
`testing,external_test_bins` profile (no `ext2_lock_race` feature — the
same construction as the preserved `728-live-repro`), landed fix bytes
(`85d08733`). Two batches of 5 (the same batching the last round used),
**not** 10-at-once — see the disclosed contention artifact below for
why.

**10 of 10 boots: zero occurrences of the #728 stall shape** (no
`sys_mkdir`-adjacent freeze, no silence after a write-family syscall,
no non-terminating boot with active concurrent threads and no forward
progress):

- Batch 1 (`historical-repro/batch1-5boots-stdout.txt`): **5/5 PASS**,
  clean `USERSPACE TEST COMPLETE`, `exited=20 nonzero=0` on every boot.
- Batch 2 (`historical-repro/batch2-5boots-stdout.txt`): **5/5 PASS**,
  same clean shape.

Statistical power stated honestly: 10 boots at `-smp 1` is a modest
sample against an unknown base rate for a race-shaped bug; a clean
10/10 here is consistent with the fix working on the real syscall path
under this specific interleaving, not proof the race can never recur.
It is also, notably, a *better* result than the previous round's own
8/10 (which hit 2 unrelated pre-existing `loopback_wake_test` family
flakes, #610/#700) — this round's 10/10 did not encounter that flake
at all, within the limits of a 10-boot sample.

### Disclosed: a self-inflicted contention artifact, not a #728 defect

My first Leg C attempt ran all 10 boots concurrently in one
`run-boot-parallel.sh 10` invocation (misreading the script as
accepting a `--no-build` second argument — it doesn't parse one, so it
was silently ignored and harmless, but the concurrency was real). On
the 8-vCPU `breenix-x86` guest, this produced a **false FAIL** on boot
1: `x86 userspace gate: FAIL - USERSPACE TEST COMPLETE was absent; boot
did not finish`. Investigated rather than discarded, per this repo's
"any failure you find is your problem" discipline:

- The script's own per-test poll loop declared FAIL and sent
  `SIGTERM` to boot 1's QEMU process (`runner.log`:
  `qemu-system-x86_64: terminating on signal 15 from pid ...`).
- The boot's own serial log (preserved at
  `historical-repro/10at-once-boot1-contention-artifact-serial.txt`,
  4224 lines) shows **no ext2/lock activity anywhere near the tail** —
  it is a normal HTTP/TCP test (`http_test`) completing, and the log's
  own last lines are the kernel's success banner (`🎯 USERSPACE TEST
  COMPLETE`, `TEST_TALLY: exited=20 nonzero=0`, `🏁 TEST RUNNER: All
  tests passed`) — i.e. **the boot actually finished successfully**,
  moments too late for the polling script's own timeout under
  10-way self-induced contention on 8 vCPUs, and was killed anyway.
- Rerunning as two batches of 5 (the concurrency the previous round
  used, and evidently the concurrency this guest tolerates without
  self-inflicted false timeouts) reproduced the clean 10/10 above with
  no FAILs at all.

This is scored as **not attributable to #728 or this fix** — it is a
concurrency artifact of my own first-attempt invocation choice, fully
explained by direct serial inspection, not silently swept aside. Raw
evidence preserved:
`historical-repro/10at-once-contention-artifact-stdout.txt` (the run
summary) and the boot-1 serial above.

## Stale state found and cleared

On arrival, two QEMU processes from a **previous round** were already
running inside `breenix-x86` (started 20:47 UTC, i.e. before this
round's host-quiet check; the previous round's own committed evidence
records leaving that same attempt-2 GREEN/RED run "running in the
background past this snapshot" — `728-prove-round2/README.md:64`).
Both were at the
identical zero-LOCKRACE, actively-scheduling state their own prior
commit (`1744d98e`) already documented and had committed evidence for
— continuing to let them run would not have produced new information,
so they were killed (`kill -9`) before this round's fresh launch, and
the fresh launch used the same output paths cleanly. This is disclosed
for the record, not to imply anything was wrong with the prior round's
own disposition of that run.

## What's still open for whoever picks this up next

- x86 GREEN via the direct oracle harness (Leg A/B) remains uncaptured
  across **four** independent attempts now (round-1 prove, round-2 fix
  investigation, round-2 prove investigation ×2, this round). This
  round's contribution is narrowing the explanation: the pace is
  essentially unchanged between a confirmed-heavily-contended host and
  a confirmed-mostly-quiet one, which weakens (does not fully refute —
  no controlled A/B mutation was run) the "it's just beast contention"
  hypothesis the last two rounds carried. A genuinely fast path to a
  verdict likely needs either a from-scratch look at why guest virtual
  time might lag real time under this double-nested TCG configuration,
  or a multi-hour unattended background run (started and checked back
  on across sessions) rather than a single session's live wait.
- Leg C's 10/10 clean result at proper concurrency is the strongest
  positive x86 evidence gathered to date that the fix holds on the real
  syscall path this round, but it remains a negative-result (absence of
  stall), not the positive "park counter nonzero" proof Leg B was
  designed to provide.
