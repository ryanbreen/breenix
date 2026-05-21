# Turn 36: gpu_mmio reachability

## Definitive answer

Parallels does not reach `kernel/src/drivers/virtio/gpu_mmio.rs` through the
normal aarch64 boot path. Parallels arrives with a non-zero PCI ECAM base from
the UEFI hardware config, so `drivers::init()` takes the PCI branch and
initializes `virtio::gpu_pci::init()` instead of `init_virtio_mmio()`
(`kernel/src/platform_config.rs:562-589`,
`kernel/src/drivers/mod.rs:156-213`). T35's boot log confirms that live path:
`[virtio-gpu-pci] MSI-X enabled: config_spi=53 queue_spi=54` and
`[drivers] VirtIO GPU (PCI) initialized`
(`turn35-artifacts/boot-1-gpu-path-lines.txt:5-20`).

The live target for `gpu_mmio.rs` is ARM64 QEMU `virt` with
`virtio-gpu-device` / VirtIO MMIO devices. The normal `run.sh` ARM64 QEMU path
uses `-M virt` and `-device virtio-gpu-device`
(`run.sh:925-934`, `run.sh:986-1001`). The Docker ARM64 test scripts do the
same (`docker/qemu/run-aarch64-test.sh:34-56`,
`docker/qemu/run-aarch64-userspace.sh:67-91`), and
`scripts/run-arm64-graphics.sh` explicitly says to use `virtio-*-device`
instead of `virtio-*-pci` for MMIO devices
(`scripts/run-arm64-graphics.sh:82-100`).

## Module and direct init callsites

`gpu_mmio` is compiled only for aarch64:

- `kernel/src/drivers/virtio/mod.rs:26-31` declares
  `pub mod gpu_mmio;` and `pub mod gpu_pci;` under `#[cfg(target_arch =
  "aarch64")]`.

Direct initialization callsites:

- `kernel/src/drivers/mod.rs:310-365`: `init_virtio_mmio()` enumerates VirtIO
  MMIO devices, then calls `virtio::gpu_mmio::init()` and
  `virtio::gpu_mmio::test_device()`.
- `kernel/src/main_aarch64.rs:1608-1614`: `init_graphics()` calls
  `gpu_mmio::init()` only if `gpu_pci::is_initialized()` is false.
- `kernel/src/main_aarch64.rs:547-550`: QEMU-only boot code calls
  `gpu_mmio::load_resolution_from_fw_cfg()` before driver init.

Other public entry points are fallback users after a backend has been chosen:

- `kernel/src/graphics/arm64_fb.rs:58-67`,
  `kernel/src/graphics/arm64_fb.rs:105-112`,
  `kernel/src/graphics/arm64_fb.rs:145-149`,
  `kernel/src/graphics/arm64_fb.rs:390-434`, and
  `kernel/src/graphics/arm64_fb.rs:476-542` fall back from PCI/GOP to MMIO for
  dimensions, flushes, and framebuffer access.
- `kernel/src/syscall/graphics.rs:2577-2585` falls back to
  `gpu_mmio::dimensions()` when framebuffer cache dimensions are unavailable.
- `kernel/src/drivers/usb/hid.rs:310-314` clamps mouse coordinates using PCI
  GPU dimensions, then MMIO GPU dimensions.
- `kernel/src/drivers/virtio/input_mmio.rs:198-201` uses
  `gpu_mmio::dimensions()` for the MMIO tablet path, defaulting to 1280x800.

## Boot-time decision points

`drivers::init()` documents the platform split: PCI ECAM means Parallels/UEFI
PCI enumeration; otherwise QEMU `virt` enumerates VirtIO MMIO devices
(`kernel/src/drivers/mod.rs:90-95`). It reads `pci_ecam_base()` and
`is_qemu()` (`kernel/src/drivers/mod.rs:101-103`).

There are three cases:

1. QEMU hybrid with PCI ECAM: if `ecam_base != 0 && is_qemu`, the kernel first
   calls `init_virtio_mmio()` for VirtIO MMIO GPU/keyboard/net, then enumerates
   PCI for AHCI (`kernel/src/drivers/mod.rs:105-153`).
2. Parallels/UEFI PCI: if `ecam_base != 0` and not QEMU, the kernel enumerates
   PCI and calls `virtio::gpu_pci::init()`; it does not call
   `init_virtio_mmio()` (`kernel/src/drivers/mod.rs:156-213`).
3. QEMU MMIO-only: if `ecam_base == 0`, the kernel calls
   `init_virtio_mmio()` (`kernel/src/drivers/mod.rs:304-306`), which includes
   the MMIO GPU init call (`kernel/src/drivers/mod.rs:354-365`).

`main_aarch64` has a second graphics fallback with the same intent. It tries
VirGL PCI, GPU PCI+GOP, GOP framebuffer, and only then QEMU MMIO graphics
(`kernel/src/main_aarch64.rs:645-728`). Inside that fallback,
`init_graphics()` still skips MMIO GPU init if PCI GPU is already initialized
(`kernel/src/main_aarch64.rs:1608-1614`).

QEMU detection is explicit: `is_qemu()` checks the QEMU UART base
(`kernel/src/platform_config.rs:362-368`), and `is_parallels()` is defined as
not QEMU with Parallels-style RAM/GIC redistributors
(`kernel/src/platform_config.rs:378-385`). Parallels stores PCI ECAM from the
UEFI hardware config when present (`kernel/src/platform_config.rs:562-589`).

## Historic log evidence

T35's Parallels serial log has only PCI GPU markers:

- `[virtio-gpu-pci] MSI-X enabled: config_spi=53 queue_spi=54`
  (`turn35-artifacts/boot-1-gpu-path-lines.txt:5-6`).
- `[drivers] VirtIO GPU (PCI) initialized`
  (`turn35-artifacts/boot-1-gpu-path-lines.txt:20`).

There are no T35 runtime MMIO GPU IRQ evidence lines:
`turn35-artifacts/boot-1-gpu-irq-evidence.txt` is empty, and the source diff's
new `[virtio-gpu] MMIO IRQ ... enabled` / counter logs
(`turn35-artifacts/source-diff.txt:78-103`) do not appear in the T35 boot path.

## x86 versus aarch64

The `gpu_mmio` module is behind `#[cfg(target_arch = "aarch64")]`
(`kernel/src/drivers/virtio/mod.rs:26-31`). The same module comment describes
x86_64 as legacy I/O-port VirtIO and ARM64 as MMIO VirtIO
(`kernel/src/drivers/virtio/mod.rs:3-19`). The `run.sh` display path also
separates them: ARM64 uses `virtio-gpu-device`, while x86_64 uses `virtio-vga`
or no display in headless mode (`run.sh:925-941`).

Conclusion: `gpu_mmio.rs` is live on supported ARM64 QEMU `virt` targets. It is
not live on the Parallels gold-master boot used in T35, and x86 does not use it.
