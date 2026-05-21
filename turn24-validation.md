# Turn 24 Validation - Lock-free Substep 3 retry

## 24A - T22 failure context

T22 reached early network init successfully: ARP request/resolution, ICMP echo,
and MSI-X SPI 55 enablement all completed before the later failure. Scheduler,
softirq, tracing, timer, SMP, and init also came up.

The last healthy serial window showed heartbeat output through
`uptime_ms=43283`, then init attempted `/bin/bwm`. The failing boot did not
reach confirmed bwm startup, bsshd start, or bounce start markers. The CPU0
regression panic then reported CPU0 `tick_count = 75` while a peer CPU had
advanced to `30000`.

There were no TX queue/full/timeout markers immediately before the panic. The
only explicit TX evidence was the earlier init ARP/ICMP traffic, so the T22
evidence did not prove a TX attempt directly preceded the CPU0 stall. It did,
however, fit the broader hypothesis that the IRQ-safe TX/reclaim lock window
could starve CPU0 after init traffic had established the device path.

## 24B - Lock-free design

The retry keeps the Substep 3 ownership model but removes the shared
IRQ-safe/spin lock entirely:

- PCI and MMIO each use a 16-slot static TX buffer pool.
- Slot ownership is represented by per-slot `AtomicBool` in-flight bits.
- `transmit()` claims a slot with CAS from `false` to `true`.
- The TX avail-ring producer index advances with an atomic `fetch_add`.
- Descriptor and avail-ring writes are followed by a release fence before
  publishing `avail.idx`, then another release fence before notifying the
  device.
- `reclaim_tx_completed()` volatile-reads device `used.idx`, uses an acquire
  fence, walks completed used-ring entries, and releases slots with a release
  store to the matching in-flight bit.
- `process_rx_budgeted()` calls reclaim at the top of the aarch64 NetRx path.

There is no spinlock, no IRQ-safe critical section, and no TX hot-path logging
between `transmit()` and `reclaim_tx_completed()`.

## Build gates

All required build gates passed:

- `./userspace/programs/build.sh --arch aarch64`
- `./scripts/create_ext2_disk.sh --arch aarch64`
- `cargo build --release --target aarch64-breenix.json ... -p kernel --bin kernel-aarch64`
- `cargo build --release --features testing,external_test_bins --bin qemu-uefi`
- `scripts/parallels/build-efi.sh --kernel`

Warning/error greps were empty:

- `turn24-artifacts/build-aarch64-warning-error-grep.txt`: 0 bytes
- `turn24-artifacts/build-x86-warning-error-grep.txt`: 0 bytes

## Single Parallels boot

The single fresh-deploy Parallels boot passed. `run.sh --parallels --test 60`
returned exit 0.

Serial evidence:

- fail-marker scan was empty.
- heartbeat reached `uptime_ms=141393`.
- CPU0 timer markers reached `cpu0 ticks=80000` in serial.
- bwm FPS output and virgl composite frames continued through the window.
- no `TX timeout` or `TX queue full` markers appeared.

Live procfs evidence from the same boot:

- `/proc/stat`: `cpu0 93349 83432`, `net_msi_irqs 36`
- `/proc/trace/counters`: `TIMER_TICK_TOTAL ... cpu0=94290`
- `/proc/trace/counters`: `NET_PCI_IRQ_RAISED_NETRX: 40 (cpu0=40)`
- `/proc/trace/counters`: `NET_RX_BUDGET_EXHAUSTED: 0`

## Verdict

PASS. Lock-free Substep 3 preserves the T21 IRQ delivery path, removes the
original Substep 3 TX busy-wait, and avoids the T22 CPU0 regression. This
strongly implicates the T22 IRQ-safe TX/reclaim lock window as the culprit.

Polling-elimination gate progress: P6/P7 Substep 3 is complete. Remaining
substeps are P6/P7/P8 Substep 4, Substep 5, Substep 6, then P9, P11, P12,
P13, P14, P15, P16, P17, and P18.
