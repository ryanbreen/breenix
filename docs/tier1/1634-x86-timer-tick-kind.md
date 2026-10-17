# #1634: x86 timer entries tell timer ticks from IPIs

Four Tier-1 commits landed on main with PR #1634 (merge `84d9bbbf`). They
were made and reviewed in PR #1691, which merged into #1634's branch, and
#1634's own body did not list them. This record lists them (#1715).

## The defect

The SIGKILL teardown gate counts the timer ticks each CPU took inside a kick
window (`TIMER_TICK_TOTAL`, recorded by `trace_timer_tick` in
`timer_interrupt_handler`). On x86-64 the reschedule IPI's IDT gate pointed
at `timer_interrupt_entry` too, and the retry self-IPI used the timer's
vector, so the handler counted each IPI as a delivered tick. A window whose
observer took no timer interrupt could therefore show ticks, and the counts
the starvation verdict rests on were wrong.

## The commits

| Commit | File | What it repairs, and why there | Path cost |
|---|---|---|---|
| `eb89c4c0` | `kernel/src/interrupts/timer_entry.asm` | Which interrupt arrived is known only at the IDT gate. Once a timer delivery and an IPI enter the same stub, code after it cannot tell them apart, so the kind has to be passed from the entry. The stub becomes a macro that emits `timer_interrupt_entry` and `reschedule_interrupt_entry`, identical except for `mov esi, 0` or `mov esi, 1` before the call to `timer_interrupt_handler`. | One more register move per entry. Each entry is 387 bytes (382 before), and the second entry adds 387 bytes of `.text`. No logging, locks, allocation, formatting, mapping or I/O. |
| `0ef4c689` | `kernel/src/interrupts/timer.rs` | The handler decides whether an entry is a delivered tick, so the decision is repaired where it is made: `timer_interrupt_handler` takes the kind and calls `trace_timer_tick` only for a timer entry. | One `u8` compare and branch per entry; tick tracing is skipped for IPIs. |
| `d3fe5f2b` | `kernel/src/interrupts/timer.rs` | Intermediate, superseded. The retry self-IPI still shared the timer vector, so this commit also asked `timer_vector_entry_was_tick_x86()`, which guessed from a per-CPU retry flag and elapsed TSC time. | Added one call (CPU id, `rdtsc`, two atomic loads, up to two stores). Removed by `e6e9ce46`. |
| `e6e9ce46` | `kernel/src/interrupts/timer.rs` | `cb73b503` (not Tier-1) moved hardware IRQs to 0x30-0x3f and sent both the retry vector 0x20 and the reschedule vector 0xf0 through `reschedule_interrupt_entry`, so the kind from the entry is exact and the guess is wrong for this site. This commit drops the guess: the condition is `reschedule_ipi == 0`. | Removes `d3fe5f2b`'s call. Against main before #1634, the handler's only added work is `0ef4c689`'s compare and branch. |

Each commit touches only its own file. `git diff d3fe5f2b^ e6e9ce46 -- kernel/src/interrupts/timer.rs`
changes only the comment above the condition, so main's `timer.rs` has
`0ef4c689`'s code.

One intermediate state is wrong: between `e6e9ce46` and `cb73b503`, retry
self-IPIs still entered through `timer_interrupt_entry` with kind 0 and
counted as ticks. The order is forced, because `cb73b503` deletes the helper
and the Tier-1 change has to be committed alone. Main did not have that
state: #1691 merged both commits into #1634's branch at once.

## Review

- PR #1691's body states each commit's defect and cost:
  https://github.com/ryanbreen/breenix/pull/1691
- Review round 2, finding 8, corrected the cost statement from an assembled
  measurement (382 to 387 bytes per entry):
  https://github.com/ryanbreen/breenix/pull/1691#issuecomment-6230776031
- Separate Tier-1 sign-off on `e6e9ce46`, covering the superseded
  `d3fe5f2b`, against the five conditions:
  https://github.com/ryanbreen/breenix/pull/1691#issuecomment-6231110125
- `eb89c4c0` and `0ef4c689` were reviewed only in #1691's general review
  rounds; no separate Tier-1 sign-off names them. The pull request that adds
  this record asks for that sign-off.
- The blind review of #1634 that found the record missing:
  https://github.com/ryanbreen/breenix/pull/1634#issuecomment-6237202403

## Boot evidence

`timer.rs` and `timer_entry.asm` build only for x86-64. The x86-64 full gate at
`c68d3f2c`, which contains the four commits and `cb73b503`, passed with
`USERSPACE TEST REPORT DONE` and 257 exits (run `20261017T065146Z-x86_64-gate-115f`).
Its SIGKILL kick windows were 307, 472 and 79 us against a 5,000 us tick, each
`verdict=pass`.
