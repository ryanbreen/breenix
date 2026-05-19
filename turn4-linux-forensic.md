# Turn 4: Linux AHCI/GIC instruction-level forensic capture

**Status: INCONCLUSIVE for bind-time GIC write capture; actionable for IRQ-tail divergence.**

Linux steady-state AHCI IRQ servicing was captured on the same Parallels
`linux-probe` VM. The AHCI platform device is the root disk provider, so I did
not unbind/rebind `/sys/bus/platform/drivers/ahci/PRL4010:00`; doing so would
remove `/dev/sda` under the running system. That means this turn did not capture
the AHCI bind-time GIC write sequence. It did capture the live IRQ tail, direct
IRQ metadata, partial GICD register state, and enough Linux source-backed order
to identify one concrete Breenix divergence for Turn 5.

Artifacts are under `turn4-artifacts/linux-probe/`.

## A. Linux GIC register and IRQ reference

Probe baseline:

- Kernel: `Linux probe 6.8.0-107-generic ... aarch64`
- AHCI line: Linux IRQ `15`, GICv3 hwirq `34`, `Level`, action `ahci[PRL4010:00]`
- Affinity: `/proc/irq/15/effective_affinity=1`, `effective_affinity_list=0`
- Counts at capture time: `166442,0,0,0`, so AHCI is being serviced on CPU0
- MMIO ranges: `GICD=0x02010000..0x0201ffff`, `GICR=0x02500000..0x0257ffff`, `AHCI=0x02140000..0x02141fff`

Direct `devmem2` reads from GICD were only partially readable. The successful
SPI34-relevant reads were:

| Register | Address | Value | Meaning |
| --- | ---: | ---: | --- |
| `GICD_IPRIORITYR_32_35` | `0x02010420` | `0x00000000` | Readable, but not enough to prove priority semantics because other GICD state reads faulted |
| `GICD_ITARGETSR_32_35` | `0x02010820` | `0x00000000` | Expected RAZ/WI in GICv3 ARE mode |
| `GICD_ICFGR_32_47` | `0x02010c08` | `0x00000000` | SPI34 level-triggered (`0b00`) |
| `GICD_IROUTER_34_LO` | `0x02016110` | `0x00000000` | CPU0 affinity low half |

`GICD_TYPER`, `GICD_ISENABLER_1`, `GICD_IROUTER_34_HI`, and several status
registers bus-faulted through `/dev/mem`. The IRQ metadata and live interrupt
counts are therefore the authoritative proof that Linux has hwirq 34 enabled,
level-triggered, and effectively routed to CPU0. The raw table is in
`gicd-register-readback.txt`; the single-register sanity read is in
`devmem-gicd-single-sanity.txt`.

## B. Function graph captured

Bind-time tracing was not performed because AHCI backs the root disk. Instead,
I captured steady-state IRQ service with:

- `ftrace-ahci-irq-filtered2.txt`: function graph rooted at `gic_handle_irq`,
  plus `irq_handler_entry/exit` tracepoints.
- `ftrace-ahci-irq-snippet.txt`: the compact AHCI IRQ tail excerpt.
- `bpftrace-ahci-irq-tail4.txt`: kprobes/tracepoints confirming CPU0 AHCI action
  order during 512 MiB direct disk IO.
- `perf-ahci-irq.txt`: perf call graph for IRQ tracepoints during 1 GiB direct IO.

The key ftrace excerpt is:

```text
gic_handle_irq
  __gic_handle_irq_from_irqson
    gic_read_iar
    generic_handle_domain_irq
      handle_irq_desc
        handle_fasteoi_irq
          handle_irq_event
            __handle_irq_event_percpu
              irq_handler_entry: irq=15 name=ahci[PRL4010:00]
              ahci_single_level_irq_intr
                _raw_spin_lock
                ahci_handle_port_intr
                  ahci_handle_port_interrupt
                    ahci_qc_complete
                _raw_spin_unlock
              irq_handler_exit: irq=15 ret=handled
          cond_unmask_eoi_irq
            gic_eoi_irq
```

`bpftrace-ahci-irq-tail4.txt` independently shows the AHCI action on CPU0:

```text
cpu=0 handle_irq_event
cpu=0 __handle_irq_event_percpu
cpu=0 irq_entry irq=15 name=ahci[PRL4010:00]
cpu=0 ahci_single_level_irq_intr irq_arg=15
cpu=0 ahci_handle_port_intr
cpu=0 ahci_handle_port_interrupt
cpu=0 ahci_qc_complete
cpu=0 irq_exit irq=15 ret=1
```

The attempted bpftrace probes on `gic_eoi_irq`, `gic_unmask_irq`, and
`gic_handle_irq` kretprobe were rejected by this kernel as unavailable/notrace;
those failures are preserved in `bpftrace-ahci-irq-tail*.txt`.

## C. Linux IRQ tail sequence

The captured Linux order for the AHCI IRQ is:

1. Read interrupt ID: `gic_read_iar`.
2. Resolve and dispatch hwirq 34 through `generic_handle_domain_irq`.
3. Run generic `handle_fasteoi_irq`.
4. Run AHCI action under `handle_irq_event`.
5. AHCI reads host/port status, clears port status, completes queued commands,
   and then clears `HOST_IRQ_STAT`.
6. Only after the action returns, generic IRQ core calls
   `cond_unmask_eoi_irq`.
7. Runtime chip callback is `gic_eoi_irq`, visible in ftrace after
   `irq_handler_exit`.

The source references are recorded in `linux-source-refs.txt`. The important
Linux v6.8 source points are:

- `kernel/irq/chip.c:687-722`: `handle_fasteoi_irq()` calls
  `handle_irq_event(desc)` before `cond_unmask_eoi_irq(desc, chip)`.
- `kernel/irq/chip.c:657-676`: normal non-oneshot flow calls
  `chip->irq_eoi()`; no unmask occurs first.
- `drivers/irqchip/irq-gic-v3.c:627-641`: `gic_eoi_irq()` writes
  `ICC_EOIR1_EL1` and `isb`.
- `drivers/ata/libahci.c:2011-2044`: `ahci_single_level_irq_intr()` returns
  `IRQ_NONE` if `HOST_IRQ_STAT` is zero, handles selected ports under the host
  lock, then clears `HOST_IRQ_STAT`.

`CONFIG_ARM64_PSEUDO_NMI=y` is compiled, but this boot has no dmesg line saying
`Pseudo-NMIs enabled`. Even in the pseudo-NMI source path, Linux masks regular
IRQs through PMR before clearing DAIF for NMI admission. The captured AHCI
service path does not show a regular nested IRQ admission before the AHCI action
has completed.

## D. Breenix diff against Linux capture

### IRQ routing and trigger

Linux:

- hwirq 34, Linux IRQ 15, `GICv3`, `Level`, effective CPU0.
- Direct read of `GICD_ICFGR_32_47` is `0x0`, so SPI34 is level-triggered.
- Direct read of `GICD_IROUTER_34_LO` is `0x0`, matching CPU0 affinity low half.

Breenix:

- `kernel/src/drivers/ahci/mod.rs:2247-2248` stores `AHCI_IRQ_EDGE=false` and
  `AHCI_IRQ=found_spi` for platform AHCI.
- `kernel/src/arch_impl/aarch64/gic.rs:847-883` routes SPI to CPU0 via
  `GICD_IROUTER` and enables the SPI.
- `kernel/src/arch_impl/aarch64/gic.rs:401-407` configures SPIs as level.

Assessment: materially matches Linux for the discovered routing target and
level-triggered configuration. Direct byte-for-byte proof is incomplete because
some GICD registers fault through `/dev/mem`.

### IRQ tail and reentrancy

Linux:

- ftrace shows AHCI handler runs before `cond_unmask_eoi_irq -> gic_eoi_irq`.
- The normal runtime path does not unmask/reopen regular IRQ handling before
  `ahci_single_level_irq_intr` and `irq_handler_exit`.

Breenix:

- `kernel/src/arch_impl/aarch64/exception.rs:1317-1323` acknowledges the IRQ and
  immediately calls `gic::priority_drop_irq(irq_id)`, which writes
  `ICC_EOIR1_EL1` in `gic.rs:694-705`.
- `exception.rs:1328-1337` then runs `msr daifclr, #3` when the interrupted
  context had IRQs unmasked.
- `exception.rs:1339` only then dispatches AHCI.
- `exception.rs:1302-1305` deactivates with `gic::deactivate_irq(irq_id)` after
  action dispatch; `gic.rs:712-717` writes `ICC_DIR_EL1`.

Assessment: this diverges from the captured Linux tail. Breenix priority-drops
the level SPI and reopens a regular nested IRQ window before the AHCI source has
been serviced. Linux services AHCI first and reaches `gic_eoi_irq` only after
the action returns.

### AHCI device-side handling

Linux:

- `drivers/ata/libahci.c:2022-2025` returns `IRQ_NONE` if `HOST_IRQ_STAT` is
  zero.
- `libahci.c:1985-2008` handles only ports selected by `HOST_IRQ_STAT & port_map`.
- `libahci.c:1963-1966` clears `PORT_IRQ_STAT` before processing port completion.
- `libahci.c:2033-2042` clears `HOST_IRQ_STAT` after port events.

Breenix:

- `kernel/src/drivers/ahci/mod.rs:2345-2355` reads `HBA_IS`, but for wired
  level-triggered mode uses `check_all=true` and does not return when `HBA_IS`
  is zero.
- `mod.rs:2357-2368` scans every port in wired mode.
- `mod.rs:2386-2392` clears `PORT_IS` through `ack_port_interrupt`.
- `mod.rs:685-700` clears `PORT_IS`, drains with `dsb sy`, clears `HBA_IS`, then
  drains again.

Assessment: the device clear order is compatible with Linux's port-before-host
ordering, but the `HBA_IS == 0` behavior differs. This is a lower-ranked
divergence than the IRQ tail because Turn 3 failed by CPU0 timer starvation
after AHCI IRQs fired, not by absence of AHCI completion.

## E. Ranked instruction-level divergences

1. **Regular nested IRQ window before AHCI service.** Breenix executes
   `msr daifclr, #3` before `handle_irq_event()` for an already
   priority-dropped level SPI. Linux's captured path runs AHCI to
   `irq_handler_exit` before `gic_eoi_irq`, and does not reopen regular IRQ
   handling before the AHCI source has been cleared. This is the best match for
   Turn 3: AHCI IRQs work briefly on CPU0, then CPU0 timer delivery collapses.

2. **EOI/deactivate split does not match runtime Linux on this VM.** ftrace
   shows Linux calling `gic_eoi_irq` at the generic IRQ tail. Breenix writes
   EOIR before the action and DIR after it. If Breenix must use split
   EOImode1 semantics, it still should not admit regular nested IRQs between
   EOIR and device-side source clear.

3. **`HBA_IS == 0` wired-level scan policy differs from Linux.** Linux returns
   `IRQ_NONE` if `HOST_IRQ_STAT` is zero; Breenix scans all ports in wired
   mode. This may be valuable later, but it does not explain the CPU0 timer
   regression as directly as the IRQ-tail divergence.

4. **Register proof gap remains.** `/dev/mem` provided partial GICD reads, but
   not a complete byte-for-byte table for `ISENABLER`, `IGROUPR`, or the high
   IROUTER half. The live Linux IRQ metadata and ftrace/perf data are stronger
   than the partial direct MMIO table.

## F. Proposed Turn 5 scope

Fix one divergence: **prevent Breenix from reopening regular nested IRQs while
servicing AHCI/level SPIs.**

Exact target:

- `kernel/src/arch_impl/aarch64/exception.rs:1328-1349`

Exact Linux behavior to mirror:

- The AHCI action runs to completion before the generic GIC EOI tail:
  `handle_fasteoi_irq -> handle_irq_event -> irq_handler_exit -> cond_unmask_eoi_irq -> gic_eoi_irq`.

Minimal Breenix experiment:

- Keep AHCI routed to CPU0.
- For `irq_id == crate::drivers::ahci::get_irq().unwrap_or(...)` or, more
  generally, for external level SPIs, do not execute the
  `msr daifclr, #3` nested-window block before `handle_irq_event`.
- Leave GIC distributor setup and AHCI source-clear logic untouched for this
  experiment.
- Verify with one Parallels boot: AHCI ISR count must rise on CPU0, CPU0 timer
  count must keep pace with peers, and no AHCI timeout markers should appear.

