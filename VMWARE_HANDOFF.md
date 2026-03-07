# VMware Fusion Support — Handoff Document

**Branch:** `feat/vmware-support` (branched from `feat/full-gpu-compositing` at `c3744ea`)
**Worktree:** `/Users/wrb/fun/code/breenix-vmware`
**Date:** 2026-03-06

## What's Done

### 1. `run.sh --vmware` flag (COMPLETE)

Full VMware Fusion integration added to `run.sh`, mirroring the `--parallels` pattern:

- Builds UEFI loader + ARM64 kernel + userspace + ext2 disk (same pipeline as Parallels)
- Creates FAT32 ESP image with GPT ESP patch
- Converts raw images to VMDK via `qemu-img convert -f raw -O vmdk`
- Generates `.vmwarevm` bundle with unique timestamped name
- Cleans up old `breenix-*` VMs automatically
- Launches via `vmrun start`, tails serial log

Usage:
```bash
./run.sh --vmware              # Full build + boot
./run.sh --vmware --no-build   # Reuse last build
./run.sh --vmware --clean      # Clean rebuild
```

### 2. VMX Generator (COMPLETE)

`scripts/vmware/generate-vmx.sh` — generates a `.vmx` config file with:
- ARM64 EFI guest (`guestOS = "arm-other-64"`, `firmware = "efi"`)
- NVMe boot disk (boot.vmdk with FAT32 ESP)
- SATA ext2 data disk (optional)
- Serial port output to `/tmp/breenix-vmware-serial.log`
- NAT networking (e1000e)
- XHCI USB
- 4 vCPUs, 2GB RAM, 256MB VRAM

### 3. First Boot Test (DONE — partial success)

The kernel DOES boot on VMware Fusion. Evidence from `vmware.log`:
- `Guest: Firmware has transitioned to runtime` — UEFI loader executed
- vcpu-0 active with valid state: `PC=fbce8774`, `VBAR_EL1=ff3dd000`, `TTBR0_EL1=fffdf000`
- MMU enabled (`SCTLR_EL1=3050198d`), page tables set up
- Screen resized: `Screen 1 Defined: xywh(0, 0, 2048, 1536)`

## What's Broken

### Serial Output — GARBLED (not GPU-related)

The serial log (`/tmp/breenix-vmware-serial.log`) contains non-ASCII binary garbage (bytes in 0x00-0x1b range). The kernel is running but we can't see any output.

**Root cause:** The UEFI loader discovers the UART via ACPI SPCR table. When SPCR isn't found or doesn't match, it falls back to the Parallels PL011 address `0x0211_0000`. VMware Fusion almost certainly has its UART at a different base address.

**Key code path:**
1. `parallels-loader/src/acpi_discovery.rs:187-219` — SPCR parsing, falls back to `0x0211_0000`
2. `kernel/src/platform_config.rs:22` — QEMU default is `0x0900_0000`
3. `kernel/src/serial_aarch64.rs` — PL011 driver (may need 16550 for VMware)

## What Needs To Happen Next

### Priority 1: Fix Serial Output

This is the blocker — without serial we're flying blind.

**Option A: Debug from UEFI loader side**
- The UEFI loader has access to UEFI SimpleTextOutput (screen console) before handing off
- Add SPCR table dump logging in `parallels-loader/src/acpi_discovery.rs` using UEFI output services
- This will reveal what ACPI tables VMware provides and what UART address is in SPCR

**Option B: Dump ACPI tables from VMware**
- Boot a Linux ISO on the same VMware VM config
- Run `cat /sys/firmware/acpi/tables/SPCR | xxd` to see the UART address
- Or `dmesg | grep -i uart` to see what Linux discovers

**Option C: Try known VMware UART addresses**
- VMware ARM64 may use PL011 at `0x0900_0000` (same as QEMU virt)
- Or it might use 16550-compatible UART at a different address
- Could try hardcoding common addresses as a quick test

### Priority 2: Verify UART Type

VMware might not use PL011 at all on ARM64. It could use:
- **16550 UART** (like x86) — different register layout, different init sequence
- **PL011** at a different address — just need the right base
- Check SPCR `interface_type` field: 0x03 = PL011, 0x12 = 16550

### Priority 3: Platform Abstraction

Once serial works, consider:
- Adding a `VMware` variant to platform detection (currently just QEMU vs Parallels)
- VMware's virtio/GPU/network devices may differ from Parallels
- The existing Breenix VM at `~/Virtual Machines.localized/Breenix.vmwarevm/Breenix.vmx` has a known-working VMware config for reference

## File Inventory

### New files
- `scripts/vmware/generate-vmx.sh` — VMX config generator
- `VMWARE_HANDOFF.md` — this document

### Modified files
- `run.sh` — added `--vmware` flag, VMWARE block (lines ~410-610)

### Build artifacts
- `target/vmware/boot.vmdk` — EFI boot disk
- `target/vmware/ext2-data.vmdk` — ext2 data disk
- VM bundles land in `~/Virtual Machines.localized/breenix-*.vmwarevm/`

## VMware Fusion Tooling Reference

| Tool | Path | Purpose |
|------|------|---------|
| vmrun | `/Applications/VMware Fusion.app/Contents/Public/vmrun` | VM lifecycle (start/stop) |
| vmware-vdiskmanager | `/Applications/VMware Fusion.app/Contents/Library/vmware-vdiskmanager` | VMDK creation/conversion |
| vmcli | `/Applications/VMware Fusion.app/Contents/Public/vmcli` | VM configuration |
| EFI ROM | `.../Library/roms/arm64/EFIAARCH64.ROM` | ARM64 EFI firmware |

Disk conversion uses `qemu-img` (already installed at `/opt/homebrew/bin/qemu-img`) rather than vmware-vdiskmanager, since qemu-img handles raw-to-vmdk directly.

## Existing VMware VM (Manual Setup)

There's an existing manually-created Breenix VM at:
`~/Virtual Machines.localized/Breenix.vmwarevm/Breenix.vmx`

This was set up before the automated `--vmware` flow. It has the ext2 image attached as `sata0:1` (cdrom-image pointing at `target/ext2-aarch64.img`). The `.vmx` from that VM was used as reference for `generate-vmx.sh`. The VM is currently suspended with a checkpoint.
