# RCA — the cortex-a72 boot-3 PERCPU_STACK_ALIEN and the fatal that followed

Branch `fix/prb-producer-custody` @ 9d90d851. Evidence: the preserved serial, in-repo at
`docs/planning/t3g-prb/serials/prb-battery-cortexa72-boot3-alien-then-fatal.txt`,
the tree at that commit, and the other 49 battery serials + 3 Parallels rounds as controls.

## 0. The two records, decoded

```
742: [PERCPU_STACK_ALIEN:cpu=3:owner=0:sp=0xffff000043200000:tid=1201:site=…/context_switch.rs:5497]
744: [INSTRUCTION_ABORT] FAR=0x0 ELR=0xffff000040844580 ESR=0x86000005 IFSC=0x5 from_el0=0
745: [FATAL_REGS] label=INSTRUCTION_ABORT cpu=3 spsr=0xffff000040800008 … sp=0xffff0000430fff60
…    [INSTRUCTION_ABORT] deferred_tid=1201 queued=1
```

Address arithmetic (`constants.rs`: region base `HHDM+0x4300_0000`, stride `0x20_0000`,
scheduler half `0x10_0000`):

| value | is |
|---|---|
| `0xffff000043200000` | `percpu_kernel_stack_top(0)` — CPU **0**'s exception-half top |
| `0xffff000043800000` | `percpu_kernel_stack_top(3)` — CPU 3's own top (what it should have used) |
| `0xffff000043100000` | `percpu_sched_stack_top(0)` = `percpu_kernel_stack_bottom(0)` |
| `0xffff0000430fff60` | that boundary − 0xA0 — i.e. 160 bytes down **CPU 0's scheduler half** |

## 1. The faulting CPU really is CPU 3

`[INSTRUCTION_ABORT] deferred_tid=1201 queued=1` — the fatal handler attributed the fault to
thread 1201, and `[LAST_DISPATCHED_TID] cpu=3 tid=1201` says 1201 is what CPU 3 was running.
`DISPATCH_TRACE cpu=3` alternates `old=5->tid=1201` / `old=1201->tid=5` with the idle legs at
`sp=0xffff000043800000` (slot 3, correct). So the identity in the record (`cpu=3`, read fresh
from `Aarch64PerCpu::cpu_id()`, which falls back to MPIDR_EL1 when TPIDR_EL1 is 0 and therefore
cannot spuriously read 0) is the physical CPU. The system survived — heartbeats continue to
`uptime_ms=13223`.

## 2. Where `preferred` comes from for site 5497

Site 5497 is inside `inline_schedule_trampoline`'s null-scheduler fallback
(`context_switch.rs:5447-5520`). The value installed there has exactly one producer chain:

```
5354  let cpu_id = Aarch64PerCpu::cpu_id() as usize;              // = 3
5456  let idle_id  = sched.cpu_state[cpu_id].idle_thread;
5457  let candidate = sched.get_thread(idle_id)
5459                      .and_then(|t| t.kernel_stack_top…)
5460                      .unwrap_or_else(|| percpu_kernel_stack_top(cpu_id));
5461  let idle_sp   = idle_dispatch_stack(cpu_id, candidate);      // 2875
5489  reset_idle_continuation_locked(sched, cpu_id, idle_id, idle_sp);   // persists it
5497  Aarch64PerCpu::set_user_rsp_scratch(idle_sp);               // REFUSED, records
5498  Aarch64PerCpu::set_kernel_stack_top(idle_sp);               // REFUSED
5511  asm!("mov sp, {stack}", … idle_sp)                          // NOT refused
```

Both `unwrap_or_else` fallbacks are `percpu_kernel_stack_top(cpu_id)` = `…43800000` for cpu_id 3,
so they are excluded: the observed value can only have come through `candidate`, i.e. through a
thread's `kernel_stack_top` field. And exactly one thread in the system carries
`Some(0xffff000043200000)`: **`swapper/0`, CPU 0's idle/bootstrap thread**, which is given
`percpu_kernel_stack_top(0)` once at `main_aarch64.rs:1595-1625`.

`idle_dispatch_stack(cpu_id, preferred)` (line 2875) is the one function whose job is to
normalise that value, and its entire body is:

```rust
if preferred == 0 { percpu_kernel_stack_top(cpu_id) } else { preferred }
```

Every non-zero address passes through verbatim. Nothing on the path from
`cpu_state[n].idle_thread` to `mov sp` requires the thread named by CPU `n`'s idle slot to own
slot `n`, or requires the address in its `kernel_stack_top` to be attributable to CPU `n` at all.

**That is the producing structural gap.** I cannot pin the individual store that made CPU 3's
lookup resolve to slot 0's stack top — `IDLE_REDIRECT_HISTORY cpu=3` reads `idle_thread=5` for
all 64 of its retained entries and `register_idle_thread` (scheduler.rs:1447) is the only writer
of that field, so the two remaining candidates are a torn/stale read of the `cpu_state[3]` row
across the `force_unlock_scheduler()` → `with_scheduler()` re-acquire at 5452-5455, or a
corrupted `kernel_stack_top` in the TCB the lookup landed on. Both are *consumed* by the same
un-normalised pass-through, and the fix below makes either one unable to produce a foreign SP.

## 3. Why the refusal did not help — and the fatal

The guard refused both per-CPU writes (correctly: slot 0 ≠ cpu 3, published owner 0). It writes
nothing, by design. But two consumers took the value anyway:

* **5489** `reset_idle_continuation_locked` had *already* persisted `idle_sp` into CPU 3's idle
  thread's saved `context.sp`, eight lines before the guard ever saw it.
* **5511** `mov sp, idle_sp` — the pivot is unconditional.

So CPU 3 branched to `idle_loop_arm64` standing on **CPU 0's exception stack**, and 5513
(`msr daifclr, #0xf`) re-enabled interrupts one instruction later. From that moment both CPUs
carve 272-byte EL1 exception frames out of the same slot-0 image. The fatal register file is
that collision read back: `x9=x16=0xffff000043200000` (the SP itself), `x10=x17=x21=` the ELR,
`x6=x7=x18=x20=0xffff000040800008`, `spsr=0xffff000040800008` (a kernel VA, not a PSTATE), and
`x30=0x0` — a `ret` through a zeroed link register, which is precisely `FAR=0x0` with an
`IFSC=0x5` instruction abort. `sp=0xffff0000430fff60` is a restored value from that clobbered
image, not a descent: CPU 0's half-boundary canary reads `intact=1`, so nothing walked down
through `0x43100000`.

This is the same producer family as #633/#635/#637 — a whole-context corruption whose *source*
here is named for once, because the custody record caught the install that created it.

## 4. The R50 overlay is NOT the writer

Ruled out on three independent grounds:

1. `percpu_stack_custody_oracle = ["boot_tests"]` in `kernel/Cargo.toml` is a one-way implication
   — the oracle feature turns `boot_tests` on, not the reverse. The service-sequence gate builds
   `--features boot_tests` only (`run-aarch64-service-sequence-gate.sh:110`), so probe A, its
   272-byte image and its one-shot cross-CPU stimulus are compiled out entirely.
2. The serial contains **0** `[PERCPU_STACK_CUSTODY_ORACLE:` lines.
3. Probe A targets the highest **offline** CPU (slot 7 at `-smp 4`), never slot 0, and fires once
   per boot at the userspace-dispatch site — not at 12.2 s into a bsshd/bwm spawn sequence.

The record is therefore a production install, exactly as the gate-honesty argument for
`[RESUME_PC_REFUSED:]` assumed.

## 5. Repair, at source

1. `idle_dispatch_stack` becomes the producer-side custody point: it accepts `preferred` only
   when the address is **affirmatively attributable to this CPU** — outside the per-CPU stack
   region altogether (an ordinary heap-backed kernel stack, or CPU 0's UEFI boot stack on
   Parallels), or in this CPU's own slot with the published owner agreeing or unpublished —
   and otherwise substitutes `percpu_kernel_stack_top(cpu_id)` and counts the substitution.
   This is positive per-slot occupancy custody in its minimal form for this path, and it is the
   *same* predicate the setter guard applies, factored into one function so producer and guard
   cannot diverge. The foreign address is no longer produced, so it never reaches 5489, 5497
   or 5511.
2. The install of the idle-return SP pair becomes fail-closed: `install_idle_return_sp` returns
   the address custody actually granted, and the pivot uses that. A refusal now falls back to the
   CPU's own top instead of being ignored. The refusal record is still emitted by the setter.

## 6. Correction to 9d90d851's battery paragraph

That commit message reports the cortex-a72 red as the `#644`
`timer_interrupt_running` bucket. The fresh confirmation battery reproduced a
DIFFERENT red — the boot 3 documented above, which the classifier correctly
scored `UNATTRIBUTED` rather than absorbing into `#576` (FAR=0 but ELR non-zero)
or `#644`. `BOOT_TEST_FAIL` was 0 for that run.

The paragraph is corrected here rather than by rewriting the commit: 9d90d851 is
cited by SHA in the review, in the confirmation record and in this branch's later
commits, and the campaign's rule is to correct an overstated message in the PR
body, never by rewriting history. Anyone reading 9d90d851's battery claim should
read this section with it.
