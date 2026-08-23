# PR-B round 3 — saved link-register custody, and why it is a census

Branch `fix/prb-producer-custody`, base `3ee79f70`. RCA:
[`PRB-R3-RCA-LIVE-X30.md`](PRB-R3-RCA-LIVE-X30.md).

The RCA named a real structural gap. The campaign hardened every ARCHITECTURAL
resume PC — the word that reaches `ELR_EL1` — and the link register is a second,
DEFERRED PC: whatever word sits at `CpuContext`/frame offset 240 becomes the
target of the next `ret` the resumed code executes, at an unbounded later time,
with no record tying that branch back to the dispatch that installed it. The
T3-G run-3 boot was killed by exactly that, a `ret` to `0x19` — the outgoing tid.

The RCA's repair was to refuse an inadmissible word at the producers and store a
named trampoline instead. **That repair is wrong at three of the four producers
and unreachable at the fourth, and this round measured it rather than assuming
it.** What ships is the instrumentation that produced the measurement, plus the
three parts of the plan that stood on their own.

## The negative result, with its evidence

| producer | what the word is | evidence |
|---|---|---|
| EL0, any slot | USER DATA. AArch64 leaves `x30` free for a leaf to use as scratch; `_start` is entered with 0; the kernel must preserve a user register verbatim across a trap. Rewriting it hands a user program a kernel return address — #637's face, manufactured by the guard meant to prevent it. | An EL1-*named* save saw `0x400008ec` and `0x400048ac` (user link base `0x40000000`) in one 60 s boot, because `save_kernel_context_inline` is entered whenever the entry stub's `from_el0` flag is clear — including the branch whose frame SPSR says EL0. |
| exception-saved EL1, save | A live register. Kernel code may use `x30` as a scratch temporary once it has stored its own return address. | `[LR_NONTEXT:site=save-el1:tid=29:lr=0xffff00005038cf50]` — a kernel heap address. Earlier boots: `0x28` and `0x0b2d05e0`, both tid 32. |
| exception-saved EL1, restore | The same word put back. | The same boot produced the matching `restore-el1` of `0xffff00005038cf50`. |
| inline-saved EL1 (architectural) | `x30` IS a resume PC by construction (`stp x29, x30, [x0, #232]` stores a `bl` return address, `str x30, [x0, #264]` makes it `elr_el1` too) — a non-PC word here IS a corruption, and this is the run-3 specimen's class. | Already refused TWICE, upstream of any saved-LR copy: `take_inline_ret_dispatch_info` admits the same word as its ret-dispatch resume PC, and `restore_kernel_context_inline` derives its `resume_pc` from `x30` for this class and refuses through `RESUME_PC_SOURCE_EL1_RESTORE`. Leg L drives the specimen's shape in and both fire. |

A third guard on that fourth path would be unreachable code carrying a
fail-closed claim. So the accessor classifies and reports; it does not judge.

## What changed

1. **One accessor owns every Rust copy into a saved-LR slot.** `set_saved_lr`
   censuses the word into four new `[RESUME_PC_CENSUS:` rows
   (`lr-save-el1`, `lr-restore-el1`, `lr-save-el0`, `lr-restore-el0`), stores it
   verbatim, and reports an EL1 word that is not a PC as `[LR_NONTEXT:` —
   bounded, counted, read back in the fatal postmortem. Four dispatch producers
   and the two HAL helpers route through it, so a producer added later inherits
   the census instead of reopening the blind spot. Kernel-authored constants
   (`0`, the idle entry point) are not copies and are left alone; what cannot
   exist any more is `<slot>.x30 = <other>.x30`, the chain the specimen's word
   travelled.

2. **The exception level comes from the frame, never from the caller's name.**
   `saved_lr_el_of_frame` delegates to the file's single exception-level
   predicate, which also keeps the existing "exactly one pure SPSR predicate"
   ratchet true.

3. **The ret-dispatch TOCTOU is gone.** `take_inline_ret_dispatch_info` copies
   the callee-saved context into per-CPU staging under the scheduler lock and
   hands the assembly a pointer to the copy. The admitted word and the restored
   word are now the same bytes; the raw `&thread.context` that outlived
   `drop(guard)` — an unguarded pointer into the scheduler's `threads` `Vec`,
   across a window a growth, an element shift or a row free could move — is
   gone, and so is the only other Rust dereference of it (the dispatch-mismatch
   check now reads the live row's `elr_el1` under the lock). A staging copy that
   disagrees with what was admitted emits `[RET_STAGE_REFUSED:` and falls back
   to the ERET path with the lock still held; that marker is gate-failing.

4. **`CpuContext` carries an identity word**, checked by the staging copy before
   a pointer into a row is used to restore registers, plus `size_of`
   const-asserts for both saved-context types beside the existing offset
   asserts. An offset assert pins where a field starts and says nothing about
   what follows it.

5. **The dispatch trace stops lying.** `record_dispatch` had two call sites, both
   on the ERET path, so a fatal that followed a ret dispatch showed a newest
   entry naming whatever was ERET-dispatched before it — in the specimen an idle
   dispatch, while `current_tid` read the ret-dispatched thread. Both ret sites
   now record an `R` row.

6. **A self-test verdict blind spot is closed, for every leg.** They all read
   `any_fatal_postmortem_captured()` for their `fatal=` field, and that flag is
   set only by `dump_fatal_postmortem_once`. A PC-alignment abort at EL1 stops at
   `[FATAL_REGS]` without reaching it — an experiment during this round printed
   `[FATAL_REGS] label=PC_ALIGN ... esr=0x8a000000` and would have reported
   `fatal=0`. The EL1 register dump is now counted and folded into that helper.

## Leg L

`lr_poison_oracle` writes the victim's own tid — the specimen's exact shape —
into an INLINE-SAVED context's `x30`, at the point where the ret dispatch has
just decided the thread is inline-saved. It asserts that both pre-existing
admissions fire and that the boot survives. Nothing drove that shape into that
class before, so nothing would have noticed if one of them were lost.

| run | verdict |
|---|---|
| armed | `opportunities=1:injected=1:ret_dispatch_refused=1:el1_restore_refused=1:fatal=0:PASS` |
| disarmed (anti-vacuity) | `opportunities=41:injected=0:ret_dispatch_refused=0:el1_restore_refused=0:fatal=0:PASS` — the injection point is reached 41 times in a clean boot and neither admission refuses anything |

Serials are preserved under `serials/prb-r3-*`. Also preserved:
`prb-r3-legL-red-no-substitution-20260823.txt`, from the superseded
substituting design — it is kept because it is the clearest recording of the
CONSUMER face this family is filed at: `[PC_ALIGN] ELR=0xa FAR=0xa from_el0=0`,
`esr=0x8a000000`, a `ret` through a tid at EL1.

## Ratchets, each mutation applied singly

| mutation | reddens |
|---|---|
| M1 accessor substitutes | `set_saved_lr_classifies_and_reports_without_substituting` |
| M2 `[LR_NONTEXT:` report renamed away | same |
| M4 admission drops the user-address class | `saved_lr_admission_is_the_three_named_classes` |
| M5 a producer reverts to a direct copy | `saved_lr_copies_route_through_the_single_accessor` |
| M6 ret dispatch uses the live row pointer again | `ret_dispatch_restores_a_staged_copy_taken_under_the_scheduler_lock` |
| M7 an `R` dispatch row deleted | `ret_dispatch_sites_record_an_r_path_dispatch_row` |
| M8 `size_of::<CpuContext>()` assert deleted | `saved_context_layout_and_identity_are_compile_time_facts` |
| M11 EL1 restore ignores an inline-saved `x30` | `an_inline_saved_resume_pc_is_admitted_on_both_dispatch_paths` |
| M12 ret-dispatch admission deleted | same |
| M9 gate FAIL term deleted | `service_sequence_ret_stage_refusals_fail_the_profile_and_nontext_words_do_not` |
| M10 gate gates the non-PC report | same |

**Harness note, recorded because it produced a false result first.**
`text_sources_below()` reads every readable file under the tree, so a `.bak`
left beside a source is censused as an extra source. A first mutation run kept
backups in place and reported that every mutation also reddened
`every_el1_resume_pc_consumer_uses_the_shared_admission_macro` and its EL0 twin.
That coupling was the harness, not the code.

**Gate note, same category.** `run-aarch64-service-sequence-gate.sh` does not
rebuild unless given `--rebuild`. One sample in this round ran an
`lr_poison_oracle` kernel left in `target/` by a leg, which silently violated
the gate's own "this profile builds no oracle feature" premise. Every gate
sample quoted here was taken with `--rebuild`.

## Not done, deliberately

* **No exception-frame magic word, and no per-slot occupancy epoch.** Stamping a
  frame would mean writing the EL0/EL1/SVC entry stubs — hot paths — and every
  Rust-synthesized frame, for a fact nothing in evidence disputes. For the ret
  path the staging copy achieves what the epoch was proposed for: the admitted
  bytes and the restored bytes are the same bytes, taken under the lock, from
  per-CPU memory only this CPU writes, across an IRQ-masked window. The epoch
  remains open for the ERET path, where the frame is built in place.
* **#635 is not closed, and #576/#626 stay open.**
* **Leg P (`resume_pc_el1_oracle`) is red, and was red before this round.** The
  baseline run at `3ee79f70` with no changes at all is preserved at
  `serials/prb-r3-legP-baseline-3ee79f70-preexisting-red.txt`: three
  `[FATAL_REGS]` and one `[FATAL_POSTMORTEM]`, so its `fatal=` term already read
  1. Disclosed, not inherited.
