# Turn 2 Implementation: virtio-gpu Interrupt-Driven Completion

## Status

INCONCLUSIVE.

The Linux-shaped MSI-X routing is implemented, the old normal command-completion polling loops are deleted, and both x86_64 and aarch64 release builds are clean. Parallels boots past userspace and BWM renders for extended periods without command timeout or CPU0 regression panic in the best run, but `stuck_tid=13` still appears in long windows. The transport is no longer pure polling, but the overall goal is not complete.

## A. MSI-X Layout

`kernel/src/drivers/virtio/gpu_pci.rs` now requires MSI-X and rejects silent polling fallback:

```rust
const GPU_MSIX_CONFIG_VECTOR: u16 = 0;
const GPU_MSIX_QUEUE_VECTOR: u16 = 1;

pci_dev.configure_msix_entry(msix_cap, GPU_MSIX_CONFIG_VECTOR, msi_address, config_spi);
pci_dev.configure_msix_entry(msix_cap, GPU_MSIX_QUEUE_VECTOR, msi_address, queue_spi);
```

The driver writes and verifies:

```rust
let readback = virtio.set_config_msix_vector(config_vector);
if readback != config_vector {
    return Err("GPU config MSI-X vector rejected");
}

let q0_readback = virtio.set_queue_msix_vector(queue_vector);
let q1_readback = virtio.set_queue_msix_vector(queue_vector);
```

Parallels evidence from `parallels_boot_70s_pass4.serial.log`:

```text
[virtio-gpu-pci] MSI-X enabled: config_spi=53 queue_spi=54 doorbell=0x2250040 vectors=2
[virtio-gpu-pci] config_msix_vector: wrote=0x0 readback=0x0
[virtio-gpu-pci] Queue 0 msix_vector: wrote=0x1 readback=0x1
[virtio-gpu-pci] Queue 1 msix_vector: wrote=0x1 readback=0x1
[virtio-gpu-pci] MSI-X active: config_spi=53 queue_spi=54 queue_vector=1
```

## B. ISR Handler

`kernel/src/arch_impl/aarch64/exception.rs` dispatches both GPU MSI-X SPIs:

```rust
if irq_id == gpu_config_irq {
    crate::drivers::virtio::gpu_pci::handle_config_interrupt();
}
if irq_id == gpu_irq {
    crate::drivers::virtio::gpu_pci::handle_interrupt();
}
```

The queue ISR reads `used.idx`, publishes it, and completes the waiter:

```rust
let used_idx = virtgpu_trace_used_idx();
let previous = GPU_COMPLETED_USED_IDX.load(Ordering::Acquire) as u16;
if used_idx != previous {
    GPU_COMPLETED_USED_IDX.store(used_idx as u32, Ordering::Release);
    GPU_COMPLETION.complete(ctrlq_completion_token(used_idx));
}
```

## C. Polling Removal

Diff stats for the implementation:

```text
kernel/src/arch_impl/aarch64/exception.rs |   5 +
kernel/src/drivers/virtio/gpu_pci.rs      | 967 +++++++++++++-----------------
2 files changed, 434 insertions(+), 538 deletions(-)
```

The 2-desc and 3-desc normal completion paths now call `wait_for_ctrlq_completion()`, which waits on `GPU_COMPLETION.wait_timeout()` instead of looping on `used.idx`. `VRING_AVAIL_F_NO_INTERRUPT` is not set in the normal command path; `avail.flags` is explicitly cleared before notify.

The old `virgl_fence_sync()` repeated NOP/WFI loop was also removed. It is now a one-shot interrupt-completed NOP verification command that returns an error if it cannot prove the requested fence.

## D. Boot Evidence

Builds run after restoring the normal 5s completion timeout:

```text
cargo build --release --features testing,external_test_bins --bin qemu-uefi
cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64
```

Both completed with zero warnings.

Best Parallels run: `turn2-artifacts/parallels/parallels_boot_70s_pass4.serial.log`.

Positive evidence:

```text
[init] Boot script completed
[freeze-watch] uptime_ms=80440 submits=14890 completes=14892 fails=0 last_completion_ms=80439 fps_last_5s=175 ... gpu_pci_lock=ok
[bwm-fps] frames_since_last=178 elapsed_ms=1001 instantaneous_fps=177
```

Negative evidence:

```text
[SCHED] queue_empty stuck_tid=13 count=1000
[SCHED] queue_empty stuck_tid=13 count=151000
[freeze-watch] uptime_ms=50419 submits=8136 completes=8137 fails=0 last_completion_ms=19352 fps_last_5s=0 ... gpu_pci_lock=busy
```

No `GPU PCI command completion timeout`, `fence sync`, `KERNEL PANIC`, or `CPU0 REGRESSION` lines appeared in the pass4 search. However, the `stuck_tid=13` windows show the broader scheduler/compositor symptom is not solved.

## E. Honesty Test

I temporarily changed `GPU_COMPLETION_TIMEOUT_NS` from 5s to 1h and ran:

```text
./run.sh --parallels --test 70
```

Artifact: `turn2-artifacts/parallels/parallels_boot_70s_honesty_timeout_disabled.serial.log`.

Observed result:

```text
[init] Boot script completed
[freeze-watch] uptime_ms=2254 submits=413 completes=416 fails=0 last_completion_ms=2253 fps_last_5s=189 ... gpu_pci_lock=ok
[freeze-watch] uptime_ms=65362 submits=571 completes=573 fails=0 last_completion_ms=2559 fps_last_5s=0 ... gpu_pci_lock=busy
```

No timeout/panic occurred, but BWM did not remain healthy. This means the command-completion timeout was not the thing making the boot appear to work, but the IRQ-driven path still has a later wedge.

The production code was restored to the normal 5s timeout after this run.

## F. Open Issues / Proposed Turn 3 Scope

The next turn should focus on the post-IRQ residual: why BWM can sit in `stuck_tid=13` with `gpu_pci_lock=busy` even after thousands of successful interrupt completions. The highest-signal next probe is to use existing trace records or GDB to identify the exact PC/state of tid 13 during a `gpu_pci_lock=busy` window without adding serial logging to hot paths. Likely suspects are remaining GPU lock ownership across a scheduler wait, a missed completion wake after a specific command type, or scheduler wake/requeue behavior around `Completion::wait_timeout()`.

