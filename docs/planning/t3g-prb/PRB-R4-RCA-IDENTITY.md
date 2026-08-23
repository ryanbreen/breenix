# RCA — the site-5985 alien install (`cpu=3 owner=0 sp=0xffff000043200000 tid=1201`)

Branch `fix/prb-producer-custody` @ `63a814c5`. Specimens:

* FATAL — `r3/confirm-max-boot21-UNATTRIBUTED-percpu-stack-alien.txt` (= `r3/ssgate-clean-full-output/max/serial-21.txt`)
* SURVIVED — `r3/confirm-cortexa72-boot5-PERCPU_STACK_ALIEN.txt` (= `r3/ssgate-clean-full-output/cortex-a72/serial-5.txt`)
* Round-2 specimen of the same signature at site 5497 — `cortex-a72-boot3-UNATTRIBUTED-serial.txt`
* Peer records of the same family from the leg builds at `3ee79f70` — `r3/legP-baseline-3ee79f70.serial.txt`, `r3/legP-green.serial.txt`

## 1. What site 5985 is, and why the round-2 fix could not cover it

`kernel/src/arch_impl/aarch64/context_switch.rs:5985` is

```rust
let granted = Aarch64PerCpu::install_idle_return_sp(idle_sp);
```

inside the **null-scheduler fallback of `inline_schedule_trampoline`** (function at 5834, fallback at
5928-6005). That is the *same* fallback the round-2 specimen recorded from at 5497. The round-2 repair
did not move this path off the alien; it replaced the two silent setters (`set_user_rsp_scratch` /
`set_kernel_stack_top`) with the fail-closed `install_idle_return_sp`, so the record simply moved from
5497/5498 to 5985. The producer side of the same path is 5942
(`idle_dispatch_stack(cpu_id, candidate)`) with the `unwrap_or_else` fallback at 5975.

The only reachable caller is `schedule_from_kernel` (6310) → `aarch64_inline_schedule_switch(old_ctx,
scheduler_stack_top(cpu_id), inline_schedule_trampoline)` at 6476-6484. There is no second entry.

The other three `install_idle_return_sp` call sites (4484, 6577) and the other three
`idle_dispatch_stack` sites (4442, 6219, 6531) recorded nothing in this corpus.

## 2. The forced deduction: producer and setter evaluated custody for *different CPUs*

`percpu_stack_top_owned_by(cpu, addr)` (constants.rs:442) is a pure function of `(cpu, addr)` plus the
slot-0 owner record. For `addr = 0xffff000043200000`:
`percpu_stack_slot_of` = `(0x43200000 - 1 - base) >> 21` = **slot 0**, and the record itself printed
`owner=0`, so the owner word was well-formed and named CPU 0.

Therefore the predicate answers **true for cpu 0 and false for cpu 3**, deterministically, at both call
sites, microseconds apart. `percpu_stack_region_base()` cannot have moved between them
(`RAM_BASE_OFFSET` is set once in early boot and is 0 on QEMU — proven by the record's own arithmetic).

Both specimens emit **exactly one** `[PERCPU_STACK_ALIEN:` line, at 5985, well under the 16-per-boot
emission cap. The producer at 5942 (and the fallback at 5975) therefore did **not** refuse. Since the
predicate is deterministic, that forces:

> the producer evaluated custody with **cpu_id = 0**, while `percpu_stack_install_permitted` at 5985
> evaluated it with a freshly-read hardware identity of **3**.

This is not a predicate failure. The round-2 repair unified the *predicate* between producer and setter;
what is not unified is the **CPU identity** each side feeds it. The producer uses the `usize` captured
once at 5835 and kept live across `force_unlock_scheduler()`, a lock re-acquire, and the whole
`with_scheduler` closure; the setter re-reads TPIDR_EL1. The comment at 5980-5983 already names this
exact case ("the guard's independent `cpu_id()` read disagreeing with the local one") and dismisses it as
benign. It is not benign — it means the entire invocation belongs to another CPU.

## 3. Which value it is, and the identity it names

`0xffff000043200000` is `percpu_kernel_stack_top(0)` exactly. Two things in the system hold it:
`swapper/0`'s `Thread.kernel_stack_top` (stamped at `main_aarch64.rs:1595-1625`) and CPU 0's own per-CPU
`KERNEL_STACK_TOP` word. The producer chain at 5936-5942 reads
`cpu_state[cpu_id].idle_thread` → that thread's `kernel_stack_top`, so a `cpu_id` of 0 selects
`swapper/0` and yields precisely this address. No torn row, no corrupted TCB field, and no migrated
`kernel_stack_top` is needed to explain the value — only the wrong CPU index.

`IDLE_REDIRECT_HISTORY cpu=0` confirms `idle_thread=1` (swapper/0) for all 64 retained entries in both
fatal specimens, and `DEFER_SNAP`/`DISPATCH_TRACE cpu=3` confirm CPU 3's idle is tid 5 standing on
`0xffff000043800000` — its own slot. So `cpu_state[3]` was never the source.

## 4. The stack corroborates it: the invocation is standing on CPU 0's scheduler stack

Both fatal specimens fault on **cpu=3** with SP inside **CPU 0's scheduler half**
(`[base, base+0x100000)`), a couple of frames deep:

| specimen | fault SP | depth below `percpu_sched_stack_top(0)` = `0xffff000043100000` |
|---|---|---|
| r3 max boot 21 | `0xffff0000430fff10` | 240 bytes |
| r2 a72 boot 3  | `0xffff0000430fff60` | 160 bytes |

The r3 postmortem labels it itself: `STACK=sched_cpu0`. Both also carry a garbage SPSR
(`0x1`, and `0xffff000040800008` — a kernel VA, not a PSTATE), the #635 "corrupted whole context" face.

`aarch64_inline_schedule_switch` (asm at 1443-1468) saves `sp` **before** `mov sp, x1`, so an outgoing
thread's saved `context.sp` is never a scheduler stack. The only way a CPU stands on a scheduler stack
is the pivot itself, i.e. `scheduler_stack_top(cpu_id)` at 6476 evaluated with `cpu_id = 0` on CPU 3.

So the stack and the custody record agree: **CPU 3 was executing a `schedule_from_kernel` /
`inline_schedule_trampoline` invocation whose CPU identity, and whose stack, are CPU 0's.**

## 5. The producing defect: `cpu_id` is captured before interrupts are masked

```
6310  pub fn schedule_from_kernel() {
6311      let saved_daif = read_daif();
6312      let cpu_id = Aarch64PerCpu::cpu_id() as usize;      // <-- IRQs still enabled
6313      cpu0_breadcrumb(cpu_id, 1);
6314      unsafe {
6315          crate::arch_impl::aarch64::cpu::disable_interrupts();
6316      }
```

`schedule_from_kernel` is called from ordinary preemptible kernel context with interrupts **on** —
`waitqueue.rs:305`, `completion.rs:350`, `scheduler.rs:4000` (`schedule()`), `context_switch.rs:6752`,
each preceded by `run_deferred_reclamation()` (locks + allocation). That is exactly why 6311 has to save
DAIF and 6315 has to mask explicitly.

Between 6312 and 6315 the thread is preemptible. A timer IRQ there saves the thread's context and
requeues it; there is no CPU pinning in the scheduler (only soft cache-affinity routing on wakeup,
scheduler.rs:3429-3432), so any CPU may pick it up. On resume, `cpu_id` — a plain `usize` in a
callee-saved register or a stack slot — still holds the **old** CPU's index, and the rest of the function
runs with it: `DEFERRED_REQUEUE[cpu_id]`, `cpu_state[cpu_id]`, `INLINE_SCHEDULE_STATE[cpu_id]` (6461-6472),
and finally `scheduler_stack_top(cpu_id)` at 6476.

The consequences chain exactly onto the evidence:

1. CPU 3 pivots onto **CPU 0's scheduler stack** (fact 4).
2. The trampoline re-reads the identity at 5835 and gets **3**, so it reads `INLINE_SCHEDULE_STATE[3]`
   while the state was published in slot **0** → `scheduler_ptr` is null → **the null-scheduler fallback
   is taken**. This is why every specimen of this signature, in both rounds, is in that fallback and
   nowhere else.
3. If CPU 0 is concurrently inside its own trampoline invocation, the two CPUs are now running on one
   stack: CPU 3's `mov sp, scheduler_top` resets SP to the top and its frames overwrite CPU 0's at the
   same offsets. Locals spilled across the `with_scheduler` call at 5936-5972 — `cpu_id` and `idle_sp`
   among them — are then read back as the *other* CPU's values. That is the mechanism by which the
   producer at 5942 sees `cpu_id = 0` while the setter at 5985 reads 3, and it is the same trampling
   that produces the garbage SPSR and the whole-context face in the fatal register file.

`assert_pivot_free` (2192) cannot see any of this: it only fires when the *current* SP already lies
inside the destination range, i.e. self-aliasing. A pivot from CPU 3's stack onto CPU 0's is silent, and
the corpus shows no `[STACK_PIVOT_ALIAS_HISTORY]` at all.

Honest scope: step 3 is inferred, not instrumented. Steps 1 and 2 follow from the code plus the two
measured facts (SP in `sched_cpu0`, null fallback taken). What is *proven* is the identity split in §2;
6312 is the one place in the path that manufactures a carried, stale CPU index, and it reproduces every
observable.

## 6. Who tid 1201 is

**init — PID 1's main thread**, an ordinary user-privilege thread with a heap-backed kernel stack.
Proven, not inferred:

* `FATAL_REGS` thread line: `tid=1201 name=init`, `thread_kst=0xffff000054266000`,
  `percpu_kst=0xffff000054266000`
* `[DATA_ABORT] deferred_tid=1201 queued=1`
* `[LAST_DISPATCHED_TID] cpu=3 tid=1201 kstack_slot=5` — a heap kernel-stack slot, **not** a per-CPU slot
* `DISPATCH_TRACE cpu=3` alternates `K old=5->tid=1201 … sp=0xffff00005426xxxx` with
  `I old=1201->tid=5 … sp=0xffff000043800000`

It is **not** idle-adjacent and does **not** bear a per-CPU stack, so the "migrated thread whose
`kernel_stack_top` is genuinely slot 0's top" hypothesis is falsified for 1201.

Why the same tid three times: tid allocation is deterministic through this boot script (heartbeat is
always 1204, the census oracle always 1200), and all three specimens fire inside init's `[spawn]`
sequence — max boot 21 at `[spawn] path='/bin/block_eintr_oracle'` (~3.0 s), a72 boot 5 at
`[spawn] path='/bin/bwm'` (~12.9 s), r2 a72 boot 3 at the same `/bin/bwm` point (~12.2 s). That phase is
where init blocks and yields into `schedule_from_kernel` most often, which is exactly the window at
6312-6315. 1201 is the *victim* — the thread the recording CPU last dispatched, stamped by
`last_dispatched_tid(cpu)` — not the producer.

## 7. The family this belongs to: per-CPU stack addresses are not bound to a CPU

The leg-build serials at `3ee79f70` carry the same class at two other sites:

```
PERCPU_STACK_ALIEN:cpu=1:owner=3:sp=0xffff0000437ffee0:tid=4:site=…:3723   (legP-baseline)
PERCPU_STACK_ALIEN:cpu=3:owner=1:sp=0xffff000043400000:tid=4:site=…:3723   (legP-baseline)
PERCPU_STACK_ALIEN:cpu=3:owner=2:sp=0xffff000043600000:tid=1208:site=…:4175 (legP-green)
```

Line 3723 in that tree is `Aarch64PerCpu::set_user_rsp_scratch(thread.context.sp)` on the kernel-thread
restore path. `0xffff0000437ffee0` is 0x120 below CPU 3's exception top — an EL1 exception-frame SP on
CPU 3's *per-CPU* stack, sitting in a thread's saved `context.sp` and being installed on **CPU 1**. So
threads do acquire per-CPU stack addresses in their saved contexts (the documented
`fix_stale_current_thread_when_idle_executing` hazard at 6358-6368 is one way in) and the scheduler will
dispatch them anywhere.

Nothing on the dispatch path refuses that:

* the resume-PC predicate passes — a trampoline PC is ordinary kernel `.text`;
* `thread_kernel_stack_contains` (3272) is consulted at 3727/3905/4051 only for
  `owner_pid.is_some() && blocked_in_syscall` threads, and only to **log**, never to refuse — its own
  `debug_assert_ne!(privilege, Kernel)` excludes kernel threads by construction;
* the `fresh_idle_sp` pivot at 6250-6259 uses the value without any `install_*` adjudication at all.

## 8. Fix plan

**F1 — kill the carried CPU index (the producing write).** Move the `cpu_id` capture in
`schedule_from_kernel` to *after* `disable_interrupts()` (6312 → below 6315) so no stale index can
survive a preemption into `DEFERRED_REQUEUE[cpu_id]`, `cpu_state[cpu_id]`,
`INLINE_SCHEDULE_STATE[cpu_id]` or `scheduler_stack_top(cpu_id)`. Then make the class unrepresentable
rather than fixed once: introduce a `CpuId` token that can only be minted from a fresh
`Aarch64PerCpu::cpu_id()` read under masked interrupts, and take it (not `usize`) in
`scheduler_stack_top`, `percpu_kernel_stack_top`, `idle_dispatch_stack` and the `INLINE_SCHEDULE_STATE`
/ `cpu_state` accessors on this path, so a carried index cannot be spent on a per-CPU stack decision.

**F2 — one identity for producer and setter.** In the trampoline's fallback, derive the cpu for 5942 and
5975 from the *same* fresh read `percpu_stack_install_permitted` uses, so the two sides cannot disagree,
and re-validate it after `with_scheduler` returns (it is a spill point across a lock). Add a refusal
record for the disagreement itself — today it is silently absorbed by 5985's fallback and reads as an
ordinary alien, which is what made two rounds of RCA chase the wrong producer.

**F3 — custody on the SP a CPU runs on, not just on the tops it publishes.** Extend
`assert_pivot_free` from self-aliasing to full custody: refuse (fail-closed, substituting this CPU's own
top) any pivot whose destination `percpu_stack_slot_of` names a slot this CPU does not own. Extend
`thread_kernel_stack_contains` to kernel-privilege threads and convert 3727/3905/4051 from log to
refusal, and adjudicate `thread.context.sp` at every ret/ERET dispatch: a resume SP that lands in *any*
per-CPU slot is refused unless the slot is the dispatching CPU's own. That closes the 3723/4175 peers as
well as this one.

**F4 — the pre-registered occupancy epoch.** The positive scheduler-owned per-slot occupancy epoch
already scheduled for PR-B is what makes F3 decidable rather than address-inferred: with "who is standing
on slot N" a published fact, a second CPU pivoting onto an occupied slot is refusable at the pivot
instead of being discovered later as a trampled register file.

**F5 — acceptance.** With F1-F3 in, `PERCPU_STACK_ALIEN` must be 0 across both profiles, the 5985
refusal converts from record-and-continue to hard FAIL, and the `#635` / `#576` / `#626` tolerances get
their close retakes. Ratchet on the census shape (`refusals == 0` plus a mutation that reddens it), never
on the literal site list — the site line already moved 5497 → 5985 once.
