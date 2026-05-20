# Turn 7 Fault Patterns

Status: COMPLETE.

Turn 7 restored the Turn 5 xHCI IRQ-completion patch, rebuilt the aarch64 kernel, resolved the Turn 5 ELR, and ran 20 consecutive Parallels boots. The build warning greps are empty:

- `turn7-artifacts/build-warning-grep.log`
- `turn7-artifacts/build-all-warning-grep.log`

## ELR resolution

Turn 5's clean DATA_ABORT line reported:

```text
FAR=0x2a0 ELR=0xffff0000401bfe50 ESR=0x96000005 DFSC=0x5 cpu=2
x19=0x60 x20=0xffff0000401e7268 x29=0x3a x30=0xffff0000401bfe50 tid=14 name=bwm
AHCI arm port=1 cmd=1185 isr port=1 cmd=1185 waiter_tid=11
```

`addr2line` resolves `0xffff0000401bfe50` to:

```text
kernel::arch_impl::aarch64::exception::dump_fatal_postmortem_once
```

The disassembly at the faulting instruction is:

```text
ffff0000401bfe4c: bl dump_defer_requeue_snapshots
ffff0000401bfe50: ldr x8, [x19, #0x240]
ffff0000401bfe54: mov w9, #0x54
ffff0000401bfe58: strb w21, [x8, x20]
```

With `x19=0x60`, the instruction dereferences `0x60 + 0x240 = 0x2a0`, exactly matching `FAR=0x2a0`. That means the clean Turn 5 DATA_ABORT is a nested fault while fatal postmortem output was dumping, not necessarily the original faulting subsystem.

## Stress results

The 20-boot stress run captured one DATA_ABORT sample:

```text
DATA_ABORT count: 1 / 20
```

Per-boot summary from `turn7-artifacts/stress-summary.tsv`:

```text
1 run_status=0 cpu=98702 msi=59 irq=58 lock=0 failures=0 data_abort=0 pid1=yes
2 run_status=0 cpu=72978 msi=36 irq=35 lock=1 failures=0 data_abort=0 pid1=yes
3 run_status=0 cpu=78068 msi=60 irq=60 lock=0 failures=0 data_abort=0 pid1=yes
4 run_status=0 cpu=85016 msi=40 irq=39 lock=0 failures=0 data_abort=0 pid1=yes
5 run_status=0 cpu=47434 msi=59 irq=60 lock=1 failures=0 data_abort=0 pid1=yes
6 run_status=0 cpu=95688 msi=60 irq=60 lock=0 failures=0 data_abort=0 pid1=yes
7 run_status=0 cpu=76159 msi=30 irq=26 lock=0 failures=0 data_abort=0 pid1=yes
8 run_status=0 cpu=79418 msi=49 irq=47 lock=0 failures=0 data_abort=0 pid1=yes
9 run_status=0 cpu=86965 msi=36 irq=36 lock=1 failures=0 data_abort=0 pid1=yes
10 run_status=0 cpu=76647 msi=60 irq=60 lock=0 failures=0 data_abort=0 pid1=yes
11 run_status=0 cpu=82904 msi=60 irq=60 lock=0 failures=0 data_abort=0 pid1=yes
12 run_status=0 cpu=83063 msi=60 irq=60 lock=0 failures=0 data_abort=0 pid1=yes
13 run_status=0 cpu=76561 msi=59 irq=60 lock=1 failures=0 data_abort=0 pid1=yes
14 run_status=0 cpu=79130 msi=60 irq=60 lock=0 failures=0 data_abort=0 pid1=yes
15 run_status=0 cpu=93585 msi=60 irq=60 lock=0 failures=0 data_abort=0 pid1=yes
16 run_status=0 cpu=104128 msi=59 irq=60 lock=1 failures=0 data_abort=0 pid1=yes
17 run_status=0 cpu=39759 msi=59 irq=59 lock=1 failures=51 data_abort=51 pid1=yes
18 run_status=0 cpu=73221 msi=60 irq=58 lock=0 failures=0 data_abort=0 pid1=yes
19 run_status=0 cpu=85671 msi=59 irq=60 lock=1 failures=0 data_abort=0 pid1=yes
20 run_status=0 cpu=83130 msi=59 irq=60 lock=1 failures=0 data_abort=0 pid1=yes
```

Only boot 17 failed. The repeated `data_abort=51` count is the same trace event being dumped repeatedly by soft-lockup postmortem, not 51 independent first faults.

## Sample table

Expanded TSV is in `turn7-artifacts/fault-sample-table.tsv`.

| Sample | Source | Primary CPU/tid | Fault | Symbol | AHCI snapshot | Pattern |
| --- | --- | --- | --- | --- | --- | --- |
| T5-B4 | Turn 5 boot-4 reference | cpu=2, tid=14 bwm | `DFSC=0x5 FAR=0x2a0 ELR=0xffff0000401bfe50` | `dump_fatal_postmortem_once` | `port=1 cmd=1185 waiter_tid=11` | Nested postmortem fault; `x19=0x60` explains `FAR=0x2a0`. |
| T7-B17 | Turn 7 boot-17 | trace cpu=7; deferred snapshots include tid=14 | trace `DFSC=0x6`; raw DATA_ABORT line is UART-interleaved, likely `ELR=0xffff0000400d0bd8` | likely `idle_loop_arm64` | No AHCI fault snapshot near initial fault | Multi-CPU exception storm after deferred-requeue activity. |

## Boot 17 pattern

Boot 17 does not reproduce the Turn 5 AHCI-looking signature. The initial fatal output is heavily interleaved across CPUs and contains:

- `INSTRUCTION_ABORT` and `EL1_INLINE_ABORT` output before the trace-visible `DATA_ABORT`.
- `EL1_INLINE_ABORT` trace events on CPUs 5 and 6 with `x30_low32=0`.
- Deferred-requeue snapshots involving tid 14, including `elr=0x400d17f8` (`rust_syscall_handler_aarch64`) and `elr=0x400d0bd4`/`x30=0x400d0b80` (`idle_loop_arm64`).
- A CPU7 trace sequence ending in `DATA_ABORT dfsc=6` immediately after `CTX_SWITCH_ENTRY` and `DEFER_REQUEUE_*` events.

The likely DATA_ABORT ELR from the garbled raw output is `0xffff0000400d0bd8`, which disassembles inside `idle_loop_arm64`:

```text
ffff0000400d0bd0: msr DAIFClr, #0xf
ffff0000400d0bd4: isb
ffff0000400d0bd8: ldarb w8, [x20]
```

The trace window before the DATA_ABORT is:

```text
CTX_SWITCH_ENTRY old_tid<<16|new_tid=655374
UNKNOWN payload=14
DEFER_REQUEUE_STAGE stage<<16|tid=196618
DEFER_REQUEUE_SP sp_low32=1140850496
DEFER_REQUEUE_ELR elr_low32=1074588060
DEFER_REQUEUE_X30 x30_low32=1074588060
DEFER_REQUEUE_FLAGS aux_tid<<16|flags=1281
DATA_ABORT pid<<16|dfsc=6
```

The stable pattern across Turn 5 and Turn 7 is not AHCI. It is scheduler/context-switch exception cleanup state: deferred requeue, inline-saved frames, multi-CPU exception output, and postmortem dumping.
