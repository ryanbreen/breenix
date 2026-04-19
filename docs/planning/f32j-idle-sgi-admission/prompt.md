# Factory: F32j - Idle Sleep Gate + GIC SGI Admission Fix

## Goals

Implement F32i Options 1 and 3 together:

- Add a Linux-style ARM64 idle sleep gate so CPU0 does not enter WFI when `need_resched` or pending wake work is already visible.
- Fix the GIC SGI admission bug where SGI0 pends in CPU0's redistributor but is not visible to `ICC_HPPIR1_EL1` / `ICC_IAR1_EL1`.

## Non-goals

- Do not implement Linux's `ttwu_queue_wakelist` path in this factory.
- Do not add timer-driven wake fallbacks, polling intervals, SEV/WFE substitutions, or CPU-routing workarounds.

## Hard constraints

- Preserve F32e and F32f waitqueue lock scope and immediate wake semantics.
- Do not touch Tier 1 files.
- Keep `kernel/src/arch_impl/aarch64/gic.rs` changes minimal and documented with root-cause evidence.
- No serial breadcrumbs in IRQ/syscall/idle paths.
- Preserve the existing `dsb sy; wfi` idle instruction sequence.
- Build clean on AArch64 and x86_64 with zero warnings.
- Merge only if wait-stress and 5/5 Parallels 120s gates pass.

## Reference artifacts

- `docs/planning/f32i-cpu0-wfi-wake/diagnosis.md`
- `docs/planning/f32i-cpu0-wfi-wake/linux-audit.md`
- `docs/planning/f32i-cpu0-wfi-wake/linux-probe-validation.md`
- `docs/planning/f32i-cpu0-wfi-wake/proposal.md`
- `/tmp/linux-v6.8/kernel/sched/idle.c`
- `/tmp/linux-v6.8/drivers/irqchip/irq-gic-v3.c`

## Runbook

Follow `/Users/wrb/getfastr/code/fastr-ai-skills/general-dev/factory-orchestration/implement.md`.
