# Factory: F11 - `gic::send_sgi()` and `IsrWakeupBuffer::push()` Internal Breadcrumbs

## Goals

- Add fine-grained AHCI ring breadcrumbs inside `gic::send_sgi(target_cpu)`.
- Add breadcrumbs immediately before and after `IsrWakeupBuffer::push()`.
- Run five `./run.sh --parallels --test 60` samples under
  `logs/breenix-parallels-cpu0/f11-send-sgi/run{1..5}/`.
- Document the last emitted SGI or wake-buffer site and name the F12 next action.

## Non-goals

- Do not change SGI delivery semantics.
- Do not change wake-buffer push semantics.
- Do not change F7/F8/F9/F10 breadcrumbs.
- Do not touch prohibited files.

## Hard constraints

- Base branch: `diagnostic/f10-isr-unblock-boundary`.
- Branch: `diagnostic/f11-send-sgi-boundary`.
- Ring pushes inside `send_sgi()` must be minimal: atomics only, no allocations,
  no locks.
- Do not add new register reads or writes inside `send_sgi()`.
- Encode `target_cpu` in `slot_mask`.
- Build clean.

## Deliverables

- New AHCI site tags for SGI and wake-buffer push boundaries.
- Seven breadcrumbs inside `send_sgi()` in the requested order.
- Two breadcrumbs around `IsrWakeupBuffer::push()` inside
  `isr_unblock_for_io()`.
- Investigation doc update with verbatim ring extracts and F12 recommendation.
- `exit.md` with standard factory sections and last-site verdicts.

## Runbook

Follow `/Users/wrb/getfastr/code/fastr-ai-skills/general-dev/factory-orchestration/implement.md`.
