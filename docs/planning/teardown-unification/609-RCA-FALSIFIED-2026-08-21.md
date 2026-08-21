# #609 — the RCA mechanism was falsified by its own forcing arm (T3-G, ruling R33)

Branch `fix/609-early-kthread-dispatch`, off main `7fbee231`. This file is the durable in-repo
record of a forced-oracle round whose result was negative, so that the deleted stimulus and the
removed gate tolerance are both explained by something other than a commit message.

## The filed signature, corrected

#609 is filed as *"a `network:early` kthread that produces nothing was spawned and then never
dispatched."* The preserved serials do not say that. In both of them the only `[SUBSYSTEM:` lines in
the whole early stage are `memory:early:START` and `memory:early:COMPLETE:24/24` — **ten of the
eleven subsystem kthreads are missing, not one.** The census agrees: the healthy boot's first sample
line is `samples=8:checked=94` (~11.8 threads/sample, the eleven kthreads), the stalled boot's is
`samples=11:checked=28` (~2.5/sample) from the very first sample. The ten were never created.

So the thread that stops is the **spawner** — the aarch64 bootstrap thread running
`run_staged_tests` — not a network kthread. Every future attribution should be read against that.

## What the RCA proposed, and what the forced arm measured

The RCA's causal chain was: tid 0 is simultaneously CPU 0's registered idle thread and the thread
carrying the boot-test control flow; the scheduler treats idle tids as stateless and disposable at
four sites keyed on tid identity; therefore one CPU-0 context switch inside the boot-test window
destroys the boot unresumably (L1–L3). L4 asserted CPU 0 is eligible to take that switch on every
tick; L6 attributed the observed ~2–3% tail to it.

R32(a) made the forcing arm the oracle. `--features force_609` armed, exactly once per boot and
immediately after the first successful `EarlyBoot` `kthread_run`, a single CPU-0-pinned kthread
whose body only incremented a counter.

| arm | build | boots | STALL | CRASH | OTHER | GREEN | armed | HITS |
|---|---|---|---|---|---|---|---|---|
| A — forcing on | `--features force_609` | 10 | **0** | 0 | 0 | 10 | 10/10 | **0 every boot** |
| A starved — 10 hogs `nice -n 19` | `--features force_609` | 10 | **0** | 0 | 0 | 10 | 10/10 | **0 every boot** |
| B — forcing off | `--features boot_tests` | 60 | 0 | 0 | 0 | 60 | n/a | n/a |
| B starved | `--features boot_tests` | 30 | 0 | 0 | 0 | 30 | n/a | n/a |
| powered control — forcing off | `--features boot_tests` | 200 | 0 | 0 | 0 | 200 | n/a | n/a |

All boots `-M virt,gic-version=3 -cpu cortex-a72 -m 512 -smp 4`, block IOPS throttle 2000, 45 s
per-boot timeout, every serial preserved.

`[FORCE609:ARMED]` on 20/20 forcing boots and `[FORCE609:HITS=0]` on 20/20 is the decisive field:
the CPU-0-pinned kthread was created and **never dispatched at all** for the entire EarlyBoot stage.
Widening the stimulus body — the RCA's own fallback if arm A came in under 100% — cannot change a
body that never runs.

Why it is inert, from source: `main_aarch64.rs` calls `per_cpu_aarch64::preempt_disable()` before
`init_scheduler()` and does not balance it until `preempt_enable()` in `launch_init_from_elf`, i.e.
after `run_all_tests()`; `check_need_resched_and_switch_arm64` honours that via
`PREEMPT_GUARD_MASK`; and `kthread_join` waits with `arch_halt()` rather than yielding, so the boot
thread takes no voluntary schedule either. The tree already stated the conclusion in
`kernel/src/tracing/providers/teardown.rs`: *"CPU 0 runs the boot-test executor outside the
scheduler and cannot service a pinned kthread."*

**L4 is false, and L6 rests on L4.** L1 and L2 remain correct from source: the double role and the
four identity-keyed disposability gates are real as a latent hazard. What is not established is that
they ever fire, and #609 was the only evidence offered that they do. No fix was landed on this
chain, because there is no red to turn green and campaign law is mutation-first.

## The powered control #609 asked for

#609's own attribution section says: *"This needs a powered control (several hundred boots per arm)
before anyone says 'pre-existing' or 'branch-caused'."*

Non-forcing boots on main this round: **60 + 30 starved + 200 = 290, zero occurrences.** At the
filed p = 0.03, P(0 in 290) ≈ 1.4e-4. Pooled with the earlier `pure main 0/50`, main is not running
at the filed rate. Both preserved #609 serials are from `fix/589-deferred-requeue-drift` @
`18dcb2ef`, not from main.

## Disposition under ruling R33

1. **The stimulus is deleted** — the `force_609` cargo feature, the arming and reporting hooks in
   `kernel/src/test_framework/executor.rs`, and `docker/qemu/run-aarch64-force609-arm.sh`. Its
   purpose is served and its result is recorded here; leaving it would be dead code carrying a
   falsified theory.
2. **The #609 gate tolerance is removed.** `is_609_network_early_stall` survives as the FIELD
   detector — deleting it would send a recurrence to UNATTRIBUTED with a generic reason — but its
   bucket is now a hard FAIL in `run_profile`, and the run-wide `TOTAL_609_CEILING` rate ceiling is
   gone. Any recurrence reds the gate and yields a fresh, preserved serial. Removing a tolerance
   narrows the set of runs the gate passes, so this is a tightening.
3. **The strand census is widened** — the identity-keyed idle skip is replaced by a parked
   predicate, and a `worst_nonprogress_ms` axis is added — proven by a synthetic in-kernel injection
   rather than by the falsified mechanism. See the census-widening commit and its oracle.
4. **#609 stays OPEN and untolerated.** The mechanism is unknown; the class is simply not
   reproducible on main at the filed rate, and the next occurrence is now a gate red with evidence.

## By-catch, filed as its own issues

* **#620** — `-smp 1` on this kernel wedges before any subsystem kthread starts: soft lockup at
  5000 ticks with `CTX_SWITCH_TOTAL: 0`, `Ready queue: [2,3,4,5,6,7]`, `tid=0 state=X`.
  Uniprocessor aarch64 never performs a single context switch. Reproduced twice.
* **#621** — the #609 comment-2 `memory:early`/`scheduler:early` variant leg: `old_id` in
  `check_need_resched_and_switch_arm64` is read from `cpu_state[cpu].current_thread`, and if that is
  stale-idle while the CPU is really running a kthread, the four identity-keyed disposability gates
  skip that kthread's save *and* its requeue — the same disposal applied to the wrong thread. Filed
  as a hazard; not demonstrated.

## Landmines recorded for anyone who later lands the double-role fix

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
