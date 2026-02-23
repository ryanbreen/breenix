//! XHCI (USB 3.0) Host Controller Driver
//!
//! Implements the xHCI specification for USB 3.0 host controller support,
//! targeting the NEC uPD720200 XHCI controller at PCI slot 00:03.0
//! (vendor 0x1033, device 0x0194) on Parallels ARM64.
//!
//! # Architecture
//!
//! The xHCI controller uses memory-mapped registers from BAR0:
//! - Capability Registers (base + 0x00): CAPLENGTH, HCSPARAMS, HCCPARAMS, etc.
//! - Operational Registers (base + CAPLENGTH): USBCMD, USBSTS, CRCR, DCBAAP, etc.
//! - Port Registers (base + CAPLENGTH + 0x400): PORTSC per port
//! - Runtime Registers (base + RTSOFF): Interrupter registers
//! - Doorbell Registers (base + DBOFF): Per-slot doorbells
//!
//! Guest memory structures (all statically allocated):
//! - DCBAA: Device Context Base Address Array
//! - Command Ring: TRBs for host->controller commands
//! - Event Ring: TRBs for controller->host notifications
//! - Transfer Rings: Per-endpoint TRB rings for data transfer
//! - ERST: Event Ring Segment Table
//!
//! # Design
//!
//! All memory structures use static allocations (no heap/alloc), following the
//! same pattern as the VirtIO drivers in this kernel. DMA cache maintenance
//! is performed for ARM64 coherency.

#![cfg(target_arch = "aarch64")]

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering, fence};
use spin::Mutex;

use super::descriptors::{
    class_code, descriptor_type, hid_protocol, hid_request, hid_subclass, request,
    DeviceDescriptor, ConfigDescriptor, InterfaceDescriptor, EndpointDescriptor, SetupPacket,
};

// =============================================================================
// Constants
// =============================================================================

/// HHDM base for memory-mapped access (ARM64).
const HHDM_BASE: u64 = 0xFFFF_0000_0000_0000;

/// Minimal init test: skip bandwidth dance and HID class setup.
/// When true, the driver does only: Address → ConfigureEndpoint → SET_CONFIG → queue TRB.
/// Used to isolate whether CC=12 is caused by the bandwidth dance or HID setup steps.
const MINIMAL_INIT: bool = false;

/// Skip the bandwidth dance (StopEndpoint + re-ConfigureEndpoint per EP).
/// When true, only the initial batch ConfigureEndpoint is issued.
/// Linux performs this dance (3 ConfigureEndpoint commands per HID device),
/// so we match it here. BSR=1 + bandwidth dance together = Linux's exact sequence.
const SKIP_BW_DANCE: bool = false;

/// NEC XHCI vendor ID.
pub const NEC_VENDOR_ID: u16 = 0x1033;
/// NEC uPD720200 XHCI device ID.
pub const NEC_XHCI_DEVICE_ID: u16 = 0x0194;

/// Maximum device slots we support.
const MAX_SLOTS: usize = 32;
/// Command ring size in TRBs (last entry reserved for Link TRB).
/// Large command ring to avoid wrapping via Link TRB.
/// Parallels XHCI does not follow Link TRBs (tested: transfer rings fail,
/// command ring also fails after first wrap at 63 commands). With 4096 entries
/// (4095 usable), we get ~2044 ring resets before exhaustion ≈ hours of use.
const CMD_RING_SIZE: usize = 4096;
/// Event ring size in TRBs.
const EVENT_RING_SIZE: usize = 64;
/// Transfer ring size per endpoint in TRBs (last entry reserved for Link TRB).
/// Larger transfer ring reduces the number of Stop EP + Set TR Dequeue resets.
/// Each reset costs 2 command ring entries. With 256 entries (~85 GET_REPORTs
/// per fill) and 4095 usable command ring entries, we get ~2044 resets ≈ 29 min
/// of continuous keyboard polling at 100Hz before command ring exhaustion.
const TRANSFER_RING_SIZE: usize = 256;
/// Maximum number of HID transfer rings (keyboard + mouse).

// USE_BULK_FOR_INTERRUPT removed: Linux reference VM testing proved Interrupt IN
// (ep_type=7) works on Parallels xHCI. The CC=12 was caused by incorrect init
// ordering (SET_CONFIGURATION before ConfigureEndpoint), not endpoint type.

// =============================================================================
// TRB Type Constants
// =============================================================================

/// xHCI TRB type codes (complete per specification; not all used yet)
#[allow(dead_code)]
mod trb_type {
    pub const NORMAL: u32 = 1;
    pub const SETUP_STAGE: u32 = 2;
    pub const DATA_STAGE: u32 = 3;
    pub const STATUS_STAGE: u32 = 4;
    pub const LINK: u32 = 6;
    pub const ENABLE_SLOT: u32 = 9;
    pub const DISABLE_SLOT: u32 = 10;
    pub const ADDRESS_DEVICE: u32 = 11;
    pub const CONFIGURE_ENDPOINT: u32 = 12;
    pub const EVALUATE_CONTEXT: u32 = 13;
    pub const RESET_ENDPOINT: u32 = 14;
    pub const STOP_ENDPOINT: u32 = 15;
    pub const SET_TR_DEQUEUE_POINTER: u32 = 16;
    pub const NOOP: u32 = 23;
    pub const TRANSFER_EVENT: u32 = 32;
    pub const COMMAND_COMPLETION: u32 = 33;
    pub const PORT_STATUS_CHANGE: u32 = 34;
}

/// xHCI completion codes
#[allow(dead_code)]
mod completion_code {
    pub const SUCCESS: u32 = 1;
    pub const USB_TRANSACTION_ERROR: u32 = 4;
    pub const STALL_ERROR: u32 = 6;
    pub const ENDPOINT_NOT_ENABLED: u32 = 12;
    pub const SHORT_PACKET: u32 = 13;
}

// =============================================================================
// TRB (Transfer Request Block)
// =============================================================================

/// Transfer Request Block - the fundamental data structure for xHCI communication.
///
/// All TRBs are 16 bytes and must be 16-byte aligned. The controller and host
/// communicate via rings of TRBs.
#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct Trb {
    /// Data pointer or inline parameters
    param: u64,
    /// Transfer length, completion code, etc.
    status: u32,
    /// TRB type (bits 15:10), cycle bit (bit 0), flags
    control: u32,
}

impl Trb {
    const fn zeroed() -> Self {
        Trb {
            param: 0,
            status: 0,
            control: 0,
        }
    }

    /// Get the TRB type field (bits 15:10).
    fn trb_type(&self) -> u32 {
        (self.control >> 10) & 0x3F
    }

    /// Get the completion code from the status field (bits 31:24).
    fn completion_code(&self) -> u32 {
        (self.status >> 24) & 0xFF
    }

    /// Get the slot ID from the control field (bits 31:24).
    fn slot_id(&self) -> u8 {
        ((self.control >> 24) & 0xFF) as u8
    }
}

// =============================================================================
// Event Ring Segment Table Entry
// =============================================================================

/// Event Ring Segment Table entry.
///
/// Each entry points to a contiguous segment of the event ring.
#[repr(C, align(64))]
#[derive(Clone, Copy)]
struct ErstEntry {
    /// Physical base address of the event ring segment
    base: u64,
    /// Number of TRBs in this segment
    size: u32,
    /// Reserved
    _rsvd: u32,
}

impl ErstEntry {
    const fn zeroed() -> Self {
        ErstEntry {
            base: 0,
            size: 0,
            _rsvd: 0,
        }
    }
}

// =============================================================================
// Aligned Wrapper for Static Allocations
// =============================================================================

/// 64-byte aligned wrapper for static DMA structures.
#[repr(C, align(64))]
struct Aligned64<T>(T);

/// 4096-byte (page) aligned wrapper for structures requiring page alignment.
#[repr(C, align(4096))]
struct AlignedPage<T>(T);

// =============================================================================
// Static Memory Allocations
// =============================================================================

/// Device Context Base Address Array: 256 entries x 8 bytes = 2KB, 64-byte aligned.
///
/// Entry 0 is the scratchpad buffer array pointer (or 0 if not needed).
/// Entries 1..MaxSlots are device context pointers.
static mut DCBAA: AlignedPage<[u64; 256]> = AlignedPage([0u64; 256]);

/// Command Ring: 64 TRBs x 16 bytes = 1KB.
static mut CMD_RING: Aligned64<[Trb; CMD_RING_SIZE]> = Aligned64([Trb::zeroed(); CMD_RING_SIZE]);
/// Command ring enqueue pointer index.
static mut CMD_RING_ENQUEUE: usize = 0;
/// Command ring producer cycle state.
static mut CMD_RING_CYCLE: bool = true;

/// Event Ring: 64 TRBs x 16 bytes = 1KB.
static mut EVENT_RING: Aligned64<[Trb; EVENT_RING_SIZE]> =
    Aligned64([Trb::zeroed(); EVENT_RING_SIZE]);
/// Event ring dequeue pointer index.
static mut EVENT_RING_DEQUEUE: usize = 0;
/// Event ring consumer cycle state.
static mut EVENT_RING_CYCLE: bool = true;

/// Event Ring Segment Table (1 entry).
static mut ERST: Aligned64<[ErstEntry; 1]> = Aligned64([ErstEntry::zeroed(); 1]);

/// Base index for HID interrupt transfer rings, placed after per-slot EP0 rings
/// to avoid index collisions. Keyboard = HID_RING_BASE + 0, Mouse = HID_RING_BASE + 1.
const HID_RING_BASE: usize = MAX_SLOTS;

/// Total number of transfer rings: MAX_SLOTS for EP0 + 3 for HID interrupt endpoints.
/// hid_idx 0 = boot keyboard (DCI 3), 1 = mouse, 2 = NKRO keyboard (DCI 5).
const NUM_TRANSFER_RINGS: usize = MAX_SLOTS + 3;

/// Transfer rings for device endpoints.
///
/// Indices [0..MAX_SLOTS): EP0 control rings, indexed by slot_idx (slot_id - 1).
/// Indices [HID_RING_BASE..HID_RING_BASE+3): HID interrupt rings (keyboard, mouse, NKRO keyboard).
static mut TRANSFER_RINGS: [[Trb; TRANSFER_RING_SIZE]; NUM_TRANSFER_RINGS] =
    [[Trb::zeroed(); TRANSFER_RING_SIZE]; NUM_TRANSFER_RINGS];
/// Transfer ring enqueue indices.
static mut TRANSFER_ENQUEUE: [usize; NUM_TRANSFER_RINGS] = [0; NUM_TRANSFER_RINGS];
/// Transfer ring cycle state.
static mut TRANSFER_CYCLE: [bool; NUM_TRANSFER_RINGS] = [true; NUM_TRANSFER_RINGS];

/// Input Contexts for device setup (2048 bytes each for 64-byte contexts).
/// Used temporarily during AddressDevice and ConfigureEndpoint commands.
static mut INPUT_CONTEXTS: [AlignedPage<[u8; 4096]>; MAX_SLOTS] =
    [const { AlignedPage([0u8; 4096]) }; MAX_SLOTS];

/// Separate Input Context page for bandwidth dance re-ConfigureEndpoint.
/// Must be at a DIFFERENT physical address than INPUT_CONTEXTS to avoid
/// a caching bug in the Parallels virtual xHC where re-ConfigureEndpoint
/// is silently ignored if the Input Context pointer matches the initial
/// ConfigureEndpoint command.
/// Separate Input Context for BW dance re-ConfigureEndpoint commands.
/// Must be at a DIFFERENT physical address than INPUT_CONTEXTS — the Parallels
/// virtual xHC requires a distinct pointer for re-ConfigureEndpoint to take effect.
static mut RECONFIG_INPUT_CTX: AlignedPage<[u8; 4096]> = AlignedPage([0u8; 4096]);

/// Device Contexts (output contexts, 2048 bytes each).
/// Managed by the controller; we provide physical addresses via DCBAA.
static mut DEVICE_CONTEXTS: [AlignedPage<[u8; 4096]>; MAX_SLOTS] =
    [const { AlignedPage([0u8; 4096]) }; MAX_SLOTS];

/// HID report buffer for keyboard boot interface (8 bytes: modifier + reserved + 6 keycodes).
static mut KBD_REPORT_BUF: Aligned64<[u8; 8]> = Aligned64([0u8; 8]);

/// HID report buffer for NKRO keyboard interface (64 bytes to accommodate any report ID).
/// Reports include a Report ID prefix: [report_id, modifiers, reserved, key1..key6, ...].
static mut NKRO_REPORT_BUF: Aligned64<[u8; 64]> = Aligned64([0u8; 64]);

/// HID report buffer for mouse (8 bytes: buttons + X + Y + wheel + ...).
static mut MOUSE_REPORT_BUF: Aligned64<[u8; 8]> = Aligned64([0u8; 8]);

/// Scratch buffer for control transfer data stages (256 bytes).
static mut CTRL_DATA_BUF: Aligned64<[u8; 256]> = Aligned64([0u8; 256]);

/// Separate DMA buffer for mouse EP0 GET_REPORT (16 bytes for absolute report).
/// Must be separate from CTRL_DATA_BUF since keyboard and mouse EP0 polls
/// can be in flight simultaneously on different slots.
static mut MOUSE_CTRL_DATA_BUF: Aligned64<[u8; 16]> = Aligned64([0u8; 16]);

// =============================================================================
// Controller State
// =============================================================================

/// XHCI controller state, populated during initialization.
struct XhciState {
    /// HHDM virtual address of BAR0 (retained for runtime register access if needed)
    #[allow(dead_code)]
    base: u64,
    /// Length of capability registers (retained for recalculating register offsets)
    #[allow(dead_code)]
    cap_length: u8,
    /// Operational register base (base + cap_length)
    op_base: u64,
    /// Runtime register base (base + rtsoff)
    rt_base: u64,
    /// Doorbell register base (base + dboff)
    db_base: u64,
    /// Maximum enabled device slots (retained for runtime slot validation)
    #[allow(dead_code)]
    max_slots: u8,
    /// Maximum root hub ports
    #[allow(dead_code)] // Used by scan_ports (standard enumeration)
    max_ports: u8,
    /// Context entry size (32 or 64 bytes)
    context_size: usize,
    /// GIC INTID for this controller
    irq: u32,
    /// Slot ID for keyboard device (0 = not found)
    kbd_slot: u8,
    /// Endpoint DCI for keyboard boot interrupt IN (interface 0)
    kbd_endpoint: u8,
    /// Endpoint DCI for keyboard NKRO interrupt IN (interface 1, 0 = not found)
    /// Parallels sends keyboard data on this endpoint, not the boot endpoint.
    kbd_nkro_endpoint: u8,
    /// Slot ID for mouse device (0 = not found)
    mouse_slot: u8,
    /// Endpoint DCI for mouse interrupt IN
    mouse_endpoint: u8,
}

/// Global lock protecting XHCI controller access.
static XHCI_LOCK: Mutex<()> = Mutex::new(());

/// Global XHCI controller state.
static mut XHCI_STATE: Option<XhciState> = None;

/// Initialization flag, checked before accessing XHCI_STATE.
static XHCI_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// IRQ number stored early (before XHCI_INITIALIZED) so handle_interrupt
/// can disable the SPI to prevent storms during enumeration.
static XHCI_IRQ: AtomicU32 = AtomicU32::new(0);

/// Diagnostic counters for heartbeat visibility.
pub static POLL_COUNT: AtomicU64 = AtomicU64::new(0);
/// Whether start_hid_polling has been called (deferred from init to after MSI active).
static HID_POLLING_STARTED: AtomicBool = AtomicBool::new(false);
pub static EVENT_COUNT: AtomicU64 = AtomicU64::new(0);
pub static KBD_EVENT_COUNT: AtomicU64 = AtomicU64::new(0);
/// Counts transfer events from NKRO keyboard endpoint (DCI 5).
pub static NKRO_EVENT_COUNT: AtomicU64 = AtomicU64::new(0);
/// First 8 bytes of last NKRO report buffer (for heartbeat diagnostics).
pub static LAST_NKRO_REPORT_U64: AtomicU64 = AtomicU64::new(0);
/// Counts transfer events that didn't match kbd/mouse slots or had error CC.
pub static XFER_OTHER_COUNT: AtomicU64 = AtomicU64::new(0);
/// Last "other" transfer event info: (slot << 16) | (endpoint << 8) | cc.
pub static XO_LAST_INFO: AtomicU64 = AtomicU64::new(0);
/// Counts "other" events that had error completion codes (not SUCCESS/SHORT_PACKET).
pub static XO_ERR_COUNT: AtomicU64 = AtomicU64::new(0);
/// Counts port status change events.
pub static PSC_COUNT: AtomicU64 = AtomicU64::new(0);
/// Counts events processed via MSI interrupt handler (handle_interrupt).
pub static MSI_EVENT_COUNT: AtomicU64 = AtomicU64::new(0);

/// Flags set by MSI interrupt handler to request requeue from timer poll.
/// Requeuing from IRQ context causes MSI storms on virtual XHCI controllers.
static MSI_KBD_NEEDS_REQUEUE: AtomicBool = AtomicBool::new(false);
static MSI_MOUSE_NEEDS_REQUEUE: AtomicBool = AtomicBool::new(false);
static MSI_NKRO_NEEDS_REQUEUE: AtomicBool = AtomicBool::new(false);

/// Sentinel diagnostic: counts reports where the sentinel byte (0xDE) was NOT overwritten by DMA.
pub static DMA_SENTINEL_SURVIVED: AtomicU64 = AtomicU64::new(0);
/// Sentinel diagnostic: counts reports where the sentinel byte WAS overwritten (DMA worked).
pub static DMA_SENTINEL_REPLACED: AtomicU64 = AtomicU64::new(0);

/// Periodic diagnostic: last USBSTS value (controller status).
pub static DIAG_USBSTS: AtomicU32 = AtomicU32::new(0);
/// Periodic diagnostic: last PORTSC for keyboard port.
pub static DIAG_KBD_PORTSC: AtomicU32 = AtomicU32::new(0);
/// Periodic diagnostic: last endpoint state for keyboard DCI=3 and DCI=5 (packed: dci3 << 4 | dci5).
pub static DIAG_KBD_EP_STATE: AtomicU32 = AtomicU32::new(0);
/// Periodic diagnostic: SPI enable count (how many times SPI was re-enabled).
pub static DIAG_SPI_ENABLE_COUNT: AtomicU64 = AtomicU64::new(0);
/// Diagnostic: endpoint state BEFORE doorbell ring in queue_hid_transfer.
/// Format: (pre_state << 4) | post_state for the last transfer queued.
pub static DIAG_DOORBELL_EP_STATE: AtomicU32 = AtomicU32::new(0);
/// Diagnostic: endpoint state from the FIRST queue_hid_transfer call only.
/// Format: (pre_state << 4) | post_state. Written once, never overwritten.
pub static DIAG_FIRST_DB: AtomicU32 = AtomicU32::new(0xFF);
/// Diagnostic: CC of the very first Transfer Event seen.
pub static DIAG_FIRST_XFER_CC: AtomicU32 = AtomicU32::new(0xFF);
/// Diagnostic: TRB Pointer from the first Transfer Event (physical address of TRB that completed).
pub static DIAG_FIRST_XFER_PTR: AtomicU64 = AtomicU64::new(0);
/// Diagnostic: Physical address of the first HID TRB we queued.
pub static DIAG_FIRST_QUEUED_PHYS: AtomicU64 = AtomicU64::new(0);
/// Diagnostic: Slot and endpoint from the first Transfer Event (slot << 8 | endpoint).
pub static DIAG_FIRST_XFER_SLEP: AtomicU32 = AtomicU32::new(0);
/// Diagnostic: Full status DW of the first Transfer Event (CC << 24 | residual).
pub static DIAG_FIRST_XFER_STATUS: AtomicU32 = AtomicU32::new(0);
/// Diagnostic: Full control DW of the first Transfer Event.
pub static DIAG_FIRST_XFER_CONTROL: AtomicU32 = AtomicU32::new(0);

/// Flags set when Transfer Events arrive with error completion codes (e.g., CC=12
/// Endpoint Not Enabled). Checked by poll_hid_events to trigger Reset Endpoint
/// + Set TR Dequeue Pointer recovery.
static NEEDS_RESET_KBD_BOOT: AtomicBool = AtomicBool::new(false);
static NEEDS_RESET_KBD_NKRO: AtomicBool = AtomicBool::new(false);
static NEEDS_RESET_MOUSE: AtomicBool = AtomicBool::new(false);
/// Diagnostic: counts successful endpoint resets.
pub static ENDPOINT_RESET_COUNT: AtomicU64 = AtomicU64::new(0);
/// Diagnostic: counts failed endpoint reset attempts.
pub static ENDPOINT_RESET_FAIL_COUNT: AtomicU64 = AtomicU64::new(0);
/// Maximum number of endpoint resets before giving up.
/// Each reset uses 2 command ring entries. With CMD_RING_SIZE=4096 (4095 usable)
/// and ~6 entries used during init, limit to 50 resets (100 cmd ring entries)
/// to preserve command ring capacity for future use.
const MAX_ENDPOINT_RESETS: u64 = 10;

/// Whether initial HID interrupt TRBs have been queued post-init.
/// TRBs are deferred until after XHCI_INITIALIZED and SPI enable so the full
/// MSI → GIC SPI → CPU ISR → IMAN.IP ack pathway is active when the xHC
/// processes the first interrupt endpoint transfer.
static HID_TRBS_QUEUED: AtomicBool = AtomicBool::new(false);

// EP0 GET_REPORT polling state machine.
// Since interrupt endpoints always return CC=12 on Parallels virtual xHC,
// we poll for keyboard data via GET_REPORT on the working EP0 control pipe.
// State: 0 = IDLE (ready for next request), 1 = PENDING (TRBs queued)
pub static EP0_POLL_STATE: AtomicU32 = AtomicU32::new(0);
/// Count of EP0 GET_REPORT requests queued.
pub static EP0_GET_REPORT_COUNT: AtomicU64 = AtomicU64::new(0);
/// Count of successful EP0 GET_REPORT completions.
pub static EP0_GET_REPORT_OK: AtomicU64 = AtomicU64::new(0);
/// Count of failed EP0 GET_REPORT completions (non-SUCCESS CC).
pub static EP0_GET_REPORT_ERR: AtomicU64 = AtomicU64::new(0);

// EP0 GET_REPORT polling for mouse device (same approach as keyboard).
// Mouse is on a separate slot with its own EP0 transfer ring.
// State: 0 = IDLE, 1 = PENDING
pub static EP0_MOUSE_POLL_STATE: AtomicU32 = AtomicU32::new(0);
/// Count of EP0 GET_REPORT requests queued for mouse.
pub static EP0_MOUSE_GET_REPORT_COUNT: AtomicU64 = AtomicU64::new(0);
/// Count of successful EP0 GET_REPORT completions for mouse.
pub static EP0_MOUSE_GET_REPORT_OK: AtomicU64 = AtomicU64::new(0);
/// Count of failed EP0 GET_REPORT completions for mouse.
pub static EP0_MOUSE_GET_REPORT_ERR: AtomicU64 = AtomicU64::new(0);

// =============================================================================
// Memory Helpers
// =============================================================================

/// Convert a kernel virtual address to a physical address.
///
/// On QEMU aarch64, kernel statics are accessed via HHDM (>= 0xFFFF_0000_0000_0000),
/// so phys = virt - HHDM_BASE.
/// On Parallels, the kernel may be identity-mapped via TTBR0, so statics are
/// at their physical addresses already.
#[inline]
fn virt_to_phys(virt: u64) -> u64 {
    if virt >= HHDM_BASE {
        virt - HHDM_BASE
    } else {
        // Already a physical address (identity-mapped kernel on Parallels)
        virt
    }
}

/// Clean (flush) a range of memory from CPU caches to the point of coherency.
///
/// Must be called after writing DMA descriptors/data and before issuing
/// DMA commands, so the device sees the updated data in physical memory.
#[inline]
fn dma_cache_clean(ptr: *const u8, len: usize) {
    const CACHE_LINE: usize = 64;
    let start = ptr as usize & !(CACHE_LINE - 1);
    let end = (ptr as usize + len + CACHE_LINE - 1) & !(CACHE_LINE - 1);
    for addr in (start..end).step_by(CACHE_LINE) {
        unsafe {
            core::arch::asm!("dc cvac, {}", in(reg) addr, options(nostack));
        }
    }
    unsafe {
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }
}

/// Invalidate a range of memory in CPU caches after a device DMA write.
///
/// Must be called after a DMA read completes and before the CPU reads
/// the DMA buffer, to ensure the CPU sees the device-written data.
#[inline]
fn dma_cache_invalidate(ptr: *const u8, len: usize) {
    const CACHE_LINE: usize = 64;
    let start = ptr as usize & !(CACHE_LINE - 1);
    let end = (ptr as usize + len + CACHE_LINE - 1) & !(CACHE_LINE - 1);
    for addr in (start..end).step_by(CACHE_LINE) {
        unsafe {
            core::arch::asm!("dc civac, {}", in(reg) addr, options(nostack));
        }
    }
    unsafe {
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }
}

// =============================================================================
// MMIO Register Access
// =============================================================================

#[inline]
fn read32(addr: u64) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

#[inline]
fn write32(addr: u64, val: u32) {
    unsafe { core::ptr::write_volatile(addr as *mut u32, val) }
}

#[inline]
#[allow(dead_code)] // Part of MMIO register access API
fn read64(addr: u64) -> u64 {
    unsafe { core::ptr::read_volatile(addr as *const u64) }
}

#[inline]
fn write64(addr: u64, val: u64) {
    unsafe { core::ptr::write_volatile(addr as *mut u64, val) }
}

// =============================================================================
// Timeout Helper
// =============================================================================

/// Spin-wait until `f()` returns true, or fail after `max_iters` iterations.
fn wait_for<F: Fn() -> bool>(f: F, max_iters: u32) -> Result<(), &'static str> {
    for _ in 0..max_iters {
        if f() {
            return Ok(());
        }
        core::hint::spin_loop();
    }
    Err("XHCI timeout")
}

// =============================================================================
// Command Ring Operations
// =============================================================================

/// Enqueue a TRB onto the command ring.
///
/// Sets the cycle bit appropriately and handles ring wraparound via a Link TRB.
fn enqueue_command(trb: Trb) {
    unsafe {
        let idx = CMD_RING_ENQUEUE;
        let ring = &raw mut CMD_RING;
        let cycle = CMD_RING_CYCLE;

        // Set the cycle bit on the TRB
        let mut t = trb;
        if cycle {
            t.control |= 1;
        } else {
            t.control &= !1;
        }

        core::ptr::write_volatile(&mut (*ring).0[idx] as *mut Trb, t);

        // Cache clean to ensure the controller sees the TRB
        dma_cache_clean(
            &(*ring).0[idx] as *const Trb as *const u8,
            core::mem::size_of::<Trb>(),
        );

        fence(Ordering::SeqCst);

        // Advance enqueue pointer; last entry is reserved for Link TRB
        let next_idx = (idx + 1) % (CMD_RING_SIZE - 1);
        CMD_RING_ENQUEUE = next_idx;

        if next_idx == 0 {
            // Wrap: write Link TRB pointing back to start of ring
            let cmd_ring_phys = virt_to_phys(&raw const CMD_RING as u64);
            let link = Trb {
                param: cmd_ring_phys,
                status: 0,
                // Link TRB type, Toggle Cycle (TC) bit 5, plus current cycle bit
                control: (trb_type::LINK << 10)
                    | if cycle { 1 } else { 0 }
                    | (1 << 5),
            };
            core::ptr::write_volatile(
                &mut (*ring).0[CMD_RING_SIZE - 1] as *mut Trb,
                link,
            );
            dma_cache_clean(
                &(*ring).0[CMD_RING_SIZE - 1] as *const Trb as *const u8,
                core::mem::size_of::<Trb>(),
            );
            CMD_RING_CYCLE = !cycle;
        }
    }
}

/// Ring the doorbell for a given slot and target.
///
/// Slot 0, target 0 = host controller command ring.
/// Slot N, target DCI = endpoint for that device slot.
fn ring_doorbell(state: &XhciState, slot: u8, target: u8) {
    write32(state.db_base + (slot as u64) * 4, target as u32);
    // DSB to ensure the doorbell write reaches the device
    unsafe {
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }
}

/// Wait for an event on the event ring, with timeout.
///
/// Returns Command Completion and Transfer Event TRBs.
/// Skips asynchronous events (Port Status Change) that may arrive
/// at any time and don't correspond to a specific command or transfer.
///
/// If `command_only` is true, only returns Command Completion events (type 33).
/// Transfer Events from interrupt endpoints are silently consumed — this
/// prevents confusion when interrupt TRBs are queued during enumeration
/// and the xHC returns Transfer Events before Command Completions.
fn wait_for_event_inner(state: &XhciState, command_only: bool) -> Result<Trb, &'static str> {
    let mut timeout = 2_000_000u32;
    loop {
        unsafe {
            let ring = &raw const EVENT_RING;
            let idx = EVENT_RING_DEQUEUE;
            let cycle = EVENT_RING_CYCLE;

            // Invalidate cache to see controller-written TRBs
            dma_cache_invalidate(
                &(*ring).0[idx] as *const Trb as *const u8,
                core::mem::size_of::<Trb>(),
            );

            let trb = core::ptr::read_volatile(&(*ring).0[idx]);
            let trb_cycle = trb.control & 1 != 0;

            if trb_cycle == cycle {
                // New event available — advance dequeue
                EVENT_RING_DEQUEUE = (idx + 1) % EVENT_RING_SIZE;
                if EVENT_RING_DEQUEUE == 0 {
                    EVENT_RING_CYCLE = !cycle;
                }

                // Update ERDP to acknowledge the event
                let erdp_phys = virt_to_phys(&raw const EVENT_RING as u64)
                    + (EVENT_RING_DEQUEUE as u64) * 16;
                let ir0 = state.rt_base + 0x20; // Interrupter 0
                write64(ir0 + 0x18, erdp_phys | (1 << 3));

                let trb_type_val = trb.trb_type();
                if trb_type_val == trb_type::COMMAND_COMPLETION {
                    return Ok(trb);
                }
                if trb_type_val == trb_type::TRANSFER_EVENT && !command_only {
                    return Ok(trb);
                }
                // Log consumed Transfer Events — these indicate interrupt TRBs
                // completing while we're waiting for a command completion.
                if trb_type_val == trb_type::TRANSFER_EVENT && command_only {
                    let slot = trb.slot_id();
                    let ep = (trb.control >> 16) & 0x1F;
                    let cc = trb.completion_code();
                    crate::serial_println!(
                        "[xhci] wait_for_command consumed Transfer Event: slot={} ep={} cc={}",
                        slot, ep, cc,
                    );
                }
                // Consumed non-matching event (Port Status Change, or Transfer
                // Event in command_only mode) — fall through to timeout check.
            }
        }
        timeout -= 1;
        if timeout == 0 {
            return Err("XHCI event timeout");
        }
        core::hint::spin_loop();
    }
}

/// Wait for any command completion or transfer event.
fn wait_for_event(state: &XhciState) -> Result<Trb, &'static str> {
    wait_for_event_inner(state, false)
}

/// Wait for a command completion event only (skip transfer events).
///
/// Used during enumeration when interrupt TRBs may be queued on the ring
/// and the xHC may return Transfer Events before Command Completions.
fn wait_for_command(state: &XhciState) -> Result<Trb, &'static str> {
    wait_for_event_inner(state, true)
}

// =============================================================================
// Slot and Device Commands
// =============================================================================

/// Issue an Enable Slot command and return the assigned slot ID.
fn enable_slot(state: &XhciState) -> Result<u8, &'static str> {
    let trb = Trb {
        param: 0,
        status: 0,
        control: trb_type::ENABLE_SLOT << 10,
    };
    enqueue_command(trb);
    ring_doorbell(state, 0, 0);

    let event = wait_for_command(state)?;
    let cc = event.completion_code();
    if cc != completion_code::SUCCESS {
        crate::serial_println!("[xhci] EnableSlot failed: completion code {}", cc);
        return Err("XHCI EnableSlot failed");
    }

    let slot_id = event.slot_id();
    crate::serial_println!("[xhci] Enabled slot {}", slot_id);
    Ok(slot_id)
}

/// Issue an Address Device command for the given slot and root hub port.
///
/// Builds an Input Context with Slot Context and Endpoint 0 (control) Context,
/// then submits the AddressDevice TRB.
fn address_device(state: &XhciState, slot_id: u8, port_id: u8) -> Result<(), &'static str> {
    if slot_id as usize > MAX_SLOTS || slot_id == 0 {
        return Err("XHCI: invalid slot_id for address_device");
    }

    let slot_idx = (slot_id - 1) as usize;
    let ctx_size = state.context_size;

    unsafe {
        // Zero the input context
        let input_ctx = &raw mut INPUT_CONTEXTS[slot_idx];
        core::ptr::write_bytes((*input_ctx).0.as_mut_ptr(), 0, 4096);

        // Zero the output (device) context and flush to RAM.
        // ARM64 DMA: the xHC writes Output Context to RAM (bypassing cache).
        // If we leave dirty zeros in cache, a later cache eviction or our own
        // dma_cache_invalidate (dc civac = clean+invalidate) could write stale
        // zeros back to RAM, overwriting the xHC's data. Flush immediately.
        let dev_ctx = &raw mut DEVICE_CONTEXTS[slot_idx];
        core::ptr::write_bytes((*dev_ctx).0.as_mut_ptr(), 0, 4096);
        dma_cache_clean((*dev_ctx).0.as_ptr(), 4096);

        let input_base = (*input_ctx).0.as_mut_ptr();

        // --- Input Control Context (first context entry) ---
        // Add Context flags: bits 0 and 1 (add Slot Context and EP0 Context)
        // The Add Context flags are at offset 0x04 in the Input Control Context
        let add_flags_ptr = input_base.add(0x04) as *mut u32;
        core::ptr::write_volatile(add_flags_ptr, 0x03); // A0=1 (Slot), A1=1 (EP0)

        // --- Slot Context (second context entry, at offset ctx_size) ---
        let slot_ctx = input_base.add(ctx_size);

        // Slot Context DW0: Route String = 0, Speed, Context Entries = 1
        // Speed: We'll read PORTSC to determine speed
        let portsc = read32(state.op_base + 0x400 + ((port_id - 1) as u64) * 0x10);
        let port_speed = (portsc >> 10) & 0xF; // Port Speed bits [13:10]

        // DW0: Context Entries (bits 31:27) = 1, Speed (bits 23:20)
        let slot_dw0: u32 = (1u32 << 27) | (port_speed << 20);
        core::ptr::write_volatile(slot_ctx as *mut u32, slot_dw0);

        // DW1: Root Hub Port Number (bits 23:16), Max Exit Latency (bits 15:0) = 0
        // Per Linux xhci.h: ROOT_HUB_PORT(p) = ((p) & 0xff) << 16
        let slot_dw1: u32 = (port_id as u32) << 16;
        core::ptr::write_volatile(slot_ctx.add(4) as *mut u32, slot_dw1);

        // --- Endpoint 0 Context (third context entry, at offset ctx_size * 2) ---
        let ep0_ctx = input_base.add(ctx_size * 2);

        // Determine max packet size based on port speed
        // Speed: 1=Full (64), 2=Low (8), 3=High (64), 4=Super (512)
        let max_packet_size: u16 = match port_speed {
            1 => 64,   // Full Speed
            2 => 8,    // Low Speed
            3 => 64,   // High Speed
            4 => 512,  // SuperSpeed
            _ => 64,   // Default to Full Speed
        };

        // EP0 DW1: Max Packet Size (bits 31:16), EP Type (bits 5:3) = 4 (Control Bidir), CErr (bits 2:1) = 3
        // Linux reference: EP0 DW1 = 0x02000026 for SuperSpeed (MaxPktSize=512, EPType=4, CErr=3)
        let ep0_dw1: u32 = ((max_packet_size as u32) << 16) | (4u32 << 3) | (3u32 << 1);
        core::ptr::write_volatile(ep0_ctx.add(0x04) as *mut u32, ep0_dw1);

        // EP0 DW2-DW3: TR Dequeue Pointer
        // Each device slot uses its own transfer ring during enumeration.
        let ring_ptr = &raw mut TRANSFER_RINGS[slot_idx];
        core::ptr::write_bytes(ring_ptr as *mut u8, 0, TRANSFER_RING_SIZE * 16);
        dma_cache_clean(
            &TRANSFER_RINGS[slot_idx] as *const [Trb; TRANSFER_RING_SIZE] as *const u8,
            TRANSFER_RING_SIZE * 16,
        );
        TRANSFER_ENQUEUE[slot_idx] = 0;
        TRANSFER_CYCLE[slot_idx] = true;

        let ep0_ring_phys = virt_to_phys(&raw const TRANSFER_RINGS[slot_idx] as u64);

        // DW2: TR Dequeue Pointer (low 32 bits) with DCS (Dequeue Cycle State) = 1
        core::ptr::write_volatile(
            ep0_ctx.add(0x08) as *mut u32,
            (ep0_ring_phys as u32) | 1, // DCS = 1
        );
        // DW3: TR Dequeue Pointer (high 32 bits)
        core::ptr::write_volatile(
            ep0_ctx.add(0x0C) as *mut u32,
            (ep0_ring_phys >> 32) as u32,
        );

        // EP0 DW4: Max ESIT Payload Lo (bits 31:16) = 0 for control, Average TRB Length (bits 15:0) = 8
        // Linux reference: EP0 DW4 = 0x00000000 (AvgTRBLen=0 for control EP).
        // We use 8 as a more accurate hint for 8-byte setup packets.
        let ep0_dw4: u32 = 8; // Avg TRB len = 8 for control, no ESIT payload
        core::ptr::write_volatile(ep0_ctx.add(0x10) as *mut u32, ep0_dw4);

        // Cache-clean the input context
        dma_cache_clean(input_base, 4096);

        // Set the output device context pointer in DCBAA
        let dev_ctx_phys = virt_to_phys(&raw const DEVICE_CONTEXTS[slot_idx] as u64);
        let dcbaa = &raw mut DCBAA;
        (*dcbaa).0[slot_id as usize] = dev_ctx_phys;
        dma_cache_clean(
            &(*dcbaa).0[slot_id as usize] as *const u64 as *const u8,
            8,
        );

        // Build AddressDevice TRB (BSR=0)
        // BSR=0: the xHC assigns an address and sends SET_ADDRESS to the device,
        // transitioning the slot to Addressed state. Required before ConfigureEndpoint.
        // Note: the ftrace agent misidentified the cycle bit (b:C) as the BSR bit —
        // BSR=1 causes CC=19 (Context State Error) on ConfigureEndpoint.
        let input_ctx_phys = virt_to_phys(&raw const INPUT_CONTEXTS[slot_idx] as u64);
        let trb = Trb {
            param: input_ctx_phys,
            status: 0,
            // AddressDevice type, Slot ID in bits 31:24
            control: (trb_type::ADDRESS_DEVICE << 10) | ((slot_id as u32) << 24),
        };
        enqueue_command(trb);
        ring_doorbell(state, 0, 0);

        let event = wait_for_command(state)?;
        let cc = event.completion_code();
        if cc != completion_code::SUCCESS {
            crate::serial_println!(
                "[xhci] AddressDevice slot {} failed: completion code {}",
                slot_id,
                cc
            );
            return Err("XHCI AddressDevice failed");
        }

        crate::serial_println!("[xhci] Addressed device in slot {}", slot_id);
        Ok(())
    }
}

// =============================================================================
// Transfer Ring Operations
// =============================================================================

/// Enqueue a TRB on a HID transfer ring (keyboard index 0, mouse index 1).
fn enqueue_transfer(hid_idx: usize, trb: Trb) {
    unsafe {
        let idx = TRANSFER_ENQUEUE[hid_idx];
        let cycle = TRANSFER_CYCLE[hid_idx];

        let mut t = trb;
        if cycle {
            t.control |= 1;
        } else {
            t.control &= !1;
        }

        core::ptr::write_volatile(
            &mut TRANSFER_RINGS[hid_idx][idx] as *mut Trb,
            t,
        );
        dma_cache_clean(
            &TRANSFER_RINGS[hid_idx][idx] as *const Trb as *const u8,
            core::mem::size_of::<Trb>(),
        );

        fence(Ordering::SeqCst);

        let next_idx = (idx + 1) % (TRANSFER_RING_SIZE - 1);
        TRANSFER_ENQUEUE[hid_idx] = next_idx;

        if next_idx == 0 {
            // Write Link TRB
            let ring_phys = virt_to_phys(&raw const TRANSFER_RINGS[hid_idx] as u64);
            let link = Trb {
                param: ring_phys,
                status: 0,
                control: (trb_type::LINK << 10)
                    | if cycle { 1 } else { 0 }
                    | (1 << 1), // TC (Toggle Cycle) bit — xHCI spec bit 1, not bit 5
            };
            core::ptr::write_volatile(
                &mut TRANSFER_RINGS[hid_idx][TRANSFER_RING_SIZE - 1] as *mut Trb,
                link,
            );
            dma_cache_clean(
                &TRANSFER_RINGS[hid_idx][TRANSFER_RING_SIZE - 1] as *const Trb as *const u8,
                core::mem::size_of::<Trb>(),
            );
            TRANSFER_CYCLE[hid_idx] = !cycle;
        }
    }
}

// =============================================================================
// Control Transfers (Setup -> Data -> Status)
// =============================================================================

/// Reset EP0 (DCI=1) after a STALL or USB Transaction Error.
///
/// Per xHCI spec section 4.6.8 and 4.10.2.2, any error on a control endpoint
/// halts it. Software must issue Reset Endpoint + Set TR Dequeue Pointer to
/// recover before subsequent control transfers can proceed.
fn reset_control_endpoint(state: &XhciState, slot_id: u8) {
    let slot_idx = (slot_id - 1) as usize;

    // Step 1: Reset Endpoint Command for DCI=1 (EP0)
    let reset_trb = Trb {
        param: 0,
        status: 0,
        control: (trb_type::RESET_ENDPOINT << 10)
            | ((slot_id as u32) << 24)
            | (1u32 << 16),
    };
    enqueue_command(reset_trb);
    ring_doorbell(state, 0, 0);
    let _ = wait_for_command(state);

    // Step 2: Reset EP0 transfer ring
    unsafe {
        let ring = &raw mut TRANSFER_RINGS[slot_idx];
        core::ptr::write_bytes(ring as *mut u8, 0, TRANSFER_RING_SIZE * 16);
        dma_cache_clean(
            &TRANSFER_RINGS[slot_idx] as *const [Trb; TRANSFER_RING_SIZE] as *const u8,
            TRANSFER_RING_SIZE * 16,
        );
        TRANSFER_ENQUEUE[slot_idx] = 0;
        TRANSFER_CYCLE[slot_idx] = true;
    }

    // Step 3: Set TR Dequeue Pointer to ring start with DCS=1
    let ring_phys = virt_to_phys(unsafe { &raw const TRANSFER_RINGS[slot_idx] } as u64);
    let set_deq_trb = Trb {
        param: ring_phys | 1,
        status: 0,
        control: (trb_type::SET_TR_DEQUEUE_POINTER << 10)
            | ((slot_id as u32) << 24)
            | (1u32 << 16),
    };
    enqueue_command(set_deq_trb);
    ring_doorbell(state, 0, 0);
    let _ = wait_for_command(state);

    crate::serial_println!(
        "[xhci] Reset EP0 after error on slot {}",
        slot_id,
    );
}

/// Execute a control transfer on a device's default control endpoint (EP0).
///
/// Sends a Setup stage TRB, optional Data stage TRB, and Status stage TRB
/// on the device's EP0 transfer ring, then waits for completion.
///
/// # Arguments
/// * `state` - Controller state
/// * `slot_id` - Device slot ID (1-based)
/// * `setup` - USB Setup Packet
/// * `data_buf_phys` - Physical address of data buffer (0 if no data stage)
/// * `data_len` - Length of data transfer (0 if no data stage)
/// * `direction_in` - true for device-to-host (IN), false for host-to-device (OUT)
fn control_transfer(
    state: &XhciState,
    slot_id: u8,
    setup: &SetupPacket,
    data_buf_phys: u64,
    data_len: u16,
    direction_in: bool,
) -> Result<(), &'static str> {
    let slot_idx = (slot_id - 1) as usize;

    // Setup Stage TRB
    // The setup packet (8 bytes) is inlined in the TRB param field
    let setup_data: u64 = unsafe {
        core::ptr::read_unaligned(setup as *const SetupPacket as *const u64)
    };

    // TRT (Transfer Type): 0=No Data, 2=OUT Data, 3=IN Data
    let trt: u32 = if data_len == 0 {
        0
    } else if direction_in {
        3
    } else {
        2
    };

    let setup_trb = Trb {
        param: setup_data,
        status: 8, // Transfer length = 8 (setup packet size)
        // Setup Stage: TRB type = 2, IDT (Immediate Data) bit 6, TRT bits 17:16
        control: (trb_type::SETUP_STAGE << 10) | (1 << 6) | (trt << 16),
    };
    enqueue_transfer(slot_idx, setup_trb);

    // Data Stage TRB (if any)
    if data_len > 0 {
        // Direction bit 16: 1 = IN (device-to-host), 0 = OUT
        let dir_bit: u32 = if direction_in { 1 << 16 } else { 0 };
        let data_trb = Trb {
            param: data_buf_phys,
            status: data_len as u32,
            control: (trb_type::DATA_STAGE << 10) | dir_bit,
        };
        enqueue_transfer(slot_idx, data_trb);
    }

    // Status Stage TRB
    // Direction is opposite of data stage (or IN if no data stage)
    let status_dir: u32 = if data_len == 0 || direction_in { 0 } else { 1 << 16 };
    let status_trb = Trb {
        param: 0,
        status: 0,
        // IOC (Interrupt On Completion) bit 5
        control: (trb_type::STATUS_STAGE << 10) | status_dir | (1 << 5),
    };
    enqueue_transfer(slot_idx, status_trb);

    // Ring doorbell for EP0 (DCI = 1 for the default control endpoint)
    ring_doorbell(state, slot_id, 1);

    // Wait for EP0 completion event, skipping stale interrupt endpoint events.
    // When interrupt TRBs are queued inline during enumeration, their CC=12
    // Transfer Events may arrive before our EP0 completion. We must skip them.
    loop {
        let event = wait_for_event(state)?;
        let trb_type_val = event.trb_type();

        if trb_type_val == trb_type::TRANSFER_EVENT {
            let ev_slot = event.slot_id();
            let ev_ep = ((event.control >> 16) & 0x1F) as u8;

            if ev_slot == slot_id && ev_ep == 1 {
                // This is our EP0 completion
                let cc = event.completion_code();
                if cc != completion_code::SUCCESS && cc != completion_code::SHORT_PACKET {
                    crate::serial_println!(
                        "[xhci] Control transfer failed: slot={} cc={}",
                        slot_id,
                        cc
                    );
                    // Any error on EP0 halts the endpoint (xHCI spec 4.10.2.2).
                    // Reset it so subsequent control transfers on this slot work.
                    reset_control_endpoint(state, slot_id);
                    return Err("XHCI control transfer failed");
                }
                return Ok(());
            }

            // Not our EP0 event — stale interrupt endpoint event, skip it
            crate::serial_println!(
                "[xhci] Skipping stale xfer event: slot={} ep={} cc={}",
                ev_slot, ev_ep, event.completion_code(),
            );
            continue;
        }

        // Command Completion shouldn't arrive here, skip
        continue;
    }
}

/// Get the device descriptor from a USB device.
/// Read the first 8 bytes of the device descriptor.
///
/// Linux reads 8 bytes first to learn bMaxPacketSize0, then sends
/// SET_ISOCH_DELAY before reading the full 18-byte descriptor.
fn get_device_descriptor_short(
    state: &XhciState,
    slot_id: u8,
) -> Result<(), &'static str> {
    let setup = SetupPacket {
        bm_request_type: 0x80,
        b_request: request::GET_DESCRIPTOR,
        w_value: (descriptor_type::DEVICE as u16) << 8,
        w_index: 0,
        w_length: 8,
    };

    unsafe {
        let data_buf = &raw mut CTRL_DATA_BUF;
        core::ptr::write_bytes((*data_buf).0.as_mut_ptr(), 0, 18);
        dma_cache_clean((*data_buf).0.as_ptr(), 18);

        let data_phys = virt_to_phys(&raw const CTRL_DATA_BUF as u64);
        control_transfer(state, slot_id, &setup, data_phys, 8, true)?;
        dma_cache_invalidate((*data_buf).0.as_ptr(), 8);

        crate::serial_println!(
            "[xhci] Device descriptor (8B): maxpkt0={}",
            (*data_buf).0[7],
        );
    }

    Ok(())
}

/// Read the full 18-byte device descriptor.
fn get_device_descriptor(
    state: &XhciState,
    slot_id: u8,
    buf: &mut [u8; 18],
) -> Result<(), &'static str> {
    let setup = SetupPacket {
        bm_request_type: 0x80,
        b_request: request::GET_DESCRIPTOR,
        w_value: (descriptor_type::DEVICE as u16) << 8,
        w_index: 0,
        w_length: 18,
    };

    unsafe {
        let data_buf = &raw mut CTRL_DATA_BUF;
        core::ptr::write_bytes((*data_buf).0.as_mut_ptr(), 0, 18);
        dma_cache_clean((*data_buf).0.as_ptr(), 18);

        let data_phys = virt_to_phys(&raw const CTRL_DATA_BUF as u64);
        control_transfer(state, slot_id, &setup, data_phys, 18, true)?;
        dma_cache_invalidate((*data_buf).0.as_ptr(), 18);

        buf.copy_from_slice(&(&(*data_buf).0)[..18]);
    }

    let desc = unsafe { &*(buf.as_ptr() as *const DeviceDescriptor) };
    let bcd_usb = desc.bcd_usb;
    let id_vendor = desc.id_vendor;
    let id_product = desc.id_product;
    crate::serial_println!(
        "[xhci] Device descriptor: USB{}.{} class={:#04x} subclass={:#04x} protocol={:#04x} vendor={:#06x} product={:#06x} maxpkt0={}",
        bcd_usb >> 8,
        (bcd_usb >> 4) & 0xF,
        desc.b_device_class,
        desc.b_device_sub_class,
        desc.b_device_protocol,
        id_vendor,
        id_product,
        desc.b_max_packet_size0,
    );

    Ok(())
}

/// Send USB 3.0 SET_ISOCH_DELAY request (bRequest=0x31).
///
/// Linux sends this immediately after the first 8-byte device descriptor read.
/// wValue = isochronous delay in nanoseconds (Linux uses 40ns = 0x0028).
/// This is a USB 3.0 standard request that may be required by the Parallels
/// virtual xHC for proper endpoint activation.
fn set_isoch_delay(
    state: &XhciState,
    slot_id: u8,
) -> Result<(), &'static str> {
    let setup = SetupPacket {
        bm_request_type: 0x00, // Host-to-device, Standard, Device
        b_request: request::SET_ISOCH_DELAY,
        w_value: 0x0028, // 40 nanoseconds (matches Linux)
        w_index: 0,
        w_length: 0,
    };

    control_transfer(state, slot_id, &setup, 0, 0, false)?;
    crate::serial_println!("[xhci] SET_ISOCH_DELAY(40ns) sent to slot {}", slot_id);
    Ok(())
}

/// Read string descriptors from the device, matching Linux's enumeration.
///
/// Linux reads string descriptors #0 (language IDs), #2 (Product), #1 (Manufacturer),
/// and #3 (Serial Number) during enumeration BEFORE ConfigureEndpoint. The Parallels
/// virtual xHC may require this full enumeration sequence before interrupt endpoints
/// are armed. The string index values come from the device descriptor fields
/// iManufacturer, iProduct, iSerialNumber.
fn read_string_descriptors(
    state: &XhciState,
    slot_id: u8,
    i_manufacturer: u8,
    i_product: u8,
    i_serial: u8,
) {
    // Read string descriptor #0 (supported languages) first
    let indices = [0u8, i_product, i_manufacturer, i_serial];
    for &idx in &indices {
        if idx == 0 && indices[0] != 0 {
            // Only skip if not the language ID descriptor itself
            continue;
        }
        // For string index 0, use wIndex=0. For others, use 0x0409 (English US)
        let lang_id: u16 = if idx == 0 { 0 } else { 0x0409 };
        let setup = SetupPacket {
            bm_request_type: 0x80,
            b_request: request::GET_DESCRIPTOR,
            w_value: ((descriptor_type::STRING as u16) << 8) | (idx as u16),
            w_index: lang_id,
            w_length: 255,
        };

        unsafe {
            let data_buf = &raw mut CTRL_DATA_BUF;
            core::ptr::write_bytes((*data_buf).0.as_mut_ptr(), 0, 255);
            dma_cache_clean((*data_buf).0.as_ptr(), 255);
            let data_phys = virt_to_phys(&raw const CTRL_DATA_BUF as u64);
            match control_transfer(state, slot_id, &setup, data_phys, 255, true) {
                Ok(()) => {
                    dma_cache_invalidate((*data_buf).0.as_ptr(), 4);
                    let actual_len = (*data_buf).0[0] as usize;
                    crate::serial_println!(
                        "[xhci] String descriptor #{}: {} bytes",
                        idx, actual_len,
                    );
                }
                Err(_) => {
                    crate::serial_println!(
                        "[xhci] String descriptor #{} failed (non-fatal)", idx,
                    );
                }
            }
        }
    }
}

/// Read the BOS (Binary Object Store) descriptor from a USB 3.0 device.
///
/// Linux reads this after the full device descriptor and before the config descriptor.
/// The BOS descriptor contains USB 3.0 device capabilities.
fn get_bos_descriptor(
    state: &XhciState,
    slot_id: u8,
) -> Result<(), &'static str> {
    unsafe {
        // First read: 5-byte BOS header to get wTotalLength
        let data_buf = &raw mut CTRL_DATA_BUF;
        core::ptr::write_bytes((*data_buf).0.as_mut_ptr(), 0, 64);
        dma_cache_clean((*data_buf).0.as_ptr(), 64);

        let data_phys = virt_to_phys(&raw const CTRL_DATA_BUF as u64);

        let setup_header = SetupPacket {
            bm_request_type: 0x80,
            b_request: request::GET_DESCRIPTOR,
            w_value: (descriptor_type::BOS as u16) << 8,
            w_index: 0,
            w_length: 5,
        };

        control_transfer(state, slot_id, &setup_header, data_phys, 5, true)?;

        dma_cache_invalidate((*data_buf).0.as_ptr(), 5);
        let total_len = u16::from_le_bytes([(*data_buf).0[2], (*data_buf).0[3]]) as usize;
        let num_caps = (*data_buf).0[4];

        crate::serial_println!(
            "[xhci] BOS descriptor: total_length={} num_device_caps={}",
            total_len, num_caps,
        );

        // Second read: full BOS descriptor
        if total_len > 5 && total_len <= 64 {
            core::ptr::write_bytes((*data_buf).0.as_mut_ptr(), 0, total_len);
            dma_cache_clean((*data_buf).0.as_ptr(), total_len);

            let setup_full = SetupPacket {
                bm_request_type: 0x80,
                b_request: request::GET_DESCRIPTOR,
                w_value: (descriptor_type::BOS as u16) << 8,
                w_index: 0,
                w_length: total_len as u16,
            };

            control_transfer(state, slot_id, &setup_full, data_phys, total_len as u16, true)?;
            dma_cache_invalidate((*data_buf).0.as_ptr(), total_len);

            crate::serial_println!(
                "[xhci] BOS descriptor read complete ({} bytes)",
                total_len,
            );
        }
    }

    Ok(())
}

/// Get the configuration descriptor (and all subordinate descriptors) from a USB device.
fn get_config_descriptor(
    state: &XhciState,
    slot_id: u8,
    buf: &mut [u8; 256],
) -> Result<usize, &'static str> {
    // First, read just the 9-byte config descriptor header to get wTotalLength
    let setup_header = SetupPacket {
        bm_request_type: 0x80,
        b_request: request::GET_DESCRIPTOR,
        w_value: (descriptor_type::CONFIGURATION as u16) << 8,
        w_index: 0,
        w_length: 9,
    };

    unsafe {
        let data_buf = &raw mut CTRL_DATA_BUF;
        core::ptr::write_bytes((*data_buf).0.as_mut_ptr(), 0, 256);
        dma_cache_clean((*data_buf).0.as_ptr(), 256);

        let data_phys = virt_to_phys(&raw const CTRL_DATA_BUF as u64);
        control_transfer(state, slot_id, &setup_header, data_phys, 9, true)?;

        dma_cache_invalidate((*data_buf).0.as_ptr(), 9);

        let config_desc = &*((*data_buf).0.as_ptr() as *const ConfigDescriptor);
        let total_len = config_desc.w_total_length as usize;

        crate::serial_println!(
            "[xhci] Config descriptor: total_length={} num_interfaces={} config_value={}",
            total_len,
            config_desc.b_num_interfaces,
            config_desc.b_configuration_value,
        );

        if total_len > 256 {
            crate::serial_println!("[xhci] Config descriptor too large ({} bytes), truncating", total_len);
        }

        let fetch_len = total_len.min(256) as u16;

        // Now read the full configuration descriptor set
        let setup_full = SetupPacket {
            bm_request_type: 0x80,
            b_request: request::GET_DESCRIPTOR,
            w_value: (descriptor_type::CONFIGURATION as u16) << 8,
            w_index: 0,
            w_length: fetch_len,
        };

        core::ptr::write_bytes((*data_buf).0.as_mut_ptr(), 0, 256);
        dma_cache_clean((*data_buf).0.as_ptr(), 256);

        control_transfer(state, slot_id, &setup_full, data_phys, fetch_len, true)?;

        dma_cache_invalidate((*data_buf).0.as_ptr(), fetch_len as usize);

        buf[..fetch_len as usize].copy_from_slice(&(&(*data_buf).0)[..fetch_len as usize]);
        Ok(fetch_len as usize)
    }
}

/// Send SET_CONFIGURATION request to select a configuration.
fn set_configuration(
    state: &XhciState,
    slot_id: u8,
    config_value: u8,
) -> Result<(), &'static str> {
    let setup = SetupPacket {
        bm_request_type: 0x00, // Host-to-device, standard, device
        b_request: request::SET_CONFIGURATION,
        w_value: config_value as u16,
        w_index: 0,
        w_length: 0,
    };

    control_transfer(state, slot_id, &setup, 0, 0, false)?;
    crate::serial_println!("[xhci] Set configuration {} on slot {}", config_value, slot_id);
    Ok(())
}

/// Send SET_INTERFACE request to select an alternate setting for an interface.
///
/// Linux's USB core sends SET_INTERFACE(alt=0) for each interface during driver
/// probe. Parallels' virtual USB device model may require this to activate the
/// interface's interrupt endpoints.
#[allow(dead_code)]
fn set_interface(
    state: &XhciState,
    slot_id: u8,
    interface: u8,
    alt_setting: u8,
) -> Result<(), &'static str> {
    let setup = SetupPacket {
        bm_request_type: 0x01, // Host-to-device, standard, interface
        b_request: request::SET_INTERFACE,
        w_value: alt_setting as u16,
        w_index: interface as u16,
        w_length: 0,
    };

    control_transfer(state, slot_id, &setup, 0, 0, false)?;
    crate::serial_println!(
        "[xhci] SET_INTERFACE(alt={}) on slot {} iface {}",
        alt_setting, slot_id, interface,
    );
    Ok(())
}

/// Send SET_IDLE request to a HID interface (duration=0 = indefinite).
fn set_idle(
    state: &XhciState,
    slot_id: u8,
    interface: u8,
) -> Result<(), &'static str> {
    let setup = SetupPacket {
        bm_request_type: 0x21, // Host-to-device, class, interface
        b_request: hid_request::SET_IDLE,
        w_value: 0, // Duration = 0 (indefinite), Report ID = 0
        w_index: interface as u16,
        w_length: 0,
    };

    control_transfer(state, slot_id, &setup, 0, 0, false)?;
    Ok(())
}

/// Send SET_REPORT(Output) to clear LED indicators on a HID keyboard interface.
///
/// Linux sends this during keyboard init: SET_REPORT(Output, Report ID=0, data=0x00).
/// This clears NumLock/CapsLock/ScrollLock LEDs and may be required for proper
/// interrupt endpoint activation on some virtual controllers.
fn set_report_leds(
    state: &XhciState,
    slot_id: u8,
    interface: u8,
) -> Result<(), &'static str> {
    // SET_REPORT: bmRequestType=0x21 (H2D, Class, Interface),
    //   bRequest=0x09, wValue=0x0200 (Output report, ID=0), wLength=1
    let setup = SetupPacket {
        bm_request_type: 0x21,
        b_request: hid_request::SET_REPORT,
        w_value: 0x0200, // Report Type = Output (2), Report ID = 0
        w_index: interface as u16,
        w_length: 1,
    };

    // Data stage: 1 byte = 0x00 (all LEDs off)
    unsafe {
        let data_buf = &raw mut CTRL_DATA_BUF;
        (*data_buf).0[0] = 0x00;
        dma_cache_clean((*data_buf).0.as_ptr(), 1);
    }
    let data_phys = virt_to_phys((&raw const CTRL_DATA_BUF) as u64);

    control_transfer(state, slot_id, &setup, data_phys, 1, false)?;
    Ok(())
}

/// Send GET_REPORT(Feature) to a HID interface, matching Linux's init sequence.
///
/// Linux's HID driver reads Feature reports during probe. The Parallels virtual
/// xHC may require this to "arm" interrupt endpoints — without it, interrupt
/// transfers return CC=12 (Endpoint Not Enabled).
///
/// If the device doesn't support the requested Feature report, it will STALL.
/// We handle the STALL gracefully (return Err).
fn get_report_feature(
    state: &XhciState,
    slot_id: u8,
    interface: u8,
    report_id: u8,
) -> Result<(), &'static str> {
    // GET_REPORT: bmRequestType=0xA1 (D2H, Class, Interface),
    //   bRequest=0x01, wValue=(ReportType << 8) | ReportID
    //   ReportType: 1=Input, 2=Output, 3=Feature
    let setup = SetupPacket {
        bm_request_type: 0xA1, // Device-to-host, class, interface
        b_request: hid_request::GET_REPORT,
        w_value: (3u16 << 8) | (report_id as u16), // Feature report
        w_index: interface as u16,
        w_length: 64, // Max length (device may return less)
    };

    unsafe {
        let data_buf = &raw mut CTRL_DATA_BUF;
        core::ptr::write_bytes((*data_buf).0.as_mut_ptr(), 0, 64);
        dma_cache_clean((*data_buf).0.as_ptr(), 64);
    }
    let data_phys = virt_to_phys((&raw const CTRL_DATA_BUF) as u64);

    control_transfer(state, slot_id, &setup, data_phys, 64, true)?;
    Ok(())
}

/// Fetch and log the HID Report Descriptor for diagnostic purposes.
///
/// The Report Descriptor reveals the actual report format: whether Report IDs
/// are used, the field layout, and the report size. This is critical for
/// understanding what data the interrupt endpoint delivers.
fn fetch_hid_report_descriptor(state: &XhciState, slot_id: u8, interface: u8, exact_len: u16) {
    // Use exact length from HID descriptor if available, fall back to 128
    let req_len = if exact_len > 0 && exact_len <= 256 { exact_len } else { 128 };

    let setup = SetupPacket {
        bm_request_type: 0x81, // Device-to-host, standard, interface
        b_request: request::GET_DESCRIPTOR,
        w_value: (descriptor_type::HID_REPORT as u16) << 8, // Report descriptor type
        w_index: interface as u16,
        w_length: req_len,
    };

    unsafe {
        let data_buf = &raw mut CTRL_DATA_BUF;
        core::ptr::write_bytes((*data_buf).0.as_mut_ptr(), 0, req_len as usize);
        dma_cache_clean((*data_buf).0.as_ptr(), req_len as usize);

        let data_phys = virt_to_phys(&raw const CTRL_DATA_BUF as u64);

        match control_transfer(state, slot_id, &setup, data_phys, req_len, true) {
            Ok(()) => {
                dma_cache_invalidate((*data_buf).0.as_ptr(), req_len as usize);
                let buf = &(*data_buf).0;

                // Find actual length (trim trailing zeros)
                let mut len = req_len as usize;
                while len > 0 && buf[len - 1] == 0 {
                    len -= 1;
                }

                crate::serial_println!(
                    "[xhci] HID Report Descriptor (iface {}, {} bytes):",
                    interface, len
                );

                // Print in hex, 16 bytes per line
                let mut i = 0;
                while i < len {
                    let end = if i + 16 < len { i + 16 } else { len };
                    let mut hex_buf = [0u8; 48]; // 16 * 3 = 48
                    let mut pos = 0;
                    for j in i..end {
                        let hi = buf[j] >> 4;
                        let lo = buf[j] & 0x0F;
                        hex_buf[pos] = if hi < 10 { b'0' + hi } else { b'a' + hi - 10 };
                        pos += 1;
                        hex_buf[pos] = if lo < 10 { b'0' + lo } else { b'a' + lo - 10 };
                        pos += 1;
                        hex_buf[pos] = b' ';
                        pos += 1;
                    }
                    // Convert to str for serial_println
                    if let Ok(s) = core::str::from_utf8(&hex_buf[..pos]) {
                        crate::serial_println!("  {}", s);
                    }
                    i += 16;
                }
            }
            Err(e) => {
                crate::serial_println!(
                    "[xhci] Failed to get HID Report Descriptor (iface {}): {}",
                    interface, e
                );
            }
        }
    }
}

// =============================================================================
// Endpoint Configuration (Configure Endpoint Command)
// =============================================================================

/// Endpoint info collected during descriptor walk for batch ConfigureEndpoint.
struct PendingEp {
    dci: u8,
    hid_idx: usize,
    max_pkt: u16,
    b_interval: u8,
    ss_max_burst: u8,
    ss_bytes_per_interval: u16,
}

/// Configure all HID interrupt endpoints in a single ConfigureEndpoint command.
///
/// Linux's xHCI driver (`xhci_check_bandwidth`) issues ONE ConfigureEndpoint
/// with ALL endpoints from the configuration. Parallels' virtual xHCI does not
/// handle incremental ConfigureEndpoint commands correctly — a second
/// ConfigureEndpoint adding DCI 5 silently fails to activate the endpoint
/// even though CC=SUCCESS is returned. Batching all endpoints matches Linux
/// and works around this emulation limitation.
///
/// After the initial batch ConfigureEndpoint, performs a "bandwidth dance"
/// (StopEndpoint + re-ConfigureEndpoint per endpoint) matching Linux's
/// xhci_check_bandwidth() behavior. The re-ConfigureEndpoint uses a SEPARATE
/// Input Context buffer (RECONFIG_INPUT_CTX) with ALL endpoint Add flags set,
/// matching Linux's observed behavior where the full Input Context is reused.
fn configure_endpoints_batch(
    state: &XhciState,
    slot_id: u8,
    endpoints: &[Option<PendingEp>; 4],
    ep_count: usize,
) -> Result<(), &'static str> {
    if ep_count == 0 {
        return Ok(());
    }

    let slot_idx = (slot_id - 1) as usize;
    let ctx_size = state.context_size;

    unsafe {
        // Zero and rebuild the input context
        let input_ctx = &raw mut INPUT_CONTEXTS[slot_idx];
        core::ptr::write_bytes((*input_ctx).0.as_mut_ptr(), 0, 4096);
        let input_base = (*input_ctx).0.as_mut_ptr();

        // Build Add flags: A0 (Slot Context) + A[dci] for each endpoint
        let mut add_flags: u32 = 1; // A0 = Slot Context
        let mut max_dci: u32 = 0;
        for i in 0..ep_count {
            if let Some(ref ep) = endpoints[i] {
                add_flags |= 1u32 << ep.dci;
                if (ep.dci as u32) > max_dci {
                    max_dci = ep.dci as u32;
                }
            }
        }
        core::ptr::write_volatile(input_base.add(0x04) as *mut u32, add_flags);

        crate::serial_println!(
            "[xhci] ConfigureEndpoint(batch): slot={} add_flags={:#010x} max_dci={}",
            slot_id, add_flags, max_dci,
        );

        // Slot Context: copy DW0-DW2 from device output context, zero DW3.
        // Linux's xhci_slot_copy() copies DW0 (dev_info), DW1 (dev_info2),
        // DW2 (tt_info), but explicitly zeroes DW3 (dev_state = 0).
        // DW3 contains USB Device Address and Slot State which the xHCI spec
        // says "should" be 0 in the Input Context for ConfigureEndpoint.
        // DW4-DW7 (reserved) are also zeroed.
        let slot_ctx = input_base.add(ctx_size);
        let dev_ctx = &raw const DEVICE_CONTEXTS[slot_idx];
        dma_cache_invalidate((*dev_ctx).0.as_ptr(), 4096);
        // Copy DW0, DW1, DW2 from output context
        for dw_offset in (0..12).step_by(4) {
            let val = core::ptr::read_volatile(
                (*dev_ctx).0.as_ptr().add(dw_offset) as *const u32,
            );
            core::ptr::write_volatile(slot_ctx.add(dw_offset) as *mut u32, val);
        }
        // DW3 = 0 (zero Address and Slot State, matching Linux)
        core::ptr::write_volatile(slot_ctx.add(12) as *mut u32, 0u32);
        // DW4-DW7 = 0 (reserved)
        for dw_offset in (16..32).step_by(4) {
            core::ptr::write_volatile(slot_ctx.add(dw_offset) as *mut u32, 0u32);
        }
        // Update CtxEntries in DW0
        let current_slot_dw0 = core::ptr::read_volatile(slot_ctx as *const u32);
        let current_entries = (current_slot_dw0 >> 27) & 0x1F;
        let new_entries = current_entries.max(max_dci);
        let new_slot_dw0 = (current_slot_dw0 & !(0x1F << 27)) | (new_entries << 27);
        core::ptr::write_volatile(slot_ctx as *mut u32, new_slot_dw0);

        // Diagnostic: dump Slot Context DW4-DW7 (reserved fields) from device output
        {
            let dw4 = core::ptr::read_volatile(
                (*dev_ctx).0.as_ptr().add(16) as *const u32,
            );
            let dw5 = core::ptr::read_volatile(
                (*dev_ctx).0.as_ptr().add(20) as *const u32,
            );
            let dw6 = core::ptr::read_volatile(
                (*dev_ctx).0.as_ptr().add(24) as *const u32,
            );
            let dw7 = core::ptr::read_volatile(
                (*dev_ctx).0.as_ptr().add(28) as *const u32,
            );
            if dw4 != 0 || dw5 != 0 || dw6 != 0 || dw7 != 0 {
                crate::serial_println!(
                    "[xhci] Slot {} reserved DW4-7: {:#010x} {:#010x} {:#010x} {:#010x}",
                    slot_id, dw4, dw5, dw6, dw7,
                );
            }
        }

        // Fill in each endpoint context
        for i in 0..ep_count {
            if let Some(ref ep) = endpoints[i] {
                let ring_idx = HID_RING_BASE + ep.hid_idx;
                let ep_ctx = input_base.add((1 + ep.dci as usize) * ctx_size);
                let max_pkt = (ep.max_pkt & 0x07FF) as u32;
                let max_burst = ep.ss_max_burst as u32;

                // EP DW0: Interval, Max ESIT Payload Hi
                // SuperSpeed (port_speed >= 3): interval = bInterval - 1
                // Full/Low speed: convert ms to 125us exponent
                let port_speed = {
                    let dev_ctx = &raw const DEVICE_CONTEXTS[slot_idx];
                    let slot_dw0 = core::ptr::read_volatile((*dev_ctx).0.as_ptr() as *const u32);
                    (slot_dw0 >> 20) & 0xF
                };
                let interval: u32 = if port_speed >= 3 {
                    let bi = ep.b_interval.clamp(1, 16);
                    (bi - 1) as u32
                } else {
                    let bi = ep.b_interval.clamp(1, 255);
                    let ms_interval = bi as u32;
                    let mut n = 0u32;
                    while (125u32 << n) < ms_interval * 1000 && n < 15 {
                        n += 1;
                    }
                    n
                };
                let esit_payload = if ep.ss_bytes_per_interval > 0 {
                    ep.ss_bytes_per_interval as u32
                } else {
                    max_pkt * (max_burst + 1)
                };
                let esit_hi = (esit_payload >> 16) & 0xFF;
                // Mult=1 (bits 9:8) matching Linux's Input Context for this Parallels
                // virtual xHC. Despite xHCI spec §6.2.3 saying Mult should be 0 for
                // non-isoch, Linux sets Mult=1 and the Parallels vxHC requires it.
                // Confirmed via byte-for-byte Input Context dump: Linux DW0=0x00030100.
                let mult: u32 = 1;
                let ep_dw0: u32 = (esit_hi << 24) | (interval << 16) | (mult << 8);
                core::ptr::write_volatile(ep_ctx as *mut u32, ep_dw0);

                // EP DW1: CErr=3, EP Type = Interrupt IN (7)
                // Bulk IN (6) was tested and also produces CC=12 — not type-specific.
                let ep_type: u32 = 7;
                let cerr: u32 = 3;
                let ep_dw1: u32 = (max_pkt << 16) | (max_burst << 8) | (ep_type << 3) | (cerr << 1);
                core::ptr::write_volatile(ep_ctx.add(0x04) as *mut u32, ep_dw1);

                // Clear and initialize transfer ring
                let ring = &raw mut TRANSFER_RINGS[ring_idx];
                core::ptr::write_bytes(ring as *mut u8, 0, TRANSFER_RING_SIZE * 16);
                TRANSFER_ENQUEUE[ring_idx] = 0;
                TRANSFER_CYCLE[ring_idx] = true;

                // Initialize Link TRB at the end of the ring (matching Linux).
                // Linux's xhci_set_link_trb() places a Link TRB at the last entry
                // of each ring segment during ring allocation, BEFORE ConfigureEndpoint.
                // The Parallels vxHC may validate ring structure on ConfigureEndpoint.
                let ring_phys_for_link = virt_to_phys(&raw const TRANSFER_RINGS[ring_idx] as u64);
                let link_trb = Trb {
                    param: ring_phys_for_link,
                    status: 0,
                    // Link TRB type=6, TC (Toggle Cycle) bit 1, cycle=1 (matches DCS)
                    control: (trb_type::LINK << 10) | (1 << 1) | 1,
                };
                core::ptr::write_volatile(
                    &mut (*ring)[TRANSFER_RING_SIZE - 1] as *mut Trb,
                    link_trb,
                );

                // Cache-clean the entire transfer ring including Link TRB.
                dma_cache_clean(
                    &TRANSFER_RINGS[ring_idx] as *const [Trb; TRANSFER_RING_SIZE] as *const u8,
                    TRANSFER_RING_SIZE * 16,
                );

                let ring_phys = virt_to_phys(&raw const TRANSFER_RINGS[ring_idx] as u64);

                // EP DW2-DW3: TR Dequeue Pointer with DCS = 1
                core::ptr::write_volatile(
                    ep_ctx.add(0x08) as *mut u32,
                    (ring_phys as u32) | 1,
                );
                core::ptr::write_volatile(
                    ep_ctx.add(0x0C) as *mut u32,
                    (ring_phys >> 32) as u32,
                );

                // EP DW4: Average TRB Length + Max ESIT Payload Lo
                let esit_lo = esit_payload & 0xFFFF;
                let avg_trb_len = esit_payload;
                let ep_dw4: u32 = (esit_lo << 16) | avg_trb_len;
                core::ptr::write_volatile(ep_ctx.add(0x10) as *mut u32, ep_dw4);

                crate::serial_println!(
                    "[xhci]   EP DCI={}: type=Intr_IN maxpkt={} interval={} ring={:#x}",
                    ep.dci, max_pkt, interval, ring_phys,
                );
            }
        }

        // Cache-clean the entire input context
        dma_cache_clean(input_base, 4096);

        // DIAGNOSTIC: Hex-dump the Input Context that the xHC will read.
        // This verifies that our cache-cleaned memory matches what we intended.
        // Read back via invalidate to see what physical memory actually contains.
        dma_cache_invalidate(input_base as *const u8, 4096);
        crate::serial_println!("[xhci] Input Context hex dump for slot {} (ctx_size={}):", slot_id, ctx_size);
        // Input Control Context (first 32 bytes)
        {
            let dw0 = core::ptr::read_volatile(input_base as *const u32);
            let dw1 = core::ptr::read_volatile(input_base.add(4) as *const u32);
            crate::serial_println!("  ICC: DW0(drop)={:#010x} DW1(add)={:#010x}", dw0, dw1);
        }
        // Slot Context (at ctx_size offset)
        {
            let sc = input_base.add(ctx_size);
            let dw0 = core::ptr::read_volatile(sc as *const u32);
            let dw1 = core::ptr::read_volatile(sc.add(4) as *const u32);
            let dw2 = core::ptr::read_volatile(sc.add(8) as *const u32);
            let dw3 = core::ptr::read_volatile(sc.add(12) as *const u32);
            crate::serial_println!("  Slot: DW0={:#010x} DW1={:#010x} DW2={:#010x} DW3={:#010x}", dw0, dw1, dw2, dw3);
        }
        // Each endpoint context
        for i in 0..ep_count {
            if let Some(ref ep) = endpoints[i] {
                let ec = input_base.add((1 + ep.dci as usize) * ctx_size);
                let dw0 = core::ptr::read_volatile(ec as *const u32);
                let dw1 = core::ptr::read_volatile(ec.add(4) as *const u32);
                let dw2 = core::ptr::read_volatile(ec.add(8) as *const u32);
                let dw3 = core::ptr::read_volatile(ec.add(12) as *const u32);
                let dw4 = core::ptr::read_volatile(ec.add(16) as *const u32);
                crate::serial_println!(
                    "  EP DCI={}: DW0={:#010x} DW1={:#010x} DW2={:#010x} DW3={:#010x} DW4={:#010x}",
                    ep.dci, dw0, dw1, dw2, dw3, dw4,
                );
                // Also dump the first TRB at the TR Dequeue Pointer location
                let ring_idx = HID_RING_BASE + ep.hid_idx;
                let trb0 = core::ptr::read_volatile(&TRANSFER_RINGS[ring_idx][0]);
                let ring_phys = virt_to_phys(&raw const TRANSFER_RINGS[ring_idx] as u64);
                let ring_virt = &raw const TRANSFER_RINGS[ring_idx] as u64;
                crate::serial_println!(
                    "    Ring[{}] virt={:#x} phys={:#x} TRB0: param={:#010x} status={:#010x} control={:#010x}",
                    ring_idx, ring_virt, ring_phys, trb0.param, trb0.status, trb0.control,
                );
            }
        }

        // Issue batch ConfigureEndpoint using INPUT_CONTEXTS directly.
        let input_ctx_phys = virt_to_phys(&raw const INPUT_CONTEXTS[slot_idx] as u64);
        {
            let trb = Trb {
                param: input_ctx_phys,
                status: 0,
                control: (trb_type::CONFIGURE_ENDPOINT << 10) | ((slot_id as u32) << 24),
            };
            enqueue_command(trb);
            ring_doorbell(state, 0, 0);

            let event = wait_for_command(state)?;
            let cc = event.completion_code();
            if cc != completion_code::SUCCESS {
                crate::serial_println!(
                    "[xhci] ConfigureEndpoint failed: slot={} cc={}",
                    slot_id, cc,
                );
                return Err("XHCI ConfigureEndpoint failed");
            }
        }

        // Bandwidth dance: StopEndpoint + re-ConfigureEndpoint per endpoint.
        //
        // Linux's xhci_check_bandwidth() issues the batch ConfigureEndpoint
        // above, then for each endpoint does: StopEndpoint, followed by a
        // re-ConfigureEndpoint. This is required by the Parallels virtual xHC —
        // the initial batch ConfigureEndpoint shows endpoints in Running state
        // (output context), but the xHC returns CC=12 (Endpoint Not Enabled)
        // on interrupt transfers unless the bandwidth dance is performed.
        //
        // CRITICAL: Linux uses a DIFFERENT Input Context physical address for
        // re-ConfigureEndpoint commands. The Parallels vxHC appears to cache or
        // ignore re-ConfigureEndpoint commands that use the same Input Context
        // address as the initial ConfigureEndpoint. We use the separate
        // RECONFIG_INPUT_CTX buffer to ensure a distinct address.
        //
        // The re-ConfigureEndpoint uses ALL-EP add_flags (same as initial) and
        // the same endpoint contexts. Only the Slot Context is refreshed from
        // the Output Context.
        if !SKIP_BW_DANCE {
            // Build RECONFIG_INPUT_CTX once: copy initial Input Context, then
            // refresh Slot Context from Output Context (xHC may have updated it).
            let reconfig = &raw mut RECONFIG_INPUT_CTX;
            let src = &raw const INPUT_CONTEXTS[slot_idx];
            core::ptr::copy_nonoverlapping(
                (*src).0.as_ptr(),
                (*reconfig).0.as_mut_ptr(),
                4096,
            );
            // Refresh Slot Context (DW0-DW2) from Output Context, zero DW3
            let reconfig_base = (*reconfig).0.as_mut_ptr();
            let rc_slot = reconfig_base.add(ctx_size);
            for dw_offset in (0..12).step_by(4) {
                let val = core::ptr::read_volatile(
                    (*dev_ctx).0.as_ptr().add(dw_offset) as *const u32,
                );
                core::ptr::write_volatile(rc_slot.add(dw_offset) as *mut u32, val);
            }
            core::ptr::write_volatile(rc_slot.add(12) as *mut u32, 0u32);
            // Update ctx_entries in DW0
            let rc_dw0 = core::ptr::read_volatile(rc_slot as *const u32);
            let rc_entries = (rc_dw0 >> 27) & 0x1F;
            let new_entries = rc_entries.max(max_dci);
            let new_dw0 = (rc_dw0 & !(0x1F << 27)) | (new_entries << 27);
            core::ptr::write_volatile(rc_slot as *mut u32, new_dw0);

            dma_cache_clean(reconfig_base, 4096);

            let reconfig_phys = virt_to_phys(&raw const RECONFIG_INPUT_CTX as u64);

            crate::serial_println!(
                "[xhci] BW dance: ep_count={} slot={} reconfig_phys={:#x}",
                ep_count, slot_id, reconfig_phys,
            );

            for i in 0..ep_count {
                if let Some(ref ep) = endpoints[i] {
                    let dci = ep.dci;

                    // Step 1: Stop Endpoint
                    let stop_trb = Trb {
                        param: 0,
                        status: 0,
                        control: (trb_type::STOP_ENDPOINT << 10)
                            | ((slot_id as u32) << 24)
                            | ((dci as u32) << 16),
                    };
                    enqueue_command(stop_trb);
                    ring_doorbell(state, 0, 0);
                    let stop_event = wait_for_command(state)?;
                    let stop_cc = stop_event.completion_code();
                    crate::serial_println!(
                        "[xhci] BW dance: StopEP slot={} DCI={} cc={}",
                        slot_id, dci, stop_cc,
                    );

                    // Step 2: re-ConfigureEndpoint with RECONFIG_INPUT_CTX
                    // Uses ALL-EP add_flags and same endpoint contexts as initial.
                    // The different physical address is critical for Parallels vxHC.
                    let recfg_trb = Trb {
                        param: reconfig_phys,
                        status: 0,
                        control: (trb_type::CONFIGURE_ENDPOINT << 10)
                            | ((slot_id as u32) << 24),
                    };
                    enqueue_command(recfg_trb);
                    ring_doorbell(state, 0, 0);
                    let recfg_event = wait_for_command(state)?;
                    let recfg_cc = recfg_event.completion_code();
                    crate::serial_println!(
                        "[xhci] BW dance: re-ConfigEP slot={} DCI={} cc={}",
                        slot_id, dci, recfg_cc,
                    );
                }
            }
        }

        // Verify: read back device context after ConfigureEndpoint
        dma_cache_invalidate((*dev_ctx).0.as_ptr(), 4096);

        let slot_out_dw0 = core::ptr::read_volatile((*dev_ctx).0.as_ptr() as *const u32);
        let ctx_entries = (slot_out_dw0 >> 27) & 0x1F;
        let slot_out_dw3 = core::ptr::read_volatile((*dev_ctx).0.as_ptr().add(12) as *const u32);
        let slot_st = (slot_out_dw3 >> 27) & 0x1F;
        crate::serial_println!(
            "[xhci] Slot {} after ConfigureEndpoint: ctx_entries={} slot_state={}",
            slot_id, ctx_entries, slot_st,
        );

        for i in 0..ep_count {
            if let Some(ref ep) = endpoints[i] {
                let ep_out = (*dev_ctx).0.as_ptr().add((ep.dci as usize) * ctx_size);
                let ep_out_dw0 = core::ptr::read_volatile(ep_out as *const u32);
                let ep_state = ep_out_dw0 & 0x7;
                let ep_out_dw1 = core::ptr::read_volatile(ep_out.add(4) as *const u32);
                crate::serial_println!(
                    "[xhci]   DCI={}: state={} type={} DW0={:#010x}",
                    ep.dci, ep_state, (ep_out_dw1 >> 3) & 0x7, ep_out_dw0,
                );
                if ep_state == 0 {
                    crate::serial_println!(
                        "[xhci]   WARNING: DCI {} still Disabled after ConfigureEndpoint!",
                        ep.dci,
                    );
                }
            }
        }
    }

    Ok(())
}

// =============================================================================
// HID Configuration and Transfer Queueing
// =============================================================================

/// Parse configuration descriptor, find HID interfaces, configure endpoints,
/// and start polling for HID reports.
///
/// # Initialization Ordering (matches Linux xHCI driver)
///
/// Linux's `usb_set_configuration()` calls `xhci_check_bandwidth()` which issues
/// the ConfigureEndpoint xHCI command BEFORE sending SET_CONFIGURATION to the USB
/// device. This ensures the xHC has transfer rings ready before the device
/// activates its endpoints.
///
/// Phase 1: Walk descriptors, configure xHCI endpoints (ConfigureEndpoint command)
/// Phase 2: SET_CONFIGURATION (USB control transfer to device)
/// Phase 3: HID interface setup (SET_IDLE, GET_REPORT_DESCRIPTOR, SET_REPORT)
fn configure_hid(
    state: &mut XhciState,
    slot_id: u8,
    config_buf: &[u8],
    config_len: usize,
) -> Result<(), &'static str> {
    // Parse the configuration descriptor header
    if config_len < 9 {
        return Err("Config descriptor too short");
    }
    let config_desc = unsafe { &*(config_buf.as_ptr() as *const ConfigDescriptor) };
    let config_value = config_desc.b_configuration_value;

    // Info collected per HID interface during Phase 1, used in Phase 3
    struct HidIfaceInfo {
        interface_number: u8,
        is_keyboard: bool,
        is_nkro: bool, // Non-boot HID interface (subclass=0, Report ID protocol)
        dci: u8,
        hid_report_len: u16,   // wDescriptorLength from HID descriptor (0 = unknown)
    }
    let mut ifaces: [Option<HidIfaceInfo>; 4] = [None, None, None, None];
    let mut iface_count: usize = 0;
    let mut found_boot_keyboard = false;
    let mut found_mouse = false;

    // Pending endpoints for batch ConfigureEndpoint (one command for all EPs)
    let mut pending_eps: [Option<PendingEp>; 4] = [None, None, None, None];
    let mut ep_count: usize = 0;

    // =========================================================================
    // Phase 1: Walk descriptors and configure xHCI endpoints BEFORE SET_CONFIGURATION
    // =========================================================================
    let mut offset = config_desc.b_length as usize;

    while offset + 2 <= config_len {
        let desc_len = config_buf[offset] as usize;
        let desc_type = config_buf[offset + 1];

        if desc_len == 0 {
            break; // Prevent infinite loop
        }
        if offset + desc_len > config_len {
            break;
        }

        if desc_type == descriptor_type::INTERFACE && desc_len >= 9 {
            let iface = unsafe {
                &*(config_buf.as_ptr().add(offset) as *const InterfaceDescriptor)
            };

            if iface.b_interface_class == class_code::HID {
                let is_boot = iface.b_interface_sub_class == hid_subclass::BOOT;
                crate::serial_println!(
                    "[xhci] Found HID interface: number={} subclass={} protocol={} endpoints={}{}",
                    iface.b_interface_number,
                    iface.b_interface_sub_class,
                    iface.b_interface_protocol,
                    iface.b_num_endpoints,
                    if is_boot { " (boot)" } else { " (report)" },
                );

                // Parse HID descriptor (type 0x21) for wDescriptorLength.
                // The HID descriptor immediately follows the interface descriptor.
                let mut hid_report_len: u16 = 0;
                {
                    let mut hid_off = offset + desc_len;
                    while hid_off + 2 <= config_len {
                        let hd_len = config_buf[hid_off] as usize;
                        let hd_type = config_buf[hid_off + 1];
                        if hd_len == 0 || hid_off + hd_len > config_len { break; }
                        if hd_type == descriptor_type::INTERFACE || hd_type == descriptor_type::ENDPOINT {
                            break; // Past the HID descriptor
                        }
                        if hd_type == 0x21 && hd_len >= 9 {
                            // HID Descriptor: offset 7-8 = wDescriptorLength (Report Desc)
                            hid_report_len = u16::from_le_bytes([
                                config_buf[hid_off + 7],
                                config_buf[hid_off + 8],
                            ]);
                            crate::serial_println!(
                                "[xhci] HID descriptor: reportDescLen={}",
                                hid_report_len,
                            );
                            break;
                        }
                        hid_off += hd_len;
                    }
                }

                // Look for the interrupt IN endpoint following this interface
                let mut ep_offset = offset + desc_len;
                while ep_offset + 2 <= config_len {
                    let ep_len = config_buf[ep_offset] as usize;
                    let ep_type = config_buf[ep_offset + 1];

                    if ep_len == 0 {
                        break;
                    }
                    if ep_offset + ep_len > config_len {
                        break;
                    }

                    // Stop if we hit the next interface descriptor
                    if ep_type == descriptor_type::INTERFACE {
                        break;
                    }

                    if ep_type == descriptor_type::ENDPOINT && ep_len >= 7 {
                        let ep_desc = unsafe {
                            &*(config_buf.as_ptr().add(ep_offset) as *const EndpointDescriptor)
                        };

                        if ep_desc.is_interrupt() && ep_desc.is_in() {
                            // Check for SS Endpoint Companion Descriptor (type 0x30)
                            // immediately following this endpoint descriptor
                            let mut ss_max_burst: u8 = 0;
                            let mut ss_bytes_per_interval: u16 = 0;
                            let ss_offset = ep_offset + ep_len;
                            if ss_offset + 2 <= config_len {
                                let ss_len = config_buf[ss_offset] as usize;
                                let ss_type = config_buf[ss_offset + 1];
                                if ss_type == 0x30 && ss_len >= 6 && ss_offset + ss_len <= config_len {
                                    ss_max_burst = config_buf[ss_offset + 2];
                                    ss_bytes_per_interval = u16::from_le_bytes([
                                        config_buf[ss_offset + 4],
                                        config_buf[ss_offset + 5],
                                    ]);
                                    crate::serial_println!(
                                        "[xhci] SS EP Companion: maxBurst={} bytesPerInterval={}",
                                        ss_max_burst, ss_bytes_per_interval,
                                    );
                                }
                            }

                            // Determine HID device type and transfer ring index:
                            //   hid_idx 0 = boot keyboard (protocol=1, DCI 3)
                            //   hid_idx 1 = boot mouse (protocol=2)
                            //   hid_idx 2 = NKRO keyboard (subclass=0 protocol=0, DCI 5)
                            let (hid_idx, is_keyboard, is_nkro) =
                                if iface.b_interface_protocol == hid_protocol::KEYBOARD {
                                    found_boot_keyboard = true;
                                    (0usize, true, false)
                                } else if iface.b_interface_protocol == hid_protocol::MOUSE {
                                    if found_mouse {
                                        // Skip duplicate mouse interface to avoid ring collision
                                        // (both endpoints would share the same transfer ring)
                                        break;
                                    }
                                    found_mouse = true;
                                    (1usize, false, false)
                                } else if found_boot_keyboard
                                    && iface.b_interface_sub_class == 0
                                    && iface.b_interface_protocol == 0
                                {
                                    // Non-boot HID interface on same device as boot keyboard
                                    // = NKRO keyboard (Parallels sends keystrokes here)
                                    (2usize, false, true)
                                } else {
                                    // Unknown HID interface — treat as mouse/generic
                                    (1usize, false, false)
                                };

                            // Calculate DCI (Device Context Index) for this endpoint
                            let ep_num = ep_desc.endpoint_number();
                            let dci = ep_num * 2 + if ep_desc.is_in() { 1 } else { 0 };

                            // Collect endpoint info for batch ConfigureEndpoint
                            // (all EPs configured in one command, matching Linux)
                            if ep_count < pending_eps.len() {
                                pending_eps[ep_count] = Some(PendingEp {
                                    dci,
                                    hid_idx,
                                    max_pkt: ep_desc.w_max_packet_size,
                                    b_interval: ep_desc.b_interval,
                                    ss_max_burst,
                                    ss_bytes_per_interval,
                                });
                                ep_count += 1;
                            }

                            // Store info for Phase 3
                            if iface_count < ifaces.len() {
                                ifaces[iface_count] = Some(HidIfaceInfo {
                                    interface_number: iface.b_interface_number,
                                    is_keyboard,
                                    is_nkro,
                                    dci,
                                    hid_report_len,
                                });
                                iface_count += 1;
                            }

                            break; // Found the endpoint for this interface
                        }
                    }

                    ep_offset += ep_len;
                }
            }
        }

        offset += desc_len;
    }

    if iface_count == 0 {
        crate::serial_println!("[xhci] No HID interfaces found on slot {}", slot_id);
        return Ok(());
    }

    // =========================================================================
    // Phase 1b: ConfigureEndpoint BEFORE SET_CONFIGURATION (Linux ordering)
    // =========================================================================
    if ep_count > 0 {
        configure_endpoints_batch(state, slot_id, &pending_eps, ep_count)?;
    }

    // =========================================================================
    // Phase 2: SET_CONFIGURATION (USB control transfer to device)
    // =========================================================================
    set_configuration(state, slot_id, config_value)?;

    // Diagnostic: dump endpoint states after SET_CONFIGURATION
    if ep_count > 0 {
        let slot_idx = (slot_id - 1) as usize;
        let ctx_size = state.context_size;
        unsafe {
            let dev_ctx = &raw const DEVICE_CONTEXTS[slot_idx];
            dma_cache_invalidate((*dev_ctx).0.as_ptr(), 4096);
            for i in 0..ep_count {
                if let Some(ref ep) = pending_eps[i] {
                    let ep_out = (*dev_ctx).0.as_ptr().add((ep.dci as usize) * ctx_size);
                    let ep_out_dw0 = core::ptr::read_volatile(ep_out as *const u32);
                    crate::serial_println!(
                        "[xhci]   After SET_CONFIG: DCI={} state={} DW0={:#010x}",
                        ep.dci, ep_out_dw0 & 0x7, ep_out_dw0,
                    );
                }
            }
        }
    }

    // NOTE: Linux does NOT send SET_INTERFACE for HID devices (confirmed via
    // ftrace). Only slot 3 (composite device, class 0xEF) receives SET_INTERFACE.
    // We previously added SET_INTERFACE calls here but that was incorrect.

    // NOTE: CLEAR_FEATURE(ENDPOINT_HALT) was tested but did NOT fix CC=12.
    // StopEndpoint + SetTRDequeuePointer was also tested and didn't fix it.
    // The Parallels vxHC appears to have a fundamental limitation with interrupt
    // endpoint transfers. EP0 GET_REPORT polling is used as a workaround.

    // =========================================================================
    // Phase 3: HID interface setup + INLINE interrupt TRB queueing
    // =========================================================================
    // Linux's exact sequence per interface (from ftrace):
    //   SET_IDLE(0, iface N) → GET_DESCRIPTOR(HID Report, exact_len) →
    //   SET_REPORT(Output, 1 byte, keyboard only) → queue interrupt TRB + doorbell
    //
    // The interrupt TRB is queued IMMEDIATELY after each interface's HID setup.
    // This is critical: the Parallels virtual xHC may internally disable endpoints
    // if no TRBs are available on the transfer ring shortly after ConfigureEndpoint.
    // wait_for_command() ensures these Transfer Events don't interfere with
    // subsequent command ring operations during port scanning.
    for i in 0..iface_count {
        if let Some(ref info) = ifaces[i] {
            if !MINIMAL_INIT {
            // SET_IDLE (all interfaces) — matching Linux's HID driver probe sequence
            match set_idle(state, slot_id, info.interface_number) {
                Ok(()) => {
                    crate::serial_println!(
                        "[xhci] SET_IDLE(0) on slot {} iface {}",
                        slot_id, info.interface_number,
                    );
                }
                Err(e) => {
                    crate::serial_println!(
                        "[xhci] SET_IDLE failed on slot {} iface {}: {}",
                        slot_id, info.interface_number, e,
                    );
                }
            }

            // GET_DESCRIPTOR(HID Report) with exact length from HID descriptor
            fetch_hid_report_descriptor(state, slot_id, info.interface_number, info.hid_report_len);

            // GET_REPORT(Feature) — matching Linux's HID driver probe sequence.
            let feature_id: u8 = if i == 0 { 0x11 } else { 0x12 };
            match get_report_feature(state, slot_id, info.interface_number, feature_id) {
                Ok(()) => {
                    crate::serial_println!(
                        "[xhci] GET_REPORT(Feature, ID={:#04x}) on slot {} iface {}",
                        feature_id, slot_id, info.interface_number,
                    );
                }
                Err(_) => {
                    crate::serial_println!(
                        "[xhci] GET_REPORT(Feature, ID={:#04x}) STALL on slot {} iface {} (expected)",
                        feature_id, slot_id, info.interface_number,
                    );
                }
            }
            } // end if !MINIMAL_INIT

            if info.is_nkro {
                // NKRO keyboard interface
                state.kbd_slot = slot_id;
                state.kbd_nkro_endpoint = info.dci;
                crate::serial_println!(
                    "[xhci] NKRO keyboard configured: slot={} DCI={}",
                    slot_id, info.dci
                );

                // Queue interrupt TRB inline (matching Linux timing).
                // wait_for_command already handles Transfer Events that arrive
                // during subsequent enumeration commands.
                let _ = queue_hid_transfer(state, 2, slot_id, info.dci);
                crate::serial_println!(
                    "[xhci] Inline-queued TRB: NKRO (slot={} DCI={})",
                    slot_id, info.dci
                );
            } else {
                // Boot/standard HID interface

                // SET_REPORT(LED=0) for keyboard interfaces
                if !MINIMAL_INIT && info.is_keyboard {
                    match set_report_leds(state, slot_id, info.interface_number) {
                        Ok(()) => {
                            crate::serial_println!(
                                "[xhci] SET_REPORT(LED=0) on slot {} iface {}",
                                slot_id, info.interface_number
                            );
                        }
                        Err(e) => {
                            crate::serial_println!(
                                "[xhci] SET_REPORT(LED) failed on slot {} iface {}: {}",
                                slot_id, info.interface_number, e
                            );
                        }
                    }
                }

                // Record slot/endpoint and queue TRB inline.
                if info.is_keyboard {
                    state.kbd_slot = slot_id;
                    state.kbd_endpoint = info.dci;
                    crate::serial_println!(
                        "[xhci] Boot keyboard configured: slot={} DCI={}",
                        slot_id, info.dci
                    );
                    // Queue interrupt TRB inline (matching Linux timing).
                    let _ = queue_hid_transfer(state, 0, slot_id, info.dci);
                    crate::serial_println!(
                        "[xhci] Inline-queued TRB: kbd boot (slot={} DCI={})",
                        slot_id, info.dci
                    );
                } else {
                    // Try SET_PROTOCOL(boot) so EP0 GET_REPORT works with
                    // standard 3-byte boot mouse format (no Report ID).
                    let set_proto = SetupPacket {
                        bm_request_type: 0x21,  // Host-to-Device, Class, Interface
                        b_request: hid_request::SET_PROTOCOL,
                        w_value: 0,             // Boot Protocol
                        w_index: info.interface_number as u16,
                        w_length: 0,
                    };
                    match control_transfer(state, slot_id, &set_proto, 0, 0, false) {
                        Ok(()) => {
                            crate::serial_println!(
                                "[xhci] SET_PROTOCOL(boot) on slot {} iface {}",
                                slot_id, info.interface_number
                            );
                        }
                        Err(e) => {
                            crate::serial_println!(
                                "[xhci] SET_PROTOCOL(boot) failed on slot {} iface {}: {}",
                                slot_id, info.interface_number, e
                            );
                        }
                    }

                    state.mouse_slot = slot_id;
                    state.mouse_endpoint = info.dci;
                    crate::serial_println!(
                        "[xhci] Mouse configured: slot={} DCI={}",
                        slot_id, info.dci
                    );
                    // Queue interrupt TRB inline (matching Linux timing).
                    let _ = queue_hid_transfer(state, 1, slot_id, info.dci);
                    crate::serial_println!(
                        "[xhci] Inline-queued TRB: mouse (slot={} DCI={})",
                        slot_id, info.dci
                    );
                }
            }
        }
    }

    Ok(())
}

/// Queue a Normal TRB on a HID transfer ring to receive an interrupt IN report.
fn queue_hid_transfer(
    state: &XhciState,
    hid_idx: usize, // 0 = keyboard, 1 = mouse, 2 = NKRO keyboard
    slot_id: u8,
    dci: u8,
) -> Result<(), &'static str> {
    let ring_idx = HID_RING_BASE + hid_idx;

    // Determine the physical address and size of the report buffer
    let (buf_phys, buf_len) = match hid_idx {
        0 => (virt_to_phys((&raw const KBD_REPORT_BUF) as u64), 8usize),
        2 => (virt_to_phys((&raw const NKRO_REPORT_BUF) as u64), 9usize),
        _ => (virt_to_phys((&raw const MOUSE_REPORT_BUF) as u64), 8usize),
    };

    // Fill report buffer with sentinel (0xDE) before giving it to the controller.
    // After DMA completion, we check if the sentinel was overwritten — this tells
    // us definitively whether the XHCI DMA wrote actual data to the buffer.
    unsafe {
        match hid_idx {
            0 => {
                let buf = &raw mut KBD_REPORT_BUF;
                core::ptr::write_bytes((*buf).0.as_mut_ptr(), 0xDE, 8);
                dma_cache_clean((*buf).0.as_ptr(), 8);
            }
            2 => {
                let buf = &raw mut NKRO_REPORT_BUF;
                core::ptr::write_bytes((*buf).0.as_mut_ptr(), 0xDE, 9);
                dma_cache_clean((*buf).0.as_ptr(), 9);
            }
            _ => {
                let buf = &raw mut MOUSE_REPORT_BUF;
                core::ptr::write_bytes((*buf).0.as_mut_ptr(), 0xDE, 8);
                dma_cache_clean((*buf).0.as_ptr(), 8);
            }
        }
    }

    // Normal TRB for interrupt IN transfer
    let trb = Trb {
        param: buf_phys,
        status: buf_len as u32,
        // Normal TRB type, IOC (bit 5), ISP (Interrupt on Short Packet, bit 2)
        control: (trb_type::NORMAL << 10) | (1 << 5) | (1 << 2),
    };
    // Record the index before enqueue (enqueue advances it)
    let enq_idx = unsafe { TRANSFER_ENQUEUE[ring_idx] };
    enqueue_transfer(ring_idx, trb);

    // Record physical address of queued TRB (first time only)
    unsafe {
        let trb_phys = virt_to_phys(&raw const TRANSFER_RINGS[ring_idx] as u64)
            + (enq_idx as u64) * 16;
        let _ = DIAG_FIRST_QUEUED_PHYS.compare_exchange(
            0, trb_phys, Ordering::AcqRel, Ordering::Relaxed,
        );
    }

    // Read endpoint state BEFORE doorbell ring (diagnostic)
    let pre_state = unsafe {
        let slot_idx = (slot_id - 1) as usize;
        let ctx_size = state.context_size;
        let dev_ctx = &raw const DEVICE_CONTEXTS[slot_idx];
        dma_cache_invalidate((*dev_ctx).0.as_ptr(), 4096);
        let ep_base = (*dev_ctx).0.as_ptr().add(dci as usize * ctx_size);
        core::ptr::read_volatile(ep_base as *const u32) & 0x7
    };

    crate::serial_println!(
        "[xhci] queue_hid_transfer: slot={} DCI={} ring_idx={} enq_idx={} pre_ep_state={}",
        slot_id, dci, ring_idx, enq_idx, pre_state,
    );

    // Ring the doorbell for this endpoint
    ring_doorbell(state, slot_id, dci);

    // Read endpoint state AFTER doorbell ring (diagnostic)
    // Small spin to let the xHC process the doorbell
    for _ in 0..100 {
        core::hint::spin_loop();
    }
    let post_state = unsafe {
        let slot_idx = (slot_id - 1) as usize;
        let ctx_size = state.context_size;
        let dev_ctx = &raw const DEVICE_CONTEXTS[slot_idx];
        dma_cache_invalidate((*dev_ctx).0.as_ptr(), 4096);
        let ep_base = (*dev_ctx).0.as_ptr().add(dci as usize * ctx_size);
        core::ptr::read_volatile(ep_base as *const u32) & 0x7
    };

    let db_val = (pre_state << 4) | post_state;
    DIAG_DOORBELL_EP_STATE.store(db_val, Ordering::Relaxed);
    // Record first-time only (0xFF = unset sentinel)
    let _ = DIAG_FIRST_DB.compare_exchange(0xFF, db_val, Ordering::AcqRel, Ordering::Relaxed);

    Ok(())
}

/// Queue a GET_REPORT control transfer on EP0 for the keyboard device.
///
/// This is an asynchronous enqueue — completion is handled in the event loop
/// of poll_hid_events and handle_interrupt. Uses CTRL_DATA_BUF as the DMA
/// target (unused after init).
///
/// GET_REPORT: bmRequestType=0xA1, bRequest=0x01, wValue=0x0100 (Input, ID=0),
/// wIndex=0 (interface 0 = boot keyboard), wLength=8.
fn queue_ep0_get_report(state: &XhciState) {
    let slot_id = state.kbd_slot;
    if slot_id == 0 {
        return;
    }
    let slot_idx = (slot_id - 1) as usize;

    // Check ring capacity: if near the end, recycle the ring via
    // StopEndpoint + SetTRDequeuePointer since the Parallels virtual xHC
    // does not follow Link TRBs on transfer rings.
    unsafe {
        if TRANSFER_ENQUEUE[slot_idx] + 4 >= TRANSFER_RING_SIZE - 1 {
            // Recycle the EP0 transfer ring
            // Step 1: StopEndpoint for DCI=1 (EP0)
            let stop_trb = Trb {
                param: 0,
                status: 0,
                control: (trb_type::STOP_ENDPOINT << 10)
                    | ((slot_id as u32) << 24)
                    | (1u32 << 16), // DCI=1
            };
            enqueue_command(stop_trb);
            ring_doorbell(state, 0, 0);
            let _ = wait_for_command(state);

            // Step 2: Zero the ring and reset state
            let ring = &raw mut TRANSFER_RINGS[slot_idx];
            core::ptr::write_bytes(ring as *mut u8, 0, TRANSFER_RING_SIZE * 16);
            dma_cache_clean(
                &TRANSFER_RINGS[slot_idx] as *const [Trb; TRANSFER_RING_SIZE] as *const u8,
                TRANSFER_RING_SIZE * 16,
            );
            TRANSFER_ENQUEUE[slot_idx] = 0;
            TRANSFER_CYCLE[slot_idx] = true;

            // Step 3: SetTRDequeuePointer for DCI=1 back to ring start
            let ring_phys = virt_to_phys(&raw const TRANSFER_RINGS[slot_idx] as u64);
            let set_deq_trb = Trb {
                param: ring_phys | 1, // DCS = 1
                status: 0,
                control: (trb_type::SET_TR_DEQUEUE_POINTER << 10)
                    | ((slot_id as u32) << 24)
                    | (1u32 << 16), // DCI=1
            };
            enqueue_command(set_deq_trb);
            ring_doorbell(state, 0, 0);
            let _ = wait_for_command(state);
        }
    }

    // Prepare DMA buffer with sentinel
    unsafe {
        let data_buf = &raw mut CTRL_DATA_BUF;
        core::ptr::write_bytes((*data_buf).0.as_mut_ptr(), 0xBB, 8);
        dma_cache_clean((*data_buf).0.as_ptr(), 8);
    }

    let data_phys = virt_to_phys((&raw const CTRL_DATA_BUF) as u64);

    let setup = SetupPacket {
        bm_request_type: 0xA1,  // Class, Interface, Device-to-Host
        b_request: 0x01,        // GET_REPORT
        w_value: 0x0100,        // Input report, Report ID 0
        w_index: 0,             // Interface 0 (boot keyboard)
        w_length: 8,
    };

    let setup_data: u64 = unsafe {
        core::ptr::read_unaligned(&setup as *const SetupPacket as *const u64)
    };

    // Setup Stage TRB: IDT (bit 6), TRT=3 (IN data stage), type = SETUP_STAGE
    let setup_trb = Trb {
        param: setup_data,
        status: 8,
        control: (trb_type::SETUP_STAGE << 10) | (1 << 6) | (3 << 16),
    };
    enqueue_transfer(slot_idx, setup_trb);

    // Data Stage TRB: DIR=IN (bit 16), type = DATA_STAGE
    let data_trb = Trb {
        param: data_phys,
        status: 8,
        control: (trb_type::DATA_STAGE << 10) | (1 << 16),
    };
    enqueue_transfer(slot_idx, data_trb);

    // Status Stage TRB: DIR=OUT (0) for IN data, IOC (bit 5)
    let status_trb = Trb {
        param: 0,
        status: 0,
        control: (trb_type::STATUS_STAGE << 10) | (1 << 5),
    };
    enqueue_transfer(slot_idx, status_trb);

    // Ring doorbell for EP0 (DCI=1)
    ring_doorbell(state, slot_id, 1);

    EP0_POLL_STATE.store(1, Ordering::Release);
    EP0_GET_REPORT_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// Queue a GET_REPORT control transfer on EP0 for the mouse device.
///
/// Uses MOUSE_CTRL_DATA_BUF (separate from keyboard's CTRL_DATA_BUF).
/// After SET_PROTOCOL(boot) during init, the mouse uses boot protocol format:
/// 3 bytes (buttons, dx, dy) without Report ID.
///
/// GET_REPORT: bmRequestType=0xA1, bRequest=0x01, wValue=0x0100 (Input, ID=0),
/// wIndex=0 (interface 0), wLength=8.
fn queue_ep0_mouse_get_report(state: &XhciState) {
    let slot_id = state.mouse_slot;
    if slot_id == 0 {
        return;
    }

    // Stop trying after 5 consecutive errors — the device likely doesn't
    // support GET_REPORT for Input reports.
    let err_count = EP0_MOUSE_GET_REPORT_ERR.load(Ordering::Relaxed);
    let ok_count = EP0_MOUSE_GET_REPORT_OK.load(Ordering::Relaxed);
    if err_count >= 5 && ok_count == 0 {
        return;
    }

    let slot_idx = (slot_id - 1) as usize;

    // Recycle the EP0 transfer ring:
    // - When near the end (Parallels vxHC doesn't follow Link TRBs), OR
    // - After errors (orphaned TRBs remain from failed transfers)
    unsafe {
        let needs_recycle = TRANSFER_ENQUEUE[slot_idx] + 4 >= TRANSFER_RING_SIZE - 1
            || (err_count > ok_count && TRANSFER_ENQUEUE[slot_idx] > 0);

        if needs_recycle {
            // Step 1: StopEndpoint for DCI=1 (EP0)
            let stop_trb = Trb {
                param: 0,
                status: 0,
                control: (trb_type::STOP_ENDPOINT << 10)
                    | ((slot_id as u32) << 24)
                    | (1u32 << 16), // DCI=1
            };
            enqueue_command(stop_trb);
            ring_doorbell(state, 0, 0);
            let _ = wait_for_command(state);

            // Step 2: Zero the ring and reset state
            let ring = &raw mut TRANSFER_RINGS[slot_idx];
            core::ptr::write_bytes(ring as *mut u8, 0, TRANSFER_RING_SIZE * 16);
            dma_cache_clean(
                &TRANSFER_RINGS[slot_idx] as *const [Trb; TRANSFER_RING_SIZE] as *const u8,
                TRANSFER_RING_SIZE * 16,
            );
            TRANSFER_ENQUEUE[slot_idx] = 0;
            TRANSFER_CYCLE[slot_idx] = true;

            // Step 3: SetTRDequeuePointer for DCI=1 back to ring start
            let ring_phys = virt_to_phys(&raw const TRANSFER_RINGS[slot_idx] as u64);
            let set_deq_trb = Trb {
                param: ring_phys | 1, // DCS = 1
                status: 0,
                control: (trb_type::SET_TR_DEQUEUE_POINTER << 10)
                    | ((slot_id as u32) << 24)
                    | (1u32 << 16), // DCI=1
            };
            enqueue_command(set_deq_trb);
            ring_doorbell(state, 0, 0);
            let _ = wait_for_command(state);
        }
    }

    // Prepare DMA buffer with sentinel
    unsafe {
        let data_buf = &raw mut MOUSE_CTRL_DATA_BUF;
        core::ptr::write_bytes((*data_buf).0.as_mut_ptr(), 0xBB, 8);
        dma_cache_clean((*data_buf).0.as_ptr(), 8);
    }

    let data_phys = virt_to_phys((&raw const MOUSE_CTRL_DATA_BUF) as u64);

    // After SET_PROTOCOL(boot) during init, use boot protocol format:
    // 3 bytes (buttons, dx, dy), no Report ID.
    let setup = SetupPacket {
        bm_request_type: 0xA1,  // Class, Interface, Device-to-Host
        b_request: 0x01,        // GET_REPORT
        w_value: 0x0100,        // Input report, Report ID 0 (boot protocol)
        w_index: 0,             // Interface 0 (mouse)
        w_length: 8,            // Request up to 8 bytes (boot mouse: 3-4 bytes)
    };

    let setup_data: u64 = unsafe {
        core::ptr::read_unaligned(&setup as *const SetupPacket as *const u64)
    };

    // Setup Stage TRB: IDT (bit 6), TRT=3 (IN data stage), type = SETUP_STAGE
    let setup_trb = Trb {
        param: setup_data,
        status: 8,
        control: (trb_type::SETUP_STAGE << 10) | (1 << 6) | (3 << 16),
    };
    enqueue_transfer(slot_idx, setup_trb);

    // Data Stage TRB: DIR=IN (bit 16), type = DATA_STAGE, length=8
    let data_trb = Trb {
        param: data_phys,
        status: 8,
        control: (trb_type::DATA_STAGE << 10) | (1 << 16),
    };
    enqueue_transfer(slot_idx, data_trb);

    // Status Stage TRB: DIR=OUT (0) for IN data, IOC (bit 5)
    let status_trb = Trb {
        param: 0,
        status: 0,
        control: (trb_type::STATUS_STAGE << 10) | (1 << 5),
    };
    enqueue_transfer(slot_idx, status_trb);

    // Ring doorbell for EP0 (DCI=1)
    ring_doorbell(state, slot_id, 1);

    EP0_MOUSE_POLL_STATE.store(1, Ordering::Release);
    EP0_MOUSE_GET_REPORT_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// Drain any stale events left in the event ring after enumeration.
///
/// During enumeration, some xHCI controllers may leave Transfer Events
/// (e.g., Short Packet events for Data Stage TRBs) that weren't consumed
/// by wait_for_event. These must be drained before starting HID polling.
fn drain_stale_events(state: &XhciState) {
    let mut drained = 0u32;
    unsafe {
        loop {
            let ring = &raw const EVENT_RING;
            let idx = EVENT_RING_DEQUEUE;
            let cycle = EVENT_RING_CYCLE;

            dma_cache_invalidate(
                &(*ring).0[idx] as *const Trb as *const u8,
                core::mem::size_of::<Trb>(),
            );

            let trb = core::ptr::read_volatile(&(*ring).0[idx]);
            let trb_cycle = trb.control & 1 != 0;

            if trb_cycle != cycle {
                break; // No more events
            }

            let trb_type_val = trb.trb_type();
            crate::serial_println!(
                "[xhci] Draining stale event #{}: type={} slot={} ep={} cc={} param={:#x}",
                drained,
                trb_type_val,
                trb.slot_id(),
                (trb.control >> 16) & 0x1F,
                trb.completion_code(),
                trb.param,
            );

            // Advance dequeue pointer
            EVENT_RING_DEQUEUE = (idx + 1) % EVENT_RING_SIZE;
            if EVENT_RING_DEQUEUE == 0 {
                EVENT_RING_CYCLE = !cycle;
            }

            // Update ERDP with EHB bit
            let ir0 = state.rt_base + 0x20;
            let erdp_phys = virt_to_phys(&raw const EVENT_RING as u64)
                + (EVENT_RING_DEQUEUE as u64) * 16;
            write64(ir0 + 0x18, erdp_phys | (1 << 3));

            drained += 1;
            if drained >= 32 {
                break; // Safety limit
            }
        }
    }
    if drained > 0 {
        crate::serial_println!("[xhci] Drained {} stale events from event ring", drained);
    } else {
        crate::serial_println!("[xhci] No stale events in event ring");
    }
}

/// Test synchronous GET_REPORT and GET_PROTOCOL during init.
///
/// Called after keyboard is configured to diagnose whether Parallels echoes
/// setup packet bytes for class-specific requests or if the issue is
/// specific to the async EP0 polling path.
///
/// NOTE: Currently disabled in the init sequence because the GET_REPORT
/// responses contaminate KBD_REPORT_BUF with setup packet echoes.
/// Kept for future diagnostics.
#[allow(dead_code)]
fn test_sync_class_requests(state: &XhciState, slot_id: u8) {
    // Log physical addresses for diagnostic comparison
    let ctrl_buf_phys = virt_to_phys((&raw const CTRL_DATA_BUF) as u64);
    let kbd_buf_phys = virt_to_phys((&raw const KBD_REPORT_BUF) as u64);
    crate::serial_println!(
        "[xhci] Buffer phys addrs: CTRL_DATA_BUF={:#010x} KBD_REPORT_BUF={:#010x}",
        ctrl_buf_phys, kbd_buf_phys,
    );

    // Test 1: Synchronous GET_REPORT using CTRL_DATA_BUF
    {
        let setup = SetupPacket {
            bm_request_type: 0xA1,
            b_request: 0x01, // GET_REPORT
            w_value: 0x0100, // Input report, ID 0
            w_index: 0,
            w_length: 8,
        };

        unsafe {
            let data_buf = &raw mut CTRL_DATA_BUF;
            core::ptr::write_bytes((*data_buf).0.as_mut_ptr(), 0xBB, 8);
            dma_cache_clean((*data_buf).0.as_ptr(), 8);
            dma_cache_invalidate((*data_buf).0.as_ptr(), 8);

            let data_phys = virt_to_phys(&raw const CTRL_DATA_BUF as u64);

            match control_transfer(state, slot_id, &setup, data_phys, 8, true) {
                Ok(()) => {
                    dma_cache_invalidate((*data_buf).0.as_ptr(), 8);
                    let buf = &(*data_buf).0;
                    crate::serial_println!(
                        "[xhci] Sync GET_REPORT(CTRL_DATA): {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}",
                        buf[0], buf[1], buf[2], buf[3], buf[4], buf[5], buf[6], buf[7],
                    );
                }
                Err(e) => {
                    crate::serial_println!("[xhci] Sync GET_REPORT(CTRL_DATA) failed: {}", e);
                }
            }
        }
    }

    // Test 2: Synchronous GET_REPORT using KBD_REPORT_BUF
    {
        let setup = SetupPacket {
            bm_request_type: 0xA1,
            b_request: 0x01,
            w_value: 0x0100,
            w_index: 0,
            w_length: 8,
        };

        unsafe {
            let kbd_buf = &raw mut KBD_REPORT_BUF;
            core::ptr::write_bytes((*kbd_buf).0.as_mut_ptr(), 0xCC, 8);
            dma_cache_clean((*kbd_buf).0.as_ptr(), 8);
            dma_cache_invalidate((*kbd_buf).0.as_ptr(), 8);

            match control_transfer(state, slot_id, &setup, kbd_buf_phys, 8, true) {
                Ok(()) => {
                    dma_cache_invalidate((*kbd_buf).0.as_ptr(), 8);
                    let buf = &(*kbd_buf).0;
                    crate::serial_println!(
                        "[xhci] Sync GET_REPORT(KBD_BUF):   {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}",
                        buf[0], buf[1], buf[2], buf[3], buf[4], buf[5], buf[6], buf[7],
                    );
                }
                Err(e) => {
                    crate::serial_println!("[xhci] Sync GET_REPORT(KBD_BUF) failed: {}", e);
                }
            }
        }
    }

    // Test 3: Synchronous GET_PROTOCOL (should return 1 byte: 0=boot, 1=report)
    {
        let setup = SetupPacket {
            bm_request_type: 0xA1,
            b_request: 0x03, // GET_PROTOCOL
            w_value: 0,
            w_index: 0,
            w_length: 1,
        };

        unsafe {
            let data_buf = &raw mut CTRL_DATA_BUF;
            core::ptr::write_bytes((*data_buf).0.as_mut_ptr(), 0xDD, 8);
            dma_cache_clean((*data_buf).0.as_ptr(), 8);

            let data_phys = virt_to_phys(&raw const CTRL_DATA_BUF as u64);

            match control_transfer(state, slot_id, &setup, data_phys, 1, true) {
                Ok(()) => {
                    dma_cache_invalidate((*data_buf).0.as_ptr(), 8);
                    let buf = &(*data_buf).0;
                    crate::serial_println!(
                        "[xhci] Sync GET_PROTOCOL: {:02x} ({})",
                        buf[0],
                        if buf[0] == 0 { "boot" } else if buf[0] == 1 { "report" } else { "?" },
                    );
                }
                Err(e) => {
                    crate::serial_println!("[xhci] Sync GET_PROTOCOL failed: {}", e);
                }
            }
        }
    }

    // Test 4: GET_DESCRIPTOR(Device) via sync to verify control transfers still work
    {
        let setup = SetupPacket {
            bm_request_type: 0x80,
            b_request: 0x06, // GET_DESCRIPTOR
            w_value: 0x0100, // Device descriptor
            w_index: 0,
            w_length: 8,
        };

        unsafe {
            let data_buf = &raw mut CTRL_DATA_BUF;
            core::ptr::write_bytes((*data_buf).0.as_mut_ptr(), 0xEE, 8);
            dma_cache_clean((*data_buf).0.as_ptr(), 8);

            let data_phys = virt_to_phys(&raw const CTRL_DATA_BUF as u64);

            match control_transfer(state, slot_id, &setup, data_phys, 8, true) {
                Ok(()) => {
                    dma_cache_invalidate((*data_buf).0.as_ptr(), 8);
                    let buf = &(*data_buf).0;
                    crate::serial_println!(
                        "[xhci] Sync GET_DESC(Device): {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}",
                        buf[0], buf[1], buf[2], buf[3], buf[4], buf[5], buf[6], buf[7],
                    );
                }
                Err(e) => {
                    crate::serial_println!("[xhci] Sync GET_DESC(Device) failed: {}", e);
                }
            }
        }
    }
}

/// Dump endpoint context states from the output device context.
///
/// Reads the output device context (updated by the xHC) to verify endpoint
/// states and TR Dequeue Pointers are correct AFTER all init is complete.
fn dump_endpoint_contexts(state: &XhciState) {
    // Dump for keyboard slot
    if state.kbd_slot != 0 {
        let slot_idx = (state.kbd_slot - 1) as usize;
        let ctx_size = state.context_size;
        unsafe {
            let dev_ctx = &raw const DEVICE_CONTEXTS[slot_idx];
            dma_cache_invalidate((*dev_ctx).0.as_ptr(), 4096);

            // Slot Context DW0
            let slot_dw0 = core::ptr::read_volatile((*dev_ctx).0.as_ptr() as *const u32);
            let slot_state = (slot_dw0 >> 27) & 0x1F;
            let ctx_entries = slot_dw0 >> 27;

            crate::serial_println!(
                "[xhci] Post-init slot {} context: state={} entries={}",
                state.kbd_slot, slot_state, ctx_entries & 0x1F,
            );

            // Dump each endpoint DCI we care about (full 5 DWORDs)
            for &dci in &[state.kbd_endpoint, state.kbd_nkro_endpoint] {
                if dci == 0 { continue; }
                let ep_base = (*dev_ctx).0.as_ptr().add(dci as usize * ctx_size);
                let ep_dw0 = core::ptr::read_volatile(ep_base as *const u32);
                let ep_dw1 = core::ptr::read_volatile(ep_base.add(4) as *const u32);
                let ep_dw2 = core::ptr::read_volatile(ep_base.add(8) as *const u32);
                let ep_dw3 = core::ptr::read_volatile(ep_base.add(12) as *const u32);
                let ep_dw4 = core::ptr::read_volatile(ep_base.add(16) as *const u32);
                let ep_state = ep_dw0 & 0x7;
                let ep_type = (ep_dw1 >> 3) & 0x7;
                let max_pkt = (ep_dw1 >> 16) & 0xFFFF;
                let cerr = (ep_dw1 >> 1) & 0x3;
                let interval = (ep_dw0 >> 16) & 0xFF;
                let tr_deq = ((ep_dw3 as u64) << 32) | (ep_dw2 as u64 & !0xF);
                let dcs = ep_dw2 & 1;
                let avg_trb = ep_dw4 & 0xFFFF;
                let max_esit_lo = (ep_dw4 >> 16) & 0xFFFF;

                crate::serial_println!(
                    "[xhci]   DCI={}: state={} type={} maxpkt={} cerr={} interval={} DCS={}",
                    dci, ep_state, ep_type, max_pkt, cerr, interval, dcs,
                );
                crate::serial_println!(
                    "[xhci]     DW0={:#010x} DW1={:#010x} DW2={:#010x} DW3={:#010x} DW4={:#010x}",
                    ep_dw0, ep_dw1, ep_dw2, ep_dw3, ep_dw4,
                );
                crate::serial_println!(
                    "[xhci]     TR_deq={:#010x} avg_trb={} max_esit_lo={}",
                    tr_deq, avg_trb, max_esit_lo,
                );
            }
        }
    }

    // Also dump mouse endpoint
    if state.mouse_slot != 0 && state.mouse_endpoint != 0 {
        let slot_idx = (state.mouse_slot - 1) as usize;
        let ctx_size = state.context_size;
        unsafe {
            let dev_ctx = &raw const DEVICE_CONTEXTS[slot_idx];
            dma_cache_invalidate((*dev_ctx).0.as_ptr(), 4096);
            let dci = state.mouse_endpoint;
            let ep_base = (*dev_ctx).0.as_ptr().add(dci as usize * ctx_size);
            let ep_dw0 = core::ptr::read_volatile(ep_base as *const u32);
            let ep_dw1 = core::ptr::read_volatile(ep_base.add(4) as *const u32);
            let ep_dw2 = core::ptr::read_volatile(ep_base.add(8) as *const u32);
            let ep_dw3 = core::ptr::read_volatile(ep_base.add(12) as *const u32);
            let ep_dw4 = core::ptr::read_volatile(ep_base.add(16) as *const u32);
            crate::serial_println!(
                "[xhci] Mouse slot {} DCI={}: DW0={:#010x} DW1={:#010x} DW2={:#010x} DW3={:#010x} DW4={:#010x}",
                state.mouse_slot, dci, ep_dw0, ep_dw1, ep_dw2, ep_dw3, ep_dw4,
            );
        }
    }
}

/// Wait for a Command Completion event, ignoring Transfer Events and other
/// async events. Used during endpoint recovery in timer context — no logging.
///
/// Returns the completion code, or an error on timeout.
fn wait_for_command_completion(state: &XhciState) -> Result<u32, &'static str> {
    let mut timeout = 500_000u32; // Shorter timeout for timer context
    loop {
        unsafe {
            let ring = &raw const EVENT_RING;
            let idx = EVENT_RING_DEQUEUE;
            let cycle = EVENT_RING_CYCLE;

            dma_cache_invalidate(
                &(*ring).0[idx] as *const Trb as *const u8,
                core::mem::size_of::<Trb>(),
            );

            let trb = core::ptr::read_volatile(&(*ring).0[idx]);
            let trb_cycle = trb.control & 1 != 0;

            if trb_cycle == cycle {
                // Advance dequeue
                EVENT_RING_DEQUEUE = (idx + 1) % EVENT_RING_SIZE;
                if EVENT_RING_DEQUEUE == 0 {
                    EVENT_RING_CYCLE = !cycle;
                }

                // Update ERDP
                let erdp_phys = virt_to_phys(&raw const EVENT_RING as u64)
                    + (EVENT_RING_DEQUEUE as u64) * 16;
                let ir0 = state.rt_base + 0x20;
                write64(ir0 + 0x18, erdp_phys | (1 << 3));

                let trb_type_val = trb.trb_type();
                if trb_type_val == trb_type::COMMAND_COMPLETION {
                    return Ok(trb.completion_code());
                }
                // Consumed non-command event (Transfer Event, PSC, etc.)
                // — fall through to timeout check.
            }
        }
        timeout -= 1;
        if timeout == 0 {
            return Err("XHCI command completion timeout");
        }
        core::hint::spin_loop();
    }
}

/// Reset a halted endpoint and requeue a HID transfer TRB.
///
/// Per xHCI spec section 4.6.8:
/// 1. Issue Reset Endpoint Command (TRB type 14)
/// 2. Issue Set TR Dequeue Pointer Command (TRB type 16) to ring start
/// 3. Requeue a Normal TRB for HID polling
///
/// Called from poll_hid_events (timer context). Uses raw_serial_char breadcrumbs
/// and wait_for_command_completion (no logging).
fn reset_halted_endpoint(
    state: &XhciState,
    slot_id: u8,
    dci: u8,
    hid_idx: usize,
) -> Result<(), &'static str> {
    crate::serial_aarch64::raw_serial_char(b'R'); // breadcrumb: Reset EP start

    let ring_idx = HID_RING_BASE + hid_idx;

    // Step 1: Reset Endpoint Command
    let reset_trb = Trb {
        param: 0,
        status: 0,
        // Reset Endpoint: type=14, slot_id in bits [31:24], DCI in bits [20:16]
        control: (trb_type::RESET_ENDPOINT << 10)
            | ((slot_id as u32) << 24)
            | ((dci as u32) << 16),
    };
    enqueue_command(reset_trb);
    ring_doorbell(state, 0, 0); // Command ring doorbell

    let cc = wait_for_command_completion(state)?;
    if cc != completion_code::SUCCESS {
        crate::serial_aarch64::raw_serial_char(b'!'); // breadcrumb: Reset EP failed
        ENDPOINT_RESET_FAIL_COUNT.fetch_add(1, Ordering::Relaxed);
        return Err("Reset Endpoint command failed");
    }

    crate::serial_aarch64::raw_serial_char(b'S'); // breadcrumb: Set TR Deq

    // Step 2: Zero transfer ring, add Link TRB, and reset state to beginning
    unsafe {
        let ring = &raw mut TRANSFER_RINGS[ring_idx];
        core::ptr::write_bytes(ring as *mut u8, 0, TRANSFER_RING_SIZE * 16);

        // Re-initialize Link TRB at end of ring (matching Linux ring structure)
        let ring_phys_link = virt_to_phys(&raw const TRANSFER_RINGS[ring_idx] as u64);
        let link = Trb {
            param: ring_phys_link,
            status: 0,
            control: (trb_type::LINK << 10) | (1 << 1) | 1, // Link, TC, cycle=1
        };
        core::ptr::write_volatile(
            &mut (*ring)[TRANSFER_RING_SIZE - 1] as *mut Trb,
            link,
        );

        dma_cache_clean(
            &TRANSFER_RINGS[ring_idx] as *const [Trb; TRANSFER_RING_SIZE] as *const u8,
            TRANSFER_RING_SIZE * 16,
        );
        TRANSFER_ENQUEUE[ring_idx] = 0;
        TRANSFER_CYCLE[ring_idx] = true;
    }

    // Step 3: Set TR Dequeue Pointer to ring start with DCS=1
    let ring_phys = virt_to_phys(unsafe { &raw const TRANSFER_RINGS[ring_idx] } as u64);
    let set_deq_trb = Trb {
        param: ring_phys | 1, // DCS bit 0 = 1 (matches initial cycle state)
        status: 0,
        control: (trb_type::SET_TR_DEQUEUE_POINTER << 10)
            | ((slot_id as u32) << 24)
            | ((dci as u32) << 16),
    };
    enqueue_command(set_deq_trb);
    ring_doorbell(state, 0, 0);

    let cc2 = wait_for_command_completion(state)?;
    if cc2 != completion_code::SUCCESS {
        crate::serial_aarch64::raw_serial_char(b'?'); // breadcrumb: Set TR Deq failed
        ENDPOINT_RESET_FAIL_COUNT.fetch_add(1, Ordering::Relaxed);
        return Err("Set TR Dequeue Pointer command failed");
    }

    // Step 4: Requeue a HID transfer TRB
    queue_hid_transfer(state, hid_idx, slot_id, dci)?;

    ENDPOINT_RESET_COUNT.fetch_add(1, Ordering::Relaxed);
    crate::serial_aarch64::raw_serial_char(b'r'); // breadcrumb: Reset EP complete

    Ok(())
}

/// Post-enumeration setup: drain stale events, re-queue TRBs, dump diagnostics.
///
/// Initial TRBs were already queued INLINE in configure_hid() Phase 3 to prevent
/// the Parallels virtual xHC from transitioning endpoints to Stopped during the
/// gap while scan_ports enumerates subsequent devices. This function drains any
/// completion events from those inline TRBs that weren't consumed by
/// wait_for_command, then queues fresh TRBs to keep the transfer rings populated.
fn start_hid_polling(state: &XhciState) {
    // Drain any leftover events from enumeration (including completions from
    // inline-queued TRBs that wait_for_command consumed but also Transfer Events
    // from interrupt endpoints that arrived during the rest of port scanning).
    drain_stale_events(state);

    // Diagnostic: dump DMA buffer physical addresses for verification
    unsafe {
        let kbd_buf_phys = virt_to_phys((&raw const KBD_REPORT_BUF) as u64);
        let mouse_buf_phys = virt_to_phys((&raw const MOUSE_REPORT_BUF) as u64);
        let nkro_buf_phys = virt_to_phys((&raw const NKRO_REPORT_BUF) as u64);
        let ring0_phys = virt_to_phys(&raw const TRANSFER_RINGS[HID_RING_BASE] as u64);
        let ring1_phys = virt_to_phys(&raw const TRANSFER_RINGS[HID_RING_BASE + 1] as u64);
        let ring2_phys = virt_to_phys(&raw const TRANSFER_RINGS[HID_RING_BASE + 2] as u64);
        crate::serial_println!(
            "[xhci] DMA phys: kbd_buf={:#010x} mouse_buf={:#010x} nkro_buf={:#010x}",
            kbd_buf_phys, mouse_buf_phys, nkro_buf_phys,
        );
        crate::serial_println!(
            "[xhci] DMA phys: ring0={:#010x} ring1={:#010x} ring2={:#010x}",
            ring0_phys, ring1_phys, ring2_phys,
        );
    }

    // TRBs were already queued inline during configure_hid. Drain any
    // Transfer Events (CC=12 or CC=1) that completed during port scanning,
    // then re-queue fresh TRBs for continuous polling.
    HID_TRBS_QUEUED.store(true, Ordering::Release);

    if state.kbd_slot != 0 && state.kbd_endpoint != 0 {
        let _ = queue_hid_transfer(state, 0, state.kbd_slot, state.kbd_endpoint);
        crate::serial_println!("[xhci] Re-queued TRB: kbd boot (slot={} DCI={})",
            state.kbd_slot, state.kbd_endpoint);
    }
    if state.kbd_slot != 0 && state.kbd_nkro_endpoint != 0 {
        let _ = queue_hid_transfer(state, 2, state.kbd_slot, state.kbd_nkro_endpoint);
        crate::serial_println!("[xhci] Re-queued TRB: kbd NKRO (slot={} DCI={})",
            state.kbd_slot, state.kbd_nkro_endpoint);
    }
    if state.mouse_slot != 0 && state.mouse_endpoint != 0 {
        let _ = queue_hid_transfer(state, 1, state.mouse_slot, state.mouse_endpoint);
        crate::serial_println!("[xhci] Re-queued TRB: mouse (slot={} DCI={})",
            state.mouse_slot, state.mouse_endpoint);
    }
}

// =============================================================================
// Port Scanning and Device Enumeration
// =============================================================================

/// Scan all root hub ports for connected devices, enumerate, and configure HID devices.
fn scan_ports(state: &mut XhciState) -> Result<(), &'static str> {
    crate::serial_println!(
        "[xhci] Scanning {} ports...",
        state.max_ports,
    );

    // Dump PORTSC of all ports (especially USB 2.0 ports 12-13)
    for port in 0..state.max_ports as u64 {
        let portsc_addr = state.op_base + 0x400 + port * 0x10;
        let portsc = read32(portsc_addr);
        let speed = (portsc >> 10) & 0xF;
        let ccs = portsc & 1;
        let ped = (portsc >> 1) & 1;
        if ccs != 0 || port >= 12 {
            crate::serial_println!(
                "[xhci] Port {} PORTSC={:#010x} CCS={} PED={} speed={}",
                port, portsc, ccs, ped, speed,
            );
        }
    }

    let mut slots_used: u8 = 0;
    let max_enumerate: u8 = 4; // Only enumerate first few connected devices

    for port in 0..state.max_ports as u64 {
        // DIAGNOSTIC: Don't break early — enumerate ALL connected devices.
        // Linux enumerates all 3 connected ports; skipping Port 2 may cause
        // the Parallels virtual xHC to leave keyboard interrupt endpoints disabled.
        // Limit total devices to avoid issues with unsupported devices
        if slots_used >= max_enumerate {
            break;
        }

        let portsc_addr = state.op_base + 0x400 + port * 0x10;
        let portsc = read32(portsc_addr);

        // Check CCS (Current Connect Status, bit 0)
        if portsc & 1 == 0 {
            continue;
        }


        let port_speed = (portsc >> 10) & 0xF;
        crate::serial_println!(
            "[xhci] Port {}: connected (PORTSC={:#010x}, speed={})",
            port,
            portsc,
            match port_speed {
                1 => "Full",
                2 => "Low",
                3 => "High",
                4 => "Super",
                _ => "Unknown",
            },
        );

        // Check if port is enabled (PED, bit 1)
        if portsc & (1 << 1) == 0 {
            // Port not enabled - perform a port reset
            crate::serial_println!("[xhci] Port {}: resetting...", port);

            // Write PR (Port Reset, bit 4).
            // Note: PORTSC is a mix of RW, RW1C, and RO bits. We must preserve
            // RW bits and NOT accidentally clear RW1C bits by writing 1 to them.
            // RW1C bits in PORTSC: CSC(17), PEC(18), WRC(19), OCC(20), PRC(21), PLC(22), CEC(23)
            let preserve_mask: u32 = !(
                (1 << 17) | (1 << 18) | (1 << 19) | (1 << 20) | (1 << 21) | (1 << 22) | (1 << 23)
            );
            write32(portsc_addr, (portsc & preserve_mask) | (1 << 4));

            // Wait for PRC (Port Reset Change, bit 21)
            if wait_for(
                || read32(portsc_addr) & (1 << 21) != 0,
                500_000,
            )
            .is_err()
            {
                crate::serial_println!("[xhci] Port {}: reset timeout", port);
                continue;
            }

            // Clear PRC (W1C) and check that port is now enabled
            let portsc_after = read32(portsc_addr);
            write32(portsc_addr, (portsc_after & preserve_mask) | (1 << 21));

            let portsc_final = read32(portsc_addr);
            if portsc_final & (1 << 1) == 0 {
                crate::serial_println!("[xhci] Port {}: still not enabled after reset", port);
                continue;
            }

            crate::serial_println!(
                "[xhci] Port {}: enabled after reset (PORTSC={:#010x})",
                port,
                portsc_final,
            );
        }

        // Enable Slot for this device
        let slot_id = match enable_slot(state) {
            Ok(id) => id,
            Err(e) => {
                crate::serial_println!("[xhci] Port {}: enable_slot failed: {}", port, e);
                continue;
            }
        };
        if slot_id == 0 {
            crate::serial_println!("[xhci] Port {}: got slot_id 0, skipping", port);
            continue;
        }

        slots_used += 1;

        // Address Device (port numbers are 1-based)
        if let Err(e) = address_device(state, slot_id, port as u8 + 1) {
            crate::serial_println!("[xhci] Port {}: address_device failed: {}", port, e);
            continue;
        }

        // Linux USB 3.0 enumeration sequence (from ftrace):
        //   1. GET_DESCRIPTOR(Device, 8)   — first 8 bytes for maxpkt0
        //   2. SET_ISOCH_DELAY(40ns)       — USB 3.0 isochronous delay
        //   3. GET_DESCRIPTOR(Device, 18)  — full device descriptor
        //   4. GET_DESCRIPTOR(BOS, 5+full) — BOS descriptor
        //   5. GET_DESCRIPTOR(Config, 9+full) — config descriptor
        //
        // This exact ordering is confirmed by Parallels Linux VM ftrace capture.

        // Step 1: Short device descriptor (8 bytes)
        if let Err(e) = get_device_descriptor_short(state, slot_id) {
            crate::serial_println!("[xhci] Port {}: get_device_descriptor(8) failed: {}", port, e);
            continue;
        }

        // Step 2: SET_ISOCH_DELAY (between the two descriptor reads)
        if let Err(e) = set_isoch_delay(state, slot_id) {
            crate::serial_println!(
                "[xhci] Port {}: SET_ISOCH_DELAY failed: {} (non-fatal)", port, e
            );
        }

        // Step 3: Full device descriptor (18 bytes)
        let mut desc_buf = [0u8; 18];
        if let Err(e) = get_device_descriptor(state, slot_id, &mut desc_buf) {
            crate::serial_println!("[xhci] Port {}: get_device_descriptor(18) failed: {}", port, e);
            continue;
        }

        // Step 4: BOS descriptor
        if let Err(e) = get_bos_descriptor(state, slot_id) {
            crate::serial_println!(
                "[xhci] Port {}: GET_BOS_DESCRIPTOR failed: {} (non-fatal)", port, e
            );
        }

        // Get Configuration Descriptor
        let mut config_buf = [0u8; 256];
        let config_len = match get_config_descriptor(state, slot_id, &mut config_buf) {
            Ok(len) => len,
            Err(e) => {
                crate::serial_println!("[xhci] Port {}: get_config_descriptor failed: {}", port, e);
                continue;
            }
        };

        // Step 5b: String descriptors (matching Linux enumeration sequence).
        // Linux reads string descriptors #0, #2, #1, #3 before ConfigureEndpoint.
        // The Parallels virtual xHC may require this to fully initialize the device.
        {
            let desc = unsafe { &*(desc_buf.as_ptr() as *const DeviceDescriptor) };
            read_string_descriptors(
                state,
                slot_id,
                desc.i_manufacturer,
                desc.i_product,
                desc.i_serial_number,
            );
        }

        // Configure HID devices
        if let Err(e) = configure_hid(state, slot_id, &config_buf, config_len) {
            crate::serial_println!("[xhci] Port {}: configure_hid failed: {}", port, e);
        }
    }

    Ok(())
}

// =============================================================================
// Initialization
// =============================================================================

/// Set up PCI MSI for the XHCI controller through GICv2m.
///
/// Walks the PCI capability list to find the MSI capability, probes for
/// GICv2m at the known Parallels address, allocates an SPI, programs the
/// MSI registers, configures the GIC, and enables the interrupt.
///
/// Returns the GIC INTID (SPI number) for the allocated interrupt.
/// Falls back to polling (returns 0) if MSI or GICv2m is unavailable.
fn setup_xhci_msi(pci_dev: &crate::drivers::pci::Device) -> u32 {
    use crate::arch_impl::aarch64::gic;

    // Step 1: Find MSI capability in PCI config space
    let msi_cap = match pci_dev.find_msi_capability() {
        Some(offset) => {
            crate::serial_println!("[xhci] Found MSI capability at PCI config offset {:#x}", offset);
            offset
        }
        None => {
            crate::serial_println!("[xhci] No MSI capability found, using polling mode");
            return 0;
        }
    };

    // Step 2: Probe for GICv2m
    // On Parallels ARM64, GICv2m is at 0x02250000 (discovered from MADT).
    const PARALLELS_GICV2M_BASE: u64 = 0x0225_0000;
    let gicv2m_base = crate::platform_config::gicv2m_base_phys();
    let (base, spi_base, spi_count) = if gicv2m_base != 0 {
        // Already probed
        (
            gicv2m_base,
            crate::platform_config::gicv2m_spi_base(),
            crate::platform_config::gicv2m_spi_count(),
        )
    } else if crate::platform_config::probe_gicv2m(PARALLELS_GICV2M_BASE) {
        (
            PARALLELS_GICV2M_BASE,
            crate::platform_config::gicv2m_spi_base(),
            crate::platform_config::gicv2m_spi_count(),
        )
    } else {
        crate::serial_println!("[xhci] GICv2m not found at {:#x}, using polling mode", PARALLELS_GICV2M_BASE);
        return 0;
    };

    crate::serial_println!(
        "[xhci] GICv2m at {:#x}: SPI base={}, count={}",
        base, spi_base, spi_count,
    );

    if spi_count == 0 {
        crate::serial_println!("[xhci] GICv2m has no available SPIs");
        return 0;
    }

    // Step 3: Allocate first available SPI for XHCI
    let spi = spi_base;
    let intid = spi; // GIC INTID = SPI number for GICv2m

    // Step 4: Program PCI MSI registers
    // MSI address = GICv2m doorbell (MSI_SETSPI_NS at offset 0x40)
    let msi_address = (base + 0x40) as u32;
    let msi_data = spi as u16;
    pci_dev.configure_msi(msi_cap, msi_address, msi_data);
    pci_dev.disable_intx();

    crate::serial_println!(
        "[xhci] MSI configured: address={:#010x} data={:#06x} (SPI {}, INTID {})",
        msi_address, msi_data, spi, intid,
    );

    // Step 5: Configure GIC for this SPI (edge-triggered).
    //
    // The SPI is NOT enabled here — init() enables it after disabling IMAN.IE
    // to prevent an interrupt storm. With IMAN.IE=0, the XHCI won't write MSI
    // doorbell writes, so the SPI won't fire even though it's enabled.
    gic::configure_spi_edge_triggered(intid);

    crate::serial_println!("[xhci] GIC SPI {} configured (edge-triggered, INTID {})", spi, intid);

    intid
}

/// Initialize the XHCI controller from a discovered PCI device.
///
/// Performs the full xHCI initialization sequence:
/// 1. Enable PCI bus mastering and memory space
/// 2. Map BAR0 via HHDM
/// 3. Read capability registers
/// 4. Stop and reset the controller
/// 5. Configure DCBAA, command ring, event ring
/// 6. Start the controller
/// 7. Scan ports and enumerate connected USB devices
pub fn init(pci_dev: &crate::drivers::pci::Device) -> Result<(), &'static str> {
    crate::serial_println!("[xhci] Initializing XHCI controller...");
    crate::serial_println!(
        "[xhci] PCI device: {:02x}:{:02x}.{} [{:04x}:{:04x}] IRQ={}",
        pci_dev.bus,
        pci_dev.device,
        pci_dev.function,
        pci_dev.vendor_id,
        pci_dev.device_id,
        pci_dev.interrupt_line,
    );

    // 1. Enable bus mastering + memory space
    pci_dev.enable_bus_master();
    pci_dev.enable_memory_space();

    // 2. Map BAR0 via HHDM
    let bar = pci_dev.get_mmio_bar().ok_or("XHCI: no MMIO BAR found")?;
    crate::serial_println!(
        "[xhci] BAR0: phys={:#010x} size={:#x}",
        bar.address,
        bar.size,
    );
    let base = HHDM_BASE + bar.address;

    // 3. Read capability registers
    let cap_word = read32(base);
    let cap_length = (cap_word & 0xFF) as u8;
    let hci_version = (cap_word >> 16) & 0xFFFF;

    let hcsparams1 = read32(base + 0x04);
    let hcsparams2 = read32(base + 0x08);
    let hccparams1 = read32(base + 0x10);
    let db_offset = read32(base + 0x14) & !0x3u32;
    let rts_offset = read32(base + 0x18) & !0x1Fu32;

    let max_slots = (hcsparams1 & 0xFF) as u8;
    let max_intrs = ((hcsparams1 >> 8) & 0x7FF) as u16;
    let max_ports = ((hcsparams1 >> 24) & 0xFF) as u8;
    let context_size = if hccparams1 & (1 << 2) != 0 { 64 } else { 32 };

    // Check for scratchpad buffers
    let scratch_hi = (hcsparams2 >> 21) & 0x1F;
    let scratch_lo = (hcsparams2 >> 27) & 0x1F;
    let num_scratch = (scratch_hi << 5) | scratch_lo;

    let op_base = base + cap_length as u64;
    let rt_base = base + rts_offset as u64;
    let db_base = base + db_offset as u64;

    crate::serial_println!(
        "[xhci] Capabilities: version={:#06x} caplength={} max_slots={} max_ports={} max_intrs={} ctx_size={} scratch={}",
        hci_version,
        cap_length,
        max_slots,
        max_ports,
        max_intrs,
        context_size,
        num_scratch,
    );
    crate::serial_println!(
        "[xhci] Offsets: op={:#x} rt={:#x} db={:#x}",
        cap_length,
        rts_offset,
        db_offset,
    );

    // 3b. Walk Extended Capabilities list for Supported Protocol info.
    // HCCPARAMS1 bits 31:16 = xECP (xHCI Extended Capabilities Pointer) in DWORDs from base.
    let xecp_offset = ((hccparams1 >> 16) & 0xFFFF) as u64;
    if xecp_offset != 0 {
        let mut ecap_addr = base + xecp_offset * 4;
        for _ in 0..16 {
            let ecap_dw0 = read32(ecap_addr);
            let cap_id = ecap_dw0 & 0xFF;
            let next_ptr = (ecap_dw0 >> 8) & 0xFF;

            if cap_id == 2 {
                // Supported Protocol Capability (ID=2)
                // DW0: cap_id(7:0), next(15:8), minor_rev(23:16), major_rev(31:24)
                let minor_rev = (ecap_dw0 >> 16) & 0xFF;
                let major_rev = (ecap_dw0 >> 24) & 0xFF;
                // DW1: Name String (ASCII, e.g., "USB ")
                let name = read32(ecap_addr + 4);
                // DW2: compatible_port_offset(7:0), compatible_port_count(15:8),
                //       protocol_defined(27:16), protocol_speed_id_count(31:28)
                let dw2 = read32(ecap_addr + 8);
                let port_offset = dw2 & 0xFF;
                let port_count = (dw2 >> 8) & 0xFF;
                // DW3: protocol slot type (3:0)
                let _dw3 = read32(ecap_addr + 12);

                let name_bytes = name.to_le_bytes();
                crate::serial_println!(
                    "[xhci] Supported Protocol: USB {}.{} name='{}{}{}{}' ports={}-{} (offset={} count={})",
                    major_rev, minor_rev,
                    name_bytes[0] as char, name_bytes[1] as char,
                    name_bytes[2] as char, name_bytes[3] as char,
                    port_offset, port_offset + port_count - 1,
                    port_offset, port_count,
                );
            } else if cap_id != 0 {
                crate::serial_println!("[xhci] ExtCap ID={} at offset {:#x}", cap_id, ecap_addr - base);
            }

            if next_ptr == 0 {
                break;
            }
            ecap_addr += next_ptr as u64 * 4;
        }
    } else {
        crate::serial_println!("[xhci] No Extended Capabilities list");
    }

    // 4. Stop controller: clear USBCMD.RS, wait for USBSTS.HCH
    let usbcmd = read32(op_base);
    if usbcmd & 1 != 0 {
        // Controller is running, stop it
        write32(op_base, usbcmd & !1);
        wait_for(|| read32(op_base + 0x04) & 1 != 0, 100_000)
            .map_err(|_| "XHCI: timeout waiting for HCH")?;
        crate::serial_println!("[xhci] Controller stopped");
    }

    // 5. Reset: set USBCMD.HCRST, wait for clear
    write32(op_base, read32(op_base) | 2);
    wait_for(|| read32(op_base) & 2 == 0, 100_000)
        .map_err(|_| "XHCI: timeout waiting for HCRST clear")?;
    // Wait for CNR (Controller Not Ready, bit 11 of USBSTS) to clear
    wait_for(|| read32(op_base + 0x04) & (1 << 11) == 0, 100_000)
        .map_err(|_| "XHCI: timeout waiting for CNR clear")?;
    crate::serial_println!("[xhci] Controller reset complete");

    // 6. Set MaxSlotsEn
    let slots_en = max_slots.min(MAX_SLOTS as u8);
    write32(op_base + 0x38, slots_en as u32); // CONFIG register
    crate::serial_println!("[xhci] MaxSlotsEn set to {}", slots_en);

    // 6b. Set DNCTRL (Device Notification Control) — match Linux (0x02)
    // Bit 1 (N1) enables Function Wake device notifications.
    write32(op_base + 0x14, 0x02);
    crate::serial_println!("[xhci] DNCTRL set to {:#06x}", read32(op_base + 0x14));

    // 7. Set DCBAAP (Device Context Base Address Array Pointer)
    let dcbaa_phys = virt_to_phys((&raw const DCBAA) as u64);
    unsafe {
        // Zero the DCBAA (256 u64 entries)
        let dcbaa = &raw mut DCBAA;
        core::ptr::write_bytes((*dcbaa).0.as_mut_ptr(), 0, 256);
        dma_cache_clean((*dcbaa).0.as_ptr() as *const u8, 256 * core::mem::size_of::<u64>());
    }
    write64(op_base + 0x30, dcbaa_phys);
    crate::serial_println!("[xhci] DCBAAP set to phys={:#010x}", dcbaa_phys);

    // 8. Set Command Ring Control Register (CRCR)
    let cmd_ring_phys = virt_to_phys((&raw const CMD_RING) as u64);
    unsafe {
        // Zero the command ring (CMD_RING_SIZE Trb entries)
        let ring = &raw mut CMD_RING;
        core::ptr::write_bytes((*ring).0.as_mut_ptr(), 0, CMD_RING_SIZE);
        CMD_RING_ENQUEUE = 0;
        CMD_RING_CYCLE = true;
        dma_cache_clean((*ring).0.as_ptr() as *const u8, CMD_RING_SIZE * core::mem::size_of::<Trb>());
    }
    // CRCR: physical address | RCS (Ring Cycle State) = 1
    write64(op_base + 0x18, cmd_ring_phys | 1);
    crate::serial_println!("[xhci] CRCR set to phys={:#010x}", cmd_ring_phys);

    // 9. Set up Event Ring for Interrupter 0
    let event_ring_phys = virt_to_phys((&raw const EVENT_RING) as u64);
    let erst_phys = virt_to_phys((&raw const ERST) as u64);

    unsafe {
        // Zero the event ring (EVENT_RING_SIZE Trb entries)
        let ering = &raw mut EVENT_RING;
        core::ptr::write_bytes((*ering).0.as_mut_ptr(), 0, EVENT_RING_SIZE);
        EVENT_RING_DEQUEUE = 0;
        EVENT_RING_CYCLE = true;
        dma_cache_clean((*ering).0.as_ptr() as *const u8, EVENT_RING_SIZE * core::mem::size_of::<Trb>());

        // Set up ERST entry
        let erst = &raw mut ERST;
        (*erst).0[0] = ErstEntry {
            base: event_ring_phys,
            size: EVENT_RING_SIZE as u32,
            _rsvd: 0,
        };
        dma_cache_clean((*erst).0.as_ptr() as *const u8, core::mem::size_of::<ErstEntry>());
    }

    let ir0 = rt_base + 0x20; // Interrupter 0 register set

    // ERSTSZ (Event Ring Segment Table Size) = 1 segment
    write32(ir0 + 0x08, 1);
    // ERDP (Event Ring Dequeue Pointer) = start of event ring
    write64(ir0 + 0x18, event_ring_phys);
    // ERSTBA (Event Ring Segment Table Base Address) - must be written AFTER ERSTSZ
    write64(ir0 + 0x10, erst_phys);

    crate::serial_println!(
        "[xhci] Event ring: phys={:#010x} ERST phys={:#010x}",
        event_ring_phys,
        erst_phys,
    );

    // 10. Enable interrupts on Interrupter 0
    // Set IMOD (Interrupt Moderation) — match Linux (0xa0 = 160 * 250ns = 40µs)
    write32(ir0 + 0x04, 0x000000a0);
    crate::serial_println!("[xhci] IMOD set to {:#06x}", read32(ir0 + 0x04));
    let iman = read32(ir0);
    write32(ir0, iman | 2); // IMAN.IE = 1

    // 11. Start controller: USBCMD.RS=1, INTE=1
    let usbcmd = read32(op_base);
    write32(op_base, usbcmd | 1 | (1 << 2)); // RS=1, INTE=1
    crate::serial_println!("[xhci] Controller started (USBCMD={:#010x})", read32(op_base));

    // 12. Set up PCI MSI AFTER starting the controller.
    //
    // NOTE: Linux configures MSI before RS=1 (via xhci_try_enable_msi).
    // However, configuring MSI before RS=1 on the Parallels virtual xHC
    // causes the timer interrupt to stop firing (PPI 27 dead). This appears
    // to be a virtualization issue where MSI writes to GICv2m SET_SPI_NS
    // interfere with the virtual GIC's PPI routing.
    //
    // Configuring MSI after RS=1 works reliably. SPI is NOT enabled here —
    // it's deferred to poll_hid_events after XHCI_INITIALIZED.
    let irq = setup_xhci_msi(pci_dev);
    XHCI_IRQ.store(irq, Ordering::Release);

    // Wait a bit for ports to detect connections
    for _ in 0..100_000 {
        core::hint::spin_loop();
    }

    // Verify controller is running
    let usbsts = read32(op_base + 0x04);
    if usbsts & 1 != 0 {
        crate::serial_println!("[xhci] WARNING: Controller halted after start (USBSTS={:#010x})", usbsts);
    }

    // 13. Create state with IRQ already set
    let mut xhci_state = XhciState {
        base,
        cap_length,
        op_base,
        rt_base,
        db_base,
        max_slots: slots_en,
        max_ports,
        context_size,
        irq,
        kbd_slot: 0,
        kbd_endpoint: 0,
        kbd_nkro_endpoint: 0,
        mouse_slot: 0,
        mouse_endpoint: 0,
    };

    // 14. Scan ports and configure HID devices.
    //
    // MSI is configured at PCI level (address/data written to xHC before RS=1,
    // matching Linux's pci_alloc_irq_vectors). GIC SPI is NOT yet enabled —
    // enumeration uses direct event ring polling via wait_for_event/wait_for_command.
    if let Err(e) = scan_ports(&mut xhci_state) {
        crate::serial_println!("[xhci] Port scanning error: {}", e);
    }

    crate::serial_println!(
        "[xhci] Scan complete: kbd_slot={} kbd_ep={} kbd_nkro_ep={} mouse_slot={} mouse_ep={}",
        xhci_state.kbd_slot,
        xhci_state.kbd_endpoint,
        xhci_state.kbd_nkro_endpoint,
        xhci_state.mouse_slot,
        xhci_state.mouse_endpoint,
    );

    // 15. Store state and set INITIALIZED before enabling GIC SPI.
    //
    // Once the SPI is enabled, pending MSI writes will immediately fire
    // the interrupt handler. The handler needs XHCI_INITIALIZED=true and
    // XHCI_STATE=Some to process events correctly.
    unsafe {
        *(&raw mut XHCI_STATE) = Some(xhci_state);
    }
    XHCI_INITIALIZED.store(true, Ordering::Release);

    // 16. Queue initial HID transfers.
    //
    // NOTE: test_sync_class_requests is intentionally DISABLED. It sends
    // GET_REPORT on EP0 which writes stale data to KBD_REPORT_BUF. The Parallels
    // xHCI emulation appears to cache this response and replay it on the interrupt
    // endpoint, causing all interrupt transfers to return the GET_REPORT setup
    // packet echo instead of actual HID reports. Linux doesn't do GET_REPORT
    // during HID init — it just queues the interrupt URB and waits.
    let xhci_state_ref = unsafe {
        (*(&raw const XHCI_STATE)).as_ref().unwrap()
    };

    // Diagnostic: dump endpoint context states AFTER all init (Phase 1-3 complete).
    // This verifies SET_CONFIGURATION and Phase 3 didn't reset the endpoint states.
    dump_endpoint_contexts(xhci_state_ref);

    // Drain stale events and re-queue HID transfer TRBs.
    // Initial TRBs were already queued inline in configure_hid() Phase 3 to prevent
    // the Parallels vxHC from stopping endpoints during the scan_ports gap.
    // start_hid_polling drains any leftover events and queues fresh TRBs.
    start_hid_polling(xhci_state_ref);
    HID_POLLING_STARTED.store(true, Ordering::Release);

    // Synchronous diagnostic: wait for the first Transfer Event right after
    // queueing TRBs. This happens during init (before timer), so we get
    // immediate feedback without any concurrency issues.
    crate::serial_println!("[xhci] Waiting for first Transfer Event (sync)...");
    unsafe {
        let mut timeout = 5_000_000u32;
        loop {
            let ring = &raw const EVENT_RING;
            let idx = EVENT_RING_DEQUEUE;
            let cycle = EVENT_RING_CYCLE;

            dma_cache_invalidate(
                &(*ring).0[idx] as *const Trb as *const u8,
                core::mem::size_of::<Trb>(),
            );

            let trb = core::ptr::read_volatile(&(*ring).0[idx]);
            let trb_cycle = trb.control & 1 != 0;

            if trb_cycle == cycle {
                let tt = trb.trb_type();
                let cc = trb.completion_code();
                let slot = trb.slot_id();
                let ep = (trb.control >> 16) & 0x1F;
                crate::serial_println!(
                    "[xhci] SYNC event: type={} CC={} slot={} ep={} param={:#010x} status={:#010x} control={:#010x}",
                    tt, cc, slot, ep, trb.param, trb.status, trb.control,
                );

                // Advance dequeue
                EVENT_RING_DEQUEUE = (idx + 1) % EVENT_RING_SIZE;
                if EVENT_RING_DEQUEUE == 0 {
                    EVENT_RING_CYCLE = !cycle;
                }
                let ir0 = xhci_state_ref.rt_base + 0x20;
                let erdp_phys = virt_to_phys(&raw const EVENT_RING as u64)
                    + (EVENT_RING_DEQUEUE as u64) * 16;
                write64(ir0 + 0x18, erdp_phys | (1 << 3));
                break;
            }

            timeout -= 1;
            if timeout == 0 {
                crate::serial_println!("[xhci] SYNC: No event within timeout (5M spins)");
                break;
            }
            core::hint::spin_loop();
        }
    }

    crate::serial_println!("[xhci] Initialization complete (MSI IRQ={})", irq);

    Ok(())
}

// =============================================================================
// Interrupt Handling
// =============================================================================

/// Handle an XHCI interrupt.
///
/// Called from the GIC interrupt handler when the XHCI IRQ fires.
/// Immediately disables the GIC SPI to prevent re-delivery storms,
/// then processes all pending events. The SPI is re-enabled by
/// poll_hid_events() on the next timer tick (~5ms later).
pub fn handle_interrupt() {
    if !XHCI_INITIALIZED.load(Ordering::Acquire) {
        // SPI should not be enabled during init (it's deferred until
        // XHCI_INITIALIZED), but if we somehow get here, disable it
        // to prevent a storm.
        let irq = XHCI_IRQ.load(Ordering::Relaxed);
        if irq != 0 {
            crate::arch_impl::aarch64::gic::disable_spi(irq);
            crate::arch_impl::aarch64::gic::clear_spi_pending(irq);
        }
        return;
    }

    let state = unsafe {
        match (*(&raw const XHCI_STATE)).as_ref() {
            Some(s) => s,
            None => return,
        }
    };

    // Disable the GIC SPI FIRST to prevent continuous MSI re-delivery.
    // The Parallels virtual xHC generates new MSIs on every IMAN/ERDP
    // acknowledgment, causing an infinite loop that starves the timer.
    //
    // disable_spi includes DSB+ISB to ensure the GICD write completes
    // before we touch xHC registers (which could trigger new MSIs).
    // clear_spi_pending removes any MSI that arrived between the GIC
    // delivering this interrupt and the disable taking effect.
    crate::serial_aarch64::raw_serial_char(b'I'); // breadcrumb: ISR entry
    if state.irq != 0 {
        crate::arch_impl::aarch64::gic::disable_spi(state.irq);
        crate::arch_impl::aarch64::gic::clear_spi_pending(state.irq);
    }
    crate::serial_aarch64::raw_serial_char(b'D'); // breadcrumb: SPI disabled

    // try_lock: IRQ context must never spin on a lock.
    let _guard = match XHCI_LOCK.try_lock() {
        Some(g) => g,
        None => return, // Lock contended, skip — poll_hid_events will handle events
    };

    // Acknowledge IMAN and USBSTS
    let ir0 = state.rt_base + 0x20;
    let iman = read32(ir0);
    if iman & 1 != 0 {
        write32(ir0, iman | 1); // W1C to clear IP
    }
    let usbsts = read32(state.op_base + 0x04);
    if usbsts & (1 << 3) != 0 {
        write32(state.op_base + 0x04, 1 << 3);
    }

    // Process all pending events
    loop {
        unsafe {
            let ring = &raw const EVENT_RING;
            let idx = EVENT_RING_DEQUEUE;
            let cycle = EVENT_RING_CYCLE;

            // Invalidate cache to see controller-written TRBs
            dma_cache_invalidate(
                &(*ring).0[idx] as *const Trb as *const u8,
                core::mem::size_of::<Trb>(),
            );

            let trb = core::ptr::read_volatile(&(*ring).0[idx]);
            let trb_cycle = trb.control & 1 != 0;

            if trb_cycle != cycle {
                break; // No more events
            }

            MSI_EVENT_COUNT.fetch_add(1, Ordering::Relaxed);

            let trb_type_val = trb.trb_type();
            match trb_type_val {
                trb_type::TRANSFER_EVENT => {
                    let slot = trb.slot_id();
                    let endpoint = ((trb.control >> 16) & 0x1F) as u8;
                    let cc = trb.completion_code();

                    if cc == completion_code::SUCCESS || cc == completion_code::SHORT_PACKET {
                        // NKRO keyboard (DCI 5, interface 1) — check first
                        if slot == state.kbd_slot
                            && state.kbd_nkro_endpoint != 0
                            && endpoint == state.kbd_nkro_endpoint
                        {
                            NKRO_EVENT_COUNT.fetch_add(1, Ordering::Relaxed);
                            let report_buf = &raw const NKRO_REPORT_BUF;
                            dma_cache_invalidate((*report_buf).0.as_ptr(), 9);
                            let nkro = &(*report_buf).0;
                            let nkro_snap = u64::from_le_bytes([
                                nkro[0], nkro[1], nkro[2], nkro[3],
                                nkro[4], nkro[5], nkro[6], nkro[7],
                            ]);
                            LAST_NKRO_REPORT_U64.store(nkro_snap, Ordering::Relaxed);
                            if nkro[0] == 1 {
                                KBD_EVENT_COUNT.fetch_add(1, Ordering::Relaxed);
                                super::hid::process_keyboard_report(&nkro[1..9]);
                            }
                            MSI_NKRO_NEEDS_REQUEUE.store(true, Ordering::Release);
                        }
                        // Boot keyboard (DCI 3, interface 0)
                        else if slot == state.kbd_slot && endpoint == state.kbd_endpoint {
                            KBD_EVENT_COUNT.fetch_add(1, Ordering::Relaxed);
                            let report_buf = &raw const KBD_REPORT_BUF;
                            dma_cache_invalidate((*report_buf).0.as_ptr(), 8);
                            let report = &(*report_buf).0;
                            super::hid::process_keyboard_report(report);
                            // DON'T requeue here — let the timer poll requeue.
                            // Requeuing from IRQ context creates an MSI storm
                            // (virtual XHCI has no bus latency, so completions
                            // fire instantly, starving the main thread).
                            MSI_KBD_NEEDS_REQUEUE.store(true, Ordering::Release);
                        } else if slot == state.mouse_slot && endpoint == state.mouse_endpoint {
                            let report_buf = &raw const MOUSE_REPORT_BUF;
                            dma_cache_invalidate((*report_buf).0.as_ptr(), 8);
                            let report = &(*report_buf).0;
                            super::hid::process_mouse_report(report);
                            MSI_MOUSE_NEEDS_REQUEUE.store(true, Ordering::Release);
                        }
                        // EP0 GET_REPORT completion (DCI=1, boot keyboard)
                        else if endpoint == 1 && slot == state.kbd_slot
                            && EP0_POLL_STATE.load(Ordering::Acquire) == 1
                        {
                            let data_buf = &raw const CTRL_DATA_BUF;
                            dma_cache_invalidate((*data_buf).0.as_ptr(), 8);
                            let buf = &(*data_buf).0;
                            super::hid::process_keyboard_report(buf);
                            KBD_EVENT_COUNT.fetch_add(1, Ordering::Relaxed);
                            EP0_GET_REPORT_OK.fetch_add(1, Ordering::Relaxed);
                            EP0_POLL_STATE.store(0, Ordering::Release);
                        }
                        // EP0 GET_REPORT completion (DCI=1, mouse)
                        else if endpoint == 1 && slot == state.mouse_slot
                            && EP0_MOUSE_POLL_STATE.load(Ordering::Acquire) == 1
                        {
                            let data_buf = &raw const MOUSE_CTRL_DATA_BUF;
                            dma_cache_invalidate((*data_buf).0.as_ptr(), 16);
                            let buf = &(*data_buf).0;
                            super::hid::process_mouse_report(buf);
                            EP0_MOUSE_GET_REPORT_OK.fetch_add(1, Ordering::Relaxed);
                            EP0_MOUSE_POLL_STATE.store(0, Ordering::Release);
                        }
                    } else {
                        // Error CC (e.g., CC=12 Endpoint Not Enabled) —
                        // set recovery flags for poll_hid_events to handle.
                        // Don't attempt recovery from IRQ context.
                        XO_ERR_COUNT.fetch_add(1, Ordering::Relaxed);
                        XO_LAST_INFO.store(
                            ((slot as u64) << 16) | ((endpoint as u64) << 8) | (cc as u64),
                            Ordering::Relaxed,
                        );
                        if endpoint == 1 && slot == state.kbd_slot
                            && EP0_POLL_STATE.load(Ordering::Acquire) == 1
                        {
                            EP0_GET_REPORT_ERR.fetch_add(1, Ordering::Relaxed);
                            EP0_POLL_STATE.store(0, Ordering::Release);
                        } else if endpoint == 1 && slot == state.mouse_slot
                            && EP0_MOUSE_POLL_STATE.load(Ordering::Acquire) == 1
                        {
                            EP0_MOUSE_GET_REPORT_ERR.fetch_add(1, Ordering::Relaxed);
                            EP0_MOUSE_POLL_STATE.store(0, Ordering::Release);
                        } else if slot == state.kbd_slot && endpoint == state.kbd_endpoint {
                            NEEDS_RESET_KBD_BOOT.store(true, Ordering::Release);
                        } else if slot == state.kbd_slot
                            && state.kbd_nkro_endpoint != 0
                            && endpoint == state.kbd_nkro_endpoint
                        {
                            NEEDS_RESET_KBD_NKRO.store(true, Ordering::Release);
                        } else if slot == state.mouse_slot && endpoint == state.mouse_endpoint {
                            NEEDS_RESET_MOUSE.store(true, Ordering::Release);
                        }
                    }
                }
                trb_type::COMMAND_COMPLETION => {
                    // Command completions during enumeration are handled by wait_for_event.
                    // Any stray completions during interrupt handling are ignored.
                }
                trb_type::PORT_STATUS_CHANGE => {
                    // Port status change - don't log from IRQ context (deadlock risk
                    // with serial lock). Hot-plug not supported yet.
                }
                _ => {
                    // Unknown event type
                }
            }

            // Advance dequeue pointer
            EVENT_RING_DEQUEUE = (idx + 1) % EVENT_RING_SIZE;
            if EVENT_RING_DEQUEUE == 0 {
                EVENT_RING_CYCLE = !cycle;
            }

            // Update ERDP with EHB bit to acknowledge
            let erdp_phys = virt_to_phys(&raw const EVENT_RING as u64)
                + (EVENT_RING_DEQUEUE as u64) * 16;
            write64(ir0 + 0x18, erdp_phys | (1 << 3));
        }
    }

    crate::serial_aarch64::raw_serial_char(b'i'); // breadcrumb: ISR exit
}

// =============================================================================
// Polling Mode (fallback for systems without interrupt support)
// =============================================================================

/// Poll for HID events without relying on interrupts.
///
/// Called from the timer interrupt at ~200 Hz. Uses `try_lock()` to avoid
/// deadlocking if the lock is held by non-interrupt code. Bypasses the
/// IMAN.IP check since that may not be set without a wired interrupt line.
pub fn poll_hid_events() {
    if !XHCI_INITIALIZED.load(Ordering::Acquire) {
        return;
    }

    POLL_COUNT.fetch_add(1, Ordering::Relaxed);

    // try_lock: if someone else holds the lock, skip this poll cycle
    let _guard = match XHCI_LOCK.try_lock() {
        Some(g) => g,
        None => return,
    };

    let state = unsafe {
        match (*(&raw const XHCI_STATE)).as_ref() {
            Some(s) => s,
            None => return,
        }
    };

    let ir0 = state.rt_base + 0x20;

    // Clear IMAN.IP and USBSTS.EINT if set (acknowledge any pending state)
    let iman = read32(ir0);
    if iman & 1 != 0 {
        write32(ir0, iman | 1); // W1C to clear IP
    }
    let usbsts = read32(state.op_base + 0x04);
    if usbsts & (1 << 3) != 0 {
        write32(state.op_base + 0x04, 1 << 3); // W1C to clear EINT
    }

    // Process all pending events on the event ring
    loop {
        unsafe {
            let ring = &raw const EVENT_RING;
            let idx = EVENT_RING_DEQUEUE;
            let cycle = EVENT_RING_CYCLE;

            dma_cache_invalidate(
                &(*ring).0[idx] as *const Trb as *const u8,
                core::mem::size_of::<Trb>(),
            );

            let trb = core::ptr::read_volatile(&(*ring).0[idx]);
            let trb_cycle = trb.control & 1 != 0;

            if trb_cycle != cycle {
                break; // No more events
            }

            let _evt_num = EVENT_COUNT.fetch_add(1, Ordering::Relaxed);

            let trb_type_val = trb.trb_type();

            // No serial_println here — this runs in timer interrupt context.
            // Use atomic counters (reported by heartbeat) instead of logging.

            match trb_type_val {
                trb_type::TRANSFER_EVENT => {
                    let slot = trb.slot_id();
                    let endpoint = ((trb.control >> 16) & 0x1F) as u8;
                    let cc = trb.completion_code();
                    // Record first Transfer Event details (CC, TRB Pointer, slot/ep)
                    let _ = DIAG_FIRST_XFER_PTR.compare_exchange(
                        0, trb.param, Ordering::AcqRel, Ordering::Relaxed,
                    );
                    let _ = DIAG_FIRST_XFER_STATUS.compare_exchange(
                        0, trb.status | 1, Ordering::AcqRel, Ordering::Relaxed,
                    );
                    let _ = DIAG_FIRST_XFER_CONTROL.compare_exchange(
                        0, trb.control | 1, Ordering::AcqRel, Ordering::Relaxed,
                    );
                    let _ = DIAG_FIRST_XFER_SLEP.compare_exchange(
                        0, ((slot as u32) << 8) | (endpoint as u32),
                        Ordering::AcqRel, Ordering::Relaxed,
                    );
                    // Record first Transfer Event CC (0xFF = unset sentinel)
                    let _ = DIAG_FIRST_XFER_CC.compare_exchange(
                        0xFF, cc, Ordering::AcqRel, Ordering::Relaxed,
                    );

                    if cc == completion_code::SUCCESS || cc == completion_code::SHORT_PACKET {
                        // NKRO keyboard interrupt endpoint (DCI 5, interface 1)
                        // Parallels sends actual keystrokes on this endpoint.
                        if slot == state.kbd_slot
                            && state.kbd_nkro_endpoint != 0
                            && endpoint == state.kbd_nkro_endpoint
                        {
                            NKRO_EVENT_COUNT.fetch_add(1, Ordering::Relaxed);
                            DMA_SENTINEL_REPLACED.fetch_add(1, Ordering::SeqCst);

                            let report_buf = &raw const NKRO_REPORT_BUF;
                            dma_cache_invalidate((*report_buf).0.as_ptr(), 9);
                            let nkro = &(*report_buf).0;

                            // Store first 8 bytes for heartbeat diagnostics
                            let nkro_snap = u64::from_le_bytes([
                                nkro[0], nkro[1], nkro[2], nkro[3],
                                nkro[4], nkro[5], nkro[6], nkro[7],
                            ]);
                            LAST_NKRO_REPORT_U64.store(nkro_snap, Ordering::Relaxed);

                            // NKRO reports have Report ID prefix:
                            //   [report_id=1, modifiers, reserved, key1..key6] = 9 bytes
                            // Strip the Report ID byte and pass the standard 8-byte
                            // boot keyboard report format to process_keyboard_report.
                            if nkro[0] == 1 {
                                KBD_EVENT_COUNT.fetch_add(1, Ordering::Relaxed);
                                super::hid::process_keyboard_report(&nkro[1..9]);
                            }

                            let _ = queue_hid_transfer(state, 2, state.kbd_slot, state.kbd_nkro_endpoint);
                        }
                        // Boot keyboard interrupt endpoint (DCI 3, interface 0)
                        else if slot == state.kbd_slot
                            && endpoint == state.kbd_endpoint
                        {
                            KBD_EVENT_COUNT.fetch_add(1, Ordering::Relaxed);
                            DMA_SENTINEL_REPLACED.fetch_add(1, Ordering::SeqCst);

                            let report_buf = &raw const KBD_REPORT_BUF;
                            dma_cache_invalidate((*report_buf).0.as_ptr(), 8);
                            let report = &(*report_buf).0;

                            super::hid::process_keyboard_report(report);
                            let _ = queue_hid_transfer(state, 0, state.kbd_slot, state.kbd_endpoint);
                        }
                        // Mouse interrupt endpoint event
                        else if slot == state.mouse_slot && endpoint == state.mouse_endpoint {
                            let report_buf = &raw const MOUSE_REPORT_BUF;
                            dma_cache_invalidate((*report_buf).0.as_ptr(), 8);
                            let report = &(*report_buf).0;
                            super::hid::process_mouse_report(report);
                            let _ = queue_hid_transfer(state, 1, state.mouse_slot, state.mouse_endpoint);
                        }
                        // EP0 GET_REPORT completion (DCI=1, boot keyboard)
                        else if endpoint == 1 && slot == state.kbd_slot
                            && EP0_POLL_STATE.load(Ordering::Acquire) == 1
                        {
                            let data_buf = &raw const CTRL_DATA_BUF;
                            dma_cache_invalidate((*data_buf).0.as_ptr(), 8);
                            let buf = &(*data_buf).0;
                            super::hid::process_keyboard_report(buf);
                            KBD_EVENT_COUNT.fetch_add(1, Ordering::Relaxed);
                            EP0_GET_REPORT_OK.fetch_add(1, Ordering::Relaxed);
                            EP0_POLL_STATE.store(0, Ordering::Release);
                        }
                        // EP0 GET_REPORT completion (DCI=1, mouse)
                        else if endpoint == 1 && slot == state.mouse_slot
                            && EP0_MOUSE_POLL_STATE.load(Ordering::Acquire) == 1
                        {
                            let data_buf = &raw const MOUSE_CTRL_DATA_BUF;
                            dma_cache_invalidate((*data_buf).0.as_ptr(), 16);
                            let buf = &(*data_buf).0;
                            super::hid::process_mouse_report(buf);
                            EP0_MOUSE_GET_REPORT_OK.fetch_add(1, Ordering::Relaxed);
                            EP0_MOUSE_POLL_STATE.store(0, Ordering::Release);
                        } else {
                            XFER_OTHER_COUNT.fetch_add(1, Ordering::Relaxed);
                            XO_LAST_INFO.store(
                                ((slot as u64) << 16) | ((endpoint as u64) << 8) | (cc as u64),
                                Ordering::Relaxed,
                            );
                        }
                    } else {
                        // Error CC — set recovery flags
                        XFER_OTHER_COUNT.fetch_add(1, Ordering::Relaxed);
                        XO_ERR_COUNT.fetch_add(1, Ordering::Relaxed);
                        XO_LAST_INFO.store(
                            ((slot as u64) << 16) | ((endpoint as u64) << 8) | (cc as u64),
                            Ordering::Relaxed,
                        );
                        if endpoint == 1 && slot == state.kbd_slot
                            && EP0_POLL_STATE.load(Ordering::Acquire) == 1
                        {
                            EP0_GET_REPORT_ERR.fetch_add(1, Ordering::Relaxed);
                            EP0_POLL_STATE.store(0, Ordering::Release);
                        } else if endpoint == 1 && slot == state.mouse_slot
                            && EP0_MOUSE_POLL_STATE.load(Ordering::Acquire) == 1
                        {
                            EP0_MOUSE_GET_REPORT_ERR.fetch_add(1, Ordering::Relaxed);
                            EP0_MOUSE_POLL_STATE.store(0, Ordering::Release);
                        } else if slot == state.kbd_slot && endpoint == state.kbd_endpoint {
                            NEEDS_RESET_KBD_BOOT.store(true, Ordering::Release);
                        } else if slot == state.kbd_slot
                            && state.kbd_nkro_endpoint != 0
                            && endpoint == state.kbd_nkro_endpoint
                        {
                            NEEDS_RESET_KBD_NKRO.store(true, Ordering::Release);
                        } else if slot == state.mouse_slot && endpoint == state.mouse_endpoint {
                            NEEDS_RESET_MOUSE.store(true, Ordering::Release);
                        }
                    }
                }
                trb_type::COMMAND_COMPLETION => {
                    // Stray command completions — may arrive from recovery commands.
                    // Ignore safely; wait_for_command_completion handles expected ones.
                }
                trb_type::PORT_STATUS_CHANGE => {
                    PSC_COUNT.fetch_add(1, Ordering::Relaxed);
                }
                _ => {}
            }

            // Advance dequeue pointer
            EVENT_RING_DEQUEUE = (idx + 1) % EVENT_RING_SIZE;
            if EVENT_RING_DEQUEUE == 0 {
                EVENT_RING_CYCLE = !cycle;
            }

            // Update ERDP with EHB bit
            let erdp_phys = virt_to_phys(&raw const EVENT_RING as u64)
                + (EVENT_RING_DEQUEUE as u64) * 16;
            write64(ir0 + 0x18, erdp_phys | (1 << 3));
        }
    }

    // Requeue HID transfers requested by the MSI interrupt handler.
    // The IRQ handler can't requeue directly (MSI storm on virtual XHCI).
    if MSI_KBD_NEEDS_REQUEUE.swap(false, Ordering::AcqRel) && state.kbd_slot != 0 {
        let _ = queue_hid_transfer(state, 0, state.kbd_slot, state.kbd_endpoint);
    }
    if MSI_NKRO_NEEDS_REQUEUE.swap(false, Ordering::AcqRel)
        && state.kbd_slot != 0
        && state.kbd_nkro_endpoint != 0
    {
        let _ = queue_hid_transfer(state, 2, state.kbd_slot, state.kbd_nkro_endpoint);
    }
    if MSI_MOUSE_NEEDS_REQUEUE.swap(false, Ordering::AcqRel) && state.mouse_slot != 0 {
        let _ = queue_hid_transfer(state, 1, state.mouse_slot, state.mouse_endpoint);
    }

    // EP0 GET_REPORT polling: queue periodic GET_REPORT on EP0 control pipe.
    // Interrupt endpoints always return CC=12 on Parallels virtual xHC (a
    // fundamental limitation of the virtual xHC — not fixable through command
    // sequencing). EP0 control transfers work reliably. Poll at 20 Hz (every
    // 10 timer ticks at 200 Hz = 50 ms latency).
    let poll = POLL_COUNT.load(Ordering::Relaxed);
    if EP0_POLL_STATE.load(Ordering::Acquire) == 0
        && poll >= 400       // Wait 2 seconds after boot
        && poll % 10 == 0    // 20 Hz
    {
        queue_ep0_get_report(state);
    }

    // EP0 GET_REPORT polling for mouse: same approach, staggered by 5 ticks
    // so keyboard and mouse polls don't collide in the same timer tick.
    if EP0_MOUSE_POLL_STATE.load(Ordering::Acquire) == 0
        && poll >= 425       // Wait 2+ seconds after boot
        && poll % 10 == 5    // 20 Hz, offset from keyboard
        && state.mouse_slot != 0
    {
        queue_ep0_mouse_get_report(state);
    }

    // Recover halted endpoints (CC=12 Endpoint Not Enabled, etc.)
    // Reset Endpoint + Set TR Dequeue Pointer + requeue transfer TRB.
    // Rate-limited to preserve command ring capacity (each reset uses 2 entries).
    let resets_so_far = ENDPOINT_RESET_COUNT.load(Ordering::Relaxed);
    if resets_so_far < MAX_ENDPOINT_RESETS {
        if NEEDS_RESET_KBD_BOOT.swap(false, Ordering::AcqRel)
            && state.kbd_slot != 0
            && state.kbd_endpoint != 0
        {
            let _ = reset_halted_endpoint(state, state.kbd_slot, state.kbd_endpoint, 0);
        }
        if NEEDS_RESET_KBD_NKRO.swap(false, Ordering::AcqRel)
            && state.kbd_slot != 0
            && state.kbd_nkro_endpoint != 0
        {
            let _ = reset_halted_endpoint(state, state.kbd_slot, state.kbd_nkro_endpoint, 2);
        }
        if NEEDS_RESET_MOUSE.swap(false, Ordering::AcqRel)
            && state.mouse_slot != 0
            && state.mouse_endpoint != 0
        {
            let _ = reset_halted_endpoint(state, state.mouse_slot, state.mouse_endpoint, 1);
        }
    } else {
        // Clear flags without acting — command ring capacity exhausted
        NEEDS_RESET_KBD_BOOT.store(false, Ordering::Relaxed);
        NEEDS_RESET_KBD_NKRO.store(false, Ordering::Relaxed);
        NEEDS_RESET_MOUSE.store(false, Ordering::Relaxed);
    }

    // Deferred MSI activation.
    // SPI 53 is enabled after a stabilization period (200 polls = 1 second)
    // to avoid interfering with init. Initial TRBs are queued at poll=250
    // (after SPI enable) so the full interrupt pathway is active.
    let poll = POLL_COUNT.load(Ordering::Relaxed);
    if state.irq != 0 && poll >= 200 {
        // Enable SPI for MSI delivery (handle_interrupt disables on each fire)
        crate::arch_impl::aarch64::gic::clear_spi_pending(state.irq);
        crate::arch_impl::aarch64::gic::enable_spi(state.irq);
        DIAG_SPI_ENABLE_COUNT.fetch_add(1, Ordering::Relaxed);
    }

    // Fallback TRB queueing: if inline queueing in configure_hid and
    // start_hid_polling both failed to set HID_TRBS_QUEUED, queue here.
    // This should not normally be reached.
    if poll >= 250 && !HID_TRBS_QUEUED.load(Ordering::Acquire) {
        HID_TRBS_QUEUED.store(true, Ordering::Release);
        if state.mouse_slot != 0 && state.mouse_endpoint != 0 {
            let _ = queue_hid_transfer(state, 1, state.mouse_slot, state.mouse_endpoint);
        }
        if state.kbd_slot != 0 && state.kbd_endpoint != 0 {
            let _ = queue_hid_transfer(state, 0, state.kbd_slot, state.kbd_endpoint);
        }
        if state.kbd_slot != 0 && state.kbd_nkro_endpoint != 0 {
            let _ = queue_hid_transfer(state, 2, state.kbd_slot, state.kbd_nkro_endpoint);
        }
    }

    // Periodic diagnostic: dump controller + endpoint state every 2000 polls (~10s)
    if poll > 0 && poll % 2000 == 0 {
        unsafe {
            // USBSTS
            let usbsts = read32(state.op_base + 0x04);
            DIAG_USBSTS.store(usbsts, Ordering::Relaxed);

            // PORTSC for keyboard port (port 1 = slot 2 typically)
            let kbd_port = 1u64; // Port index for keyboard
            let portsc = read32(state.op_base + 0x400 + kbd_port * 0x10);
            DIAG_KBD_PORTSC.store(portsc, Ordering::Relaxed);

            // Endpoint context state for keyboard DCI=3 and DCI=5
            if state.kbd_slot != 0 {
                let slot_idx = (state.kbd_slot - 1) as usize;
                let ctx_size = state.context_size;
                let dev_ctx = &raw const DEVICE_CONTEXTS[slot_idx];
                dma_cache_invalidate((*dev_ctx).0.as_ptr(), 4096);

                let dci3_state = if state.kbd_endpoint != 0 {
                    let ep_base = (*dev_ctx).0.as_ptr().add(state.kbd_endpoint as usize * ctx_size);
                    core::ptr::read_volatile(ep_base as *const u32) & 0x7
                } else { 0 };
                let dci5_state = if state.kbd_nkro_endpoint != 0 {
                    let ep_base = (*dev_ctx).0.as_ptr().add(state.kbd_nkro_endpoint as usize * ctx_size);
                    core::ptr::read_volatile(ep_base as *const u32) & 0x7
                } else { 0 };
                DIAG_KBD_EP_STATE.store((dci3_state << 4) | dci5_state, Ordering::Relaxed);
            }
        }
    }
}

// =============================================================================
// Public API
// =============================================================================

/// Check if the XHCI controller has been initialized.
pub fn is_initialized() -> bool {
    XHCI_INITIALIZED.load(Ordering::Acquire)
}

/// Get the GIC INTID for the XHCI controller's interrupt.
pub fn get_irq() -> Option<u32> {
    // Check XHCI_IRQ first (available earlier than XHCI_INITIALIZED)
    let irq = XHCI_IRQ.load(Ordering::Relaxed);
    if irq != 0 {
        return Some(irq);
    }
    if !XHCI_INITIALIZED.load(Ordering::Acquire) {
        return None;
    }
    unsafe { (*(&raw const XHCI_STATE)).as_ref().map(|s| s.irq) }
}
