# RCA — the corrupted-live-x30 face (Parallels run 3, PC_ALIGN at 0x19)

Branch `fix/prb-producer-custody` @ `3ee79f70`, clean tree. Specimen: Parallels run 3,
`parallels-run3-serial-FAULT.log` line 1866, uptime_ms 130556.

**Verdict: structural gap named, producing write NOT pinned.** The gap is exact,
enumerable and provable from code: the campaign has hardened every *architectural*
resume PC and has left the *deferred* resume PC — the link register — with zero
custody at all six sites that make it live.

---

## 0. The binary was recovered, so every address below is a real symbol

The ELF in `target/` is a **different build** than run 3 (`idle_loop_arm64` sits at
`0xffff0000404641dc` there; the serial's idle dispatches carry
`elr=0xffff00004045e184`). The run-3 kernel was carved out of the deployed disk
image — `target/parallels/breenix-efi.hdd/…hds` at file offset `0x20e800`, 3034024
bytes — and it resolves `idle_loop_arm64 = 0xffff00004045e184` exactly. Working copy:
`/private/tmp/claude-501/-Users-wrb-fun-code-breenix/d69ffb9d-4539-4cf3-8a3d-a872ff7c830b/scratchpad/run3-kernel.elf`.
Every symbolisation in this document is against that binary. (An earlier pass against
`target/` mis-resolved these addresses to `inline_schedule_trampoline` and
`reclaim_unschedulable_cpu_queues`; those readings are wrong and are discarded.)

| record value | run-3 symbol |
|---|---|
| `saved_elr` = `saved_x30` = `last_dispatch_elr` = `0xffff00004045bd84` | `schedule_from_kernel+0x119c` |
| `0xffff00004045bd80` (the instruction before it) | `bl aarch64_inline_schedule_switch` |
| `elr` of every `I` trace entry `0xffff00004045e184` | `idle_loop_arm64` |
| `x28 = 0xffff000040927c00` | `TRACE_BUFFERS` (CPU 0's ring base) |
| `x5  = 0xffff00004092bc10` | `TRACE_BUFFERS+0x4010` (CPU 0's drop counter) |
| `x23 = x25 = 0xffff000040927000` | the `adrp` page those two are formed from |
| `x8  = 0xffff0000408302a0` | `ALL_CPU_DATA+0x20` |
| `x27 = 0xffff000040409374` | `Scheduler::schedule_deferred_requeue+0x788` |

---

## 1. What the last dispatch actually was — and why the trace does not say so

`saved_elr == saved_x30 == last_dispatch_elr == schedule_from_kernel+0x119c`, and
`0xffff00004045bd80` is `bl aarch64_inline_schedule_switch`. That equality is not a
coincidence; it is *written by one function*:

* `aarch64_inline_schedule_switch` (context_switch.rs:1194-1218) saves the outgoing
  thread's callee-saved set with `stp x29, x30, [x0, #232]` and then `str x30, [x0, #264]`.
  Offset 240 is `CpuContext.x30`; offset 264 is `CpuContext.elr_el1` (const-asserts at
  context_switch.rs:1474-1482). So an inline-saved context has `x30 == elr_el1 ==` the
  return address of that `bl`.
* `take_inline_ret_dispatch_info` (context_switch.rs:3060) takes `resume_pc =
  thread.context.x30`, admits it with `resume_pc_is_dispatchable`, and then writes
  `thread.context.elr_el1 = resume_pc`.
* The IRQ-path ret-dispatch site (context_switch.rs:5102-5183) is the only caller of
  `Aarch64PerCpu::set_dispatch_elr(resume_pc)` outside the two ERET sites
  (context_switch.rs:5155), and it ends in `aarch64_ret_to_kernel_context(ctx_ptr, resume_pc)`.

So **the last dispatch on CPU 0 was the ret-based kernel dispatch of tid 27, with a
valid, admitted resume PC.** The dispatch-trace ring appears to disagree — its newest
entry is `[7] I old=25->tid=1` — because `record_dispatch` has exactly two call sites
(context_switch.rs:5236 and 5732), **both on the ERET path**. The ret-based kernel
dispatch is never recorded. That is a diagnostic defect in its own right: the trace's
last entry names idle while `current_tid` reads 27, and any reader who trusts the ring
to name the last dispatch is misled. (Fix item 5 below.)

## 2. Where the fault happened

* `spsr=0x22000305` → `M=0b0101` = **EL1h**, `I=0` and `F=0` (IRQs live), `A=1`, `D=1`,
  `C` set. So the corrupted branch was taken by kernel code at EL1 with interrupts enabled.
* `esr=0x8a000000` → `EC=0x22`, PC alignment fault; `elr=far=0x19`, i.e. the CPU branched
  to `0x19`. `0x19` is in no register but `x30`, so this is `ret` / `br x30`.
* `sp=0xffff000054298ef0`. Trace entry `[4] U old=25->tid=27 … sp=0xffff000054299000`
  records tid 27's kernel-stack top, so the faulting SP is **exactly `top − 272`** —
  one `Aarch64ExceptionFrame` below the top of its own kernel stack. The stack is the
  right thread's; nothing walked off it.
* `x1 = x13 = 0x1b = 27` — the running thread's own tid, twice, in the live file.
  `x30 = 0x19 = 25` — the tid on the other side of every `U` dispatch in the ring
  (`[3] U old=27->tid=25`, `[4] U old=25->tid=27`).
* `x29 = 0x1e71c57554` = 130,757,727,572 — a monotonic-ns value landing between the
  `uptime_ms=130556` and `uptime_ms=131557` heartbeats that bracket the fault. So x29 is
  a live timestamp, not a frame pointer; the register file is a genuine mid-flight kernel
  context, not a scrambled image.
* The whole boot contains **zero** `RESUME_PC_REFUSED`, `RET_DISPATCH_REFUSED`,
  `PERCPU_STACK_ALIEN`, `CTX596_ELR_DIVERGENCE`, `DISPATCH_MISMATCH` or allocator
  records. Every existing detector was silent, and correctly so — none of them looks at
  the register that killed the boot.

## 3. The structural gap: x30 is a PC in waiting, and nothing admits it

PR #642 unified the *resume-PC* consumers. A resume PC is the word that goes into
`ELR_EL1` (or into the `br` target). The link register is the **other** PC in a saved
context: whatever word sits at frame/context offset 240 becomes the target of the next
`ret` executed by the resumed code, at an unbounded later time, with no record tying the
branch back to the dispatch that installed it.

Complete census of the sites that load the live `x30` from memory:

| site | what it admits | what it loads into x30 |
|---|---|---|
| `boot.S:590` (EL1 sync ERET epilogue) | `frame.elr` @248 via `RESUME_PC_EL1_OK`/`EL0_OK` | `ldr x30, [sp, #240]` — **nothing** |
| `boot.S:761` (IRQ ERET epilogue) | `frame.elr` @248 | `ldr x30, [sp, #240]` — **nothing** |
| `boot.S:841` (early-boot fast path) | nothing | `ldr x30, [sp, #240]` — **nothing** |
| `syscall_entry.S:325` (SVC ERET epilogue) | `frame.elr` @248 | `ldr x30, [sp, #240]` — **nothing** |
| `context_switch.rs:1380` (`aarch64_enter_exception_frame` epilogue) | `frame.elr` @248 | `ldr x30, [sp, #240]` — **nothing** |
| `context_switch.rs:1241` (`aarch64_ret_to_kernel_context`) | `x1 = resume_pc` via `RESUME_PC_EL1_OK` | `ldp x29, x30, [x0, #232]` — **nothing** |

Six consumers, zero predicates. `resume_pc_is_dispatchable` (context_switch.rs:164) would
have rejected `0x19` on the alignment test alone (`0x19 & 3 == 1`), before ever reaching
the text-window test. **Had the x30 slot been held to the same standard as the resume PC,
this fault would have been a refusal record and a survived boot instead of a fatal.**

The copy chain that feeds those slots is equally uncustodied:
`frame.x30` (written by the entry stub) → `thread.context.x30 = frame.x30`
(context_switch.rs:3293 and 3409) → `frame.x30 = ctx.x30` (context.rs:155) → live LR.
No hop applies a predicate; no hop can tell a return address from a tid.

### 3a. And on the ret path the admitted word and the loaded word are not even the same read

`take_inline_ret_dispatch_info` reads `resume_pc = thread.context.x30` **under the
scheduler lock**, admits it, and returns a raw `ctx_ptr = &thread.context`. The caller
then runs `drop(guard)`, `reset_quantum()`, `rearm_timer()`,
`stamp_last_dispatched_tid_for_stack()` — and only then does the asm `ldp x29, x30,
[x0, #232]` re-read the same word. Between the admission and the load, `ctx_ptr` is an
unguarded raw pointer into the scheduler's `threads` Vec, dereferenced with the lock
released. The code one screen away (context_switch.rs:5713) already documents this exact
hazard for a different read ("realloc/element-shift -> data race/UAF") and avoids it there
by snapshotting under the lock; the dispatch itself does not.

## 4. Discriminating the three candidate mechanisms

* **(a) calling-convention breach in an asm path — RULED OUT.** A census of every Rust
  `asm!` block in `kernel/src` that mentions `x30`/`lr`/`bl`/`blr`/`clobber_abi` returns
  nine blocks; the only ones that touch x30 (`context_switch.rs:1056`, `:1151`,
  `completion.rs:73`) *read* it into an output operand under
  `options(nomem, nostack, preserves_flags)`. A disassembly-wide search of the run-3
  binary for `mov x30, #imm` finds two sites, both `#0x0`; for `mov x30, x<n>` it finds
  eighteen, none carrying a small integer. No asm path writes a tid into x30.
  (Separately noted, not this bug: `smp.rs:290/306` issue `hvc #0` for PSCI with
  `options(nomem, nostack)` and no clobber declaration for the SMCCC-clobbered x4-x17.
  Boot-only, but it should be declared.)
* **(b) a save writing a tid into the wrong slot of a live frame (R50 overlay, save
  side) — NOT PINNED, still live.** No static writer of a small integer to offset 240 was
  found. `CpuContext` and `Aarch64ExceptionFrame` carry offset const-asserts but **no
  `size_of` assert and no magic word**, so a foreign 16-byte write landing at +232 is
  restored silently as `(x29, x30)`. The fault's `x29` (a timestamp) / `x30` (a tid) pair
  has exactly the shape of a two-word record, which is what keeps this candidate alive.
* **(c) restore from a wrong/stale frame — PARTIALLY SUPPORTED.** The stale-`ctx_ptr`
  window in 3a is real and is the strongest un-eliminated producer. It is *not* proven for
  this specimen: the fatal dump reads thread 27's row through the same
  `current_thread_ptr` and finds it intact and self-consistent (`id=27`, `owner_pid=15`,
  `saved_elr == saved_x30 == schedule_from_kernel+0x119c`), so the row was not sitting
  moved or freed at dump time.

What is *proven* is the consumer-side gap in §3, and it is sufficient: whichever of (b)
or (c) produced the word, the class exists only because six consumers turn an unchecked
memory word into a live PC.

## 5. Repair — producer-side, leaving the ERET epilogue alone

The standing ruling keeps the ERET epilogue unhardened (a redirect there strands the
thread and destroys FATAL_REGS evidence). So the fix must make an inadmissible word
*unable to reach offset 240*, rather than catching it on the way out.

1. **One accessor owns every write to a saved LR slot.** `set_saved_lr(slot, value, el)`
   applies `resume_pc_is_dispatchable` (EL1) / `resume_pc_is_user_dispatchable` (EL0) and,
   on refusal, stores the address of a new `aarch64_lr_poisoned_trampoline` in kernel text
   plus a refusal record carrying the tid, the refused word and the site. Convert the three
   Rust producers (`context_switch.rs:3293`, `:3409`, `context.rs:155`). The architectural
   saves (`stp x29, x30, [x0, #232]`, the entry stubs) store the real LR by construction
   and are covered by item 2 instead. A refused LR then becomes a named, survivable fatal
   at the moment of use, naming its thread — not an anonymous `PC_ALIGN` at `0x19`.
2. **`CpuContext` / `Aarch64ExceptionFrame` get a magic word, a `size_of` const-assert
   beside the existing offset asserts, and the scheduler-owned per-slot occupancy epoch
   already planned for PR-B**, stamped at save and checked in the Rust admission. A foreign
   overlay at +232 then fails a cheap equality test instead of being restored as `(x29, x30)`.
3. **Close the ret-dispatch TOCTOU.** `take_inline_ret_dispatch_info` returns a *copy* of
   the callee-saved context, staged in per-CPU memory under the scheduler lock, and
   `aarch64_ret_to_kernel_context` restores from that staging slot. The admitted word and
   the restored word are then provably the same bytes, and the raw `ctx_ptr` outliving
   `drop(guard)` disappears with it.
4. **Oracle and acceptance.** A mutation that writes a small integer into a saved x30 slot
   must redden a new leg: with the fix, the boot survives and emits `[LR_REFUSED:…]`;
   with the fix removed, the leg reproduces a `PC_ALIGN` fatal. Acceptance for the family
   is the refusal converted to a hard gate failure and the tid-as-PC bucket at zero with
   the tolerance removed.
5. **Make the dispatch trace stop lying.** Record a `R`-path entry at the ret-based
   dispatch site (context_switch.rs:5155) so the ring's newest entry names the dispatch
   that actually happened.

## 6. Provenance

* Fault record: `parallels-run3-serial-FAULT.log:1866-1885`.
* Run-3 binary: carved from `target/parallels/breenix-efi.hdd/breenix-efi.hdd.0.{5fbaabe3-6958-40ff-92a7-860e329aab41}.hds`
  at offset `0x20e800`, verified by `idle_loop_arm64 == 0xffff00004045e184`.
* No Parallels VM left running (`breenix-1787504519` is `stopped`).
