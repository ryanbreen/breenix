# F12 SGI Linux Parity Probe Exit

## VERDICT

FAIL.

F12 matched Linux v6.8 SGI ordering for the GICv3 `ICC_SGI1R_EL1` write and added an `ICC_SRE_EL1` audit. The 5x Parallels sweep still produced AHCI command timeouts in 2/5 runs, so the probe did not meet the all-clean pass criteria.

## Original ask

- Create `probe/f12-sgi-linux-parity` from `diagnostic/f11-send-sgi-boundary`.
- Compare Linux v6.8 GICv3 SGI emission with Breenix.
- Match Breenix `send_sgi()` to Linux's barrier/register-access sequence.
- Add one-time per-CPU `ICC_SRE_EL1` audit output.
- Build clean.
- Run 5x `./run.sh --parallels --test 60`.
- Append investigation documentation and provide a PASS/FAIL exit.

## What changed

- `kernel/src/arch_impl/aarch64/gic.rs`
  - Added `dsb ishst` before the GICv3 `msr icc_sgi1r_el1` write.
  - Kept the existing post-MSR `isb`.
  - Added `[SRE_AUDIT] cpu=<id> sre=<value> raw=<value>` after `ICC_SRE_EL1` readback.
  - Moved secondary GICv3 ICC system-register init before the GICR range guard so secondary CPUs can emit the SRE audit before any redistributor MMIO skip.
- `docs/planning/ARM64_CPU0_SMP_INVESTIGATION.md`
  - Added the 2026-04-16 F12 investigation section with Linux parity comparison, sweep table, SRE audit output, and F13 recommendation.

## Linux cite comparison

| Linux reference | Breenix reference | Divergence | Applied fix |
| --- | --- | --- | --- |
| `/tmp/linux-v6.8/drivers/irqchip/irq-gic-v3.c:1365-1387` | `kernel/src/arch_impl/aarch64/gic.rs:1027-1066` | Linux runs `dsb(ishst)` before SGI emission and `isb()` after the `ICC_SGI1R_EL1` writes. Breenix already had `isb`, but lacked the pre-MSR `dsb ishst`. | Added `dsb ishst` before composing/writing `ICC_SGI1R_EL1`; retained post-MSR `isb`. |
| `/tmp/linux-v6.8/drivers/irqchip/irq-gic-v3.c:1350-1363` | `kernel/src/arch_impl/aarch64/gic.rs:1040-1061` | Linux composes SGIR fields and writes through `gic_write_sgi1r(val)`. Breenix composes INTID plus simple same-affinity target list. Linux does not disable DAIF/preemption here and does not use WFE/SEV. | No DAIF, WFE, or SEV change. Kept direct `msr icc_sgi1r_el1, <sgir>`. |
| F12 requirement | `kernel/src/arch_impl/aarch64/gic.rs:482-515`, `kernel/src/arch_impl/aarch64/gic.rs:1478-1499` | No per-CPU SRE audit existed. Secondary ICC init could be skipped by the GICR guard. | Added `ICC_SRE_EL1` readback audit and moved secondary ICC init before the GICR MMIO guard. |

## Build verification

Command:

```text
cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64
```

Result: clean. No `warning` or `error` lines were emitted in the compile stage.

## Validation sweep

Command for each run:

```text
./run.sh --parallels --test 60
```

Artifacts:

```text
logs/breenix-parallels-cpu0/f12-sgi-parity/run{1..5}/
```

### run1 summary

```text
exit_status=1
bsshd_started=1
ahci_timeouts=0
corruption_markers=0
sre_audit_lines=2
sre_unexpected=0
ahci_ring_entries=0
before_complete=0
after_complete=0
wake_enter=0
wake_exit=0
unblock_entry=0
unblock_after_cpu=0
unblock_after_buffer=0
unblock_after_need_resched=0
unblock_before_sgi_scan=0
unblock_per_sgi=0
unblock_exit=0
sgi_entry=0
sgi_after_mpidr=0
sgi_after_compose=0
sgi_before_msr=0
sgi_after_msr=0
sgi_after_isb=0
sgi_exit=0
wakebuf_before_push=0
wakebuf_after_push=0
```

### run2 summary

```text
exit_status=1
bsshd_started=1
ahci_timeouts=0
corruption_markers=0
sre_audit_lines=4
sre_unexpected=1
ahci_ring_entries=0
before_complete=0
after_complete=0
wake_enter=0
wake_exit=0
unblock_entry=0
unblock_after_cpu=0
unblock_after_buffer=0
unblock_after_need_resched=0
unblock_before_sgi_scan=0
unblock_per_sgi=0
unblock_exit=0
sgi_entry=0
sgi_after_mpidr=0
sgi_after_compose=0
sgi_before_msr=0
sgi_after_msr=0
sgi_after_isb=0
sgi_exit=0
wakebuf_before_push=0
wakebuf_after_push=0
```

### run3 summary

```text
exit_status=1
bsshd_started=1
ahci_timeouts=2
corruption_markers=0
sre_audit_lines=1
sre_unexpected=0
ahci_ring_entries=134
before_complete=3
after_complete=0
wake_enter=1
wake_exit=0
unblock_entry=1
unblock_after_cpu=1
unblock_after_buffer=1
unblock_after_need_resched=1
unblock_before_sgi_scan=0
unblock_per_sgi=0
unblock_exit=0
sgi_entry=16
sgi_after_mpidr=16
sgi_after_compose=16
sgi_before_msr=17
sgi_after_msr=17
sgi_after_isb=17
sgi_exit=19
wakebuf_before_push=1
wakebuf_after_push=1
```

Failure signature: two AHCI timeouts after bsshd start. SPI34 was pending+active on cpu=3 and cpu=6 with `ICC_PMR_EL1=0xf8`, `ICC_RPR_EL1=0xff`, `DAIF=0x300`, and `AHCI_PORT1_IS=0x1`.

### run4 summary

```text
exit_status=1
bsshd_started=1
ahci_timeouts=2
corruption_markers=0
sre_audit_lines=1
sre_unexpected=0
ahci_ring_entries=130
before_complete=0
after_complete=0
wake_enter=0
wake_exit=0
unblock_entry=0
unblock_after_cpu=0
unblock_after_buffer=0
unblock_after_need_resched=0
unblock_before_sgi_scan=0
unblock_per_sgi=2
unblock_exit=0
sgi_entry=17
sgi_after_mpidr=18
sgi_after_compose=18
sgi_before_msr=18
sgi_after_msr=18
sgi_after_isb=18
sgi_exit=19
wakebuf_before_push=0
wakebuf_after_push=0
```

Failure signature: two AHCI timeouts after bsshd start. SPI34 was pending+active on cpu=4 and cpu=5, followed by `[init] Failed to exec bsh: EIO` and `SOFT LOCKUP DETECTED`.

### run5 summary

```text
exit_status=1
bsshd_started=1
ahci_timeouts=0
corruption_markers=0
sre_audit_lines=1
sre_unexpected=0
ahci_ring_entries=0
before_complete=0
after_complete=0
wake_enter=0
wake_exit=0
unblock_entry=0
unblock_after_cpu=0
unblock_after_buffer=0
unblock_after_need_resched=0
unblock_before_sgi_scan=0
unblock_per_sgi=0
unblock_exit=0
sgi_entry=0
sgi_after_mpidr=0
sgi_after_compose=0
sgi_before_msr=0
sgi_after_msr=0
sgi_after_isb=0
sgi_exit=0
wakebuf_before_push=0
wakebuf_after_push=0
```

## SRE audit output

```text
run1:
[SRE_AUDIT] cpu=0 sre=1 raw=0x7
[3@1ABgic]CDE eFG3[SRE_AUDIT] cIpu=3 sre=1 raw=0x7

run2:
[SRE_AUDIT] cpu=0 sre=1 raw=0x7
T1[smp] 1C@PU1A 1:B CDPSCI CPU_ONEe suFGc1[SRE_AUDIT] ccpuess=
[g6@1ABCDEeFG6[SRE_AUDIT] cpu=6i sre=1 rawc=] ICC0_xCTL7
7@1ABCDEeFG7[SRE_AUDIT] cpu=7 sre=1 raw=0x7

run3:
[SRE_AUDIT] cpu=0 sre=1 raw=0x7

run4:
[SRE_AUDIT] cpu=0 sre=1 raw=0x7

run5:
[SRE_AUDIT] cpu=0 sre=1 raw=0x7
```

CPU0 consistently read `ICC_SRE_EL1=0x7`. Visible secondary samples also show `sre=1`, but raw UART interleaving and incomplete secondary coverage mean F12 did not prove clean one-line-per-CPU SRE state for all CPUs.

## Known risks and gaps

- The probe did not meet pass criteria because runs 3 and 4 had AHCI command timeouts.
- SRE audit output is not serialized, so secondary lines can interleave with other boot output.
- Secondary audit coverage is inconsistent across runs, which points to the next probe needing a more reliable per-CPU audit channel.

## Recommendation

Do not proceed to F-final. F13 should pivot to GICR redistributor and per-CPU ICC state, not another SGI-side barrier probe. The next probe should capture clean per-CPU values for `ICC_SRE_EL1`, `ICC_CTLR_EL1`, `ICC_PMR_EL1`, `ICC_IGRPEN1_EL1`, MPIDR affinity, and selected GICR frame/waker/config state, then correlate that with CPUs reporting SPI34 pending+active.
