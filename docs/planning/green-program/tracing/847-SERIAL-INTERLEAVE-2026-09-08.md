# 847: aarch64 serial ownership — R247 — 2026-09-08

Final code revision: `c959cfce63bfa966c72c588ddabaa3a52b7d4afc` on
`tracing/847-serial-interleave`. The baseline was
`4394409fca3932296f3468914b5be325ce0d48a6`. Issue 847 and its comments were
read for this round. PR 874 had moved the ring-span report to a thread;
this round addresses the remaining shared-UART raw-writer class.

**x86 is not covered by this UART serialization fix.** Its raw writers remain
excluded under R247. The x86 gate is a compatibility check because the shared
tracing callers now use a small writer adapter; it does not test x86 UART
atomicity.

## Census at HEAD

The aarch64 architecture census rejects raw serial API references, including
aliases, and direct UART stores recognized by the existing primitive scanner.
Its current result is empty outside the owned hardware emitter. Architecture
exclusion comes from cfg predicates and cfg-gated module paths, not writer
names. Feature predicates remain possible, so this scan includes optional
instrumentation as a conservative superset of the strict and production
profiles. The older cross-architecture literal census remains a separate
historical ratchet; it is not the R247 enforcement mechanism.

Census implementation: `tests/serial_line_atomicity_structure.rs:1289`. Cfg/path rules: `tests/serial_line_atomicity_structure.rs:1205` and `tests/serial_line_atomicity_structure.rs:1254`.
The inventory below names stack-record producers at this revision, including
`raw_uart_*` and the first-syscall direct-MMIO writer found during the UART
audit. Numbers in the buffer-sites column are source line numbers for
record construction/append sites, not counts of runtime emissions. Primitive
numeric/string helpers were replaced by the stack builder at `kernel/src/serial_line.rs:177`;
the capture byte sink now appends to its own record using that builder.
Single-character entry breadcrumbs also use the owned API, but are not
counted as multibyte records in this table.

| Producer | HEAD source / buffer sites | Context | Output |
|---|---|---|---|
| `set_saved_lr` | `kernel/src/arch_impl/aarch64/context_switch.rs:457` (buffer sites: 483) | IRQ / dispatch / fatal diagnostics | `[LR_NONTEXT…]` |
| `record_resume_pc_refusal` | `kernel/src/arch_impl/aarch64/context_switch.rs:536` (buffer sites: 545) | IRQ / dispatch / fatal diagnostics | `[RESUME_PC_REFUSED…]` |
| `emit_resume_pc_census` | `kernel/src/arch_impl/aarch64/context_switch.rs:869` (buffer sites: 872) | IRQ / dispatch / fatal diagnostics | `[RESUME_PC_CENSUS…]` |
| `log_last_defer_requeue_snapshot` | `kernel/src/arch_impl/aarch64/context_switch.rs:1162` (buffer sites: 1163) | IRQ / dispatch / fatal diagnostics | `[DEFER_SNAP…]` |
| `dump_all_save_skew_snapshots` | `kernel/src/arch_impl/aarch64/context_switch.rs:2034` (buffer sites: 2048, 2072) | IRQ / dispatch / fatal diagnostics | `[SAVE_SKEW…]` |
| `dump_all_idle_redirect_histories` | `kernel/src/arch_impl/aarch64/context_switch.rs:2166` (buffer sites: 2168) | IRQ / dispatch / fatal diagnostics | `[IDLE_REDIRECT_HISTORY…]` |
| `dump_stack_pivot_alias_history` | `kernel/src/arch_impl/aarch64/context_switch.rs:2281` (buffer sites: 2283) | IRQ / dispatch / fatal diagnostics | `[STACK_PIVOT_ALIAS_HISTORY…]` |
| `dump_all_inline_save_skew_snapshots` | `kernel/src/arch_impl/aarch64/context_switch.rs:2402` (buffer sites: 2407, 2422) | IRQ / dispatch / fatal diagnostics | `[INLINE_SAVE_SKEW…]` |
| `dump_all_dispatch_mismatch_snapshots` | `kernel/src/arch_impl/aarch64/context_switch.rs:2500` (buffer sites: 2504, 2520) | IRQ / dispatch / fatal diagnostics | `[DISPATCH_MISMATCH…]` |
| `dump_all_last_dispatched_tids` | `kernel/src/arch_impl/aarch64/context_switch.rs:2660` (buffer sites: 2662) | IRQ / dispatch / fatal diagnostics | `[LAST_DISPATCHED_TID…]` |
| `dump_all_eret_guard_records` | `kernel/src/arch_impl/aarch64/context_switch.rs:2678` (buffer sites: 2680) | IRQ / dispatch / fatal diagnostics | `[ERET_GUARD_REDIRECT…]` |
| `dump_all_eret_frame_anomaly_snapshots` | `kernel/src/arch_impl/aarch64/context_switch.rs:2755` (buffer sites: 2759, 2781) | IRQ / dispatch / fatal diagnostics | `[ERET_ANOMALY…]` |
| `record_ret_stage_refusal` | `kernel/src/arch_impl/aarch64/context_switch.rs:3030` (buffer sites: 3031) | IRQ / dispatch / fatal diagnostics | `[RET_STAGE_REFUSED…]` |
| `dump_dispatch_trace` | `kernel/src/arch_impl/aarch64/context_switch.rs:3145` (buffer sites: 3153, 3164) | IRQ / dispatch / fatal diagnostics | Dispatch history: path, old/new TID, ELR, SPSR, X30 and SP |
| `log_bad_thread_sp` | `kernel/src/arch_impl/aarch64/context_switch.rs:3373` (buffer sites: 3374) | IRQ / dispatch / fatal diagnostics | Caller tag plus TID/PID/CPU, SP, kernel stack top and saved registers |
| `log_idle_thread_context` | `kernel/src/arch_impl/aarch64/context_switch.rs:3397` (buffer sites: 3398) | IRQ / dispatch / fatal diagnostics | Caller tag plus idle TID/PID/CPU, stack and saved registers |
| `check_inline_save_resume_point` | `kernel/src/arch_impl/aarch64/context_switch.rs:3438` (buffer sites: 3439) | IRQ / dispatch / fatal diagnostics | `[CTX596_ORACLE…]` |
| `check_inline_eret_resume_pc` | `kernel/src/arch_impl/aarch64/context_switch.rs:3463` (buffer sites: 3464) | IRQ / dispatch / fatal diagnostics | `[CTX596_ORACLE…]` |
| `record_inline_elr_divergence` | `kernel/src/arch_impl/aarch64/context_switch.rs:3484` (buffer sites: 3485) | IRQ / dispatch / fatal diagnostics | `[CTX596_ELR_DIVERGENCE…]` |
| `take_inline_ret_dispatch_info` | `kernel/src/arch_impl/aarch64/context_switch.rs:3530` (buffer sites: 3556) | IRQ / dispatch / fatal diagnostics | `[RET_DISPATCH_REFUSED…]` |
| `save_userspace_context_inline` | `kernel/src/arch_impl/aarch64/context_switch.rs:3761` (buffer sites: 3763, 3868) | IRQ / dispatch / fatal diagnostics | `[SKIP_ZERO_FRAME_SAVE…]`, `[ZERO_CTX_SAVE…]` |
| `save_kernel_context_inline` | `kernel/src/arch_impl/aarch64/context_switch.rs:3886` (buffer sites: 3888, 3955, 4047) | IRQ / dispatch / fatal diagnostics | `[INLINE_SAVE_OVERWRITE…]`, `[SKIP_ZERO_FRAME_SAVE…]`, `[ZERO_CTX_SAVE…]` |
| `restore_kernel_context_inline` | `kernel/src/arch_impl/aarch64/context_switch.rs:4070` (buffer sites: 4131, 4251) | IRQ / dispatch / fatal diagnostics | Invalid kernel dispatch context fields or bad-ELR idle redirect warning |
| `dispatch_thread_locked` | `kernel/src/arch_impl/aarch64/context_switch.rs:4710` (buffer sites: 4889, 5035, 5130) | IRQ / dispatch / fatal diagnostics | `[BUG…]`, `[TTBR_GONE…]`, `[TTBR_GONE_K…]` |
| `check_need_resched_and_switch_arm64` | `kernel/src/arch_impl/aarch64/context_switch.rs:5191` (buffer sites: 5676) | IRQ / dispatch / fatal diagnostics | `[DEFER_EVICT…]` |
| `set_next_ttbr0_for_thread` | `kernel/src/arch_impl/aarch64/context_switch.rs:6969` (buffer sites: 6990) | IRQ / dispatch / fatal diagnostics | `[TTBR_DIAG…]` |
| `emit_schedule_boot_marker` | `kernel/src/arch_impl/aarch64/context_switch.rs:7195` (buffer sites: 7197) | IRQ / dispatch / fatal diagnostics | Scheduler-return boot marker |
| `emit_el0_entry_marker` | `kernel/src/arch_impl/aarch64/context_switch.rs:7207` (buffer sites: 7209) | IRQ / dispatch / fatal diagnostics | EL0_ENTER and EL0_SMOKE first-userspace lines |
| `dump_el1_fatal_frame_and_dispatch_trace` | `kernel/src/arch_impl/aarch64/exception.rs:199` (buffer sites: 206) | IRQ / dispatch / fatal diagnostics | `[FATAL_REGS…]` |
| `dump_el1_first_fault` | `kernel/src/arch_impl/aarch64/exception.rs:242` (buffer sites: 250) | IRQ / dispatch / fatal diagnostics | `[EL1_FIRST_FAULT…]`, `[FATAL_REGS…]`, `[UNHANDLED_EC…]` |
| `defer_current_user_thread_sigsegv_exit` | `kernel/src/arch_impl/aarch64/exception.rs:387` (buffer sites: 407, 419) | IRQ / dispatch / fatal diagnostics | Fault label, deferred TID and queued flag |
| `dump_fatal_postmortem_section` | `kernel/src/arch_impl/aarch64/exception.rs:465` (buffer sites: 469) | IRQ / dispatch / fatal diagnostics | Caller-supplied postmortem section heading |
| `dump_fatal_postmortem_once` | `kernel/src/arch_impl/aarch64/exception.rs:482` (buffer sites: 489, 515) | IRQ / dispatch / fatal diagnostics | `[BXCAP…]`, `[FATAL_POSTMORTEM…]` |
| `dump_stack_classification` | `kernel/src/arch_impl/aarch64/exception.rs:615` (buffer sites: 621, 632, 645) | IRQ / dispatch / fatal diagnostics | STACK classification, CPU or allocated stack slot and last TID |
| `handle_sync_exception` | `kernel/src/arch_impl/aarch64/exception.rs:684` (buffer sites: 806, 912, 1053, 1102, 1411, 1452, 1486, 1546, 1683) | IRQ / dispatch / fatal diagnostics | `[DATA_ABORT…]`, `[DIAG…]`, `[DISPATCH_MISMATCH…]`, `[EL0_DIAG…]`, `[EL1_INLINE_ABORT…]`, `[ERET_ANOMALY…]`, `[FATAL_REGS…]`, `[FATAL_THREAD…]`, `[INLINE_SAVE_SKEW…]`, `[INSTRUCTION_ABORT…]`, `[PC_ALIGN…]`, `[PT_WALK…]`, `[SAVE_SKEW…]`, `[SP_ALIGN…]`, `[UNHANDLED_EC…]` |
| `record_cpu_identity_split` | `kernel/src/arch_impl/aarch64/percpu.rs:201` (buffer sites: 206) | IRQ / dispatch / fatal diagnostics | `[CPU_IDENTITY_SPLIT…]` |
| `record_percpu_stack_alien` | `kernel/src/arch_impl/aarch64/percpu.rs:282` (buffer sites: 290) | IRQ / dispatch / fatal diagnostics | `[PERCPU_STACK_ALIEN…]` |
| `emit_el0_syscall_marker` | `kernel/src/arch_impl/aarch64/syscall_entry.rs:48` (buffer sites: 49) | Syscall (thread, may be masked) | EL0_SYSCALL and syscall-path-verified lines |
| `emit_tick_breadcrumb` | `kernel/src/arch_impl/aarch64/timer_interrupt.rs:985` (buffer sites: 986) | Timer IRQ / tick | Two-byte T + tick digit for existing first ten ticks |
| `report_cpu0_regression` | `kernel/src/arch_impl/aarch64/timer_interrupt.rs:991` (buffer sites: 992) | Timer IRQ / tick | CPU0 regression alarm, local and maximum peer tick counts |
| `dump_lockup_state` | `kernel/src/arch_impl/aarch64/timer_interrupt.rs:1062` (buffer sites: 1065) | Timer IRQ / tick | End soft-lockup dump trailer (also invokes progress and BXCAP emitters) |
| `emit_lockup_progress` | `kernel/src/arch_impl/aarch64/timer_interrupt.rs:1070` (buffer sites: 1071) | Timer IRQ / tick | Soft-lockup heading and elapsed seconds/ticks |
| `put` | `kernel/src/capture/record.rs:137` (buffer sites: 149) | Fault / watchdog IRQ; also thread self-test | BXCAP record bytes |
| `write_str` | `kernel/src/serial_aarch64.rs:231` (buffer sites: 232) | Thread / early boot serial API | Caller-supplied bytes/text or diagnostic fields |
| `write_bytes_atomic` | `kernel/src/serial_aarch64.rs:327` (buffer sites: 328) | Thread / early boot serial API | Caller-supplied bytes/text or diagnostic fields |
| `_print` | `kernel/src/serial_aarch64.rs:332` (buffer sites: 334) | Thread / early boot serial API | Caller-supplied bytes/text or diagnostic fields |
| `try_print` | `kernel/src/serial_aarch64.rs:337` (buffer sites: 339) | Thread / early boot serial API | Caller-supplied bytes/text or diagnostic fields |
| `writer` | `kernel/src/serial_line_oracle.rs:12` (buffer sites: 24) | Boot-test thread | `[SERIAL_INTERLEAVE…]` |
| `record` | `kernel/src/syscall/futex_timeout_record.rs:49` (buffer sites: 57) | Syscall (thread, may be masked) | Futex timed-wait arbitration: TID, removal, signal, deadlines, timer pop, errno and count |
| `inject_ret_zero_pc_if_armed` | `kernel/src/task/ret_zero_pc_oracle.rs:247` (buffer sites: 248) | IRQ / dispatch; optional boot-test injection features | `[RET_ZERO_PC_ORACLE…]` |
| `inject_ret_stack_pc_if_armed` | `kernel/src/task/ret_zero_pc_oracle.rs:277` (buffer sites: 278) | IRQ / dispatch; optional boot-test injection features | `[RET_STACK_PC_ORACLE…]` |
| `inject_ret_floor_if_armed` | `kernel/src/task/ret_zero_pc_oracle.rs:311` (buffer sites: 326) | IRQ / dispatch; optional boot-test injection features | `[RET_FLOOR_ORACLE…]` |
| `inject_exec_commit_if_armed` | `kernel/src/task/ret_zero_pc_oracle.rs:352` (buffer sites: 353) | IRQ / dispatch; optional boot-test injection features | `[EXEC_COMMIT_DISARM_ORACLE…]` |
| `inject_el1_frame_resume_pc_if_armed` | `kernel/src/task/ret_zero_pc_oracle.rs:415` (buffer sites: 451) | IRQ / dispatch; optional boot-test injection features | `[RESUME_PC_EL1_ORACLE…]` |
| `inject_el0_resume_pc_if_armed` | `kernel/src/task/ret_zero_pc_oracle.rs:486` (buffer sites: 544) | IRQ / dispatch; optional boot-test injection features | `[RESUME_PC_EL0_ORACLE…]` |
| `inject_el0_frame_resume_pc_if_armed` | `kernel/src/task/ret_zero_pc_oracle.rs:585` (buffer sites: 637) | IRQ / dispatch; optional boot-test injection features | `[RESUME_PC_EL0_ORACLE…]` |
| `inject_saved_lr_if_armed` | `kernel/src/task/ret_zero_pc_oracle.rs:1646` (buffer sites: 1664) | IRQ / dispatch; optional boot-test injection features | `[LR_POISON_ORACLE…]` |
| `dump_cpu_state_history` | `kernel/src/task/scheduler.rs:1754` (buffer sites: 1754) | Fatal/debug dump, IRQ or thread | CPU state history header and setter/old/new rows |
| `hold_pinned_wake_for_home` | `kernel/src/task/scheduler.rs:4729` (buffer sites: 4741) | Scheduler lock held; IRQ or thread | `[PINNED_HOME_CPU_UNAVAILABLE…]` |
| `test_serial_output` | `kernel/src/test_framework/registry.rs:2106` (buffer sites: 2117) | Boot-test thread | `[LOGGING_TEST…]` |
| `format_event_to_serial` | `kernel/src/tracing/output.rs:523` (buffer sites: 524) | Thread / panic / debugger dump | `[TRACE…]` |
| `dump_buffer` | `kernel/src/tracing/output.rs:563` (buffer sites: 564) | Thread / panic / debugger dump | `[TRACE…]` |
| `dump_all_buffers` | `kernel/src/tracing/output.rs:635` (buffer sites: 636) | Thread / panic / debugger dump | `[TRACE…]` |
| `dump_latest_events` | `kernel/src/tracing/output.rs:672` (buffer sites: 673) | Thread / panic / debugger dump | `[TRACE…]` |
| `dump_counters` | `kernel/src/tracing/output.rs:747` (buffer sites: 748) | Thread / panic / debugger dump | `[COUNTER…]` |
| `dump_providers` | `kernel/src/tracing/output.rs:795` (buffer sites: 796) | Thread / panic / debugger dump | `[PROVIDER…]` |
| `dump_event_summary` | `kernel/src/tracing/output.rs:896` (buffer sites: 897) | Thread / panic / debugger dump | `[SUMMARY…]` |
| `send_signal_to_process_nonblock` | `kernel/src/tty/driver.rs:840` (buffer sites: 864) | IRQ or thread | `[TTY…]` |

The hardware sink is `kernel/src/serial_aarch64.rs:359`. Its argument requires an unconstructible-
outside-the-module ownership token. The only two Rust MMIO data-store legs
there select the PL011 or 16550 UART. The loop invoking that sink is
`kernel/src/serial_line.rs:88`, with the token borrowed across the record.

## EXCLUDED — x86-only raw writers (R247)

These sites retain the existing x86 behavior. A cfg/path exclusion is applied
before the aarch64 census, while the historical cross-architecture census
continues to expose them.

| Writer / consumer | HEAD source | What it emits / context | R247 exclusion reason |
|---|---|---|---|
| `raw_serial_char / raw_serial_str / raw_serial_u64` | `kernel/src/interrupts/context_switch.rs:46` | COM1 bytes, strings and integers; dispatch/IRQ | Whole interrupts module is x86-only via kernel/src/lib.rs; x86 UART serialization is outside R247. |
| `refuse_unpublished_dispatch` | `kernel/src/interrupts/context_switch.rs:92` | PMGUARD refusal fields; dispatch/IRQ | x86-only interrupts module; x86 UART serialization is outside R247. |
| `note_dispatch_guard_unavailable` | `kernel/src/interrupts/context_switch.rs:122` | PMGUARD guard-unavailable streak and CPU/TID fields; dispatch/IRQ | x86-only interrupts module; x86 UART serialization is outside R247. |
| `check_need_resched_and_switch` | `kernel/src/interrupts/context_switch.rs:156` | Scheduler boot / first-ring3 markers; IRQ return | x86-only interrupts module; x86 UART serialization is outside R247. |
| `switch_to_thread` | `kernel/src/interrupts/context_switch.rs:791` | Dispatch breadcrumbs and no-CR3 PMGUARD refusal; dispatch | x86-only interrupts module; x86 UART serialization is outside R247. |
| `restore_userspace_thread_context` | `kernel/src/interrupts/context_switch.rs:1314` | User restore breadcrumbs and PMGUARD refusal; dispatch | x86-only interrupts module; x86 UART serialization is outside R247. |
| `raw_serial_str_local` | `kernel/src/syscall/handler.rs:151` | Raw COM1 string; first syscall | handler module declaration is cfg(target_arch = x86_64); x86 UART serialization is outside R247. |
| `emit_ring3_syscall_marker` | `kernel/src/syscall/handler.rs:167` | RING3_SYSCALL and syscall-path-verified lines; syscall | handler module declaration is cfg(target_arch = x86_64); x86 UART serialization is outside R247. |
| `raw_serial_char` | `kernel/src/tracing/output.rs:151` | COM1 byte; x86 capture/dump callers | cfg(target_arch = x86_64) on this item; x86 UART serialization is outside R247. |
| `raw_serial_str` | `kernel/src/tracing/output.rs:163` | Byte loop over caller string; dump/diagnostics | cfg(target_arch = x86_64) on this item; x86 UART serialization is outside R247. |
| `raw_serial_newline` | `kernel/src/tracing/output.rs:172` | CRLF; dump/diagnostics | cfg(target_arch = x86_64) on this item; x86 UART serialization is outside R247. |
| `raw_serial_dec` | `kernel/src/tracing/output.rs:202` | Decimal integer byte loop; dump/diagnostics | cfg(target_arch = x86_64) on this item; x86 UART serialization is outside R247. |
| `raw_serial_hex` | `kernel/src/tracing/output.rs:179` | Hex integer byte loop; dump/diagnostics | cfg(target_arch = x86_64) on this item; x86 UART serialization is outside R247. |
| `raw_serial_hex16` | `kernel/src/tracing/output.rs:225` | Fixed-width hex digits; dump/diagnostics | cfg(target_arch = x86_64) on this item; x86 UART serialization is outside R247. |
| `Line adapter methods` | `kernel/src/tracing/output.rs:1033` | Pass through to the x86 raw primitives; shared callers | cfg(target_arch = x86_64) on the impl; x86 UART serialization is outside R247. |
| `send_signal_to_process_nonblock (x86 arm)` | `kernel/src/tty/driver.rs:840` | TTY signal diagnostic line; IRQ or thread | cfg(target_arch = x86_64) on the emitting block; x86 UART serialization is outside R247. |
| `Writer::put (x86 arm)` | `kernel/src/capture/record.rs:137` | Capture record bytes, composed by the capture writer | cfg(target_arch = x86_64) on import and byte call; x86 UART serialization is outside R247. |
| `emergency_print` | `kernel/src/serial.rs:167` | Formatted emergency bytes; panic | File-level cfg(target_arch = x86_64); outside aarch64 scope; x86 UART serialization is outside R247. |
| `can_schedule` | `kernel/src/per_cpu.rs:1364` | Direct diagnostic byte; scheduler | cfg-gated x86 per_cpu module; not a multibyte record; x86 UART serialization is outside R247. |

Module exclusion declarations: `kernel/src/lib.rs:26` and `kernel/src/syscall/mod.rs:24`. In particular,
`kernel/src/syscall/handler.rs` was read for the census and was not edited.
The Tier-1 file blobs are unchanged from the baseline. The aarch64
context-switch/per-CPU edits are at existing raw emission sites; they add no
logging calls. They address the defect where those diagnostics bypassed the
shared UART serializer.

## Mechanism and bounds

- Stack record construction: `kernel/src/serial_line.rs:177`. Newline submits the assembled record; drop
  submits a caller-defined unterminated tail. Numeric fields are appended
  without the formatting machinery. The backing storage is uninitialized
  until written, avoiding a capacity-sized zero-fill on each diagnostic.
- CAS ownership and Release: `kernel/src/serial_line.rs:72` and `kernel/src/serial_line.rs:79`. Submission at
  `kernel/src/serial_line.rs:151` makes one attempt for IRQ/already-masked callers, or at most
  256 attempts for an interrupt-enabled thread. It masks local IRQ/FIQ while
  owning or staging, preserving the incoming interrupt state.
- Staging: `kernel/src/serial_line.rs:114`. A CPU has 64 fixed slots, each holding up to 1024
  bytes. Slot ownership follows FREE -> WRITING -> READY -> READING -> FREE
  with Acquire/Release edges protecting the payload. A producer does not
  wait for a consumer. An oversize record or exhausted ring increments
  `DROPPED` and discards a complete record instead of emitting a fragment.
- Thread drain: `kernel/src/serial_line.rs:291` and `kernel/src/serial_line.rs:272`; startup is `kernel/src/main_aarch64.rs:869`.
  The reporter sleeps on the scheduler timer between passes. Boot-test
  drivers may also call the thread drain while awaiting their transmitted
  record. The UART owner does not acquire the logger or scheduler mutex.
- Normal serial printing: `kernel/src/serial_aarch64.rs:332`. `TeeLine` shares the ticket but has a
  separate compile-time specialization for log capture. Raw diagnostic
  `Line` does not reach that specialization's capture-buffer bounds-panic
  path. The linked-code lockup guard checks this distinction.
- The ordinary timer path gains no reporter polling or per-tick UART
  arbitration after its existing first-ten-ticks breadcrumb condition.
  Record storage for that condition and the existing regression alarm is
  confined to cold out-of-line helpers at `kernel/src/arch_impl/aarch64/timer_interrupt.rs:985` and `kernel/src/arch_impl/aarch64/timer_interrupt.rs:991`.
  Ring-span measurement/publication and its thread report remain at
  `kernel/src/tracing/providers/irq.rs:295` and `kernel/src/test_framework/registry.rs:1825`.

## Forced-interleave oracle

The two pinned writer contexts are driven by `kernel/src/serial_line_oracle.rs:48` and `kernel/src/serial_line_oracle.rs:12`.
Each CPU emits 200 records with a 768-byte payload. A per-round rendezvous
aligns competing writers; a completion counter advances only after UART
transmission, so each CPU has at most one outstanding oracle record. This
avoids substituting staging-capacity exhaustion for the interleaving test.
The test-only byte delay in `kernel/src/serial_line.rs:88` widens the competing-write window; it
is absent from the production profile and does not delay other records.

The grammar implemented by `scripts/score-serial-interleave.py` is:

```text
[SERIAL_INTERLEAVE:cpu=<0..7>:seq=<0..199>:payload=<768 copies of ASCII ('A'+cpu)>]
```

`scripts/score-serial-interleave.py:8` requires two distinct CPUs, 200/200 unique sequence values per
CPU, and zero malformed/duplicate/payload-mismatch candidate lines. It also
rejects orphaned payload fragments. Missing records fail independently of
corruption detection. The strict gate invokes it at `docker/qemu/run-aarch64-boot-test-strict.sh:839`; the kernel
registration is `kernel/src/test_framework/registry.rs:10624`. Host-only scorer fixtures are explicitly
synthetic; they are not guest evidence and do not alter archived serials.

## Corruption grep

For each preserved strict serial, this is the corruption grep used in this
round (CR is normalized first). It checks TEST/RING_SPAN tokens and residual
fields, plus the forced-interleave oracle and its payload fragments. A split
token can be detected through its remaining fields. Missing oracle records
are separately rejected by the exact-count scorer; this grep does not establish
detection of arbitrary byte deletions.

```bash
tr -d '\r' < "$serial" |
  grep -aEn '(\[TEST:|\[RING_SPAN|SERIAL_INTERLEAVE|:span_ms=|:ticks_total=|:tick_events=|:payload=|:seq=|[A-H]{32})' |
  grep -avE '^[0-9]+:\[(TEST:[^][]+|RING_SPAN:[^][]+|SERIAL_INTERLEAVE:[^][]+)\]$'
```

A zero-line result is clean (the final grep exits 1 on no matches). Oracle
payload length, CPU binding and completeness are scored by the Python grammar
checker in addition to this broad line-boundary check.

## Validation at the code revision

`serials/847/strict20-transcript.txt` records revision
`69a2bfae8d7e7b994dde43607ba58dd3285aba68`, the kernel digest and
`bash docker/qemu/run-aarch64-boot-test-strict.sh 20`: 20 passed,
0 failed, 0 inconclusive. Its structure preflight passed 70/70 suites.
The same transcript records the linked soft-lockup allocation guard passing
with 23 reachable functions, 51 call edges and zero allocation sinks.
The aarch64 build used the soft-float kernel target; the pinned core
future-incompatibility notice is the accepted toolchain notice, with no
project-source warnings suppressed.
`serials/847/strict20-scores.txt` records 400 grammar-valid records in each
of 20 serials (8,000 total), zero corrupted records.
`serials/847/strict20-corruption-grep.txt` records zero matching corrupt
lines for the quoted grep on each preserved `strict-01.txt` through
`strict-20.txt`; final grep exit 1 means no matches.

`serials/847/service-transcript.txt` records the same code revision and
`bash docker/qemu/run-aarch64-service-sequence-gate.sh --boots 2`:
2/2 GREEN on max and 2/2 GREEN on cortex-a72 (4/4 total), gate PASS.
The per-profile serials and census TSVs are preserved alongside it.
The structure preflight was enabled. Production ran after this gate.

Rustfmt checked the 20 changed kernel Rust files with exit 0. A baseline-to-code
revision diff of the five Tier-1 paths was empty.

The first production attempt at `69a2bfae8d7e7b994dde43607ba58dd3285aba68`
exposed an unused import and a stale copied userspace driver. It is retained
in `serials/847/prod-initial-transcript.txt` and `prod-initial-serial.txt`.
The missing INPUT_INJECT negative-control marker failed the production scorer.
The import's existing boot_tests cfg was restored in
`c959cfce63bfa966c72c588ddabaa3a52b7d4afc`; that sole code follow-up leaves
boot_tests compilation unchanged. Strict and service results above are
at the explicitly recorded earlier revision. The userspace driver was
rebuilt from this branch and replaced on the lane-local ext2 image.
No production pass is attributed to that initial attempt.

`serials/847/prod-final-transcript.txt` records
`bash docker/qemu/run-aarch64-prod-profile-boot-test.sh` at
`c959cfce63bfa966c72c588ddabaa3a52b7d4afc`: PASS, exit 0, including
70/70 structure suites, the INPUT_INJECT ENOTTY negative control and
bsshd startup with boot-test seams absent. The transcript includes the
userspace driver and image digests; `prod-final-serial.txt` is its guest
capture. This was the last aarch64 kernel build/boot in this round.

The x86 compatibility lane followed the load rule: its initial one-minute
load exceeded 8, so it waited for a reading below 6. After userspace
preparation it checked again and waited a second time. The boot-tests gate
launched at one-minute load 1.33 on revision
`c959cfce63bfa966c72c588ddabaa3a52b7d4afc`. This is compatibility coverage
for the shared adapter/import changes, not x86 UART atomicity coverage.
Its structure preflight passed 70/70 suites without a timeout or retry.
Disk preparation lacked the optional BusyBox build prerequisite and omitted
those coreutils; no BusyBox coverage is claimed.

**x86 compatibility verification remains incomplete.**
`serials/847/x86-incomplete-transcript.txt` records the command
`bash docker/qemu/run-x86-boot-tests.sh`, revision, launch load, clean
kernel compilation and 70/70 preflight results. The boot step waited more
than 16 minutes for the shared gate lock and never launched QEMU. This
lane's waiting gate was terminated by its recorded PID (exit 143); the
other owner's lock and processes were left untouched. This is a blocked
verification step, not a kernel test failure or a passing x86 boot.
Issue 847 remains open pending that compatibility run and review.

To resume, run the same x86 boot-tests command from this branch after the
shared lock becomes available, applying the load rule again. No structure
suite timed out, so the 900-second retry exception was not used.

## Mutation evidence

The new aarch64 census was red against the original raw writers before
conversion (`serials/847/baseline-census-red.txt`). Its positive structure tests also insert new raw write sites,
including aliases and numeric writers, rather than comparing a literal
list of accepted sites.

Deleting the real ticket Release store made the ownership ratchet fail:
`serials/847/delete-mutation.txt` records exit 101. Returning a ticket
without CAS acquisition likewise failed the ratchet (exit 101 in
`serials/847/bypass-ratchet.txt`). The bypass kernel was built from the
code revision plus that mutation, then captured independently; the source
mutation was reverted before rebuilding and running the strict gate.
No gate preflight was skipped.

`serials/847/bypass-score.txt` records the exact scorer result for
`serials/847/bypass-serial.txt`: 2 valid records, 324 corrupted candidate
lines, verdict FAIL, exit 1. `serials/847/bypass-capture.txt` records the
bounded capture ending at its 90-second timeout (124); the corruption
verdict comes from the preserved bytes, not the timeout. The development
first-syscall failure and corrected smoke serials are also retained under
`serials/847/`; they exposed and verified removal of a direct-MMIO writer
outside the initial raw API inventory.

## Claim checks

Before the code commit:

```text
claim-lint: python3 scripts/claim-lint.py -> exit 0
claim-lint: python3 scripts/claim-lint.py --commit-msg .tmp/847-code-commit.txt -> exit 0
```

Before the import-guard correction commit:

```text
claim-lint: python3 scripts/claim-lint.py -> exit 0
claim-lint: python3 scripts/claim-lint.py --commit-msg .tmp/847-import-commit.txt -> exit 0
```

Before the evidence documentation commit:

```text
claim-lint: python3 scripts/claim-lint.py -> exit 0
claim-lint: python3 scripts/claim-lint.py --commit-msg .tmp/847-doc-commit.txt -> exit 0
```

The tree lint checks changed hunks against the baseline. It reports existing
findings outside those hunks separately, and skips the two census TSV files
because that extension is outside its supported source/document types.

Citations were re-derived from the committed source: 106 file:line anchors
resolve at the final code revision, including the scheduler's one-line shift.
The documentation commit adds evidence only; the source anchors are checked
again after it is created.

## Not claimed

- A passing x86 compatibility boot is not claimed; its preflight passed,
  but the live boot was blocked before launch.
- Optional BusyBox coreutils are not covered by the x86 preparation.
- x86 UART atomicity is not covered. Its compatibility gate does not change
  that scope.
- Lossless delivery after staging exhaustion, oversize records, a stopped
  reporter, or a failed UART is not claimed. Whole-record discard is counted.
- Arbitrary caller-split `serial_print!` fragments are not automatically
  combined into one logical line across separate calls.
- The Rust source census does not cover assembly-only diagnostics or
  arbitrary newly invented MMIO-address calculations.
- This round does not claim a cycle-exact timing comparison, exhaustive scheduler
  schedules, UART hardware coverage, or byte-for-byte ordering across different
  CPUs' diagnostic records.
