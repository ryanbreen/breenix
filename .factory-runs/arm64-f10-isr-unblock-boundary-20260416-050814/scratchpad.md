# F10 ISR Unblock Boundary Scratchpad

## 2026-04-16 05:08

Starting factory run on branch `diagnostic/f10-isr-unblock-boundary`, branched
from clean `diagnostic/f9-completion-boundary`.

Beads tracking attempted first, but `bd create` failed because the local Dolt
runtime server reports `database "breenix" not found on Dolt server at
127.0.0.1:63842`. Continuing with the factory run and recording that blocker
in exit documentation, matching F9's known gap.

Starting milestone 1: add ring-only breadcrumb sites inside
`scheduler::isr_unblock_for_io(tid)` and extend AHCI site-tag display. Scope is
limited to `kernel/src/drivers/ahci/mod.rs` and `kernel/src/task/scheduler.rs`.

Implemented milestone 1 code changes:
- `kernel/src/drivers/ahci/mod.rs`: added `AHCI_TRACE_UNBLOCK_*` constants and
  display names.
- `kernel/src/task/scheduler.rs`: added seven `push_ahci_event()` breadcrumbs in
  `isr_unblock_for_io(tid)`, with `UNBLOCK_PER_SGI` carrying target CPU in
  `slot_mask`.

Starting milestone 1 validation: format/diff hygiene and clean aarch64 build.

Milestone 1 validation results:
- `git diff --check`: passed.
- `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`: passed.
- `grep -E '^(warning|error)' /tmp/f10-aarch64-build.log`: no output.
- `cargo fmt --check`: failed on pre-existing unrelated formatting/trailing
  whitespace in files outside this F10 scope. No broad formatting applied.

Committed milestone 1 as:
`diagnostic(arm64): isr_unblock_for_io internal ring sites`.

Starting milestone 2: run the 5x Parallels sweep, summarize the new
`UNBLOCK_*` counts, append the investigation doc, and write exit documentation.

Sweep results:
- `run1`: timeout sample. Stuck token `1212` / waiter `tid=11`, last site
  `UNBLOCK_PER_SGI`, target CPU encoded as `slot_mask=0x2`.
- `run2`: timeout sample. Stuck token `1263` / waiter `tid=13`, last site
  `UNBLOCK_AFTER_CPU`; no `UNBLOCK_AFTER_BUFFER` retained for that stuck call.
- `run3`: timeout sample. Stuck token `1396` / waiter `tid=13`, last site
  `UNBLOCK_PER_SGI`, target CPU encoded as `slot_mask=0x1`.
- `run4`: reached `bsshd`, no AHCI timeout/ring output in the retained window.
- `run5`: timeout output present, but retained ring head showed token `1315`
  completing through `UNBLOCK_EXIT` / `WAKE_EXIT` / `AFTER_COMPLETE`.

Decision: rank SGI delivery as the primary F11 direction because two
discriminating timeout samples stop immediately after `UNBLOCK_PER_SGI`, with
the wake-buffer push path retained as a secondary candidate due to `run2`.

