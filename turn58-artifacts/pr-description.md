# Polling Elimination - Linux-rigor gate (Phase 1: ALLOWLIST + SHIPPED)

## Summary

This PR ships the polling-elimination work from a 57-turn campaign to bring Breenix's IO and CPU-management paths to Linux-level rigor around busy-wait polling.

Phase 1 covers all P-targets where Linux precedent supports the chosen approach:

- 7 SHIPPED IRQ-driven conversion groups: `P1`, `P2`, `P3`, `P4`, `P5/P5b`, `P10`, and bundled `P6/P7/P8`
- 10 ALLOWLIST formalizations: bounded hardware handshakes or pre-scheduler fallbacks with direct Linux precedent, each carrying a `docs/polling-allowlist.md` entry plus inline comment
- 1 BLOCKED item: `P9` VirtIO GPU MMIO IRQ completion, isolated to Parallels-aarch64 codegen sensitivity; production gate is met because Parallels uses PCI GPU and never reaches the MMIO GPU path

Four INFRASTRUCTURE items are explicitly deferred to follow-up work: `P12` Sites 1+6, `P13`, and `P14`. These are platform-driver or IRQ-discovery infrastructure questions, not unresolved production hot-path polling patterns.

## Scope

### SHIPPED IRQ-driven conversions

| Target | Commit(s) | Summary | Linux precedent |
|---|---|---|---|
| `P1` | `18c88a01`, `cb73f6e3` | xHCI timer/HID polling removed from CPU0 timer path; observability moved to `/proc/xhci/counters`. | Interrupt-driven xHCI event handling |
| `P2` | `cb73f6e3` | xHCI command/event waits converted to IRQ-side completion. | xHCI command/event ring completion via MSI/IRQ |
| `P3` | `cb73f6e3` | xHCI endpoint recovery moved out of timer polling into IRQ/workqueue-shaped recovery. | Linux xHCI event handling plus deferred recovery |
| `P4` | `cb73f6e3` | xHCI helper spins resolved as bounded hardware handshakes instead of event polling. | Bounded controller register/delay handshakes |
| `P5/P5b` | `1b4a2dfc` | Dormant VirtIO PCI input polling deleted; live VirtIO MMIO input confirmed IRQ-driven. | VirtIO input IRQ delivery |
| `P10` | `747ce88c` | VirtIO GPU PCI freeze watchdog kthread removed; diagnostics are on-demand. | VirtIO GPU callbacks/workqueues, debug surfaces |
| `P6/P7/P8` | `0d024438`, `acb62bab`, `a093f9f0`, `c3397f82`, `98949e98`, `412b2c1a` | Network RX/TX and x86 hlt-loop polling converted to IRQ/softirq/NAPI-shaped flow. | VirtIO net IRQ schedules bounded NAPI-style work |

### ALLOWLIST formalizations

Canonical document: `docs/polling-allowlist.md`.

| # | Target | Site | File / line | Turn | Commit | Linux precedent |
|---|---|---|---|---|---|---|
| 1 | `P15` | PCI PM D3hot to D0 settle delay | `kernel/src/drivers/pci.rs` | T48 | `5c882371` | PCI PM D3hot delay |
| 2 | `P11` | VirtIO reset status handshake | `kernel/src/drivers/virtio/mod.rs` | T49 | `8a6515a4` | VirtIO transport reset/status handshake |
| 3 | `P16` | GICR_WAKER ChildrenAsleep handshake | `kernel/src/arch_impl/aarch64/gic.rs` | T50 | `d10bb99f` | GICv3 redistributor wait |
| 4 | `P18` | Completion early-boot fallback | `kernel/src/task/completion.rs` | T51 | `086b7164` | Pre-scheduler bounded busy-wait |
| 5 | `P17` | SMP secondary CPU online wait | `kernel/src/main_aarch64.rs` | T52 | `fb66d0dc` | Linux `__cpu_up()` completion wait |
| 6 | `P12` Site 3 | AHCI command engine `CR`/`FR` handshakes | `kernel/src/drivers/ahci/mod.rs:1075-1106` | T54 | `65051f93` | `ahci_stop_engine()` + `ata_wait_register()` |
| 7 | `P12` Site 4 | AHCI `wait_ready()` taskfile `BSY`/`DRQ` | `kernel/src/drivers/ahci/mod.rs:1119-1128` | T54 | `65051f93` | `ata_wait_after_reset()` / `ata_wait_ready()` |
| 8 | `P12` Site 5 | AHCI platform IRQ probe port-ready wait | `kernel/src/drivers/ahci/mod.rs:2125-2132` | T54 | `65051f93` | `ata_wait_ready()` |
| 9 | `P12` Site 2 | AHCI early-boot `PORT_CI` fallback | `kernel/src/drivers/ahci/mod.rs:815-853` | T55 | `397dd3f1` | `ahci_port_intr()` at runtime + pre-scheduler bounded primitive |
| 10 | `P12` Site 7 | AHCI ISR `PORT_IS`/`PORT_CI` drain | `kernel/src/drivers/ahci/mod.rs:2413-2466` | T56 | `f78f81fc` | `ahci_port_intr()` + `ahci_handle_port_interrupt()` |

Each allowlisted site has:

- an entry in `docs/polling-allowlist.md`
- an inline source comment cross-referencing the allowlist
- a Linux file/function precedent
- a boundedness argument
- single Parallels boot evidence from its turn

### `P9` BLOCKED - production gate met via dead-code mitigation

`P9` targeted VirtIO GPU MMIO interrupt-driven completion. The Linux profile is clear: virtgpu control queue completion is callback/work driven (`virtio_gpu_ctrl_ack()` schedules dequeue work, then waiters/fences complete).

The Breenix implementation was structurally correct, but a 12-turn bisect campaign isolated a Parallels-aarch64 CPU0 starvation trigger to a single call instruction in the init-time MMIO GPU path:

- T34 (`6a720880`): Linux profile scout for VirtIO GPU MMIO control queue
- T35-T46: single-variable bisect campaign
- T46 (`338dc76`): true endpoint - the call instruction itself is the trigger
- T47 (`9d715676`): relocating the call one frame outward also trips CPU0

Production mitigation: Parallels/aarch64 uses VirtIO GPU PCI, not MMIO GPU, so the MMIO path is dead on the production target. PCI GPU completion already follows the IRQ/completion shape.

## Out of Scope: Phase 2 INFRASTRUCTURE

The retrospective (`turn57-artifacts/polling-elimination-retrospective.md`) tracks four deferred infrastructure items:

1. `P12` Site 1 - slot-0 software port I/O ownership wait
   - software mutex contention, not hardware/event polling
   - likely treatment: scheduler-aware ownership handoff, blocking mutex, or per-port command queue

2. `P12` Site 6 - platform IRQ probe `PORT_CI` completion poll
   - active command/probe used as IRQ resource discovery workaround
   - likely treatment: DTB/ACPI/platform IRQ resource discovery infrastructure

3. `P13` - e1000 polling
   - legacy x86 network driver
   - not exercised on Parallels, which uses VirtIO net
   - support decision needed: IRQ-convert, feature-gate, or remove/out-of-scope

4. `P14` - SVGA polling
   - VMware-specific graphics driver
   - not exercised on Parallels, which uses VirtIO GPU
   - support decision needed: fence/completion work, feature-gate, or remove/out-of-scope

## Production Validation

Validation discipline:

- single boot per turn for gate-relevant changes
- first-failure-abort: any boot failure stops the turn and prevents commit
- Parallels target for production evidence
- CPU0 regression guard: panic if CPU0 tick count is less than 10% of max peer at the 30s alarm window
- AHCI/ext2 sanity: ext2 root mounts via AHCI, so AHCI command-completion regressions break userspace boot
- network sanity: T33 markers and live ping with 0% packet loss

Last boot evidence, T56 (`f78f81fc`):

- `uptime_ms=152281`
- CPU0 ticks `95000`
- CPU0 regression scan `0` bytes
- ping `1/1`, `0.0% packet loss`
- `[ext2] Found ext2 superblock on AHCI device 1`
- userspace booted through AHCI ISR-driven completion path

## Linux Precedent Index

ALLOWLIST decisions cite direct Linux-equivalent classes:

- `drivers/ata/libahci.c::ahci_stop_engine()`
- `ata_wait_register()`
- `drivers/ata/libata-core.c::ata_wait_after_reset()`
- `ata_wait_ready()`
- `drivers/ata/libahci.c::ahci_port_intr()`
- `ahci_handle_port_interrupt()`
- GICv3 redistributor `GICR_WAKER.ChildrenAsleep` wait
- PCI PM D3hot-to-D0 settle delay
- VirtIO reset status handshake
- pre-scheduler bounded busy-wait primitives equivalent to `mdelay()` / `udelay()` contexts

## Methodology

- Linux-profile-first: every conversion or ALLOWLIST decision references Linux behavior before implementation.
- First-failure-abort: no repeated boot loops after a failure; a failure blocks the turn.
- Single-P-target-per-turn discipline, with one justified batch in T54 for three same-file, same-precedent AHCI handshakes.
- No fake passing tests: failures were either fixed, reverted, or documented as INCONCLUSIVE/BLOCKED.
- No prohibited hot-path logging was added to Tier-1 interrupt/syscall paths.

## Test Plan

- [ ] CI passes.
- [ ] Parallels boot reaches `uptime_ms >= 60000`.
- [ ] CPU0 ticks reach `>= 35000`.
- [ ] CPU0 regression scan is `0` bytes.
- [ ] T33 network markers are present.
- [ ] Live ping to `10.211.55.100` is `1/1`, `0% packet loss`.
- [ ] AHCI/ext2 sanity passes: ext2 root mounts from AHCI.
- [ ] `docs/polling-allowlist.md` review confirms all 10 entries.
- [ ] Inline comments at each ALLOWLIST site are present and minimal enough for their context.
- [ ] Verify no Tier-1 prohibited files or gold-master regions were modified in this PR.

## Risk

Low for Phase 1:

- ALLOWLIST changes are documentation/comment-only, with no runtime behavior change.
- SHIPPED conversions were validated by Parallels boot evidence and, where relevant, ping/AHCI/ext2 sanity.
- The remaining risk is scope acceptance: Phase 2 infrastructure items are explicitly deferred rather than hidden.

Acknowledged deferral:

- `P12` Site 6 is the most production-relevant deferred item because it is a bounded boot-only platform IRQ discovery workaround. Linux would normally avoid this through ACPI/DTB/platform resources.

## References

- Retrospective: `turn57-artifacts/polling-elimination-retrospective.md` (`3ab1d379`)
- Allowlist: `docs/polling-allowlist.md`
- P12 AHCI classification: `turn53-artifacts/p12-ahci-classification.md`
- P9 Linux profile: `turn34-validation.md`
- P9 bisect endpoint: `338dc76`, `9d715676`
- Commit list artifact: `turn58-artifacts/commits-since-main.txt`

## Commits in This Branch Since `main`

The generated full list is committed at `turn58-artifacts/commits-since-main.txt` and contains 55 commits. High-signal commits for review:

```text
3ab1d379 docs(polling): turn 57 polling-elimination gate RETROSPECTIVE
f78f81fc docs(polling): P12 AHCI Site 7 ISR PORT_IS/PORT_CI drain loop ALLOWLISTED
397dd3f1 docs(polling): P12 AHCI Site 2 early-boot PORT_CI fallback ALLOWLISTED (P18 analog)
65051f93 docs(polling): P12 AHCI engine + taskfile bounded register handshakes ALLOWLISTED (Sites 3, 4, 5)
1e7f5ed1 docs(polling): turn 53 P12 AHCI polling sites SURVEY
fb66d0dc docs(polling): P17 SMP secondary CPU online wait ALLOWLISTED
086b7164 docs(polling): P18 Completion::wait_timeout() early-boot fallback ALLOWLISTED
d10bb99f docs(polling): P16 GICR_WAKER ChildrenAsleep handshake ALLOWLISTED
8a6515a4 docs(polling): P11 VirtIO reset status handshake ALLOWLISTED
5c882371 docs(polling): P15 PCI PM D3hot->D0 settle delay ALLOWLISTED
9d715676 docs(polling): turn 47 P9 implementation step 1 - call-site relocation also trips CPU0
338dc76b docs(polling): turn 46 P9 bisect TRUE ENDPOINT - call instruction IS the T38 CPU0 trigger
6a720880 docs(polling): turn 34 P9 Linux-profile scout - VirtIO GPU MMIO control queue
412b2c1a fix: P6/P7/P8 Substep 6 - remove x86 hlt-loop polling + harden Substep 4 bootstrap
98949e98 fix(arm64): P6/P7/P8 Substep 5 - remove synchronous on-demand ARP polling
c3397f82 fix(arm64): P6/P7/P8 Substep 4 - pre-prime NetRx softirq for callback re-enable
a093f9f0 fix(arm64): P6/P7 Substep 3 lock-free - async TX completion via atomics
acb62bab fix(arm64): P6 Substep 2 - PCI MSI schedules NetRx
0d024438 fix(polling): make NetRx processing budgeted
747ce88c fix(arm64): remove P10 GPU PCI freeze watchdog kthread
1b4a2dfc docs(polling): P5a dormant input_pci cleanup + Turn 1 inventory refresh + P6-P18 survey
cb73f6e3 fix(arm64): close scheduler dequeue race and xHCI IRQ completion
18c88a01 kernel/userspace: CPU0 liveness fix + /proc/xhci/counters
361c5a6c docs: comprehensive polling inventory + Linux comparison for elimination Ralph
```

## Operator Decision Needed

This is a draft for Option C from the retrospective. No PR should be opened until the operator explicitly chooses Option A or Option C and gives a PR creation green-light.
