<!--
Staged by the T3-G PR2 repair slot (coordinator ruling R43, item 5). PR #634's "Closes #607" line
closes this issue with a bare PR reference on merge; an issue reader following only #607 would see
neither the DEVIATION D5 disclosure nor the measured-harmful design bullet. This file is the exact
text the LAND slot must post as a comment ON #607 at merge time, so both riders are visible on the
issue itself and not only inside the PR body. Do not merge this file's own content into any other
doc — it exists to be copy-pasted into `gh issue comment 607 --body-file <this file, minus this
comment block>` by the land slot.
-->

## Closed by PR #634 — the DEVIATION and the design bullet that was measured harmful

`ddd03a11` closes this issue: the `scheduler_ptr`-null fallback arm of `inline_schedule_trampoline`
now finishes the outgoing thread's transaction the way the normal arm does — normalise
`elr_el1 = x30`, requeue under the same two-part condition, clear `previous_thread`
unconditionally.

### DEVIATION D5 — leg S suppresses one recovery path, deliberately and test-only

Under `strand_inject_live_outgoing` only, the widened arm clears `previous_thread` for the CPU when
the stimulus engages, because `fix_exception_cleanup_cpu_state()` would otherwise opportunistically
re-enqueue the dropped thread and mask the very transaction under test. That backstop is
opportunistic, not guaranteed — this issue was observed in the field at 1/50 *despite* it — so
suppressing it isolates the fallback arm's own completion. It also means leg S measures that
completion, not the end-to-end field rate.

### One design bullet was measured harmful and is not here

The RCA's outgoing-handoff design had a second hunk: publish
`cpu_state[cpu].previous_thread = Some(old_id)` in `schedule_from_kernel`, symmetric with the IRQ
path. Measured in isolation on top of the containment commit:

| hunk | strand census, all three self-test profiles |
|---|---|
| the fallback-arm completion | `stranded=0:running_shape=0:ready_shape=0`, 0/3 boots |
| the `previous_thread` publication | `stranded=1:ready_shape=1`, **3/3 boots**, first strand a **user** thread at `state=Ready`, `dwell_ms` 2037/2035/2001 |

Unlike the IRQ path, the cooperative path has no `DEFERRED_REQUEUE` slot a later scheduler entry
always drains, so while the marker is up a wake publishes the thread `Ready` and declines to enqueue
it, and once the trampoline clears the marker without requeueing there is no durable owner left. It
is not in this PR, and `ddd03a11`'s commit message records the measurement rather than the
intention.

Full detail: PR #634.
