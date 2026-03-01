# xHCI Head-to-Head: Linux Module vs Breenix Driver

## Status Summary

| | Linux Module | Breenix Driver |
|---|---|---|
| **Location** | `linux_xhci_module/breenix_xhci_probe.c` | `kernel/src/drivers/usb/xhci.rs` |
| **Platform** | linux-probe VM (Alpine ARM64) | breenix-dev VM (Breenix ARM64) |
| **M1-M10** | All PASS | All PASS |
| **M11 (Event Delivery)** | PASS — HID reports received via MSI | FAIL — CC=12, all endpoints Halted |
| **Proven 100%?** | Yes, with and without xhci_hcd priming | No |

## Linux Module: Proven Working

The Linux module has been validated in two configurations:

1. **With xhci_hcd priming** — stock Linux xhci_hcd loads first, our module replaces it
2. **Without xhci_hcd priming** — stock xhci_hcd unbound before our module loads

Both produce identical results: all 11 milestones pass, HID interrupt events arrive via MSI handler, keyboard/mouse reports are printed to dmesg.

## Instrumentation Parity Gaps

The Linux module dumps full device contexts (192-256 byte hex dumps) at every configuration step. The Breenix driver only logs completion codes. **This is the core diagnostic gap.**

### Critical Missing Dumps in Breenix

| Milestone | Linux Dumps | Breenix Dumps | Gap |
|---|---|---|---|
| M2 (Reset) | USBSTS + all registers (`ms_regs`) | USBSTS only | Missing register snapshot |
| M3 (Data Structures) | Ring/ERST + all registers | Ring/ERST only | Missing register snapshot |
| M4 (Running) | USBCMD/STS/IMAN + all registers | USBCMD/STS/IMAN only | Missing register snapshot |
| **M7 (Address Device)** | **Input ctx (192B) + cmd TRB + evt TRB + output ctx (192B)** | **CC only** | **MAJOR — no context visibility** |
| **M8 (Endpoint Config)** | **Input ctx (256B) + output ctx (256B) + BW dance contexts** | **CC only** | **MAJOR — no endpoint context visibility** |
| **M9 (HID Setup)** | **Output ctx after SET_CONFIG + after HID setup** | **CC only** | **MAJOR — no post-config state** |
| M10 (Interrupt Transfer) | TRB + EP state + registers | TRB + pre/post EP state + registers | None (Breenix more detailed) |
| M11 (Event Delivery) | Register snapshot + EP contexts + DCBAA | Same + pending event check | None |

### What This Means

We cannot currently do a byte-for-byte comparison of device context contents between the two platforms. The Linux module shows exactly what input context was sent and what output context the controller returned. Breenix only shows "CC=1" (success) — we can't see if the actual context data differs.

## What We Know Works Identically

- Controller discovery (BAR, capabilities, version)
- Controller reset (HCRST completes, CNR clears)
- Data structure setup (DCBAA, command ring, event ring, ERST)
- Controller start (RS=1, INTE=1, IMAN.IE=1)
- Port detection (CCS=1, PED=1, speed correct)
- Slot enablement (EnableSlot CC=1)
- Device addressing (AddressDevice CC=1)
- Endpoint configuration (ConfigureEndpoint CC=1, BW dance CC=1)
- HID class setup (SET_CONFIGURATION, SET_IDLE, SET_PROTOCOL all succeed)
- Interrupt TRB queueing (Normal TRBs enqueued, doorbells rung)

## What Fails

- **M11 only**: First interrupt transfer event returns CC=12 (Endpoint Not Enabled)
- All 4 interrupt endpoints transition immediately from Running to Halted
- Continuous polling confirms CC=12 never clears

## Eliminated Hypotheses (26 total)

1-18: Prior session hypotheses (xHCI logic, DMA, cache, register ordering, etc.)
19. Set TR Dequeue Pointer explicit command
20. USB device state (SET_CONFIGURATION(0) before ConfigureEndpoint)
21. xhci_hcd priming (Linux module works without it)
22. PCI configuration differences (identical Command register)
23. MSI configuration/ordering
24. Timing (10s, 60s delays)
25. phymemrange_enable alone (fires but no ep create)
26. EHCI companion init (CONFIGFLAG=1, RS=1, HCRST — no effect)

## Next Steps: Close the Instrumentation Gap

### Priority 1: Add Context Dumps to Breenix

Add `ms_dump` equivalent for device contexts at M7, M8, M9:

- **M7**: Dump input context before AddressDevice, output context after
- **M8**: Dump input context before ConfigureEndpoint, output context after, plus BW dance contexts
- **M9**: Dump output context after SET_CONFIGURATION and after HID setup

### Priority 2: Extract Matching Data from Linux Module

Run the Linux module on linux-probe and capture the full dmesg output with all context dumps. This becomes the **reference dataset**.

### Priority 3: Byte-for-Byte Comparison

Compare every dumped context field between Linux and Breenix:
- Slot Context: Route String, Speed, Context Entries, Max Exit Latency
- Endpoint 0 Context: EP Type, Max Packet Size, TR Dequeue, Interval, etc.
- Interrupt EP Contexts: EP Type, Max Packet Size, Interval, Mult, MaxPStreams, etc.

Any difference is a candidate root cause for CC=12.

### Priority 4: Stop Using serial_println for Debug

All experimental debug output has been added via `serial_println!`. This violates the project's tracing policy. The Parallels workaround code should use the lock-free tracing subsystem instead.

## Parallels Host Log Observations

The Parallels host log shows the hypervisor has an internal "ep create" mechanism. On linux-probe, `ep create` events fire ~330ms after xHCI init. On breenix-dev, they never fire. This is a symptom, not a root cause — the hypervisor creates endpoints when the xHCI controller state is correct, and doesn't when it's not.

Rather than reverse-engineering Parallels' internal signaling, the right approach is to make Breenix's xHCI state **byte-identical** to Linux's. The context dumps will show us where they differ.
