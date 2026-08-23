# PR-B round 4 — results

Branch `fix/prb-producer-custody`, base `63a814c5`. RCA: `PRB-R4-RCA-IDENTITY.md`.

## What round 3 left, and what it actually was

Round 3 shipped the ret-dispatch staging copy and the saved-LR census, and its
acceptance sample still carried one production `[PERCPU_STACK_ALIEN:` record —
`cpu=3 owner=0 sp=0xffff000043200000 tid=1201`, from the null-scheduler fallback
of `inline_schedule_trampoline`. Round 2 had already unified the custody
PREDICATE between the producer that chooses an idle-dispatch SP and the setter
that installs it, so a shared predicate answering differently at two sites
microseconds apart forced one conclusion: the two sides were feeding it
different CPU identities.

They were. `schedule_from_kernel` read its index one line ABOVE
`disable_interrupts()`. Every caller of that function runs with interrupts
enabled — that is why it saves DAIF and masks explicitly — so the read happened
in a preemptible window. A timer IRQ there requeues the thread, any CPU may
resume it (the ready queues work-steal), and the carried index then names the
CPU the thread used to be on. Spent afterwards it selects
`scheduler_stack_top(cpu_id)`, `INLINE_SCHEDULE_STATE[cpu_id]`,
`cpu_state[cpu_id]` and `DEFERRED_REQUEUE[cpu_id]` — which is precisely the
observed specimen: CPU 3 standing on CPU 0's scheduler half, finding its own
inline-schedule slot empty (the state was published in slot 0) and therefore
always landing in the null fallback, with two CPUs' frames at the same offsets
of one stack.

tid 1201 is init's main thread on a heap-backed kernel stack — the victim
stamped by `last_dispatched_tid`, not the producer. No corrupted TCB field, no
migrated `kernel_stack_top` and no torn row is needed to explain
`0xffff000043200000`: it is `cpu_state[0].idle_thread`'s stack top, and a
`cpu_id` of 0 is the whole explanation.

## The repair

* **The identity is read after the mask.** `schedule_from_kernel` mints below
  `disable_interrupts()`; `setup_idle_return_arm64` mints after its
  `with_scheduler` hold rather than before it.
* **`CpuId`.** A one-word `Copy` token with no constructor from a `usize`:
  `current()` and `current_checked(carried)`, both hardware reads. Every per-CPU
  stack decision on the dispatch path takes the token, so a carried index cannot
  reach one. `current_checked` records `[CPU_IDENTITY_SPLIT:` when the carried
  value disagrees, and the hardware answer wins.
* **One identity for producer and setter.** The trampoline's fallback re-reads on
  the same side of the `with_scheduler` spill as the install. The comment that
  called that disagreement benign is deleted: it means the invocation belongs to
  another CPU. `schedule_from_kernel` re-checks at the pivot and retracts the
  `INLINE_SCHEDULE_STATE` slot it no longer owns, so its owner cannot consume it.
* **Custody on the SP a CPU runs on.** `assert_pivot_free` becomes
  `pivot_destination`: the self-alias record is unchanged, and a destination
  naming a slot this CPU does not own is refused and replaced by the same half of
  this CPU's own slot. All four pivots go through it. A per-CPU word install was
  guarded; a pivot is not an install, and `mov sp, x` runs on `x` regardless.
* **Custody-aware migration.** The dispatch adjudicates `thread.context.sp`
  before it installs anything for the dispatch. A non-idle thread whose saved
  kernel SP stands in another CPU's slot is enqueued on the CPU that OWNS the
  slot and this CPU takes an ordinary idle redirect (`PERCPU_STACK_FOREIGN`). It
  is not terminated, and it is not bounced onto the declining CPU's own queue —
  that would hand it straight back. This is the shape behind the round-3 leg
  records at the kernel-thread restore sites.

## One pre-existing gate defect, fixed here

The census row `printf` had 12 conversion specifiers for 13 arguments. Shell
reuses a format that runs out of specifiers, so every boot appended a second,
headerless row with an empty bucket field, and the gate's failure report printed
one bogus line per boot. The header named 10 columns for 11. Header, format and
argument list now agree, the `Total` census reports the three terms it was
silently dropping, and a ratchet pins the three counts to each other.

## Verification

* 13 host structural suites, 333 tests, all pass.
* Both kernel profiles (`boot_tests`, `testing,external_test_bins`) build on
  `aarch64-breenix-kernel.json` with zero warnings; `check-kernel-no-neon.sh`
  PASS with 0 FP/SIMD load/stores.
* 7 ratchet mutations, each applied singly, each reddening its own ratchet:
  identity read above the mask; a pivot that skips adjudication; a token minted
  from the carried index; a refused thread routed back to the declining CPU; a
  dispatch that stops adjudicating the resume stack; the identity-split term
  dropped from the gate FAIL condition; a census column the header stops naming.
* `run-aarch64-service-sequence-gate.sh --profile both --boots 10 --rebuild`:
  both profiles PASSED, every bucket zero, including `PERCPU_STACK_ALIEN=0` and
  the new `CPU_IDENTITY_SPLIT=0`, 20/20 GREEN.

## Standing

A 10-boot sample does not close #635, #576 or #626 — the round-3 record appeared
roughly once per profile-sample of this size, so zero here is consistent with the
fix and is not proof of it. The tolerances stay removed and the refusals stay
gate-failing; the close retakes belong to the acceptance run, not to this round.
