# T3-G PR-B round 5 — the five pre-merge items

Branch `fix/prb-producer-custody`, base `b24a673c`. Ruling R51 named five items
from the round-4 review that had to close before merge. All five closed; two of
them are fixes narrower than the item as written, and the narrowing is recorded
in `PRB-R5-ADJUDICATIONS` terms below rather than left implicit.

## 1. The round-3 specimens are in-repo and attributed

`serials/prb-r3-ssgate-max-boot21-UNATTRIBUTED-percpu-stack-alien-fatal.txt` and
`serials/prb-r3-ssgate-cortexa72-boot5-percpu-stack-alien-survived.txt` are the
two specimens round 4 RCA'd and repaired. `PRB-R4-RCA-IDENTITY.md` now cites
those paths, states that they are this PR's own #635-family records rather than
a new filing, and states the fact that settles the branch-caused question: the
producing read is present on `main` — `schedule_from_kernel` reads its CPU index
one line above `disable_interrupts()` there too — so the defect is pre-existing
and branch-SURFACED, by the round-2 fail-closed setter that turned a silent
alien install into a printed refusal.

## 2. The custody predicate itself reddens a test

The rule now lives in `kernel/src/arch_impl/aarch64/percpu_custody.rs`: slot
attribution, the ownership-record decode, and the acceptance rule. It touches
neither memory nor the platform, so `tests/percpu_stack_custody.rs` includes
that file and executes the real functions — 16 cases, including the round-3
specimen refused for CPU 3 and admitted for CPU 0, the exclusive-top attribution
that a half-open test would get wrong, an unpublished slot still admissible, and
two cases whose claim closure PANICS so the short-circuit is asserted rather
than assumed. `constants.rs` keeps the geometry and the two volatile reads and
holds no rule of its own.

Deleting the `slot == cpu` conjunct reddens it. Inverting it reddens it. Both
were green before this round, three reviews running.

Two censuses keep the coverage attached: every rule `constants.rs` delegates
must be executed by the host test (derived from the source, not listed), and the
predicate must still delegate — re-inlining it reddens.

## 3. The ownership record is bracketed, and only trusted while its guard holds

The record sat directly above the half-boundary canary with nothing above it, so
a 16-byte downward overrun of the exception half rewrote it while the canary
still read clean — and round 4 had just made it load-bearing on the dispatch
path. A second sentinel now stands above the record; the offsets assert their
own order at compile time; `percpu_stack_owner_claim` returns no claim when that
sentinel is gone; and the fatal postmortem reports the two sentinels separately.

The record did NOT move: below it in stack-growth order is the scheduler half's
top, which the first scheduler frame of every dispatch writes. The direction of
the trust rule is what makes it safe — a claim can only refuse an address the
arithmetic already attributed to THIS CPU, so distrusting a reachable record
cannot admit another CPU's slot; it can only stop a clobbered record from
refusing a CPU its own stack, which was the loop the review described.

The one remaining shape that could route a refused thread back to the CPU that
declined it — a refusal from the record rather than from the arithmetic — is
closed by filtering the destination against the identity that declined it.

## 4. The ret dispatch adjudicates the SP it admits

`take_inline_ret_dispatch_info` admitted `thread.context.sp`, dereferenced it at
`+0x20` and made it SP, all upstream of the dispatcher round 4 taught to
adjudicate — which that path never reaches. It now runs the same predicate with
the same identity token, before the dereference and before the staging copy. A
refusal returns `None`, which is this function's existing fall-through to the
ERET dispatch, i.e. to the arm that already knows how to route the thread to the
CPU that owns the slot. The identity that adjudicates is also the identity that
stages, so the path cannot become the round-4 split in miniature. Idle threads
are exempt by identity: both callers gate on `!is_idle`.

The ratchet is a census of the resume ADMISSIONS — the function that restores a
kernel context and the function that stages a ret dispatch — so a third resume
path is a count mismatch rather than a silent omission.

## 5. Work-stealing declines a thread pinned to another CPU's stack

Refusing at dispatch made a thread re-selectable, not stack-pinned: the
declining CPU could steal it straight back off the owner's queue and refuse it
again. Selection now declines to STEAL such a thread and enqueues it on the CPU
that owns the slot, at all four steal pops — the ratchet, which censuses the
steal pops themselves, immediately found two of the four in a second scheduling
function that the first patch had missed.

The rule is deliberately narrower than the dispatch's: it declines to STEAL, and
does not filter a CPU's own local queue. Stealing MANUFACTURES a foreign resume;
a thread that reaches a CPU's own queue another way still meets the dispatch
adjudication, is recorded there and is routed home from there. Filtering the
local pop would delete that evidence, and the dispatch record is gate-failing.

## What this round does not do

The ERET epilogue is untouched (hot path; a redirect there would strand the
thread and destroy FATAL_REGS evidence). The round-4 D1 save sites are still not
refusals — on the save path the SP is already what it is. No `.S`/`.asm` file
changed. `#635`, `#576` and `#626` stay open: this round adds no acceptance
evidence beyond one 10-boot-per-profile local sample, which cannot close a
defect filed at ~1.5% per boot.
