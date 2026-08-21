# #609 — R33's "falsified" verdict is retracted (T3-G, ruling R34)

Branch `fix/609-early-kthread-dispatch`, off main `7fbee231`. This file **replaces**
`609-RCA-FALSIFIED-2026-08-21.md` (same commit history, renamed under R34). R33's central claim —
that arm A falsified the RCA mechanism and that #609 "is not reproducible here at the filed rate" —
is retracted. It was reached on evidence that this same branch's own follow-up gate runs
contradicted within the hour, and the record said so anyway. This file exists so the retraction, not
the falsification, is what a reader finds first.

## What is retracted, and why

R33's arm A (`--features force_609`) armed, exactly once per boot, a single CPU-0-pinned kthread
whose body incremented a counter and returned. **Nothing joined it.** `run_staged_tests`
(`kernel/src/test_framework/executor.rs:271-313`) spawns every subsystem kthread and *then* joins
each handle; `kthread_join` (`kernel/src/task/kthread.rs:231-247`) spins on `arch_halt()` with no
timeout. A lost dispatch is fatal only because *something is joined on it*. Arm A's stimulus had no
join, so `[FORCE609:HITS=0]` was the only outcome it could ever report — on a healthy kernel or a
broken one alike. It cannot distinguish the two, so its 20/20 negative result falsifies nothing about
the class. R32(a) required arm A to reproduce the #609 signature at ~100% before the mechanism could
be closed; an arm that structurally cannot emit the signature did not close it, and reading its
silence as a falsification was the error.

The RCA specified two forcings. **F1 — pin a real, *joined* subsystem kthread to CPU 0 via
`BOOT_TEST_CPU_AFFINITY[0]`, expected 1/1 — was never run.** F2 (the unjoined synthetic stimulus
above) was run, came back negative for a reason F1 does not share, and the stimulus was then
deleted before F1 was tried.

Read correctly, arm A's own data is evidence *for* the mechanism, not against it: `[FORCE609:ARMED]`
20/20 with `[FORCE609:HITS=0]` 20/20 means a kthread queued on CPU 0 during `EarlyBoot` is **never
dispatched for the entire stage** — which is exactly "spawned and then never dispatched," the filed
shape.

## The branch's own gate reproduced #609 — before the falsification was written up

R33's write-up rested on "290 non-forcing boots on main produced zero occurrences." Four reds from
this same branch's subsequent 100-boot/profile gate runs were not folded into that count or into the
record, and one of them is #609 itself, preserved **before** the falsification comment posted to the
issue:

| gate run | result | preserved serial |
|---|---|---|
| clean 100/profile, `cortex-a72` | **FAILED**: 609=1, 612=1 | `scratchpad/t3g/preserved-serials/clean100-cortex-a72-609-serial-8.txt`, `...-612-serial-92.txt` |
| starved 100/profile, `max` | **FAILED**: 1 UNATTRIBUTED (instruction abort) | `scratchpad/t3g/preserved-serials/starved100-max-UNATTR-iabort-serial-33.txt` |
| starved 100/profile, `cortex-a72` | **FAILED**: 1 UNATTRIBUTED (oracle marker absent) | `scratchpad/t3g/preserved-serials/starved100-cortex-a72-UNATTR-strand-serial-7.txt` |

The #609 recurrence (`clean100-cortex-a72-609-serial-8.txt`) carries the filed shape exactly:
`memory:early` reaches `COMPLETE:24/24`, no other subsystem's `START` ever prints, no abort/panic/
lockup, and a clean strand census throughout (`stranded=0`, `worst_nonprogress_ms=0` on every
sample). That last point is the RCA's own L(census-blind-spot) point, not a contradiction of it: the
class is invisible to the (pre-widening) census by construction, and it stayed invisible to the
widened census too, exactly as `609-RCA-RETRACTION-2026-08-21.md`'s widening section already
disclosed.

Rate on this branch: **1/100** on the clean leg. #609 stays OPEN and UNTOLERATED — the R33 tolerance
removal was correct and survives this correction unchanged.

## The surviving mechanism hypothesis

Falsifying arm A does not restore the original RCA chain unmodified — L3 (a context-destroying
switch) is still not shown to be the firing mechanism, and neither is a bare "CPU 0 takes a tick and
switches." The chain consistent with everything measured so far, including arm A's own data and the
reproduced serial:

1. `least_loaded_cpu()` breaks ties at the lowest index (RCA L4). An `EarlyBoot` subsystem kthread
   spawned from CPU 0 while all run-queues are empty lands on **CPU 0's own queue**.
2. CPU 0 runs the whole boot-test window under `preempt_disable()` (arm A's own inertness, above) and
   `kthread_join` halts rather than yields, so CPU 0 never voluntarily reschedules either. CPU 0
   cannot dispatch anything queued on itself during this window.
3. On a healthy boot, a peer CPU steals the queued kthread before the spawner's `kthread_join` on it
   would matter (RCA L5). #609 is the case where that steal fails, or loses a race against the join
   check, closely enough that the kthread is **Ready and queued, forever**.
4. The spawner is blocked in `kthread_join` on that handle, spinning `arch_halt()` with no timeout. It
   halts at the join permanently. Nothing else in `run_staged_tests` runs after it, which is exactly
   why nine of eleven subsystems never print `START`.

This is a **Ready-queued-forever loss**, not a context-destroying switch. RCA L1/L2 (the double role
of tid 0 as both CPU 0's idle thread and the boot-test control-flow thread, and the four
identity-keyed disposability gates) remain correctly identified as a latent hazard but are not shown
to be *this* mechanism's cause — they are a separate, still-open concern (see #621).

**What would close this:** run F1 (a real, joined subsystem kthread pinned to CPU 0) to confirm the
join-side half of the chain; instrument or reason about the peer-steal path (why does the steal
sometimes not happen, or happen too late, for a CPU-0-queued kthread specifically) to confirm step 3.
The fix, once the mechanism above is confirmed, most plausibly lands as either a bounded wait/steal-
retry in `kthread_join`'s wait path, or a scheduling change that excludes CPU 0 from `least_loaded_cpu`
tie-break eligibility for its own queue during the boot-test window. No fix is landed by this
document; it corrects the record and re-opens the causal question that R33 wrongly closed.

## By-catch: three new field signatures, previously unfiled

The same 100-boot/profile batteries above produced three reds that do not match any pre-adjudicated
signature. All three are now filed as their own issues rather than left to ride an existing bucket:

* **#622** — `[DATA_ABORT] FAR=0x200 ELR=0xffff0000404b02e4 ESR=0x96000005 DFSC=0x5 from_el0=0`
  (`clean100-cortex-a72-612-serial-92.txt`). This is **not** the filed #612 signature
  (`FAR=0x292 ESR=0x96000021`) — different FAR, different ELR, different DFSC. It landed in the #612
  bucket only because that classifier arm was, until this same round, a catch-all for any
  `[DATA_ABORT] ... from_el0=0`. See "the #612 catch-all is removed," below.
* **#623** — `[INSTRUCTION_ABORT] FAR=0x18000 ELR=0x18000 ESR=0x86000005 IFSC=0x5 from_el0=0`
  (`starved100-max-UNATTR-iabort-serial-33.txt`). Not the filed #576 signature
  (`FAR=0x0 ELR=0x0 ESR=0x86000005`) — non-zero, round FAR/ELR rather than the null-page shape. The
  gate's `instruction_abort_signatures()` matcher was already field-exact and correctly sent this to
  `UNATTRIBUTED` (a hard fail); it was never silently absorbed, only unfiled.
* **#624** — starved-profile boot times out at ~21s of wall-clock heartbeat progress with
  `STRAND_INJECT_ORACLE`/`CENSUS_WIDEN_ORACLE` never printed
  (`starved100-cortex-a72-UNATTR-strand-serial-7.txt`). Also already correctly `UNATTRIBUTED`, now
  filed with its own number.

## The #612 catch-all is removed

`classify_serial`'s DATA_ABORT arm previously matched **any** `[DATA_ABORT] ... from_el0=0` line and
bucketed it as `612`, regardless of FAR/ESR. That is a catch-all, not a field-keyed signature match,
and #622 is the proof that it hides new shapes exactly the way the classifier's own comments say a
catch-all must not (see the instruction-abort arm's comment on this same file, which was already
field-exact for #576). As of this round the DATA_ABORT arm is field-keyed the same way: only
`FAR=0x292 ESR=0x96000021` (the filed #612 signature) buckets as `612`. Any other
`[DATA_ABORT] ... from_el0=0` — including #622's shape — is `UNATTRIBUTED`, a hard gate fail, with the
actual FAR/ELR/ESR reported in `CLASS_REASON` so a recurrence is diagnosable without re-deriving the
values from a raw serial.

## Acceptance measured on the final tree (corrected)

The R33 acceptance table below is preserved for the record, with the four reds it omitted appended.
It reported only the 25-boot/profile run; it should have reported all of them.

| check | result |
|---|---|
| structural suites (13 targets, incl. strand-handoff 20 and teardown 53) | all green, zero warnings |
| aarch64 production-profile build + no-NEON | zero project warnings; PASS (0 FP/SIMD in .text) |
| aarch64 `boot_tests` build + no-NEON | zero project warnings; PASS |
| `run-aarch64-boot-test-strict.sh 6` | 6/6 SUCCESS |
| `run-aarch64-service-sequence-gate.sh --boots 25 --profile both` | PASSED — 50/50 GREEN, every bucket 0 including the now-hard-failing 609 |
| `run-aarch64-service-sequence-gate.sh --boots 100 --profile cortex-a72` (clean) | **FAILED** — 609=1, 612=1 (612 later reclassified as #622 by this correction) |
| `run-aarch64-service-sequence-gate.sh --boots 100 --profile max` (starved, 10× `nice -n 19`) | **FAILED** — 1 UNATTRIBUTED, now #623 |
| `run-aarch64-service-sequence-gate.sh --boots 100 --profile cortex-a72` (starved) | **FAILED** — 1 UNATTRIBUTED, now #624 |
| `run-aarch64-prod-profile-boot-test.sh` | PASS (futex seam absent, 0 crash markers, 2 block-EINTR oracle markers, 0 failures) |
| `run-aarch64-full-test.sh --rebuild` | Phase 1 107/107 and Phases 1a–1e all PASS; Phase 2 red, attributed to open #593 |
| x86 custody gate on beast (`run-x86-boot-tests.sh 1`) | PASS, every oracle literal matched |

With the tolerance removed and no fix in tree, the service-sequence gate is *expected* to red
intermittently at ≥100 boots/profile on the #609 bucket specifically — that is the tightening R33
chose, and it is working as intended: it caught #609's own recurrence, plus three previously-unfiled
by-catch shapes, in the very next battery run after landing.

## Disposition under ruling R34

1. **#609 stays OPEN, UNTOLERATED.** The tolerance removal from R33 is correct and unchanged. The
   "not reproducible" framing is withdrawn; a reproduction with a preserved serial exists on this
   branch.
2. **The surviving mechanism hypothesis (Ready-queued-forever via failed peer-steal, above) replaces
   the falsified-and-then-nothing state.** No fix lands in this document; the campaign continues
   against the hypothesis above.
3. **The #612 catch-all is retired** in favor of a field-exact match, matching the instruction-abort
   arm's existing standard.
4. **#622, #623, #624 are filed** for the three by-catch shapes this correction's own review surfaced,
   consistent with the by-catch precedent #620/#621 set in the same round.
5. The stimulus deletion and the census-widening landing from R33 are **not** revisited here — the
   widening's disclosed inability to see #609 (the CPU genuinely is parked when the loss completes) is
   unaffected by this correction, and remains an honest limitation rather than a false claim.

## Landmines recorded for anyone who later lands the double-role fix

(Unchanged from the R33 record — reproduced here since the file was renamed.)

* `Scheduler::new`'s `EMPTY_STATE` uses `idle_thread: 0` as "not yet registered", and `0` is a real
  thread id. `is_idle_thread_inner(0)` is therefore true for every CPU that has not registered its
  idle thread. The sentinel has to become a value no thread can hold (`u64::MAX`) as part of that
  fix.
* After the boot tests, `launch_init_from_elf` makes init CPU 0's current thread. A bootstrap thread
  promoted to an ordinary `Running` kernel thread would immediately become a strand candidate
  (Running, not current, not queued) and turn the census red on every boot unless the fix retires it
  explicitly.
* `[boot] Reset N idle thread contexts` also covers the secondary CPUs' idle threads, which have the
  same double role during `secondary_cpu_entry_rust`. `smp.rs` already initialises their
  `context.elr_el1` to `idle_loop_arm64` at creation, so the guard looks dead for them — prove that
  with a non-mutating audit marker rather than assuming it.
