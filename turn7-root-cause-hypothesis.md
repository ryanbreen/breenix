# Turn 7 Root-Cause Hypothesis

## Verdict

The Turn 5 xHCI conversion appears to expose an existing aarch64 scheduler/context-switch race. The evidence does not support a direct xHCI command-completion memory-safety bug, and it does not support AHCI as the stable owner of the fault.

Subsystem owner for Turn 8 investigation: aarch64 scheduler/context-switch/deferred-requeue and exception cleanup.

## Why the Turn 5 ELR is misleading

The reported Turn 5 ELR, `0xffff0000401bfe50`, resolves to `dump_fatal_postmortem_once`, not xHCI or AHCI. The faulting instruction is a raw UART/postmortem output load:

```text
ffff0000401bfe50: ldr x8, [x19, #0x240]
```

The register dump had `x19=0x60`, so the faulting address is `0x60 + 0x240 = 0x2a0`. That exactly matches the observed `FAR=0x2a0`. This is a nested fault while the exception handler was already printing fatal postmortem state.

The AHCI line in Turn 5 is still useful diagnostic context, but it is not proof that AHCI caused the first corruption. It was printed by the exception path as a snapshot of AHCI's last armed/ISR state.

## What likely failed first

Boot 17 under the restored Turn 5 patch produced a different first visible signature:

- Interleaved `INSTRUCTION_ABORT`, `EL1_INLINE_ABORT`, `UNHANDLED_EC`, and `DATA_ABORT` output from multiple CPUs.
- `EL1_INLINE_ABORT` trace events with `x30_low32=0`.
- A trace-visible `DATA_ABORT` on CPU7 with `DFSC=0x6` immediately after `CTX_SWITCH_ENTRY` and `DEFER_REQUEUE_*` trace events.
- Deferred snapshots showing tid 14 in syscall/idle-related aarch64 state, including `rust_syscall_handler_aarch64` and `idle_loop_arm64`.
- No AHCI fault snapshot near the boot 17 initial fault.

The most plausible failure mode is still the class described by existing scheduler comments: a thread or saved kernel frame is made visible for dispatch while another CPU still owns or is finishing its saved context, causing shared stack/register corruption. Once that happens, the observed ELR can move around: raw UART postmortem output, idle-loop loads, syscall-entry accounting, or instruction fetch from a bad LR can all become secondary crash sites.

## Why Turn 5 exposes it

Turn 6 showed 0/5 DATA_ABORTs on the Turn 3 baseline. Turn 7 showed 1/20 DATA_ABORTs with the Turn 5 xHCI completion patch restored.

The xHCI patch changes timing and scheduling pressure by replacing command-completion polling with real blocking completion/wakeup behavior. Under graphics/BWM load, that increases cross-CPU wakeups and scheduler traffic. The boot 17 trace shows the failure immediately after context-switch/deferred-requeue events, while the workload was active in virtio GPU/BWM wait/timeout cycles.

That makes Turn 5 a race amplifier. It should not be treated as a proven xHCI-local bug until a sample faults inside xHCI-owned state or with xHCI-specific corrupted data.

## Proposed Turn 8 direction

Turn 8 should re-apply the Turn 5 xHCI patch and diagnose/fix the scheduler invariant before accepting the xHCI conversion:

- Verify that a thread in any deferred-requeue slot cannot also be dispatched or enqueued on another CPU.
- Verify that inline-saved kernel frames are not reused after their owner thread becomes visible to other CPUs.
- Add lock-free trace breadcrumbs or use GDB for these invariants; avoid logging in interrupt/syscall hot paths.
- Keep AHCI and xHCI as observers unless new evidence points to their owned state.
- After a scheduler/context-switch fix, rerun at least the Turn 7 20-boot stress or an equivalent higher-confidence Parallels stress before committing the xHCI source conversion.

Working hypothesis for commit summary: Turn 5 exposes a pre-existing aarch64 deferred-requeue/inline-schedule race; the clean Turn 5 `FAR=0x2a0` is a nested fatal-postmortem dereference caused by already-corrupted exception/postmortem state.
