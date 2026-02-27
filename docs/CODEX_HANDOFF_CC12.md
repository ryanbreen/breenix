# Codex Handoff: xHCI CC=12 (ENDPOINT_NOT_ENABLED) Investigation

## Problem Statement

The Breenix kernel's xHCI USB driver on Parallels Desktop ARM64 VM gets **CC=12 (ENDPOINT_NOT_ENABLED)** on **every non-EP0 interrupt IN transfer**. EP0 control transfers always succeed (CC=1). Keyboard and mouse input is completely dead.

**Hardware:** Parallels Desktop virtual xHCI controller — PCI 00:03.0, vendor 1033:0194 (NEC/Renesas uPD720200), context_size=32, 0 scratchpad buffers, 14 ports (12x USB 3.0 at offset 1, 2x USB 2.0 at offset 13).

**Devices:** Two HID devices on USB 3.0 ports:
- Slot 1: Mouse (DCI 3 = interrupt IN EP1, DCI 5 = interrupt IN EP2)
- Slot 2: Keyboard (DCI 3 = interrupt IN EP1, DCI 5 = interrupt IN EP2/NKRO)

**The CC=12 cycle (40 events/sec):**
```
EP Running → queue Normal TRB → ring doorbell → CC=12 event →
EP Halted → Reset Endpoint → EP Stopped → Set TR Dequeue →
EP Running → requeue → CC=12 → (repeat forever)
```

**Linux works perfectly** on the same VM with the same hardware. We have a complete Linux ftrace reference at `docs/linux-xhci-trace-raw.txt`.

---

## What Has Been Verified Correct (Exhaustive)

After 26+ tests across multiple sessions, the following have ALL been confirmed to match Linux or be correct per the xHCI spec:

### Endpoint Context (matches Linux byte-for-byte)
- **DW0:** `0x00030000` — Mult=0 (per spec §6.2.3, non-SS-Isoch must be 0), Interval=3
- **DW1:** `0x0040003E` — MaxPacketSize=64, MaxBurst=0, EPType=7 (Interrupt IN), CErr=3
- **DW2-DW3:** TR Dequeue Pointer = ring physical base | DCS=1
- **DW4:** `0x00400040` — AvgTRBLen=64, MaxESITPayload=64

### Input Context
- **add_flags:** `0x29` (A0 + A3 + A5) — Slot + DCI 3 + DCI 5, matches Linux
- **Context Entries:** 5 (covers DCI 3 and DCI 5)
- Slot Context, EP Contexts all properly populated

### Transfer Ring
- 256 TRBs per ring, zeroed, Link TRB at last entry with Toggle Cycle set
- TRB content: Normal TRB (type 1), IOC=1 (bit 5), ISP=1 (bit 2), correct cycle bit
- Buffer physical address correct (HHDM virt-to-phys verified)
- Transfer length = 8 bytes (keyboard boot) or 9 bytes (mouse)
- **VERIFY diagnostic confirmed:** EP Context TR Dequeue Pointer matches actual ring base physical address
- **DMA sentinel test:** Buffer filled with 0xDE before transfer, buffer unchanged after CC=12 (xHC never touched it)

### Init Sequence (matches Linux)
1. PCI bus master + memory space enabled
2. xHC halt (RS=0, wait HCH=1)
3. xHC reset (HCRST=1, wait CNR=0)
4. MaxSlotsEn = max_slots
5. DCBAAP set
6. Command ring set (CRCR)
7. Event ring set (ERST, ERSTSZ, ERDP, ERSTBA)
8. Interrupter 0 enabled (IMAN.IE, IMOD)
9. Run (RS=1, INTE=1)
10. Port reset → Enable Slot → Address Device (BSR=0) → Get Device Descriptor → Configure Endpoints → SET_CONFIGURATION → HID setup

### Other Verified Items
- DCBAA entries point to correct device context bases
- Event ring entries/dequeue pointer correct
- Doorbell writes go to correct address (db_base + slot*4), correct target value (DCI)
- MFINDEX register is running (xHC is scheduling)
- Port numbers correct (USB 3.0 ports 1-12)
- USBLEGSUP not present on this vxHC (only Supported Protocol caps, ID=2)
- `BROKEN_MSI=true` vs `false` — no difference in CC=12
- `SKIP_BW_DANCE=true` vs `false` — no difference
- Mouse-only enumeration — CC=12
- Bulk EP type instead of Interrupt — CC=12
- No-Op Transfer TRB (type 8) — CC=12
- SET_PROTOCOL, CLEAR_FEATURE — no effect
- All endpoints show Running state at doorbell time
- Slot State = Configured (3)

---

## What Has NOT Been Verified / Remaining Theories

### Theory 1: Init Command Ordering
Linux's init sequence may differ in subtle ways we haven't caught. The Linux trace shows:
- Line 729-731: ConfigureEndpoint with add_flags "slot 1in 2in" (0x29)
- Line 863-866: First interrupt IN TRB queued for slot 2
- Linux does NOT ring doorbells for mouse interrupt endpoints in the trace window

**Key question:** Does Linux queue interrupt TRBs BEFORE or AFTER SET_CONFIGURATION? Our code queues AFTER, which matches the trace, but the timing/ordering within the post-config phase may matter.

### Theory 2: Parallels vxHC Requires Specific Port/Slot Binding
The virtual xHC may enforce that interrupt endpoints only work on USB 2.0 ports (13-14), not USB 3.0 ports (1-12). Linux might be using different port assignments. Check the Linux trace carefully for which ports devices enumerate on.

### Theory 3: Missing Scratchpad/Context Memory Requirements
Even though the xHC reports 0 scratchpad buffers, Parallels might need something additional in the DCBAA or device context area.

### Theory 4: Cache Coherency Issue (ARM64-specific)
We do `dc cvac` (clean) on TRBs and contexts before commands, and `dc civac` (clean+invalidate) on buffers before reading. But the sequence or scope might be wrong. The xHC might be reading stale data from a cache line we didn't clean.

### Theory 5: Ring Segment / Alignment
Transfer rings are 4KB-aligned (page allocation). But the xHCI spec may require additional alignment for the Parallels vxHC, or the ring physical address may be computed incorrectly in edge cases.

### Theory 6: Output Context Not Being Used Correctly
After Address Device succeeds, the Output Context has the device's actual endpoint state. Our configure_endpoints_batch reads the Slot Context from Output Context correctly, but the endpoint contexts in the Input Context are built from scratch (our descriptor parsing), not modified from the Output Context. This should be correct per spec, but verify.

### Theory 7: The xHC Simply Doesn't Support Interrupt Endpoints As We're Configuring Them
This is the "something fundamentally different about Parallels vxHC" theory. Compare our full init flow against Linux's via the ftrace, byte by byte. The answer must be in the difference.

---

## Current Code State

### File: `kernel/src/drivers/usb/xhci.rs` (~4190 lines)

**No serial output** — all diagnostics go through the lock-free `xhci_trace` ring buffer.

**Key configuration flags:**
```rust
const MINIMAL_INIT: bool = false;  // Full init sequence
const SKIP_BW_DANCE: bool = true;  // Skip StopEP + re-ConfigEP per endpoint
const MOUSE_ONLY: bool = false;    // Enumerate all devices
const BROKEN_MSI: bool = true;     // Timer-based polling (matches Linux quirk)
```

**Major sections:**
| Lines | Section |
|-------|---------|
| 1-100 | Constants, configuration flags |
| 101-305 | Type definitions (Trb, XhciState, trb_type, etc.) |
| 306-464 | Diagnostic counters (45+ pub static AtomicU64/U32) |
| 465-858 | Lock-free trace infrastructure |
| 859-1100 | Memory helpers (virt_to_phys, cache ops, MMIO read/write, allocate_pages) |
| 1100-1300 | Core xHCI commands (enable_slot, address_device, enqueue_transfer) |
| 1300-1450 | Control transfers (control_transfer) |
| 1450-1850 | USB descriptor parsing and configuration |
| 1866-2226 | configure_endpoints_batch (Input Context + ConfigureEndpoint command) |
| 2226-2410 | bandwidth_settle_endpoints (StopEP + re-ConfigEP) |
| 2410-2580 | configure_hid (HID class setup: SET_IDLE, GET_REPORT_DESC, SET_REPORT) |
| 2581-2651 | queue_hid_transfer (enqueue Normal TRB + doorbell) |
| 2651-2910 | drain_stale_events, start_hid_polling, process_keyboard/mouse_report |
| 2909-3008 | reset_halted_endpoint (Reset EP → Set TR Deq → requeue) |
| 3017-3024 | start_hid_polling |
| 3263-3531 | init() — main initialization entry point |
| 3543-3735 | handle_interrupt() — MSI/SPI interrupt handler |
| 3759-4168 | poll_hid_events() — timer-driven polling at 200Hz |

### File: `kernel/src/arch_impl/aarch64/timer_interrupt.rs`

**Heartbeat** (every 2 seconds via raw_serial_str, lock-free):
```
[HB t=Ns ctx=C sys=S xe=E uk=K fc=F er=R mf=M]
```
- `t` = uptime seconds
- `ctx` = context switch count
- `sys` = syscall count
- `xe` = xHCI error events (CC!=1 and CC!=13)
- `uk` = keyboard events received
- `fc` = first transfer completion code (12=CC=12, 1=SUCCESS, 0xFF=none seen)
- `er` = endpoint resets completed
- `mf` = MFINDEX register value (proves xHC is scheduling)

### File: `docs/linux-xhci-trace-raw.txt` (234KB)

Complete Linux ftrace of xHCI initialization on the same Parallels VM. Key reference points:
- Lines 1-50: xHCI init, capability registers
- Lines 100-200: Port scanning, device enumeration
- Lines 700-750: ConfigureEndpoint for slot 2 (keyboard)
- Lines 860-870: First interrupt IN TRB queued
- Throughout: All MMIO reads/writes, TRB submissions, completions

### Trace Analysis Scripts
- `scripts/parse-xhci-trace.py` — Parse Breenix xhci_trace_dump output
- `scripts/compare-xhci-traces.py` — Compare Breenix vs Linux traces

---

## How to Build and Test

### Build
```bash
# Force recompile + build
touch kernel/src/drivers/usb/xhci.rs
scripts/parallels/build-efi.sh --kernel
```

### Deploy and Boot
```bash
# Stop any running VM
prlctl stop breenix-dev --kill
while ! prlctl status breenix-dev | grep -q stopped; do sleep 1; done

# Clean state
> /tmp/breenix-parallels-serial.log
rm -f ~/Parallels/breenix-dev.pvm/NVRAM.dat

# Deploy and boot
scripts/parallels/deploy-to-vm.sh --boot

# Wait for boot + USB enumeration + heartbeats
sleep 50

# Read output
cat /tmp/breenix-parallels-serial.log
```

### What Success Looks Like
In the heartbeat output:
- `fc=1` or `fc=13` (SUCCESS or SHORT_PACKET) instead of `fc=12`
- `uk>0` (keyboard events received)
- `xe=0` or low (few/no error events)

### What Failure Looks Like (Current State)
```
[HB t=10s ctx=1234 sys=56 xe=400 uk=0 fc=12 er=396 mf=0x1A3F]
```
- `fc=12` — first transfer CC=12 (ENDPOINT_NOT_ENABLED)
- `uk=0` — zero keyboard events
- `xe=400` — 400 error events in 10 seconds (40/sec, 10 per endpoint)
- `er=396` — endpoint resets running (but endpoints immediately fail again)

---

## Trace Infrastructure

### Recording Traces
The xhci_trace ring buffer records automatically during init. To dump:

After boot, the trace dump appears in serial output between markers:
```
=== XHCI_TRACE_START total=N ===
... hex records ...
=== XHCI_TRACE_END ===
```

### Trace Operations
```
MmioWrite32=1    MmioWrite64=2    MmioRead32=3
CommandSubmit=10 CommandComplete=11
TransferSubmit=12 TransferEvent=13
Doorbell=14      InputContext=20  OutputContext=21
TransferRingSetup=22 CacheOp=30  SetTrDeq=31
EpState=40       PortStatusChange=41 Note=50
```

### Adding Trace Points
```rust
// Label a phase
xhci_trace_note(slot_id as u8, "my_phase");

// Trace a TRB
xhci_trace_trb(XhciTraceOp::TransferSubmit, slot, dci, &trb);

// Trace raw data
xhci_trace(XhciTraceOp::Note, slot, dci, &data_bytes);
```

**Rules:**
- NO serial_println! in the xHCI layer (locking causes timing perturbation)
- NO `log::*` macros
- ONLY use xhci_trace* functions or raw_serial_str (for heartbeat in timer ISR)
- The trace is lock-free, allocation-free, safe in interrupt context

---

## Architecture Notes

### ARM64 Virtual-to-Physical Translation
```rust
const HHDM_BASE: u64 = 0xFFFF_0000_0000_0000;
fn virt_to_phys(virt: u64) -> u64 { virt - HHDM_BASE }
```
All DMA addresses (ring bases, buffer pointers, DCBAA entries) must be physical.

### Cache Coherency (Critical on ARM64)
```rust
fn cache_clean(addr: u64, len: usize) {
    // dc cvac — Clean to Point of Coherency (write-back dirty lines)
    // Required BEFORE xHC reads our data (TRBs, contexts)
}
fn cache_clean_invalidate(addr: u64, len: usize) {
    // dc civac — Clean + Invalidate
    // Required BEFORE CPU reads xHC-written data (event ring, buffers)
}
```

### Transfer Ring Layout
```
HID_RING_BASE = 32 (= MAX_SLOTS)
Ring indices: 32 (kbd boot), 33 (mouse), 34 (kbd NKRO), 35 (mouse2)
Each ring: 256 TRBs, last TRB = Link TRB with Toggle Cycle
```

### Deferred SPI/TRB Timing
```
poll=0..199  — SPI disabled, polling only
poll=200     — Enable SPI (GIC interrupt delivery)
poll=300     — Queue first keyboard TRBs (AFTER SPI is active)
```
This exists because queuing TRBs before MSI/SPI is active was hypothesized to cause CC=12 on Parallels. However, CC=12 persists even with this deferral.

---

## Key Files Reference

| File | Purpose |
|------|---------|
| `kernel/src/drivers/usb/xhci.rs` | xHCI host controller driver (main file) |
| `kernel/src/drivers/usb/hid.rs` | HID report parsing |
| `kernel/src/drivers/pci.rs` | PCI configuration space access |
| `kernel/src/drivers/mod.rs` | Driver initialization |
| `kernel/src/arch_impl/aarch64/timer_interrupt.rs` | Timer ISR, heartbeat, calls poll_hid_events |
| `kernel/src/main_aarch64.rs` | Kernel entry point |
| `kernel/build.rs` | Build ID generation |
| `docs/linux-xhci-trace-raw.txt` | Linux ftrace reference (234KB) |
| `scripts/parse-xhci-trace.py` | Parse Breenix trace output |
| `scripts/compare-xhci-traces.py` | Breenix vs Linux trace comparison |
| `scripts/parallels/build-efi.sh` | Build kernel + EFI image |
| `scripts/parallels/deploy-to-vm.sh` | Deploy to Parallels VM |

---

## Suggested Next Steps

1. **Byte-for-byte comparison of our MMIO writes vs Linux's** during the full init sequence. The xhci_trace captures every MMIO write. Compare against the Linux ftrace. The CC=12 answer MUST be in a difference we haven't found yet.

2. **Check if Linux uses USB 2.0 ports** for these HID devices. Our code enumerates on USB 3.0 ports (1-12). If Parallels routes HID devices to USB 2.0 ports (13-14), our port assignment would be wrong.

3. **Investigate the xHC's Internal State** — after ConfigureEndpoint succeeds (CC=1) and endpoints show Running, something happens between that point and the first doorbell that causes the xHC to consider the endpoint "not enabled." This could be a Parallels vxHC quirk where it requires a specific event or delay.

4. **Try the exact Linux sequence** — replicate Linux's init flow exactly as shown in the ftrace, including any MMIO writes we might be skipping (like operational register reads, PORTSC writes, etc.).

5. **Compare Output Context** after our ConfigureEndpoint vs Linux's. The xHC writes back its understanding of the endpoint configuration into the Output Context. If the output differs between our driver and Linux's, that reveals what the xHC is rejecting.

---

## Previous Commit History

```
d72c8c7 fix: xHCI CC=12 reset storm — rate limiting + cascade prevention
eb595bd feat: mouse2 interrupt EP support, remove EP0 polling workaround, CC=12 diagnostics
a978f3c feat: EP0 mouse polling, bandwidth dance, CC=12 investigation
b1899ea feat: xHCI endpoint context matching Linux, EP0 GET_REPORT polling, MSI hardening
f3f4f90 feat: EHCI driver, xHCI bulk-for-interrupt workaround, class request diagnostics
6971f9e fix: correct xHCI endpoint context layout per spec, fix transfer ring index collision
2464de4 fix: increase Parallels display to 2560x1600, document hybrid GPU+GOP architecture
d605cd7 feat: VirtIO GPU PCI + XHCI USB drivers for Parallels ARM64 display
```
