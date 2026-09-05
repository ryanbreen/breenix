# #819 follow-on — the fcntl contention oracle's arming, and the verdict it printed when arming failed

Branch: `fix/819-fcntl-oracle-arming-rendezvous`. Base: `origin/main` at `9b01687f`.
Sibling of `docs/planning/green-program/syscalls/796-FCNTL-EAGAIN-2026-09-05.md`,
which introduced the oracle this round repairs.

## The red

On the aarch64 strict gate, 2 of 40 boots of the 2026-09-05 health run printed:

```
[FCNTL_PM_CONTENTION_ORACLE:aarch64:attempts=3:armed=0:holder_cpu=18446744073709551615:pm_busy_probe=0:calls=0:eagain=0:first_errno=18446744073709551615:first_wait_us=0:hold_done=1:joined=1:FAIL]
[TEST:syscall:fcntl_pm_contention_oracle:FAIL:fcntl reported a contended process-manager lock to userspace]
```

Two separate defects on one line.

**(a) The arming lost a race it could not retry out of.** The peer CPU took
`PROCESS_MANAGER`, held it for a fixed `FCNTL_PM_HOLD_US` (8 ms) measured on
CNTVCT_EL0, and released it whether or not anybody had looked. The driver polled
for the hold with interrupts enabled and preemption on. Three things make that
window missable, and the boot-test runner supplies each of the 3:

1. Subsystem test threads run concurrently. In the boot-17 serial the
   `process:proc` cohort (`process_list_populated`, `frame_custody_healthy_counters`)
   starts and finishes inside the oracle's own window, so the driver shares its
   CPU with other runnable threads.
2. `process_list_populated` takes the *blocking* `crate::process::manager()`
   (`kernel/src/test_framework/registry.rs`), and on aarch64 that call masks DAIF
   before it spins for the lock (`kernel/src/process/mod.rs`, the aarch64 arm of
   `manager()`). A thread waiting for the lock this oracle is holding therefore
   makes its whole CPU unavailable for the duration of the hold.
3. A driver descheduled at the wrong moment cannot be dispatched again until the
   hold ends -- and by then `FCNTL_PM_HOLD_ACTIVE` is back to `false` and
   `FCNTL_PM_HOLD_DONE` is `true`, which is exactly `armed=0` with `calls=0`.

`attempts=3` did not help: each retry re-opened the same 8 ms window into the
same conditions.

**(b) The verdict text was false for that arm.** `TestResult::Fail("fcntl
reported a contended process-manager lock to userspace")` was the only failure
message the test had, so an arming failure was reported as a syscall verdict.
The same line says `calls=0`: 0 of 64 calls were issued, so no call reached
`sys_fcntl` and no errno reached userspace.

Serials: `serials/819-oracle-arming/00-main-health-boot17-armed0.txt` and
`01-main-health-boot24-armed0.txt`.

## The repair — arming is a rendezvous

### Publication and ordering

| Flag | Direction | Store | Load |
|---|---|---|---|
| `FCNTL_PM_HOLD_ACQUIRED` | holder -> driver | `Release`, inside the guard's scope | `Acquire`, after the join |
| `FCNTL_PM_HOLD_CPU` | holder -> driver | `Relaxed`, before the `HOLD_ACTIVE` release store | `Relaxed`, after the join |
| `FCNTL_PM_HOLD_ACTIVE` | holder -> driver | `Release`, inside the guard's scope; `false` again before the guard is dropped | `Acquire`, in the driver's rendezvous loop |
| `FCNTL_PM_RELEASE_REQ` | driver -> holder | `Release`, in the instructions before the measured call, and on the give-up paths | `Acquire`, in the holder's acquire loop and hold loop |
| `FCNTL_PM_HOLD_SAFETY_FIRED` | holder -> driver | `Release`, before `HOLD_ACTIVE` goes false | `Acquire`, after the join |
| `FCNTL_PM_HOLD_DONE` | holder -> driver | `Release`, last, after the fields above | `Acquire`, in the rendezvous loop and after the join |

The single release/acquire pair on `FCNTL_PM_HOLD_ACTIVE` is what makes the
driver's next action -- the independent `try_manager()` probe, then the measured
call -- happen after the acquisition; the pair on `FCNTL_PM_HOLD_DONE` is what
makes the fields read after the join a settled set.

### Deadlines

| Constant | Value | What it bounds |
|---|---|---|
| `FCNTL_PM_ARM_WAIT_US` | 2 s | the driver's wait for the publication |
| `FCNTL_PM_ARM_SPIN_US` | 20 ms | how much of that wait is spent spinning before it starts halting between polls |
| `FCNTL_PM_ACQUIRE_US` | 20 ms | the holder's masked try-loop for the lock |
| `FCNTL_PM_HOLD_SAFETY_US` | 250 ms | the holder's own release deadline, reported as `hold_safety=1` |
| `FCNTL_PM_HOLD_OVERLAP_US` | 8 ms | how much longer the holder holds after the driver asks |
| `FCNTL_PM_JOIN_US` / `FCNTL_PM_SETTLE_US` | 500 ms / 20 ms | join and quiesce, unchanged |

**Why 2 s.** It is not a timing margin; it is the deadline for the one case a
rendezvous cannot fix, a holder thread the scheduler does not dispatch. It has
to sit above the sum of the holder's own bounds (20 ms + 250 ms + 8 ms = 278 ms)
so the two do not race and report each other's failure, and under the registry
entry's `timeout_ms: 10000` together with the join and settle windows. Being
late inside it costs the run no measurement, because the hold does not close
until the driver asks.

### Holder release

The holder keeps the lock until `FCNTL_PM_RELEASE_REQ`, then holds one further
`FCNTL_PM_HOLD_OVERLAP_US`, then releases. The request has to be issued *before*
the blocking call rather than after it: the call does not return until the lock
is free, so a request after it would be a request the holder waits for while the
driver waits for the holder. The overlap is what the call's wait measures --
`first_wait_us` reads at or above 8 ms against a scored floor of 1 ms.

If the request does not arrive within `FCNTL_PM_HOLD_SAFETY_US`, the holder
releases anyway and records `hold_safety=1`, which is a **failing** verdict: a
hold released on that deadline is a hold the driver did not use.

### Two designs this round measured and discarded

Both were run on the same gate, and both are recorded here because each one
failed *and said so correctly* -- which is the second deliverable working.

1. **Rendezvous with an unpinned driver.** Holder publishes and waits; driver
   halt-polls with preemption on. 2 of 20 boots printed
   `arm_wait_us=250368:armed=0:acquired=1:hold_safety=1:FAIL:hold_safety_release`
   -- the hold was up, and the driver could not be dispatched to ask for it,
   for the reason in (a)(2) above. Serial:
   `serials/819-oracle-arming/02-falsified-unpinned-driver-hold-safety-release.txt`.
2. **Holder waits for a driver go-ahead with interrupts enabled.** Driver pinned;
   holder publishes readiness, then spins unmasked for the go-ahead before taking
   the lock. 1 of 20 boots printed
   `arm_wait_us=260560:ready=1:armed=0:acquired=0:FAIL:arming_timeout` with the
   strand census reporting `queued_on_nondispatching_cpu=1:worst_queued_nondispatch_ms=1598`:
   the holder lost its CPU during the unmasked wait and never reached the lock.
   Serial: `serials/819-oracle-arming/03-falsified-holder-handshake-arming-timeout.txt`.

The shipped design takes the lesson from both: **the holder is masked from the
acquire to the release** (as it already was, for the hang recorded in the #796
doc), and **the driver is pinned with `preempt_disable()` from before the hold
can start until after the measured calls**. Neither side can be descheduled
inside the window, so neither can be starved by a third thread spinning in
`manager()`. Interrupts stay enabled on the driver -- a masked driver froze the
tick counter its own peer needed, and 1 of 3 smoke boots reported `acquired=0`.

## The verdict arms

`FcntlPmArm` replaces the boolean. Each variant carries its own marker tag and
its own `TestResult::Fail` text; only the EAGAIN arm describes a syscall result.

| Marker tag | Meaning | Reported as |
|---|---|---|
| `PASS` | 64 calls against a held lock, 0 of them EAGAIN | pass |
| `FAIL:eagain_reached_userspace` | a measured call returned EAGAIN -- #796's defect | "fcntl reported a contended process-manager lock as EAGAIN" |
| `FAIL:hold_safety_release` | the peer released on its own safety deadline | arming, "the arming rendezvous did not complete" |
| `FAIL:arming_timeout` | the peer published no hold inside the deadline | arming, "the arming rendezvous did not complete" |
| `FAIL:contention_not_observed` | a hold was seen, but the call did not wait for it | "the contention it scores was not present" |
| `FAIL:holder_not_joined` | window closed, holder did not exit and join | arming |
| `FAIL:no_peer_cpu` | no peer CPU was dispatching | arming, "no call was measured" |
| `FAIL:holder_spawn_failed` | the holder thread could not be created | arming, "no call was measured" |

Each arming arm is still **gate-red**. What changed is that the serial says
which of them happened, and the marker now carries `arm_wait_us`, `acquired` and
`hold_safety` to say why.

## The gate

`docker/qemu/run-aarch64-boot-test-strict.sh` still requires the PASS marker.
The pattern follows the new field order and pins the two new anti-vacuity
fields it can pin (`acquired=1`, `hold_safety=0`) alongside the ones it already
did; the `FCNTL_PM_WAIT_SELFCHECK` block still checks, at gate time and in the
gate's own matcher, that the pattern rejects `first_wait_us=0` and accepts a
real wait; and the FAIL scan became `:FAIL(:[a-z_]+)?\]` so a tagged arm is seen.
`docker/qemu/run-aarch64-prod-profile-boot-test.sh`'s absence assertion is
untouched -- it keys on the marker prefix, which did not change, and the
production-profile run below reports `fcntl contention oracle marker count: 0`.

`docker/qemu/run-x86-boot-tests.sh` gains one line of output and no new
requirement. Its `passed=true` conjunction already required the uniprocessor
SKIP literal fixed-string; it now also echoes the matched line into its own log,
the way it already does for `CENSUS_WIDEN_ORACLE`. Until it did, a gate log
could not be read as a receipt for this oracle line -- see the x86 evidence
section below, where a citation in this document's own first version turned on
exactly that.

## Ratchets and mutations

`tests/fcntl_pm_contention_gate_structure.rs` gains 2 tests and re-derives 1.

* `oracle_arming_is_a_rendezvous_not_an_attempt_count` -- the marker format must
  carry `arm_wait_us` and must not carry `attempts=`; no `FCNTL_PM_*` constant
  may count attempts; the rendezvous deadline must be at least 2 s.
* `verdict_arms_are_distinct_and_only_one_describes_a_syscall_result` -- census
  over the declared variants: each needs a tag and a message, the messages must
  be distinct, exactly 1 may mention EAGAIN, and no other may contain the phrase
  "to userspace".
* `oracle_pass_predicate_carries_a_wait_floor_conjunct` -- rewritten to derive
  the floor from the binding that carries it (the rendezvous has no `passed =`
  assignment), and additionally requires that binding to be read again before the
  verdict is emitted, so a floor computed and ignored also reddens.

Mutations, each applied alone against the shipped tree, each `cargo test --test
fcntl_pm_contention_gate_structure` exit 101:

| # | Mutation | Reddens |
|---|---|---|
| M1 | arming arm's message restored to "fcntl reported a contended process-manager lock to userspace" | `verdict_arms_are_distinct_and_only_one_describes_a_syscall_result` |
| M2 | `attempts=3:` re-added to the marker format string | `oracle_arming_is_a_rendezvous_not_an_attempt_count` |
| M3 | `&& first_wait_us >= FCNTL_PM_MIN_WAIT_US` deleted from the verdict binding | `oracle_pass_predicate_carries_a_wait_floor_conjunct` |

## Evidence

Built with
`cargo build --release --features boot_tests --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`,
0 kernel warnings, and `scripts/check-kernel-no-neon.sh` PASS (0 FP/SIMD
load/stores in `.text`).

### aarch64 strict gate, 20 boots at the shipping bytes

`./docker/qemu/run-aarch64-boot-test-strict.sh 20` -> exit 0, 20/20 boots.
Gate log: `serials/819-oracle-arming/04-branch-strict-x20-gate.txt`; the 20
marker lines: `05-branch-strict-x20-markers.txt`. Every line is `PASS`, and the
arming fields read:

| boot | `arm_wait_us` | `holder_cpu` | `first_wait_us` |
|---|---|---|---|
| 1 | 96 | 1 | 8266 |
| 2 | 100 | 1 | 8164 |
| 3 | 82 | 1 | 8138 |
| 4 | 91 | 1 | 8115 |
| 5 | 93 | 2 | 8166 |
| 6 | 82 | 2 | 8282 |
| 7 | 6 | 1 | 8178 |
| 8 | 334 | 1 | 8143 |
| 9 | 88 | 2 | 8264 |
| 10 | 100 | 2 | 8122 |
| 11 | 118 | 1 | 8102 |
| 12 | 82 | 1 | 8183 |
| 13 | 112 | 2 | 8184 |
| 14 | 98 | 2 | 8201 |
| 15 | 84 | 2 | 8124 |
| 16 | 272 | 2 | 8154 |
| 17 | 8 | 2 | 8142 |
| 18 | 87 | 1 | 8150 |
| 19 | 90 | 1 | 8174 |
| 20 | 404 | 1 | 8215 |

One sample line, verbatim:

```
[FCNTL_PM_CONTENTION_ORACLE:aarch64:arm_wait_us=96:armed=1:acquired=1:holder_cpu=1:pm_busy_probe=1:calls=64:eagain=0:first_errno=9:first_wait_us=8266:hold_safety=0:hold_done=1:joined=1:PASS]
```

### The wider sample

The shipped design ran 105 boots of this gate in 6 runs (5 + 20 + 20 + 20 + 20 +
20). 105 of 105 printed a `PASS` oracle line. The review round's 25 further
boots, below, found 1 that did not, so read this paragraph with that section.
Across those runs `arm_wait_us` read 6 us to 60900 us and `first_wait_us` read
8094 us to 24089 us; both outliers come from the same boot, one the strict gate
failed for an unrelated reason (below), where the guest was running slowly
enough that the driver's wait and the post-release re-acquisition both stretched.
`first_wait_us` can exceed the 8 ms overlap because the driver's call re-enters a
queue of waiters when the hold drops.

### Reds in those 105 boots, attributed

2 of 105 boots failed the gate, both on `Exec smoke did not complete`, both with
a `PASS` oracle line in the same serial: the signature of **#826** ("aarch64
strict gate: 'Exec smoke did not complete' with no fault marker and a healthy
guest -- 2/40 at be412ee9"), which main's own 40-boot health run of the same day
hit at the same rate. One is preserved at
`serials/819-oracle-arming/07-branch-826-exec-smoke-boot4.txt`.

### The review round's 25 further boots, and the arming failure in them

Re-run for the 2026-09-05 review round on the same kernel bytes plus this
round's doc comments, on the shared Mac, at a host load average of 5.39 / 7.45 /
6.84 measured immediately after the second run:

| run | gate verdict | oracle lines | reds |
|---|---|---|---|
| `run-aarch64-boot-test-strict.sh 5` | 4/5, exit 1 | 4 `PASS`, 1 `FAIL:hold_safety_release` | the arming failure below |
| `run-aarch64-boot-test-strict.sh 20` | 16/20, exit 1 | 20 `PASS` | 4 `Exec smoke did not complete`, #826, each with a `PASS` oracle line in its own serial |

So 24 of these 25 boots printed a `PASS` oracle line, and the running total for
the shipped design is 129 of 130. The 1 that did not is **#836**, filed from this
round:

```
[FCNTL_PM_CONTENTION_ORACLE:aarch64:arm_wait_us=17:armed=0:acquired=1:holder_cpu=1:pm_busy_probe=0:calls=0:eagain=0:first_errno=18446744073709551615:first_wait_us=0:hold_safety=1:hold_done=1:joined=1:FAIL:hold_safety_release]
```

The fields decode it without a second run. `hold_safety=1` says the holder
waited its whole 250 ms without seeing `FCNTL_PM_RELEASE_REQ`. `armed=0` with
`arm_wait_us=17` says the driver's rendezvous loop exited on
`FCNTL_PM_HOLD_DONE` rather than on `FCNTL_PM_HOLD_ACTIVE`, 17 us after it
started: by the time the driver reached the loop, the holder had acquired, held
out its safety deadline and finished. The driver's first `RELEASE_REQ` store on
that path is after the loop, so at least 250 ms separated the holder's acquire
from the driver reaching `arm_start`.

The interval that admits it is the one between the spawn and the pin. The driver
calls `kthread_run_on_cpu_for_test(...)` and only then `preempt_disable()`, so
the holder can start holding while the driver is still preemptible. The pin
closes the window from that call onward, which is what the falsified
unpinned-driver design needed; it does not cover the call before it. The same
serial carries the corroboration: `process_list_populated` starts and passes
between the oracle's `START` and its marker, and that test takes the blocking
`manager()`, so it can only have passed after the holder released -- it was
masked and spinning for the hold's duration, and whichever CPU it was on could
not dispatch while it was. That is root cause (a)(2) again, arriving in the
interval the pin leaves open.

Not repaired here. The candidate is to take the pin before the spawn, which
moves `preempt_disable()` across `Thread::new_kernel()` and
`spawn_on_cpu_for_test()` -- kernel-stack allocation and scheduler publication,
a path with its own history -- and a 1-in-25 liveness signature is not something
a 5- or 20-boot run can accept anyway. #836 carries the decode, the candidate
and the serial:
`serials/819-oracle-arming/11-branch-strict-hold-safety-release.txt`.

What this round's second deliverable did do is on the same line. The arm named
itself: `hold_safety_release`, with `arm_wait_us`, `acquired` and `hold_safety`
saying which rendezvous step failed, and the verdict text is the arming one
rather than the syscall one. On `origin/main` this same boot would have printed
`attempts=3:armed=0` and "fcntl reported a contended process-manager lock to
userspace" for a run whose own line reads `calls=0`.

### aarch64 production profile

`./docker/qemu/run-aarch64-prod-profile-boot-test.sh` -> exit 0,
`PASS: production profile reached bsshd with the futex oracle seam absent`, and
`Observed fcntl contention oracle marker count: 0`. Log:
`serials/819-oracle-arming/06-branch-prod-profile-gate.txt`.

### x86 (beast, `breenix-x86` Incus container)

`cargo build --release --features testing,external_test_bins --bin qemu-uefi` --
0 warnings (`grep -c warning` over the build log: 0).

`docker/qemu/run-x86-boot-tests.sh 1`, 3 runs against the same binary: 2 PASS, 1
FAIL. The uniprocessor arm prints:

```
[FCNTL_PM_CONTENTION_ORACLE:x86:arm=none:reason=uniprocessor_no_pm_contention_peer:online_cpus=1:SKIP]
```

**What the diff does and does not change here.** The `serial_println!` format
string above and its one argument are byte-identical to `origin/main`; what the
diff does change in the `#[cfg(not(target_arch = "aarch64"))]` arm is its return
value, `false` becoming
`TestResult::Fail("fcntl process-manager contention oracle has no arm on a
uniprocessor boot")`, because the function's return type became `TestResult`.
So the printed line is unchanged by construction, and that is readable in
`git diff 9b01687f...HEAD -- kernel/src/test_framework/registry.rs`; it is not
something the boot runs below establish.

**What each x86 artifact is evidence of.** An earlier version of this section
claimed the line was unchanged "in 3 of 3, byte for byte" and cited the two logs
below as the receipts. That citation did not hold, and this is what does:

| run | verdict | artifact | what that artifact shows about this line |
|---|---|---|---|
| 1 | PASS | not checked in | no reading of this line |
| 2 | FAIL (#631) | `09-branch-x86-631-clock-gettime-red.txt` | the line, transcribed under a commentary header from that run's serial -- an excerpt, not a dump |
| 3 | PASS | `08-branch-x86-boot-tests-pass.txt` | not the line: `grep -i fcntl` over that file hits only the clone's path name |
| re-run | PASS | `10-branch-x86-fcntl-oracle-line-surfaced.txt` | the line, in the gate's own stdout |

Run 3's `GATE-EXIT=0` carries the fact by entailment rather than by display:
`passed=true` in `docker/qemu/run-x86-boot-tests.sh` is one conjunction, and
`grep -qF "$FCNTL_PM_CONTENTION_ORACLE_LITERAL"` over that run's serial is a
conjunct of it, so a run cannot reach exit 0 with the literal absent or altered.
That is a fixed-string machine check, and it is a stronger reading than a
transcription; what it is not is a file the line can be read out of, which is
what citing the log implied. The gap was in the gate: it echoed its sibling
oracle lines into its own log and not this one. This round adds that echo next
to its `CENSUS_WIDEN_ORACLE` sibling, so the entailment and the receipt are the
same artifact from here on. The `re-run` row is that gate, re-run on the same
container and the same kernel binary as runs 1-3 with this round's gate-script
bytes applied; its header records the SHA.

The red in run 2 is `clock_gettime_test:1` -- `Test 3: Sub-millisecond
precision`, `Elapsed: 1332075 ns`, `FAIL: Elapsed time >= 1ms (possible PIT
fallback)`. That is the signature of **#631**, closed on 2026-09-04 having
attributed the mechanism to the still-open **#766** (x86 timer wake dispatches
only after a full round robin, p90 2592 ms). #631's own close says a fresh
occurrence of the signature is expected and is not a reopen. The same binary
passed the same gate 3 times across runs 1, 3 and the re-run, so it is not a
property of this branch.

### Host-side suites

31 of 31 `tests/*_structure.rs` suites pass, `fcntl_pm_contention_gate_structure`
among them at 4 of 4. `python3 scripts/test_claim_lint.py` -> exit 0.

```
claim-lint: scripts/claim-lint.py                                  -> exit 0
claim-lint: scripts/claim-lint.py --commit-msg <each commit>       -> exit 0
claim-lint: scripts/claim-lint.py --files <this doc>               -> exit 0
```

## What is NOT claimed

* **The oracle's arming is not immune to starvation, and this round measured
  that rather than leaving it a possibility.** 1 boot of 130 printed
  `FAIL:hold_safety_release`; it is #836, its mechanism is the spawn-to-pin
  interval, and it is written up in the review-round section above. The pin and
  the mask remove the two starvation paths this round *measured* on the way in;
  they do not remove that one. The 7 failing arms in the verdict table above are
  gate-red and each carries its own tag, so a recurrence is a failing boot with
  its cause on the line rather than a silent one.
* **No change here touches `sys_fcntl`.** The #796 repair -- the process-lookup
  preamble blocking for `PROCESS_MANAGER` instead of reporting its contention --
  is untouched. This round changes only how the oracle sets up the contention it
  measures and what it says when the setup fails.
* **The driver's pin is a real cost.** For the duration of the rendezvous no
  other thread runs on the driver's CPU. Interrupts keep flowing and the tick
  keeps advancing, so this is a scheduling stall, not a silent CPU; the measured
  stall is `arm_wait_us` plus the call window, tens to hundreds of microseconds
  plus about 8 ms on the boots above. The 2 s deadline is the worst case, and a
  boot that reaches it is a boot the gate fails.
* **The hold masks interrupts on other CPUs too, not only on the two the
  rendezvous names.** The peer holds `PROCESS_MANAGER` with its own interrupts
  masked, and on aarch64 both blocking ways into that lock --
  `crate::process::manager()` and `with_process_manager()`
  (`kernel/src/process/mod.rs`) -- mask before they acquire, not after. A third
  CPU that enters either one while the hold is up therefore keeps its own
  interrupts masked until the hold drops: the driver's notice time plus the 8 ms
  overlap on the arms that pass, and up to `FCNTL_PM_HOLD_SAFETY_US` (250 ms) on
  `hold_safety_release`, the arm the holder takes when no request arrives. That
  arm is not hypothetical here: 1 boot of 130 took it (#836), and the serial
  shows `process_list_populated` -- the concurrent caller (a)(2) above names --
  passing only after the hold dropped. This round introduces neither half of that:
  masking before blocking is the shape of those two entry points on
  `origin/main`, and it is the same property the falsified unpinned-driver
  design ran into from the other side. What this round does about it is bound it and make reaching the bound
  loud -- the safety deadline is read from CNTVCT_EL0, which advances whatever
  any CPU's mask state is, and `hold_safety=1` is gate-red. The oracle is
  `boot_tests`-only, and the production-profile gate above asserts its marker
  count at 0.
* **`first_wait_us` measures a wait, not a fairness property.** It says the call
  waited for a lock that was held; it does not say the call was the first waiter
  to get it, and under load it reads well above the overlap window.
* **#826 is not addressed here.** It is attributed, not repaired, and it is the
  reason 2 of 105 boots were gate-red.
* **The x86 arm is still a SKIP.** A uniprocessor boot has no peer to contend
  with; that limitation is unchanged and is deliberately not a passing result.

## Landing — re-recording the slice3d strict fixture

`tests/loopback_pump_structure.rs::both_aarch64_gates_fail_on_a_pinned_placement_refusal`
and `tests/ttbr0_shadow_reconciliation_structure.rs::both_aarch64_gates_fail_on_an_untagged_publish`
each replay `docs/planning/green-program/aarch64-testing/serials/slice3d/01-strict-boot1-serial.txt`
through `docker/qemu/run-aarch64-boot-test-strict.sh`'s scoring-only mode.
`origin/main` advanced 45 commits past this branch's fork point by landing
time, including PR #833 (`fix/812-try-manager-masked`), whose own landing had
already re-recorded that fixture once to add a required `IRQ_HOLD_ORACLE`
line to the strict scorer -- but that re-record predates this branch's own
rewrite of the `FCNTL_PM_CONTENTION_ORACLE` marker, from a boolean-attempt
shape (`attempts=1:armed=1:...`) to the 7-named-arm rendezvous shape
described above (`arm_wait_us=...:armed=1:acquired=1:...:hold_safety=0:...`),
and this branch's own pre-merge copy of the fixture carried the new marker
shape but no `IRQ_HOLD_ORACLE` line at all (it branched before #812). Neither
side's copy alone satisfies both scorer requirements: `git merge --no-ff
origin/main` conflicted on the fixture file, and resolving that conflict by
taking `origin/main`'s copy outright (the merge commit, `0df78b55`) scored
`SCORE: FAIL - fcntl process-manager contention oracle marker missing or
failed` in both replay tests -- the strict gate did not regress; the scorer
it is replayed against grew a new required line on each side, the same class
of break the #812 landing hit for the same file one merge earlier.

Landing re-records `01-strict-boot1-serial.txt` a second time, from a single
strict-gate boot at the merged head (BUILD_ID `006a9c732f0d64`), which
carries `[FCNTL_PM_CONTENTION_ORACLE:aarch64:arm_wait_us=93:armed=1:acquired=1:holder_cpu=1:pm_busy_probe=1:calls=64:eagain=0:first_errno=9:first_wait_us=8107:hold_safety=0:hold_done=1:joined=1:PASS]`
alongside an `IRQ_HOLD_ORACLE` PASS line, 4 of 4 `PINNED_HOME_CPU_UNAVAILABLE`
census lines reading `count=0`, and 14 of 14 `TTBR0_ASID_CENSUS` lines
reading `untagged=0`; the strict scorer accepts the re-recorded file
(`SCORE: PASS`), and both replay tests pass against it (85 and 32 cases
respectively, both suites otherwise unchanged). `02-prod-boot1-serial.txt`
needed no re-record: the production scorer only asserts the
`FCNTL_PM_CONTENTION_ORACLE` and `IRQ_HOLD_ORACLE` markers' literal presence
count, not either one's field shape, and this branch does not change that
gate. The finding and the re-record are recorded in
`docs/planning/green-program/aarch64-testing/serials/slice3d/README.md`,
which already carried the #812 landing's own re-record note and now carries
both.

The capture used the fixed `docker/qemu/run-aarch64-boot-test-strict.sh`
`BREENIX_GATE_TMP` support #812's own landing added (`GATE-TMP-BASEDIR-AARCH64-2026-09-05.md`,
merged in the same 45 commits), so it did not need the discard-and-retake
step the #812 landing's own re-record required against the fixed `/tmp`
path; the capture ran with the host's aarch64 QEMU count read at 0
immediately before launch regardless, and the resulting serial's oracle-line
shape was checked against the built kernel's own `strings` output
(`arm_wait_us=` present, `attempts=` absent) before being adopted.

claim-lint:ok: the re-recorded file, its BUILD_ID, both oracle lines, the 4
PINNED_HOME_CPU_UNAVAILABLE and 14 TTBR0_ASID_CENSUS line counts, and both
replay tests' pass are STEP 1/STEP 2 results committed alongside this doc
update (commit `165f49c0`).

**Standing landing step, reaffirmed.** The #812 doc above already names this
as a standing landing step for any branch adding a required line to either
aarch64 gate scorer; this landing is a second, independent instance of the
same step, this time triggered by a branch that *changes* an existing
required line's shape rather than adding a new one, and hitting a fixture
that a different branch (#812) had already re-recorded once for the
unrelated reason described there. The lesson generalizes: a shared replay
fixture can go stale against either side of a merge, not just the side doing
the landing, and `git merge`'s own conflict markers on the fixture file
(rather than a silent auto-merge) were what surfaced it here before the
structure suites did.

## Landing re-smoke (merged head `165f49c0`)

Re-smoke ran at `165f49c0362e04c406cd007a3d3c7b21fbc59f79` (the fixture
re-record commit above, pushed to `fix/819-fcntl-oracle-arming-rendezvous`)
on the Mac at
`/private/tmp/claude-501/-Users-wrb-fun-code-breenix/d69ffb9d-4539-4cf3-8a3d-a872ff7c830b/scratchpad/ld-fcntl-arm`
and on beast at `/root/breenix-fcntlarm` (`BREENIX_GATE_TMP=/root/breenix-fcntlarm-tmp`).

### Host-side suites

31 of 31 `tests/*_structure.rs` suites pass, **586 cases** total (up from 469
live cases before the fixture re-record, because the two replay tests above
now run their full bodies instead of panicking partway through).
`python3 scripts/test_claim_lint.py` -> exit 0.

### aarch64

`cargo build --release --features boot_tests --target aarch64-breenix-kernel.json
-Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel
--bin kernel-aarch64` piped through `grep -E "^(warning|error)"` (excluding
the toolchain's own `core` future-incompatibility notice): empty output.
`scripts/check-kernel-no-neon.sh` against the resulting ELF: `PASS: 0 FP/SIMD
load/store instructions in kernel .text (allowlisted & suppressed: 0)`.

The strict gate ran as two 20-boot batches, one boot at a time, each launched
only after `ps aux | grep "qemu-system-aarch64 -M"` read 0 (a concurrent
lane on this same host, `ld-627`, was mid-batch at the first check and was
let finish rather than interleaved with):

| run | result | duration |
|---|---|---|
| 1 (`docker/qemu/run-aarch64-boot-test-strict.sh 20`) | `PASS: 20/20 boots succeeded` | 217s |
| 2 (`docker/qemu/run-aarch64-boot-test-strict.sh 20`) | `PASS: 20/20 boots succeeded` | 225s |

All 40 of 40 boots' `FCNTL_PM_CONTENTION_ORACLE` lines read `:PASS]`, with
`arm_wait_us` ranging 72-8247 (µs) across the 40 -- inside the pattern's
accepted range and consistent with the review round's own 25-boot table
above. `grep -anE '(:FAIL[]:]|SCORE: FAIL|^FAIL:| FAIL:)'` over all 40
serials matched 0 lines: zero fcntl-oracle reds, zero other reds, of 40.

Production profile (`docker/qemu/run-aarch64-prod-profile-boot-test.sh`) x1:
`PASS: production profile reached bsshd with the futex oracle seam absent`,
`Observed fcntl contention oracle marker count: 0`, `Observed IRQ-hold
oracle marker count: 0`.

### x86 (beast, `breenix-x86` container)

Own clone at `/root/breenix-fcntlarm`, checked out from a local branch
(`fix-819-import`) in `/root/breenix` that was force-updated to `FETCH_HEAD`
after `git -C /root/breenix fetch origin fix/819-fcntl-oracle-arming-rendezvous`
-- the container's no-outbound-GitHub rule means a fresh `git clone
/root/breenix /root/breenix-fcntlarm` does not itself carry a same-repo
`fetch`'s dangling `FETCH_HEAD` as a ref the clone can see, so the
intermediate named branch was needed. `rust-fork` symlinked to
`/root/breenix/rust-fork-real`. `cargo build --release --features
testing,external_test_bins --bin qemu-uefi` piped through `grep -E
"^(warning|error)"`: empty output (grep exit 1).

| gate | result |
|---|---|
| `docker/qemu/run-x86-boot-tests.sh 1` | `x86 userspace gate: PASS - exited=110 expected>=105 nonzero=0 allowlist=0`; `x86 frame-custody gate run 1: PASS`; both oracle SKIP literals echoed, `[FCNTL_PM_CONTENTION_ORACLE:x86:arm=none:reason=uniprocessor_no_pm_contention_peer:online_cpus=1:SKIP]` and `[IRQ_HOLD_ORACLE:x86:arm=none:reason=irq_exit_gates_softirq_on_preempt_count:online_cpus=1:SKIP]` among them -- the merge's own conflict-resolution in `docker/qemu/run-x86-boot-tests.sh` (keeping both sides' `FCNTL_PM_CONTENTION_ORACLE_LINE` and `IRQ_HOLD_ORACLE_LINE` echoes) is what put the second literal in this gate's own stdout at all <!-- claim-lint:ok: verbatim quotes of the kernel's own marker literals at kernel/src/test_framework/registry.rs:4744 and :5200 -- "arm=none" is the oracle's field value, not a claim of this doc's --> |
| `docker/qemu/run-x86-prod-profile-boot-test.sh` | `PASS: x86 production profile reached steady state with the teardown census at rest`, exit 0 |

**Unattributed reds: 0 of 6** re-smoke checks this round (2 aarch64 gate
batches -- counting the 40-boot strict campaign as one check and the
production boot as a second -- 2 x86 gates, the structure-suite sweep, and
the aarch64 build/no-neon check) matched their expected verdict; none needed
separate triage.

```
claim-lint: scripts/claim-lint.py                                                          -> exit 0
claim-lint: scripts/claim-lint.py --files docs/planning/green-program/aarch64-testing/serials/slice3d/README.md -> exit 0
claim-lint: scripts/claim-lint.py --commit-msg <fixture-fix commit>                         -> exit 0
claim-lint: scripts/claim-lint.py --files docs/planning/green-program/syscalls/819-ORACLE-ARMING-2026-09-05.md -> exit 0
```
