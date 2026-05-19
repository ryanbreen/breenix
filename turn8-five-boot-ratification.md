# Turn 8 Five-Boot Ratification

## A. Serial-log emitter diff

Source commit: `cd637f67 feat(ahci): emit polling attribution to serial after scheduler ready`.

The emitter is in `kernel/src/drivers/ahci/mod.rs`:

- `AHCI_POLL_ATTRIB_EMITTED: AtomicBool` enforces one line per boot.
- `emit_polling_attribution_once_if_scheduler_ready()` checks the same scheduler-ready predicate used by AHCI command setup.
- When ready, it emits:

```text
[ahci-poll-attrib] total=<n> post_reg=<n> pre_sched=<n> post_sched=<n>
```

The call site is `kernel/src/main_aarch64.rs`, immediately after `spawn_as_current()` in `launch_init_from_elf()`. At this point the timer has been initialized, the scheduler has a current thread, and the emitter is outside AHCI wait/IRQ hot paths.

## B. Build result

Command:

```bash
cargo build --release --target aarch64-breenix.json \
  -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem \
  -p kernel --bin kernel-aarch64
```

Result:

```text
Finished release profile with zero warnings.
```

## C. Five-boot table

Harness: `turn8-artifacts/run_5boot_serial_gate.sh`.

| Boot | Status | Total | Post-reg | Pre-sched | Post-sched | CPU0 % | Fault markers |
|------|--------|-------|----------|-----------|------------|--------|---------------|
| 1 | pass | 72 | 70 | 70 | 0 | 99.23 | 0 |
| 2 | pass | 72 | 70 | 70 | 0 | 99.36 | 0 |
| 3 | pass | 72 | 70 | 70 | 0 | 98.99 | 0 |
| 4 | pass | 71 | 69 | 69 | 0 | 99.64 | 0 |
| 5 | pass | 72 | 70 | 70 | 0 | 99.71 | 0 |

Each boot satisfied:

- `[ahci-poll-attrib]` present in serial.
- `post_sched=0`.
- AHCI SPI 34 discovered and enabled on CPU0.
- `[ OK ] syscall path verified`.
- 0 AHCI timeout markers.
- 0 panic / synchronous exception / data abort markers.
- 0 CPU0 regression alarm markers.

## D. Aggregate result

From `turn8-artifacts/aggregate-result.txt`:

```text
boot-1: pass total=72 post_reg=70 pre_sched=70 post_sched=0 cpu0_pct=99.23 reason=-
boot-2: pass total=72 post_reg=70 pre_sched=70 post_sched=0 cpu0_pct=99.36 reason=-
boot-3: pass total=72 post_reg=70 pre_sched=70 post_sched=0 cpu0_pct=98.99 reason=-
boot-4: pass total=71 post_reg=69 pre_sched=69 post_sched=0 cpu0_pct=99.64 reason=-
boot-5: pass total=72 post_reg=70 pre_sched=70 post_sched=0 cpu0_pct=99.71 reason=-
overall: pass
post_sched: max across all boots = 0
pre_sched: distribution = 70,70,70,69,70
```

Cleanup after the final boot:

```text
All QEMU processes killed
```

`prlctl list --all` showed no `breenix-*` VMs remaining; only the unrelated `linux-probe` VM remained.

## E. PR URL

TBD after `gh pr create`.

## F. Status

COMPLETE pending PR creation.

The 5-boot serial gate ratifies the Turn 7 success criterion: there is no AHCI polling after the scheduler/timer path is ready. The remaining boot-probe polling is pre-scheduler-ready and matches the Linux boot-probe policy documented in Turn 7.
