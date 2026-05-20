## Summary

Incremental progress toward interrupt-driven xHCI input. Lands the infrastructure without removing the polling fallback yet — full polling removal requires deferring the SPI activation until after all subsystems init, which will be a follow-up (Phase 5+).

## What's in here

### Phase 1: Linux-probe MSI ground truth (docs only)
- \`docs/planning/f32t-xhci-msi/linux-ground-truth.md\` — evidence from linux-probe that xHCI MSI → GICv2m SPI → xhci_msi_irq works on the same Parallels ARM64 hypervisor, 14-54µs latency

### Phase 2: Linux-order PCI MSI programming (\`pci.rs\`)
Refactor \`Device::configure_msi()\` to match Linux's sequence:
1. Clear MSI Enable bit
2. Mask vector 0 if per-vector masking supported
3. Clear Qsize (single-vector mode)
4. Write Message Address Lo (and Hi=0 for 64-bit)
5. Write Message Data
6. Read back flags (posted-write flush)
7. Disable INTx (now internal to configure_msi)
8. Set MSI Enable
9. Unmask vector 0

Previously Breenix wrote msg/mask then Enable then INTx off — this ordering is the suspected cause of the historical "MSI storm" that led commit 488d2fc2 to defensively set \`XhciState.irq=0\`.

Linux parity:
- \`/tmp/linux-v6.8/drivers/pci/msi/msi.c:359-389\` \`__pci_write_msi_msg\`
- \`/tmp/linux-v6.8/drivers/pci/msi/msi.c:184-204\` \`pci_msi_update_mask\`

Call sites updated (AHCI, xHCI, virtio-gpu, virtio-net) to remove now-redundant external \`disable_intx()\` invocations.

### Phase 4: \`XhciState.irq = early_irq\` (\`usb/xhci.rs\`)
The MSI irq was always allocated but \`state.irq\` was hardcoded to 0, which kept the deferred SPI activation path gated off. Plumb it correctly. No runtime effect yet because SPI activation still only runs inside poll_hid_events.

## What's NOT in here (follow-up)

- Phase 5: delete \`poll_hid_events\` and the 200Hz timer-driven polling
- Phase 6: migrate BWM off \`graphics::mouse_pos()\` / \`poll_modifier_state()\` polling onto waitqueue wake

First attempt at Phase 5 (enabling SPI inline at xhci::init() completion) was reverted — it fires too early, before AHCI/FS/scheduler finish initialization, causing disk reads to stall. Needs a deferred trigger tied to system-ready, not a poll counter.

## Validation

- aarch64 clean build
- User confirmed: desktop boots, cursor works, frame rates normal, CPU utilization as expected (pre-existing 800% from idle gate — separate F32s work)

🤖 Generated with [Claude Code](https://claude.com/claude-code)
