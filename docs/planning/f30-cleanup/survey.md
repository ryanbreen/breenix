# F30 Diagnostic Scaffolding Survey

Generated on 2026-04-18 from `ab351efe` on branch `f30-ahci-diagnostic-cleanup`.

Primary survey command:

```bash
rg -n "AHCI_TRACE|STUCK_SPI34|GIC_CPU_AUDIT|GICR_MAP|GICR_STATE|UNBLOCK_SCAN|LAST_USER_CTX_WRITE|CI_LOOP|TTWU_LOCAL|RESCHED_CHECK|WAKEBUF_|SGI_ENTRY|SGI_BEFORE_MSR|hello_raw|hello_println|hello_nostd_padded" \
  --glob '!target/**' --glob '!logs/**' --glob '!.factory-runs/**'
```

Secondary survey commands checked related live scaffolding names:

```bash
rg -n "GICR_STATE|dump_stuck_state|stuck_state|hello_println|hello_raw_then_println|hello_raw_padded|hello_nostd_padded|AHCI_RING|push_ahci_event|trace_sgi_boundary|dump_ahci_trace|record_gic_cpu|refresh_gicr_rdist_map|dump_gic" \
  --glob '!target/**' --glob '!logs/**' --glob '!.factory-runs/**'
rg -n "LAST_USER_CTX_WRITE|last_user_ctx_write|USER_CTX_WRITE" \
  --glob '!target/**' --glob '!logs/**' --glob '!.factory-runs/**'
```

## Live Cleanup Targets

| File | Line ranges | Markers / scaffolding |
| --- | ---: | --- |
| `kernel/src/drivers/ahci/mod.rs` | 257-294, 334, 341, 363-364, 393-409, 430-465, 475, 529-530, 547, 569, 578-587, 594-595, 605 | AHCI per-CPU trace ring, `AHCI_TRACE_*` constants, `push_ahci_event`, `dump_recent_ahci_events`, SGI target collection |
| `kernel/src/drivers/ahci/mod.rs` | 2743-2910 | AHCI ISR trace push sites: `ENTER`, `CI_LOOP`, `POST_CLEAR`, `BEFORE_COMPLETE`, `AFTER_COMPLETE`, `RETURN` |
| `kernel/src/drivers/ahci/mod.rs` | 3066-3067 | AHCI timeout diagnostic caller: `dump_stuck_state_for_spi(34)` plus recent AHCI ring dump |
| `kernel/src/task/completion.rs` | 497, 511 | Completion wake trace push sites: `WAKE_ENTER`, `WAKE_EXIT` |
| `kernel/src/task/scheduler.rs` | 164, 947-949, 2571-2583 | F17 reschedule trace budget and `RESCHED_CHECK_DRAINED_WAKE` push site |
| `kernel/src/task/scheduler.rs` | 2613, 2626-2629, 2642, 2656, 2670, 2684, 2697-2700, 2713, 2729 | `isr_unblock_for_io` breadcrumbs: `UNBLOCK_*`, `TTWU_LOCAL_*`, `WAKEBUF_*` |
| `kernel/src/arch_impl/aarch64/context_switch.rs` | 63-74 | F17 reschedule trace helper that writes AHCI ring events |
| `kernel/src/arch_impl/aarch64/context_switch.rs` | 2259, 2279, 2329, 2391, 2406, 2476, 2503, 2526, 2537, 2735 | `RESCHED_CHECK_*` push sites in IRQ-return scheduler path |
| `kernel/src/arch_impl/aarch64/exception.rs` | 1357-1364 | IRQ-tail `AHCI_TRACE_IRQ_TAIL_CHECK_RESCHED` breadcrumb |
| `kernel/src/arch_impl/aarch64/gic.rs` | 315-349, 830-1008 | `[STUCK_SPI*]` raw dump helpers and `dump_stuck_state_for_spi` |
| `kernel/src/arch_impl/aarch64/gic.rs` | 1017-1069 | SGI boundary AHCI trace pushes: `SGI_ENTRY`, `SGI_BEFORE_MSR`, related SGI breadcrumbs |
| `kernel/src/arch_impl/aarch64/gic.rs` | 1232, 1317-1337, 1395-1396, 1428, 1477-1542 | `GICR_MAP` redistributor map cache/dump scaffolding |
| `kernel/src/arch_impl/aarch64/gic.rs` | 1545-1575 | `[GICR_STATE]` dump helper |
| `kernel/src/arch_impl/aarch64/gic.rs` | 1244-1306, 1579-1625, 1858-1868 | `[GIC_CPU_AUDIT]` snapshot storage, recorder, dump function, and record call |
| `kernel/src/main_aarch64.rs` | 964-969 | Boot-time callers for `[GIC_CPU_AUDIT]` and `[GICR_MAP]` dumps |
| `kernel/src/arch_impl/aarch64/timer_interrupt.rs` | 283, 740-850 | Existing `dump_gic_state()` diagnostic path; not matched by requested prefixes except related `dump_gic` name in the secondary survey |
| `userspace/programs/src/hello_raw.rs` | 1-8 | F19 `/bin/hello_raw` userspace probe binary |
| `userspace/programs/build.sh` | 195 | `hello_raw` included in userspace image build list |
| `userspace/programs/Cargo.toml` | 339-340 | `hello_raw` bin registration |

## Historical Documentation References

These are planning/history references, not live runtime scaffolding:

| File | Line ranges |
| --- | ---: |
| `docs/planning/ARM64_CPU0_SMP_INVESTIGATION.md` | 2113-2127, 2168-2206, 2269-2270, 2319-2335, 2501-2522, 2563-2566, 2726-2759, 2767, 2771, 2778, 2907-2948, 2967-2969, 3039-3068, 3133, 3160-3173, 3200-3222, 3286, 3325-3369, 3430-3548, 3588-3653, 3679-3778, 3829-3830 |
| `docs/planning/f16-scheduler-audit.md` | 60, 202-205 |
| `docs/planning/f25-spawn-hang/phase1.md` | 1, 11, 19, 44, 47, 51 |
| `docs/planning/f25-spawn-hang/exit.md` | 17, 25, 28, 30 |

## Negative Findings

- `LAST_USER_CTX_WRITE`, `last_user_ctx_write`, and `USER_CTX_WRITE` have no current match in the repository outside ignored build/log/factory artifacts.
- No live `hello_println`, `hello_raw_then_println`, `hello_raw_padded`, or `hello_nostd_padded` source/binary registrations remain. They only appear in historical planning notes.
- `hello_raw` remains live in `userspace/programs/src/hello_raw.rs`, `userspace/programs/build.sh`, and `userspace/programs/Cargo.toml`.

## Initial Removal Grouping

1. AHCI trace ring and AHCI trace push sites.
2. Scheduler/completion/context-switch/exception breadcrumbs that only feed the AHCI trace ring.
3. GIC stuck-state, SGI boundary, GICR map/state, and CPU audit diagnostics.
4. F19 `hello_raw` probe binary and build registration.

If any candidate is found to be load-bearing during removal or validation, stop and report instead of forcing the deletion.
