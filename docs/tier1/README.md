# Tier-1 change records

CLAUDE.md ("PROHIBITED CODE SECTIONS") lets a Tier-1 file change only to repair
a defect that lives in it, in a minimal commit of its own, separately reviewed,
and landed with boot evidence. Each record here says, for one change, which
file and commit, why the defect lives there, what the change costs on the
path, who reviewed it and where, and which boot carried it. A pull request
body is not enough on its own once the pull request has merged.

| Record | Files | Commits |
|---|---|---|
| [#1634: x86 timer entries tell timer ticks from IPIs](1634-x86-timer-tick-kind.md) | `kernel/src/interrupts/timer_entry.asm`, `kernel/src/interrupts/timer.rs` | `eb89c4c0`, `0ef4c689`, `d3fe5f2b`, `e6e9ce46` |
| [#737: one `cld` on the timer interrupt gate](../planning/green-program/nic-bus/737-FIX-2026-09-04.md) | `kernel/src/interrupts/timer_entry.asm` | `e1622d7f` |
