# Turn 16 P6-P18 Polling Survey

Status: COMPLETE

This survey refreshes the remaining Turn 1 inventory entries after the P5 correction. It checks whether each polling site still exists, whether Linux has a clear IRQ/wait-completion equivalent, and whether Breenix already has enough interrupt infrastructure for a turn-sized conversion.

## Summary

| ID | Category | Current source state | Linux shape | Turn 17 suitability |
|---|---|---|---|---|
| P6 | CONVERTIBLE, large | Live timer/commented timer softirq dependence and synchronous ARP/ICMP `process_rx()` loops still exist in `kernel/src/net/mod.rs`; VirtIO PCI suppresses MSI-X until timer softirq re-enables it. | VirtIO net interrupts schedule NAPI work from the virtqueue interrupt path. | Real target, but too broad for the next small turn because init probing, IRQ suppression, and ARP/ICMP completion all interact. |
| P7 | CONVERTIBLE | Live TX used-ring busy waits remain in `virtio/net_pci.rs` and `virtio/net_mmio.rs`. | Linux reclaims TX completions from NAPI/virtqueue callbacks. | Best bundled with P6 so RX and TX re-enable/callback rules are not split across turns. |
| P8 | CONVERTIBLE after P6 | x86 test idle loops still call `net::process_rx()` after `hlt`. | e1000 IRQ schedules NAPI cleanup. | Follow-up after the normal network path is IRQ-driven. |
| P9 | INFRASTRUCTURE | VirtIO GPU MMIO command path still spins on control queue `used.idx`. | Linux virtgpu uses virtqueue callbacks and dequeue work. | Needs MMIO GPU IRQ/completion wiring or support-surface removal; not the cleanest next target. |
| P10 | CONVERTIBLE | GPU PCI freeze watchdog kthread still wakes periodically and samples GPU/scheduler/process/timer state. GPU PCI already has MSI-X completion via `handle_interrupt()` and `GPU_COMPLETION`. | Linux virtgpu completions are callback/work driven; diagnostics are on-demand through DRM/debug infrastructure. | Chosen next target. Periodic polling can be deleted or relocated to on-demand diagnostics without adding new IRQ infrastructure. |
| P11 | ALLOWLIST / CONVERTIBLE | VirtIO reset status spins remain in common MMIO/PCI transport and some drivers. | Linux centralizes reset through transport config ops. | Needs a transport-level policy decision: bounded hardware handshake allowlist vs sleep-backed helper. |
| P12 | INFRASTRUCTURE / CONVERTIBLE | AHCI now has scheduler-backed command completions for normal operation, but early boot still polls `PORT_CI`; engine handshakes and platform IRQ probe spins remain. | Linux uses IRQ completion for runtime commands and bounded register waits for controller handshakes. | Larger follow-up because early scheduler readiness and boot storage ordering are involved. |
| P13 | INFRASTRUCTURE | e1000 EEPROM/reset/TX completion spins remain; RX is also used by polling loops through the net stack. | Linux e1000 IRQ schedules NAPI cleanup and TX reclaim. | Should follow P6/P7 or be scoped to legacy x86/VMware support. |
| P14 | INFRASTRUCTURE | SVGA command-buffer status and `SVGA_REG_BUSY` spins remain. | Linux vmwgfx uses fence-based waits. | Needs fence/completion infrastructure for SVGA; not a small cleanup. |
| P15 | ALLOWLIST | PCI PM D3hot-to-D0 fixed spin delay remains. | Linux sleeps for PM readiness delays. | Not device-event polling. Convert to scheduler delay where possible or allowlist as boot-only hardware settle. |
| P16 | ALLOWLIST | GIC redistributor wake spin remains in a Tier 2 file. | Linux uses bounded atomic poll timeout for GICR_WAKER. | Hardware handshake; document/allowlist rather than convert in this Ralph. |
| P17 | INFRASTRUCTURE | Boot CPU still spins for secondary CPUs in `main_aarch64.rs`; this file is explicitly out of scope for Turn 16. | Linux CPU bring-up is state/event driven. | Requires boot CPU completion/event design and touches a hard-constrained file. |
| P18 | INFRASTRUCTURE | `Completion::wait_timeout()` still has early-boot spin fallbacks when no scheduler thread exists. | Linux completions block on scheduler wait queues. | Cross-cutting early boot architecture issue; should be handled after AHCI/storage ordering is decided. |

## Detail Notes

### P6 - Network RX and init polling

Live source still exists:

- `kernel/src/net/mod.rs` initializes ARP, sends gateway ARP, and loops on `process_rx()` with spin delays.
- `kernel/src/net/mod.rs` performs on-demand ARP resolution by polling `process_rx()`.
- `kernel/src/drivers/virtio/net_pci.rs` documents that MSI-X stays suppressed until timer softirq re-enables it.
- `kernel/src/drivers/virtio/net_mmio.rs` has a real IRQ handler that raises `NetRx`, but its enable path is after synchronous init.

Linux equivalent is clear: virtqueue interrupt dispatch schedules bounded network work, and virtio-net/e1000 use NAPI-style cleanup. Breenix has partial infrastructure, but the current init loop and MSI-X suppression rules make this more than a one-file conversion.

Category: CONVERTIBLE, large.

### P7 - VirtIO net TX completion

Live TX completion spins remain:

- `kernel/src/drivers/virtio/net_pci.rs` waits for `PCI_TX_QUEUE.used.idx`.
- `kernel/src/drivers/virtio/net_mmio.rs` waits for `TX_QUEUE.used.idx`.

Linux reclaims TX completions in the same event-driven network cleanup model as RX. Breenix should not convert TX independently before deciding the network softirq/IRQ re-enable contract.

Category: CONVERTIBLE, bundled with P6.

### P8 - x86 test RX drains

The x86 test loops in `kernel/src/main.rs` still call `net::process_rx()` after `hlt`. They should be removed or converted after the normal network path is event-driven, otherwise tests will keep a separate polling path alive.

Category: CONVERTIBLE after P6/P7.

### P9 - VirtIO GPU MMIO control queue

`kernel/src/drivers/virtio/gpu_mmio.rs` still spins on `CTRL_QUEUE.used.idx` after command submission. Linux virtgpu uses callbacks and dequeue work for control/cursor queues. Breenix has a PCI GPU completion model, but the MMIO GPU path needs equivalent IRQ plumbing or a support-surface decision.

Category: INFRASTRUCTURE.

### P10 - GPU PCI freeze watchdog

The freeze watchdog is live:

- `kernel/src/drivers/virtio/gpu_pci.rs` starts a `freeze-watch` kthread.
- The thread sleeps on a timer, wakes periodically, samples GPU/scheduler/process/timer state, and prints serial diagnostics.
- `kernel/src/main_aarch64.rs` starts it after GPU initialization.

This is separate from GPU command completion. The same file already has GPU MSI-X completion state (`GPU_COMPLETED_USED_IDX`, `GPU_COMPLETION`) and `handle_interrupt()` completes waiters from the virtqueue used index.

Linux virtgpu uses virtqueue callbacks plus workqueues for completions and exposes diagnostics through DRM/debug-style interfaces rather than a periodic driver watchdog.

Category: CONVERTIBLE. Chosen next target.

### P11 - VirtIO reset status waits

Reset spins remain in generic VirtIO MMIO/PCI paths and at least one driver-specific path. The wait is a hardware status handshake rather than event completion. Linux routes reset through transport config ops; whether Breenix keeps a bounded atomic wait or moves to a sleep-backed helper should be decided centrally.

Category: ALLOWLIST / CONVERTIBLE.

### P12 - AHCI early boot and controller handshakes

The normal AHCI command path has completion support, but early boot still polls `PORT_CI` when the scheduler is not ready. AHCI also retains controller engine and readiness handshakes plus a platform IRQ probe that polls command completion while preserving interrupt status.

Linux has both sides: IRQ-driven command completion for runtime I/O and bounded register waits for controller state changes. Breenix needs an early boot storage ordering decision before this becomes a clean conversion.

Category: INFRASTRUCTURE / CONVERTIBLE.

### P13 - e1000

e1000 still has EEPROM, reset, and TX descriptor polling, and its RX path is reached by net-stack polling loops. Linux e1000 uses interrupt/NAPI cleanup for RX/TX while retaining bounded hardware handshakes for EEPROM/reset-style operations.

Category: INFRASTRUCTURE.

### P14 - VMware SVGA

SVGA command buffer submission spins on device-written status and `sync()` spins on `SVGA_REG_BUSY`. Linux vmwgfx waits on fences. Breenix would need SVGA fence/completion support before this is a direct conversion.

Category: INFRASTRUCTURE.

### P15 - PCI PM delay

The PCI PM path still uses a fixed spin delay after D3hot-to-D0. This is not event polling; it is a hardware settle delay. Linux sleeps for readiness delays when it can. Breenix can replace this with a scheduler/timer delay when available, or document it as boot-only hardware settle.

Category: ALLOWLIST.

### P16 - GIC redistributor wake

The GICR_WAKER wait is still present in `kernel/src/arch_impl/aarch64/gic.rs`. Linux uses a bounded atomic poll timeout for this CPU-management handshake. This is a Tier 2 file and not a driver polling loop.

Category: ALLOWLIST.

### P17 - SMP secondary CPU wait

The boot CPU still spins waiting for launched secondary CPUs. This touches `kernel/src/main_aarch64.rs`, which Turn 16 is explicitly forbidden to modify. The long-term shape is a CPU bring-up completion/event with timeout.

Category: INFRASTRUCTURE.

### P18 - Generic completion early boot fallback

`Completion::wait_timeout()` still has early-boot spin/yield fallback paths for contexts without scheduler support. This is a cross-cutting fallback that exists to support early I/O paths such as AHCI. It should be removed only after early boot device waits have a scheduler-ready checkpoint or explicit asynchronous flow.

Category: INFRASTRUCTURE.

## Next Target

P10 is the highest-confidence next target because it does not need new IRQ delivery. GPU PCI command completion is already interrupt-driven, and the remaining polling behavior is periodic diagnostic sampling. Turn 17 should remove the periodic watchdog or relocate the useful counters to an on-demand proc/debug surface, then validate with the same single fresh-deploy boot gate.
