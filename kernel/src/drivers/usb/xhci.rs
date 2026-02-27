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
    class_code, descriptor_type, hid_protocol, hid_request, request,
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
/// Linux ftrace confirmed: Linux DOES perform the bandwidth dance for HID devices
/// (3 ConfigureEndpoint commands total: 1 batch + Stop+re-ConfigEP per endpoint).
/// Linux also sends SET_CONFIGURATION AFTER the BW dance, not before.
/// Key fix: EP State bits (2:0) of DW0 must be zeroed in the re-ConfigEP input
/// context (RsvdZ per spec). Previously we copied DW0 from the output context
/// which had EP State=3 (Stopped), confusing Parallels' virtual xHC.
///
/// Set to true to test hypothesis that Parallels' virtual xHC internally rejects
/// Perform Linux-style bandwidth dance (Stop EP + re-ConfigureEndpoint).
/// Parallels virtual xHC requires this sequence to actually enable interrupt
/// endpoints; skipping it leaves endpoints in a pseudo-running state that
/// returns CC=12 on the first interrupt TRB.
const SKIP_BW_DANCE: bool = false;

/// Focus debug mode: only initialize the mouse device (slot=1), skip keyboard entirely.
/// Reduces from 4 interrupt endpoints to 2, isolating whether CC=12 is caused by
/// keyboard interference or is a fundamental per-endpoint issue.
const MOUSE_ONLY: bool = false;

/// Send SET_PROTOCOL(Boot Protocol=0) to HID interfaces.
/// Linux's usbhid driver sends SET_PROTOCOL for boot keyboard (subclass=1, protocol=1)
/// during initial enumeration, but NOT during rebind (confirmed via usbmon on linux-probe VM).
/// Setting false matches the rebind sequence Linux uses. Testing whether SET_PROTOCOL
/// is causing Parallels to internally reset interrupt endpoints (producing CC=12).


/// NEC XHCI vendor ID.
pub const NEC_VENDOR_ID: u16 = 0x1033;
/// NEC uPD720200 XHCI device ID.
pub const NEC_XHCI_DEVICE_ID: u16 = 0x0194;

/// Maximum device slots we support.
const MAX_SLOTS: usize = 32;
/// Command ring size in TRBs (last entry reserved for Link TRB).
/// The Link TRB uses TC=bit1 (Toggle Cycle) so the ring wraps indefinitely.
/// Previous "command ring fails after first wrap" was caused by a bug: the
/// Link TRB was using bit5 (IOC) instead of bit1 (TC), so the HC never
/// toggled its cycle bit on wrap and stopped seeing post-wrap commands.
const CMD_RING_SIZE: usize = 4096;
/// Event ring size in TRBs.
const EVENT_RING_SIZE: usize = 64;
/// Transfer ring size per endpoint in TRBs (last entry reserved for Link TRB).
/// Larger transfer ring reduces the number of Stop EP + Set TR Dequeue resets.
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
    /// NEC vendor-specific: Get Firmware Version (sent during xhci_run() in Linux).
    pub const NEC_GET_FW: u32 = 49;
    /// NEC vendor-specific: Command Completion event type.
    pub const NEC_CMD_COMP: u32 = 48;
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

/// Total number of transfer rings: MAX_SLOTS for EP0 + 4 for HID interrupt endpoints.
/// hid_idx 0 = boot keyboard (DCI 3), 1 = mouse (DCI 3), 2 = NKRO keyboard (DCI 5), 3 = mouse2 (DCI 5).
const NUM_TRANSFER_RINGS: usize = MAX_SLOTS + 4;

/// Transfer rings for device endpoints.
///
/// Indices [0..MAX_SLOTS): EP0 control rings, indexed by slot_idx (slot_id - 1).
/// Indices [HID_RING_BASE..HID_RING_BASE+4): HID interrupt rings (kbd boot, mouse, kbd NKRO, mouse2).
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

/// Per-slot Input Context pages for bandwidth dance re-ConfigureEndpoint.
/// Each must be at a DIFFERENT physical address than INPUT_CONTEXTS to avoid
/// a caching bug in the Parallels virtual xHC where re-ConfigureEndpoint
/// is silently ignored if the Input Context pointer matches the initial
/// ConfigureEndpoint command.
static mut RECONFIG_INPUT_CTX: [AlignedPage<[u8; 4096]>; MAX_SLOTS] =
    [const { AlignedPage([0u8; 4096]) }; MAX_SLOTS];

/// Device Contexts (output contexts, 2048 bytes each).
/// Managed by the controller; we provide physical addresses via DCBAA.
static mut DEVICE_CONTEXTS: [AlignedPage<[u8; 4096]>; MAX_SLOTS] =
    [const { AlignedPage([0u8; 4096]) }; MAX_SLOTS];

/// HID report buffer for keyboard boot interface (8 bytes: modifier + reserved + 6 keycodes).
static mut KBD_REPORT_BUF: Aligned64<[u8; 64]> = Aligned64([0u8; 64]);

/// HID report buffer for NKRO keyboard interface (64 bytes to accommodate any report ID).
/// Reports include a Report ID prefix: [report_id, modifiers, reserved, key1..key6, ...].
static mut NKRO_REPORT_BUF: Aligned64<[u8; 64]> = Aligned64([0u8; 64]);

/// HID report buffer for mouse (8 bytes: buttons + X + Y + wheel + ...).
static mut MOUSE_REPORT_BUF: Aligned64<[u8; 64]> = Aligned64([0u8; 64]);

/// HID report buffer for mouse2 (second mouse interface, DCI 5).
static mut MOUSE2_REPORT_BUF: Aligned64<[u8; 64]> = Aligned64([0u8; 64]);

/// Scratch buffer for control transfer data stages (256 bytes).
static mut CTRL_DATA_BUF: Aligned64<[u8; 256]> = Aligned64([0u8; 256]);


/// Number of successful GET_REPORT polls for mouse (for heartbeat diagnostics).
pub static GET_REPORT_OK: AtomicU64 = AtomicU64::new(0);

/// Number of non-zero GET_REPORT responses for mouse (indicates actual movement).
pub static GET_REPORT_NONZERO: AtomicU64 = AtomicU64::new(0);

/// Number of successful EP0 GET_REPORT polls for keyboard.
/// Use this in heartbeat to verify keyboard polling is active (gk= equivalent).
pub static KBD_GET_REPORT_OK: AtomicU64 = AtomicU64::new(0);


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
    /// Endpoint DCI for mouse boot interrupt IN (interface 0, DCI 3)
    mouse_endpoint: u8,
    /// Endpoint DCI for mouse2 interrupt IN (interface 1, DCI 5, 0 = not found)
    /// Linux ftrace shows the Parallels virtual mouse has two interrupt endpoints.
    mouse_nkro_endpoint: u8,
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
/// First 8 bytes of last GET_REPORT EP0 response (for heartbeat diagnostics).
/// Lets us see the raw mouse report format and detect movement vs. idle.
pub static LAST_GET_REPORT_U64: AtomicU64 = AtomicU64::new(0);
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
static MSI_MOUSE2_NEEDS_REQUEUE: AtomicBool = AtomicBool::new(false);

/// Sentinel diagnostic: counts reports where the sentinel byte (0xDE) was NOT overwritten by DMA.
pub static DMA_SENTINEL_SURVIVED: AtomicU64 = AtomicU64::new(0);
/// Sentinel diagnostic: counts reports where the sentinel byte WAS overwritten (DMA worked).
pub static DMA_SENTINEL_REPLACED: AtomicU64 = AtomicU64::new(0);

/// Periodic diagnostic: last USBSTS value (controller status).
pub static DIAG_USBSTS: AtomicU32 = AtomicU32::new(0);
/// Periodic diagnostic: last USBCMD value (Run/Stop, INTE bits).
pub static DIAG_USBCMD: AtomicU32 = AtomicU32::new(0);
/// Periodic diagnostic: last IMAN value for Interrupter 0 (IP, IE bits).
pub static DIAG_IMAN: AtomicU32 = AtomicU32::new(0);
/// Periodic diagnostic: runtime TRDP from output context for mouse EP3.
pub static DIAG_RUNTIME_TRDP: AtomicU64 = AtomicU64::new(0);
/// Periodic diagnostic: raw event ring TRB control DW at current dequeue index.
pub static DIAG_ER_TRB_CONTROL: AtomicU32 = AtomicU32::new(0);
/// Periodic diagnostic: event ring dequeue index and cycle bit packed (idx << 1 | cycle).
pub static DIAG_ER_STATE: AtomicU32 = AtomicU32::new(0);
/// Periodic diagnostic: raw transfer ring TRB control DW at position 0 for mouse ring.
pub static DIAG_TR_TRB_CONTROL: AtomicU32 = AtomicU32::new(0);
/// Periodic diagnostic: ERDP register readback value.
pub static DIAG_ERDP_READBACK: AtomicU64 = AtomicU64::new(0);
/// Periodic diagnostic: last PORTSC for keyboard port.
pub static DIAG_KBD_PORTSC: AtomicU32 = AtomicU32::new(0);
/// Periodic diagnostic: last endpoint state for keyboard DCI=3 and DCI=5 (packed: dci3 << 4 | dci5).
pub static DIAG_KBD_EP_STATE: AtomicU32 = AtomicU32::new(0);
/// Periodic diagnostic: SPI enable count (how many times SPI was re-enabled).
pub static DIAG_SPI_ENABLE_COUNT: AtomicU64 = AtomicU64::new(0);
/// Diagnostic counter for doorbell/transfer events (shown as `db=` in heartbeat).
pub static DIAG_DOORBELL_EP_STATE: AtomicU32 = AtomicU32::new(0);
/// Diagnostic: last CC received for any GET_REPORT Transfer Event (0xFF = none seen yet).
/// Shown as `fd=` in heartbeat. Distinguishes "no response" (0xFF) from bad CC (12, 4, etc.)
/// vs success (1) or short packet (13).
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

/// Flags set when Transfer Events arrive with non-CC=12 error completion codes
/// (e.g., CC=4 USB_TRANSACTION_ERROR, CC=6 STALL_ERROR). These error codes halt
/// the endpoint; poll_hid_events issues Reset Endpoint + Set TR Dequeue Pointer.
/// CC=12 (Endpoint Not Enabled) is handled separately: just re-queue the TRB.
static NEEDS_RESET_KBD_BOOT: AtomicBool = AtomicBool::new(false);
static NEEDS_RESET_KBD_NKRO: AtomicBool = AtomicBool::new(false);
static NEEDS_RESET_MOUSE: AtomicBool = AtomicBool::new(false);
static NEEDS_RESET_MOUSE2: AtomicBool = AtomicBool::new(false);
/// Diagnostic: counts successful endpoint resets.
pub static ENDPOINT_RESET_COUNT: AtomicU64 = AtomicU64::new(0);
/// Diagnostic: counts failed endpoint reset attempts.
pub static ENDPOINT_RESET_FAIL_COUNT: AtomicU64 = AtomicU64::new(0);
/// Minimum poll ticks between consecutive resets of the same endpoint.
/// At 200Hz, 20 ticks = 100ms. Prevents the CC=12 reset storm from burning
/// 200 command ring entries per second when no keys are pressed.
const RESET_INTERVAL_TICKS: u64 = 20;
/// Poll tick of the last successful reset for each HID endpoint (for rate limiting).
static KBD_BOOT_RESET_POLL: AtomicU64 = AtomicU64::new(0);
static KBD_NKRO_RESET_POLL: AtomicU64 = AtomicU64::new(0);
static MOUSE_RESET_POLL: AtomicU64 = AtomicU64::new(0);
static MOUSE2_RESET_POLL: AtomicU64 = AtomicU64::new(0);
/// Diagnostic: endpoint output context state immediately after first error CC (0xFF = not seen).
/// Packed: slot<<16 | dci<<8 | state_bits[2:0]. State: 0=Disabled, 1=Running, 3=Halted, 4=Error.
pub static DIAG_EP_STATE_AFTER_CC12: AtomicU32 = AtomicU32::new(0xFF);
/// Diagnostic: endpoint output context state after NEC quirk + SetTRDeq reset (0xFF = not seen).
/// Packed: slot<<16 | dci<<8 | state_bits[2:0]. Should be 1=Running if reset worked.
pub static DIAG_EP_STATE_AFTER_RESET: AtomicU32 = AtomicU32::new(0xFF);
/// Diagnostic: MFINDEX register value (microframe index) for timing analysis.
pub static DIAG_MFINDEX: AtomicU32 = AtomicU32::new(0);
/// Diagnostic: source of first queue_hid_transfer call.
/// 0=unset, 1=inline init, 2=deferred poll=300, 3=reset_halted_endpoint,
/// 4=MSI requeue, 5=CC=SUCCESS requeue, 6=poll CC=SUCCESS requeue
pub static DIAG_FIRST_QUEUE_SOURCE: AtomicU32 = AtomicU32::new(0);
/// Diagnostic: EP output context state BEFORE first doorbell ring on interrupt EP.
/// Packed: slot<<16 | dci<<8 | ep_state. State: 0=Disabled, 1=Running, 2=Halted, 3=Stopped.
pub static DIAG_EP_STATE_BEFORE_DB: AtomicU32 = AtomicU32::new(0xFF);
/// Diagnostic: slot context DW3 (device address + slot state) before first doorbell.
pub static DIAG_SLOT_STATE_BEFORE_DB: AtomicU32 = AtomicU32::new(0);
/// Diagnostic: TR Dequeue Pointer from output context EP before first doorbell (low 32 bits).
pub static DIAG_TRDP_FROM_OUTPUT: AtomicU64 = AtomicU64::new(0);
/// Diagnostic: HCCPARAMS1 register value.
pub static DIAG_HCCPARAMS1: AtomicU32 = AtomicU32::new(0);
/// Diagnostic: HCSPARAMS2 register value.
pub static DIAG_HCSPARAMS2: AtomicU32 = AtomicU32::new(0);
/// Diagnostic: PORTSC value for the slot's port before first doorbell.
/// Contains the raw PORTSC register value including change bits (17-23).
pub static DIAG_PORTSC_BEFORE_DB: AtomicU32 = AtomicU32::new(0);
/// Diagnostic: number of PORTSC change bits cleared during init sweep.
pub static DIAG_PORTSC_CLEARED: AtomicU32 = AtomicU32::new(0);
/// Diagnostic: PORTSC value for port 1 (0-based) BEFORE the sweep clears change bits.
pub static DIAG_PORTSC_PRE_SWEEP: AtomicU32 = AtomicU32::new(0);
/// Maximum number of endpoint resets before giving up.
/// Each reset uses 2 command ring entries. With CMD_RING_SIZE=4096 (4095 usable)
/// CC=12 always halts the endpoint; resets are issued continuously until CC=1.

/// Whether a GET_REPORT EP0 control transfer is pending for the mouse.
/// Set when TRBs are queued, cleared when the Transfer Event is processed.
static MOUSE_GET_REPORT_PENDING: AtomicBool = AtomicBool::new(false);

/// Whether the first keyboard interrupt TRBs have been queued.
/// Keyboard TRBs are deferred to poll=300 (after SPI enable at poll=200)
/// to avoid CC=12 that occurs when TRBs are queued during initialization
/// before the MSI pathway is active. Only set once; re-queue via MSI/error handlers.
static KBD_TRB_FIRST_QUEUED: AtomicBool = AtomicBool::new(false);
static DEFERRED_RECFG_DONE: AtomicBool = AtomicBool::new(false);

/// Whether initial HID interrupt TRBs have been queued post-init.
/// TRBs are deferred until after XHCI_INITIALIZED and SPI enable so the full
/// MSI → GIC SPI → CPU ISR → IMAN.IP ack pathway is active when the xHC
/// processes the first interrupt endpoint transfer.
static HID_TRBS_QUEUED: AtomicBool = AtomicBool::new(false);


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
// XHCI Trace Infrastructure (lock-free ring buffer)
// =============================================================================

/// Maximum number of trace records.
const XHCI_TRACE_MAX_RECORDS: usize = 512;
/// Maximum bytes for trace payload data.
const XHCI_TRACE_DATA_SIZE: usize = 32768;

/// Trace operation codes.
#[repr(u8)]
#[derive(Clone, Copy)]
#[allow(dead_code)]
enum XhciTraceOp {
    MmioWrite32 = 1,
    MmioWrite64 = 2,
    MmioRead32 = 3,
    CommandSubmit = 10,
    CommandComplete = 11,
    TransferSubmit = 12,
    TransferEvent = 13,
    Doorbell = 14,
    InputContext = 20,
    OutputContext = 21,
    TransferRingSetup = 22,
    CacheOp = 30,
    SetTrDeq = 31,
    EpState = 40,
    PortStatusChange = 41,
    Note = 50,
}

/// A single trace record.
#[repr(C)]
struct XhciTraceRecord {
    seq: u32,
    op: u8,
    slot: u8,
    dci: u8,
    _pad: u8,
    timestamp: u64,
    data_offset: u32,
    data_len: u32,
}

/// Whether tracing is active.
static XHCI_TRACE_ACTIVE: AtomicBool = AtomicBool::new(false);
/// Monotonic sequence number for trace records.
static XHCI_TRACE_SEQ: AtomicU32 = AtomicU32::new(0);
/// Data buffer write cursor.
static XHCI_TRACE_DATA_CURSOR: AtomicU32 = AtomicU32::new(0);

/// Trace record ring buffer.
static mut XHCI_TRACE_RECORDS: [XhciTraceRecord; XHCI_TRACE_MAX_RECORDS] = {
    const ZERO: XhciTraceRecord = XhciTraceRecord {
        seq: 0, op: 0, slot: 0, dci: 0, _pad: 0,
        timestamp: 0, data_offset: 0xFFFF_FFFF, data_len: 0,
    };
    [ZERO; XHCI_TRACE_MAX_RECORDS]
};

/// Trace data payload buffer.
static mut XHCI_TRACE_DATA: [u8; XHCI_TRACE_DATA_SIZE] = [0u8; XHCI_TRACE_DATA_SIZE];

/// Read the ARM64 counter register for timestamps.
#[inline(always)]
fn trace_timestamp() -> u64 {
    let val: u64;
    unsafe {
        core::arch::asm!("mrs {}, cntvct_el0", out(reg) val, options(nostack, nomem));
    }
    val
}

/// Record a trace event with optional payload data.
fn xhci_trace_impl(op: u8, slot: u8, dci: u8, data: &[u8]) {
    if !XHCI_TRACE_ACTIVE.load(Ordering::Relaxed) {
        return;
    }

    let seq = XHCI_TRACE_SEQ.fetch_add(1, Ordering::Relaxed);
    let idx = seq as usize % XHCI_TRACE_MAX_RECORDS;
    let ts = trace_timestamp();

    let (data_offset, data_len) = if !data.is_empty() {
        let len = data.len().min(256); // cap per-record payload
        let cursor = XHCI_TRACE_DATA_CURSOR.fetch_add(len as u32, Ordering::Relaxed);
        let off = cursor as usize % XHCI_TRACE_DATA_SIZE;
        // Copy data (may wrap, but that's OK for a ring buffer)
        let copy_len = len.min(XHCI_TRACE_DATA_SIZE - off);
        unsafe {
            let data_ptr = core::ptr::addr_of_mut!(XHCI_TRACE_DATA) as *mut u8;
            core::ptr::copy_nonoverlapping(
                data.as_ptr(),
                data_ptr.add(off),
                copy_len,
            );
            if copy_len < len {
                core::ptr::copy_nonoverlapping(
                    data.as_ptr().add(copy_len),
                    data_ptr,
                    len - copy_len,
                );
            }
        }
        (off as u32, len as u32)
    } else {
        (0xFFFF_FFFF, 0)
    };

    unsafe {
        let rec = &mut *core::ptr::addr_of_mut!(XHCI_TRACE_RECORDS)
            .cast::<XhciTraceRecord>()
            .add(idx);
        rec.seq = seq;
        rec.op = op;
        rec.slot = slot;
        rec.dci = dci;
        rec.timestamp = ts;
        rec.data_offset = data_offset;
        rec.data_len = data_len;
    }
}

/// Record a trace event with optional payload data.
fn xhci_trace(op: XhciTraceOp, slot: u8, dci: u8, data: &[u8]) {
    xhci_trace_impl(op as u8, slot, dci, data);
}

#[cfg(feature = "xhci_linux_harness")]
pub(crate) fn xhci_trace_raw(op: u8, slot: u8, dci: u8, data: &[u8]) {
    xhci_trace_impl(op, slot, dci, data);
}

#[cfg(feature = "xhci_linux_harness")]
pub(crate) fn xhci_trace_set_active(active: bool) {
    if active {
        XHCI_TRACE_SEQ.store(0, Ordering::Relaxed);
        XHCI_TRACE_DATA_CURSOR.store(0, Ordering::Relaxed);
    }
    XHCI_TRACE_ACTIVE.store(active, Ordering::Relaxed);
}

#[cfg(feature = "xhci_linux_harness")]
pub(crate) fn xhci_trace_dump_public() {
    xhci_trace_dump();
}

#[cfg(feature = "xhci_linux_harness")]
#[no_mangle]
pub extern "C" fn breenix_virt_to_phys_c(addr: u64) -> u64 {
    virt_to_phys(addr)
}

#[cfg(feature = "xhci_linux_harness")]
#[no_mangle]
pub extern "C" fn breenix_dma_cache_clean_c(ptr: *const u8, len: usize) {
    dma_cache_clean(ptr, len);
}

#[cfg(feature = "xhci_linux_harness")]
#[no_mangle]
pub extern "C" fn breenix_dma_cache_invalidate_c(ptr: *const u8, len: usize) {
    dma_cache_invalidate(ptr, len);
}

/// Record a short text note (up to 64 chars) in the trace buffer.
fn xhci_trace_note(slot: u8, note: &str) {
    let bytes = note.as_bytes();
    let len = bytes.len().min(64);
    xhci_trace(XhciTraceOp::Note, slot, 0, &bytes[..len]);
}

/// Trace a TRB (16 bytes) with the given operation.
#[allow(dead_code)]
fn xhci_trace_trb(op: XhciTraceOp, slot: u8, dci: u8, trb: &Trb) {
    let bytes: [u8; 16] = unsafe {
        core::mem::transmute_copy(trb)
    };
    xhci_trace(op, slot, dci, &bytes);
}

/// Trace an Input Context before a command.
#[allow(dead_code)]
fn xhci_trace_input_ctx(slot: u8, base: *const u8, ctx_size: usize, max_dci: u8) {
    // Capture Input Control Context (first ctx_size bytes) + slot + up to max_dci EP contexts
    let total = ((2 + max_dci as usize) * ctx_size).min(256);
    let data = unsafe { core::slice::from_raw_parts(base, total) };
    xhci_trace(XhciTraceOp::InputContext, slot, max_dci, data);
}

/// Trace an Output Context after a command.
#[allow(dead_code)]
fn xhci_trace_output_ctx(slot: u8, base: *const u8, ctx_size: usize, max_dci: u8) {
    let total = ((1 + max_dci as usize) * ctx_size).min(256);
    let data = unsafe { core::slice::from_raw_parts(base, total) };
    xhci_trace(XhciTraceOp::OutputContext, slot, max_dci, data);
}

/// Trace a doorbell write.
#[allow(dead_code)]
fn xhci_trace_doorbell(db_base: u64, slot: u8, target: u8) {
    let mut buf = [0u8; 12];
    buf[0..8].copy_from_slice(&db_base.to_le_bytes());
    buf[8] = slot;
    buf[9] = target;
    xhci_trace(XhciTraceOp::Doorbell, slot, target, &buf);
}

/// Trace a 32-bit MMIO write.
#[allow(dead_code)]
fn xhci_trace_mmio_w32(addr: u64, val: u32) {
    let mut buf = [0u8; 12];
    buf[0..8].copy_from_slice(&addr.to_le_bytes());
    buf[8..12].copy_from_slice(&val.to_le_bytes());
    xhci_trace(XhciTraceOp::MmioWrite32, 0, 0, &buf);
}

/// Trace a 64-bit MMIO write.
#[allow(dead_code)]
fn xhci_trace_mmio_w64(addr: u64, val: u64) {
    let mut buf = [0u8; 16];
    buf[0..8].copy_from_slice(&addr.to_le_bytes());
    buf[8..16].copy_from_slice(&val.to_le_bytes());
    xhci_trace(XhciTraceOp::MmioWrite64, 0, 0, &buf);
}

/// Trace a cache operation.
#[allow(dead_code)]
fn xhci_trace_cache_op(addr: u64, len: u32) {
    let mut buf = [0u8; 12];
    buf[0..8].copy_from_slice(&addr.to_le_bytes());
    buf[8..12].copy_from_slice(&len.to_le_bytes());
    xhci_trace(XhciTraceOp::CacheOp, 0, 0, &buf);
}

/// Dump the xHCI trace buffer to serial in a parseable hex format.
/// Called once after init completes. Uses serial_println which is fine post-init.
#[allow(dead_code)]
fn xhci_trace_dump() {
    let total = XHCI_TRACE_SEQ.load(Ordering::Relaxed);
    if total == 0 {
        crate::serial_println!("=== XHCI_TRACE_START ===");
        crate::serial_println!("(no records)");
        crate::serial_println!("=== XHCI_TRACE_END ===");
        return;
    }

    // Determine range: if total <= MAX, dump 0..total. If wrapped, dump last MAX records.
    let start = if total as usize <= XHCI_TRACE_MAX_RECORDS {
        0u32
    } else {
        total - XHCI_TRACE_MAX_RECORDS as u32
    };

    crate::serial_println!("=== XHCI_TRACE_START total={} ===", total);

    for seq in start..total {
        let idx = seq as usize % XHCI_TRACE_MAX_RECORDS;
        let rec = unsafe {
            &*core::ptr::addr_of!(XHCI_TRACE_RECORDS)
                .cast::<XhciTraceRecord>()
                .add(idx)
        };

        // Op name for readability
        let op_name = match rec.op {
            1 => "MMIO_W32",
            2 => "MMIO_W64",
            3 => "MMIO_R32",
            10 => "CMD_SUBMIT",
            11 => "CMD_COMPLETE",
            12 => "XFER_SUBMIT",
            13 => "XFER_EVENT",
            14 => "DOORBELL",
            20 => "INPUT_CTX",
            21 => "OUTPUT_CTX",
            22 => "XFER_RING_SETUP",
            30 => "CACHE_OP",
            31 => "SET_TR_DEQ",
            40 => "EP_STATE",
            41 => "PORT_SC",
            50 => "NOTE",
            _ => "UNKNOWN",
        };

        crate::serial_println!(
            "T {:04} {:12} S={:02} E={:02} TS={:016X} LEN={:04X}",
            rec.seq, op_name, rec.slot, rec.dci, rec.timestamp, rec.data_len,
        );

        // Dump payload in 16-byte hex lines
        if rec.data_len > 0 && rec.data_offset != 0xFFFF_FFFF {
            let off = rec.data_offset as usize;
            let len = rec.data_len as usize;
            if off + len <= XHCI_TRACE_DATA_SIZE {
                let data = unsafe {
                    core::slice::from_raw_parts(
                        core::ptr::addr_of!(XHCI_TRACE_DATA)
                            .cast::<u8>()
                            .add(off),
                        len,
                    )
                };

                // For NOTE records, print as string
                if rec.op == 50 {
                    if let Ok(s) = core::str::from_utf8(data) {
                        crate::serial_println!("  \"{}\"", s);
                    }
                    continue;
                }

                // Print hex in 16-byte rows with 4-byte grouping
                let mut i = 0;
                while i < len {
                    let row_end = (i + 16).min(len);
                    let mut row_str = [0u8; 80];
                    let mut pos = 0;
                    row_str[pos] = b' ';
                    pos += 1;
                    row_str[pos] = b' ';
                    pos += 1;

                    let mut j = i;
                    while j < row_end {
                        let dw_end = (j + 4).min(row_end);
                        let mut k = j;
                        while k < dw_end {
                            let byte = data[k];
                            let hi = byte >> 4;
                            let lo = byte & 0xF;
                            row_str[pos] = if hi < 10 { b'0' + hi } else { b'A' + hi - 10 };
                            pos += 1;
                            row_str[pos] = if lo < 10 { b'0' + lo } else { b'A' + lo - 10 };
                            pos += 1;
                            k += 1;
                        }
                        if dw_end < row_end {
                            row_str[pos] = b' ';
                            pos += 1;
                        }
                        j = dw_end;
                    }

                    if let Ok(s) = core::str::from_utf8(&row_str[..pos]) {
                        crate::serial_println!("{}", s);
                    }

                    i += 16;
                }
            }
        }
    }

    crate::serial_println!("=== XHCI_TRACE_END ===");
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
                // Link TRB type, Toggle Cycle (TC=bit1) per xHCI spec.
                // TC must be bit 1, NOT bit 5 (which is IOC). Without TC set,
                // the HC never toggles its cycle bit on wrap and ignores all
                // post-wrap commands, making the ring appear exhausted.
                control: (trb_type::LINK << 10)
                    | if cycle { 1 } else { 0 }
                    | (1 << 1),
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

                // Clear IMAN.IP (W1C bit 0) and USBSTS.EINT (W1C bit 3).
                // The C harness clears these after every event — required by
                // some xHC implementations to re-arm event generation.
                let iman = read32(ir0);
                if iman & 1 != 0 {
                    write32(ir0, iman | 1); // W1C to clear IP
                }
                let usbsts = read32(state.op_base + 0x04);
                if usbsts & (1 << 3) != 0 {
                    write32(state.op_base + 0x04, usbsts | (1 << 3)); // W1C EINT
                }

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
                    let _cc = trb.completion_code();
                }
                // Port Status Change: acknowledge PORTSC change bits so the
                // xHC knows we've seen the change and won't keep re-signaling.
                if trb_type_val == trb_type::PORT_STATUS_CHANGE {
                    let port_id = ((trb.param >> 24) & 0xFF) as u8;
                    if port_id > 0 && port_id <= state.max_ports {
                        acknowledge_port_changes(state.op_base, port_id);
                    }
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
        xhci_trace_note(0, "err:enable_slot");
        return Err("XHCI EnableSlot failed");
    }

    let slot_id = event.slot_id();
    xhci_trace_note(slot_id, "enable_slot");
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
            return Err("XHCI AddressDevice failed");
        }

        xhci_trace_note(slot_id, "address_device");
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
                    // Any error on EP0 halts the endpoint (xHCI spec 4.10.2.2).
                    // Reset it so subsequent control transfers on this slot work.
                    reset_control_endpoint(state, slot_id);
                    return Err("XHCI control transfer failed");
                }
                return Ok(());
            }

            // Not our EP0 event — stale interrupt endpoint event, skip it
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
                }
                Err(_) => {
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


        if total_len > 256 {
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
    xhci_trace_note(slot_id, "set_configuration");
    Ok(())
}

/// Send SET_INTERFACE request to select an alternate setting for an interface.
///
/// Linux's USB core sends SET_INTERFACE(alt=0) for each interface during driver
/// probe. Parallels' virtual USB device model may require this to activate the
/// interface's interrupt endpoints.
/// NOTE: Tested and causes system hang on Parallels vxHC for HID devices.
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
    Ok(())
}

/// Acknowledge (clear) all pending PORTSC change bits for a given port.
///
/// Port ID is 1-based (matching xHCI convention). Writes 1 to each set RW1C
/// change bit to clear it, while preserving all RW and RO bits.
/// Returns the original PORTSC value (before clearing).
fn acknowledge_port_changes(op_base: u64, port_id: u8) -> u32 {
    let portsc_addr = op_base + 0x400 + ((port_id - 1) as u64) * 0x10;
    let portsc = read32(portsc_addr);
    // RW1C change bits: CSC(17), PEC(18), WRC(19), OCC(20), PRC(21), PLC(22), CEC(23)
    let change_mask: u32 =
        (1 << 17) | (1 << 18) | (1 << 19) | (1 << 20) | (1 << 21) | (1 << 22) | (1 << 23);
    // PED (bit 1) is W1CS — writing 1 disables the port! Always write 0.
    // LWS (bit 16) should be 0 unless doing a link state transition.
    let must_zero: u32 = (1 << 1) | (1 << 16);
    let change_bits = portsc & change_mask;
    if change_bits != 0 {
        // Zero out all RW1C/W1CS bits, then OR in only the change bits to clear.
        let clear_all: u32 = change_mask | must_zero;
        let preserve_mask: u32 = !clear_all;
        write32(portsc_addr, (portsc & preserve_mask) | change_bits);
    }
    portsc
}

/// Clear all PORTSC change bits for all root hub ports.
///
/// Called before queuing interrupt TRBs to ensure no stale port status
/// change conditions can interfere with endpoint operation.
fn clear_all_port_changes(state: &XhciState) -> u32 {
    let mut total_cleared = 0u32;
    let change_mask: u32 =
        (1 << 17) | (1 << 18) | (1 << 19) | (1 << 20) | (1 << 21) | (1 << 22) | (1 << 23);
    for port in 0..state.max_ports as u64 {
        let portsc = acknowledge_port_changes(state.op_base, (port + 1) as u8);
        // Store the first connected port's PORTSC as a pre-sweep diagnostic.
        if port <= 2 && portsc & 1 != 0 {
            let _ = DIAG_PORTSC_PRE_SWEEP.compare_exchange(
                0, portsc, Ordering::AcqRel, Ordering::Relaxed,
            );
        }
        if portsc & change_mask != 0 {
            total_cleared += 1;
        }
    }
    total_cleared
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


/// Send GET_REPORT(Feature) then SET_REPORT(Feature) for a mouse HID interface.
///
/// Linux's usbhid driver does this for mouse interfaces during enumeration:
///   GET_REPORT(Feature, feature_id, 64B) → read current state
///   SET_REPORT(Feature, feature_id, 2B) → echo first 2 bytes back
///
/// From Linux ftrace: mouse if0 uses feature_id=0x11, mouse if1 uses 0x12.
fn get_set_feature_report(
    state: &XhciState,
    slot_id: u8,
    interface: u8,
    feature_id: u8,
) -> Result<(), &'static str> {
    let w_value = 0x0300u16 | (feature_id as u16);
    let data_phys = virt_to_phys((&raw const CTRL_DATA_BUF) as u64);

    // Step 1: GET_REPORT(Feature, feature_id, 64 bytes) — read current report state
    let get_setup = SetupPacket {
        bm_request_type: 0xa1, // D2H, Class, Interface
        b_request: hid_request::GET_REPORT,
        w_value,
        w_index: interface as u16,
        w_length: 64,
    };

    unsafe {
        let data_buf = &raw mut CTRL_DATA_BUF;
        core::ptr::write_bytes((*data_buf).0.as_mut_ptr(), 0, 64);
        dma_cache_clean((*data_buf).0.as_ptr(), 64);
    }

    control_transfer(state, slot_id, &get_setup, data_phys, 64, true)?;

    // Step 2: SET_REPORT(Feature, feature_id, 2 bytes) — echo first 2 bytes back
    unsafe {
        let data_buf = &raw mut CTRL_DATA_BUF;
        dma_cache_invalidate((*data_buf).0.as_ptr(), 64);
        // First 2 bytes already contain the device's response; re-clean for DMA
        dma_cache_clean((*data_buf).0.as_ptr(), 2);
    }

    let set_setup = SetupPacket {
        bm_request_type: 0x21, // H2D, Class, Interface
        b_request: hid_request::SET_REPORT,
        w_value,
        w_index: interface as u16,
        w_length: 2,
    };

    control_transfer(state, slot_id, &set_setup, data_phys, 2, false)?;
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

        // The control transfer itself is important (matches Linux enumeration
        // sequence). Report descriptor data is captured by the trace system
        // if tracing is active.
        let _ = control_transfer(state, slot_id, &setup, data_phys, req_len, true);
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
    ss_mult: u8,
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
        // DW0 = Drop flags: 0 (no drops for initial configure).
        // Drop+Add was tested and still produces CC=12 on Parallels vxHC.
        core::ptr::write_volatile(input_base as *mut u32, 0u32);
        core::ptr::write_volatile(input_base.add(0x04) as *mut u32, add_flags);


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
                // Mult (bits 9:8): xHCI spec Table 6-17 says Mult shall be 0 for
                // all non-Isochronous endpoint types (Interrupt, Bulk, Control).
                // Only SuperSpeed Isochronous endpoints may have Mult > 0.
                // We always use EP Type 7 (Interrupt IN) for HID endpoints.
                let mult: u32 = 0;
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
                    // Link TRB type=6, TC (Toggle Cycle) bit 1, cycle=1.
                    // Linux (C harness) initializes Link TRBs with cycle=1,
                    // matching the ring's initial cycle state (DCS=1). The TC
                    // bit toggles the cycle when the xHC processes this Link TRB.
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

                // Do NOT pre-populate ring[0] before ConfigureEndpoint.
                // Linux never queues TRBs before ConfigureEndpoint completes and the
                // bandwidth dance finishes. Doing so causes Parallels vxHC to auto-scan
                // the ring on ConfigEP completion, return CC=12 (endpoint not yet truly
                // enabled), and transition the endpoint to Halted. The subsequent BW dance
                // then issues StopEP on a Halted endpoint (spec violation), and re-ConfigEP
                // causes another auto-scan → CC=12 → Halted again. TRBs are queued by
                // start_hid_polling() after the complete init sequence.
                // ring[0] remains zeroed (cycle=0), so the hardware will not process it
                // until a real TRB with cycle=1 is placed there and a doorbell is rung.

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
                // C harness uses avg_trb_len = max_esit_payload (matching Linux's
                // xhci_endpoint_init which sets avg = max_esit_payload for INT eps).
                let esit_lo = esit_payload & 0xFFFF;
                let avg_trb_len: u32 = esit_payload.max(1);
                let ep_dw4: u32 = (esit_lo << 16) | avg_trb_len;
                core::ptr::write_volatile(ep_ctx.add(0x10) as *mut u32, ep_dw4);

            }
        }

        // Cache-clean the entire input context
        dma_cache_clean(input_base, 4096);

        // Issue batch ConfigureEndpoint using INPUT_CONTEXTS directly.
        let input_ctx_phys = virt_to_phys(&raw const INPUT_CONTEXTS[slot_idx] as u64);
        {
            xhci_trace_input_ctx(slot_id, input_base, ctx_size, max_dci as u8);
            let trb = Trb {
                param: input_ctx_phys,
                status: 0,
                control: (trb_type::CONFIGURE_ENDPOINT << 10) | ((slot_id as u32) << 24),
            };
            xhci_trace_trb(XhciTraceOp::CommandSubmit, slot_id, 0, &trb);
            enqueue_command(trb);
            ring_doorbell(state, 0, 0);

            let event = wait_for_command(state)?;
            xhci_trace_trb(XhciTraceOp::CommandComplete, slot_id, 0, &event);
            let cc = event.completion_code();
            if cc != completion_code::SUCCESS {
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
            // BW dance: StopEndpoint + re-ConfigureEndpoint per endpoint.
            //
            // Linux reuses ONE re-ConfigEP Input Context (distinct from the initial
            // ConfigEP context) that contains ALL endpoints. The buffer address stays
            // the same, but the contents are rebuilt from the Output Context after
            // each StopEndpoint so TR Dequeue pointers and state are current.
            let reconfig = &raw mut RECONFIG_INPUT_CTX[slot_idx];
            let reconfig_base = (*reconfig).0.as_mut_ptr();
            let reconfig_phys = virt_to_phys(&raw const RECONFIG_INPUT_CTX[slot_idx] as u64);

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
                    xhci_trace_trb(XhciTraceOp::CommandSubmit, slot_id, dci, &stop_trb);
                    enqueue_command(stop_trb);
                    ring_doorbell(state, 0, 0);
                    let stop_event = wait_for_command(state)?;
                    xhci_trace_trb(XhciTraceOp::CommandComplete, slot_id, dci, &stop_event);
                    let _stop_cc = stop_event.completion_code();
                    // Read output context EP state after StopEP (should be 3=Stopped)
                    dma_cache_invalidate((*dev_ctx).0.as_ptr(), 4096);
                    let _stop_ep_state = core::ptr::read_volatile(
                        (*dev_ctx).0.as_ptr().add((dci as usize) * ctx_size) as *const u32
                    ) & 0x7;
                    let _stop_deq_lo = core::ptr::read_volatile(
                        (*dev_ctx).0.as_ptr().add((dci as usize) * ctx_size + 8) as *const u32
                    );

                    // Step 2: Rebuild the shared reconfig context from Output Context,
                    // then Re-ConfigureEndpoint using that buffer.
                    core::ptr::write_bytes(reconfig_base as *mut u8, 0, 4096);

                    // ICC: Drop=0, Add=A0 + all endpoints (same as initial).
                    core::ptr::write_volatile(reconfig_base as *mut u32, 0u32);
                    core::ptr::write_volatile(reconfig_base.add(4) as *mut u32, add_flags);

                    // Slot context (ctx_size offset): copy DW0-DW2 from output, zero DW3.
                    let rc_slot = reconfig_base.add(ctx_size);
                    for dw_offset in (0..12usize).step_by(4) {
                        let val = core::ptr::read_volatile(
                            (*dev_ctx).0.as_ptr().add(dw_offset) as *const u32,
                        );
                        core::ptr::write_volatile(rc_slot.add(dw_offset) as *mut u32, val);
                    }
                    core::ptr::write_volatile(rc_slot.add(12) as *mut u32, 0u32);
                    for dw_offset in (16..32).step_by(4) {
                        core::ptr::write_volatile(rc_slot.add(dw_offset) as *mut u32, 0u32);
                    }
                    let rc_slot_dw0 = core::ptr::read_volatile(rc_slot as *const u32);
                    core::ptr::write_volatile(
                        rc_slot as *mut u32,
                        (rc_slot_dw0 & !(0x1F << 27)) | (max_dci << 27),
                    );

                    // Endpoint contexts: copy all endpoints from Output Context.
                    for j in 0..ep_count {
                        if let Some(ref epj) = endpoints[j] {
                            let ep_dci = epj.dci as usize;
                            let rc_ep = reconfig_base.add((1 + ep_dci) * ctx_size);
                            let src_ep = (*dev_ctx).0.as_ptr().add(ep_dci * ctx_size);
                            for dw_offset in (0..20usize).step_by(4) {
                                let val = core::ptr::read_volatile(
                                    src_ep.add(dw_offset) as *const u32,
                                );
                                let val_clean = if dw_offset == 0 { val & !0x7u32 } else { val };
                                core::ptr::write_volatile(
                                    rc_ep.add(dw_offset) as *mut u32,
                                    val_clean,
                                );
                            }
                        }
                    }

                    dma_cache_clean(reconfig_base, 4096);

                    // Re-ConfigureEndpoint using the shared reconfig context.
                    xhci_trace_input_ctx(slot_id, reconfig_base, ctx_size, max_dci as u8);
                    let reconfig_trb = Trb {
                        param: reconfig_phys,
                        status: 0,
                        control: (trb_type::CONFIGURE_ENDPOINT << 10)
                            | ((slot_id as u32) << 24),
                    };
                    xhci_trace_trb(XhciTraceOp::CommandSubmit, slot_id, 0, &reconfig_trb);
                    enqueue_command(reconfig_trb);
                    ring_doorbell(state, 0, 0);
                    let reconfig_event = wait_for_command(state)?;
                    xhci_trace_trb(XhciTraceOp::CommandComplete, slot_id, 0, &reconfig_event);
                    let _reconfig_cc = reconfig_event.completion_code();

                    // Diagnostic: verify TR Dequeue pointer in output context after re-ConfigEP.
                    dma_cache_invalidate((*dev_ctx).0.as_ptr(), 4096);
                    let ep_out_base = (*dev_ctx).0.as_ptr().add((dci as usize) * ctx_size);
                    let _post_dw0 = core::ptr::read_volatile(ep_out_base as *const u32);
                    let _post_dw2 = core::ptr::read_volatile(ep_out_base.add(8) as *const u32);
                    let _post_dw3 = core::ptr::read_volatile(ep_out_base.add(12) as *const u32);
                    let _ring_phys_check = virt_to_phys(
                        &raw const TRANSFER_RINGS[HID_RING_BASE + ep.hid_idx] as u64
                    );
                    xhci_trace_output_ctx(slot_id, (*dev_ctx).0.as_ptr(), ctx_size, max_dci as u8);
                }
            }
        }

        // Verify: read back device context after ConfigureEndpoint
        dma_cache_invalidate((*dev_ctx).0.as_ptr(), 4096);

        let _slot_out_dw0 = core::ptr::read_volatile((*dev_ctx).0.as_ptr() as *const u32);
        let _slot_out_dw3 = core::ptr::read_volatile((*dev_ctx).0.as_ptr().add(12) as *const u32);

        for i in 0..ep_count {
            if let Some(ref ep) = endpoints[i] {
                let ep_out = (*dev_ctx).0.as_ptr().add((ep.dci as usize) * ctx_size);
                let ep_out_dw0 = core::ptr::read_volatile(ep_out as *const u32);
                let ep_state = ep_out_dw0 & 0x7;
                let _ep_out_dw1 = core::ptr::read_volatile(ep_out.add(4) as *const u32);
                let _ep_out_dw2 = core::ptr::read_volatile(ep_out.add(8) as *const u32);
                let _ep_out_dw3 = core::ptr::read_volatile(ep_out.add(12) as *const u32);
                let _ring_phys_chk = virt_to_phys(
                    &raw const TRANSFER_RINGS[HID_RING_BASE + ep.hid_idx] as u64
                );
                if ep_state == 0 {
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
        hid_idx: usize, // Transfer ring index (0=kbd boot, 1=mouse, 2=kbd NKRO, 3=mouse2)
        dci: u8,
        hid_report_len: u16,   // wDescriptorLength from HID descriptor (0 = unknown)
    }
    let mut ifaces: [Option<HidIfaceInfo>; 4] = [None, None, None, None];
    let mut iface_count: usize = 0;
    let mut found_boot_keyboard = false;
    let mut found_mouse = false;
    let mut found_mouse2 = false;

    // Pending endpoints for batch ConfigureEndpoint (one command for all EPs)
    let mut pending_eps: [Option<PendingEp>; 4] = [None, None, None, None];
    let mut ep_count: usize = 0;
    let mut max_dci: u8 = 0;

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
                            let mut ss_mult: u8 = 0;
                            let ss_offset = ep_offset + ep_len;
                            if ss_offset + 2 <= config_len {
                                let ss_len = config_buf[ss_offset] as usize;
                                let ss_type = config_buf[ss_offset + 1];
                                if ss_type == 0x30 && ss_len >= 6 && ss_offset + ss_len <= config_len {
                                    ss_max_burst = config_buf[ss_offset + 2];
                                    // bmAttributes bits[1:0] = Mult (max burst multiplier)
                                    ss_mult = config_buf[ss_offset + 3] & 0x3;
                                    ss_bytes_per_interval = u16::from_le_bytes([
                                        config_buf[ss_offset + 4],
                                        config_buf[ss_offset + 5],
                                    ]);
                                }
                            }

                            // Determine HID device type and transfer ring index:
                            //   hid_idx 0 = boot keyboard (protocol=1, DCI 3)
                            //   hid_idx 1 = boot mouse (protocol=2, DCI 3)
                            //   hid_idx 2 = NKRO keyboard (subclass=0 protocol=0, DCI 5)
                            //   hid_idx 3 = mouse2 (second mouse interface, DCI 5)
                            // Linux ftrace: mouse has TWO interrupt EPs (DCI 3 + DCI 5),
                            // both configured in one ConfigureEndpoint with add_flags=0x29.
                            let (hid_idx, is_keyboard, is_nkro) =
                                if iface.b_interface_protocol == hid_protocol::KEYBOARD {
                                    found_boot_keyboard = true;
                                    (0usize, true, false)
                                } else if iface.b_interface_protocol == hid_protocol::MOUSE {
                                    if found_mouse {
                                        // Second mouse interface — use ring 3 (DCI 5).
                                        // Linux configures both mouse endpoints in one batch.
                                        if found_mouse2 {
                                            break; // Only support two mouse interfaces
                                        }
                                        found_mouse2 = true;
                                        (3usize, false, false)
                                    } else {
                                        found_mouse = true;
                                        (1usize, false, false)
                                    }
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
                                    ss_mult,
                                });
                                if dci > max_dci {
                                    max_dci = dci;
                                }
                                ep_count += 1;
                            }

                            // Store info for Phase 3
                            if iface_count < ifaces.len() {
                                ifaces[iface_count] = Some(HidIfaceInfo {
                                    interface_number: iface.b_interface_number,
                                    is_keyboard,
                                    is_nkro,
                                    hid_idx,
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
        xhci_trace_note(slot_id, "no_hid_ifaces");
        return Ok(());
    }

    // =========================================================================
    // Phase 1b: ConfigureEndpoint BEFORE SET_CONFIGURATION (matching Linux)
    //
    // Linux's xhci_check_bandwidth() issues ConfigureEndpoint (+ BW dance)
    // BEFORE usb_set_configuration() sends the USB SET_CONFIGURATION request.
    // This ensures the xHC has transfer rings ready before the device
    // activates its endpoints. The C harness uses this order and avoids CC=12.
    // =========================================================================
    if ep_count > 0 {
        configure_endpoints_batch(state, slot_id, &pending_eps, ep_count)?;
    }

    set_configuration(state, slot_id, config_value)?;

    // Diagnostic: dump endpoint states AFTER ConfigureEndpoint
    xhci_trace_note(slot_id, "post_cfgep_ctx");
    if ep_count > 0 {
        let slot_idx = (slot_id - 1) as usize;
        let ctx_size = state.context_size;
        unsafe {
            let dev_ctx = &raw const DEVICE_CONTEXTS[slot_idx];
            dma_cache_invalidate((*dev_ctx).0.as_ptr(), 4096);
            if max_dci != 0 {
                xhci_trace_output_ctx(slot_id, (*dev_ctx).0.as_ptr(), ctx_size, max_dci);
            }
        }
    }

    // Phase 2b: SET_INTERFACE + SET_PROTOCOL REMOVED.
    //
    // Linux ftrace confirmed: Linux does NOT send SET_INTERFACE or SET_PROTOCOL
    // for HID devices on this Parallels vxHC. Per xHCI spec section 4.6.6,
    // SET_INTERFACE requires the host to Deconfigure (DC=1) then re-Configure
    // endpoints. Sending SET_INTERFACE as a raw USB control transfer without
    // the xHCI-side deconfigure/reconfigure may cause the Parallels vxHC to
    // internally mark endpoints as needing reconfiguration, leading to CC=12.

    // =========================================================================
    // Phase 3: HID interface setup (SET_IDLE, GET_REPORT_DESC, etc.)
    // =========================================================================
    for i in 0..iface_count {
        if let Some(ref info) = ifaces[i] {

            if info.is_nkro {
                // NKRO keyboard: SET_IDLE + GET_HID_REPORT_DESC then ep2in TRB.
                // Linux sends these for the NKRO interface before ep2in (ftrace lines 900, 911, 923).
                if !MINIMAL_INIT {
                    match set_idle(state, slot_id, info.interface_number) {
                        Ok(()) => {}
                        Err(_) => {}
                    }
                    fetch_hid_report_descriptor(state, slot_id, info.interface_number, info.hid_report_len);
                }
                state.kbd_slot = slot_id;
                state.kbd_nkro_endpoint = info.dci;

                // Queue interrupt TRB immediately (matching Linux: queue right after HID setup).
                let _ = DIAG_FIRST_QUEUE_SOURCE.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Relaxed);
                let _ = queue_hid_transfer(state, info.hid_idx, slot_id, info.dci);

            } else if info.is_keyboard {
                // Boot keyboard: SET_IDLE + GET_HID_REPORT_DESC + SET_REPORT(LED=0) + ep1in TRB.
                // Linux ftrace (lines 827, 838, 851, 863):
                //   SET_IDLE (iface=0) → GET_HID_REPORT_DESC (58 bytes) → SET_REPORT(LED=0) → ep1in TRB
                if !MINIMAL_INIT {
                    match set_idle(state, slot_id, info.interface_number) {
                        Ok(()) => {}
                        Err(_) => {}
                    }
                    fetch_hid_report_descriptor(state, slot_id, info.interface_number, info.hid_report_len);
                    match set_report_leds(state, slot_id, info.interface_number) {
                        Ok(()) => {}
                        Err(_) => {}
                    }
                }
                state.kbd_slot = slot_id;
                state.kbd_endpoint = info.dci;

                // Queue interrupt TRB immediately (matching Linux: queue right after HID setup).
                let _ = DIAG_FIRST_QUEUE_SOURCE.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Relaxed);
                let _ = queue_hid_transfer(state, info.hid_idx, slot_id, info.dci);

            } else {
                // Mouse: GET_REPORT(Feature) + SET_REPORT(Feature).
                if !MINIMAL_INIT {
                    let feature_id: u8 = if info.hid_idx == 3 { 0x12 } else { 0x11 };
                    match get_set_feature_report(state, slot_id, info.interface_number, feature_id) {
                        Ok(()) => {}
                        Err(_) => {}
                    }
                }
                if info.hid_idx == 3 {
                    state.mouse_nkro_endpoint = info.dci;
                } else {
                    state.mouse_slot = slot_id;
                    state.mouse_endpoint = info.dci;
                }

                // Queue interrupt TRB immediately (matching Linux: queue right after HID setup).
                let _ = DIAG_FIRST_QUEUE_SOURCE.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Relaxed);
                let _ = queue_hid_transfer(state, info.hid_idx, slot_id, info.dci);
            }
        }
    }

    Ok(())
}

/// Queue a Normal TRB on a HID transfer ring to receive an interrupt IN report.
fn queue_hid_transfer(
    state: &XhciState,
    hid_idx: usize, // 0 = kbd boot, 1 = mouse, 2 = kbd NKRO, 3 = mouse2
    slot_id: u8,
    dci: u8,
) -> Result<(), &'static str> {
    let ring_idx = HID_RING_BASE + hid_idx;

    // Determine the physical address and size of the report buffer.
    // Linux ftrace confirms: xhci_urb_enqueue lengths are the actual data sizes
    // (8 bytes for kbd boot, 8 bytes for mouse, 9 bytes for NKRO/mouse2), NOT maxp.
    let (buf_phys, buf_len) = match hid_idx {
        0 => (virt_to_phys((&raw const KBD_REPORT_BUF) as u64), 8usize),
        2 => (virt_to_phys((&raw const NKRO_REPORT_BUF) as u64), 9usize),
        3 => (virt_to_phys((&raw const MOUSE2_REPORT_BUF) as u64), 9usize),
        _ => (virt_to_phys((&raw const MOUSE_REPORT_BUF) as u64), 8usize),
    };

    // Fill report buffer with sentinel (0xDE) before giving it to the controller.
    // After DMA completion, we check if the sentinel was overwritten — this tells
    // us definitively whether the XHCI DMA wrote actual data to the buffer.
    unsafe {
        match hid_idx {
            0 => {
                let buf = &raw mut KBD_REPORT_BUF;
                core::ptr::write_bytes((*buf).0.as_mut_ptr(), 0xDE, 64);
                dma_cache_clean((*buf).0.as_ptr(), 64);
            }
            2 => {
                let buf = &raw mut NKRO_REPORT_BUF;
                core::ptr::write_bytes((*buf).0.as_mut_ptr(), 0xDE, 64);
                dma_cache_clean((*buf).0.as_ptr(), 64);
            }
            3 => {
                let buf = &raw mut MOUSE2_REPORT_BUF;
                core::ptr::write_bytes((*buf).0.as_mut_ptr(), 0xDE, 64);
                dma_cache_clean((*buf).0.as_ptr(), 64);
            }
            _ => {
                let buf = &raw mut MOUSE_REPORT_BUF;
                core::ptr::write_bytes((*buf).0.as_mut_ptr(), 0xDE, 64);
                dma_cache_clean((*buf).0.as_ptr(), 64);
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
    xhci_trace_trb(XhciTraceOp::TransferSubmit, slot_id, dci, &trb);
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

    // Diagnostic: read output context EP state + slot state BEFORE first doorbell.
    // Only on the first interrupt TRB queue (DIAG_EP_STATE_BEFORE_DB still at 0xFF).
    if DIAG_EP_STATE_BEFORE_DB.load(Ordering::Relaxed) == 0xFF {
        let slot_idx = (slot_id - 1) as usize;
        unsafe {
            let dev_ctx = &raw const DEVICE_CONTEXTS[slot_idx];
            let ctx_size = state.context_size;
            dma_cache_invalidate((*dev_ctx).0.as_ptr(), 4096);

            // Read Slot Context DW3: bits 31:27 = Slot State, bits 7:0 = USB Device Address
            let slot_dw3 = core::ptr::read_volatile(
                (*dev_ctx).0.as_ptr().add(12) as *const u32,
            );
            DIAG_SLOT_STATE_BEFORE_DB.store(slot_dw3, Ordering::Relaxed);

            // Read EP context for this DCI: DW0 bits 2:0 = EP State
            let ep_ctx_base = (*dev_ctx).0.as_ptr().add(dci as usize * ctx_size);
            let ep_dw0 = core::ptr::read_volatile(ep_ctx_base as *const u32);
            let ep_state = ep_dw0 & 0x7;
            DIAG_EP_STATE_BEFORE_DB.store(
                ((slot_id as u32) << 16) | ((dci as u32) << 8) | ep_state,
                Ordering::Relaxed,
            );

            // Read TR Dequeue Pointer (DW2 + DW3 of EP context)
            let trdp_lo = core::ptr::read_volatile(ep_ctx_base.add(8) as *const u32);
            let trdp_hi = core::ptr::read_volatile(ep_ctx_base.add(12) as *const u32);
            let trdp = ((trdp_hi as u64) << 32) | (trdp_lo as u64);
            DIAG_TRDP_FROM_OUTPUT.store(trdp, Ordering::Relaxed);

            // Read PORTSC for this slot's root hub port (from Slot Context DW1 bits 23:16).
            let slot_dw1 = core::ptr::read_volatile(
                (*dev_ctx).0.as_ptr().add(4) as *const u32,
            );
            let root_port = ((slot_dw1 >> 16) & 0xFF) as u8;
            if root_port > 0 {
                let portsc = read32(state.op_base + 0x400 + ((root_port - 1) as u64) * 0x10);
                DIAG_PORTSC_BEFORE_DB.store(portsc, Ordering::Relaxed);
            }
        }
    }

    // Ring the doorbell for this endpoint
    ring_doorbell(state, slot_id, dci);

    Ok(())
}

/// Synchronously poll the event ring for a Transfer Event.
/// Returns the completion code (0xFF = timeout after ~500ms).
/// Traces the Transfer Event TRB for diagnostic purposes.
fn sync_poll_transfer_event(state: &XhciState) -> u32 {
    let mut cc: u32 = 0xFF;
    for _attempt in 0..500_000u32 {
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
                let trb_type_val = trb.trb_type();
                if trb_type_val == trb_type::TRANSFER_EVENT {
                    cc = trb.completion_code();
                    // Trace the full Transfer Event TRB (shows slot/dci/pointer)
                    xhci_trace_trb(XhciTraceOp::TransferEvent, 0, 0, &trb);
                    let _ = DIAG_FIRST_XFER_CC.compare_exchange(
                        0xFF, cc, Ordering::AcqRel, Ordering::Relaxed,
                    );
                    let _ = DIAG_FIRST_XFER_PTR.compare_exchange(
                        0, trb.param, Ordering::AcqRel, Ordering::Relaxed,
                    );
                }
                // Advance dequeue for any event type
                EVENT_RING_DEQUEUE = (idx + 1) % EVENT_RING_SIZE;
                if EVENT_RING_DEQUEUE == 0 {
                    EVENT_RING_CYCLE = !cycle;
                }
                let ir0 = state.rt_base + 0x20;
                let erdp_phys = virt_to_phys(&raw const EVENT_RING as u64)
                    + (EVENT_RING_DEQUEUE as u64) * 16;
                write64(ir0 + 0x18, erdp_phys | (1 << 3));
                // Clear IMAN.IP + USBSTS.EINT (match C harness)
                let iman = read32(ir0);
                if iman & 1 != 0 {
                    write32(ir0, iman | 1);
                }
                let usbsts = read32(state.op_base + 0x04);
                if usbsts & (1 << 3) != 0 {
                    write32(state.op_base + 0x04, usbsts | (1 << 3));
                }
                if trb_type_val == trb_type::TRANSFER_EVENT {
                    break;
                }
                // Non-transfer event: continue polling
            }
        }
        core::hint::spin_loop();
    }
    cc
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
        xhci_trace_note(0, "drain_stale");
    } else {
    }
}

/// Read the endpoint state (bits[2:0] of output context DW0) for a given slot/DCI.
///
/// Returns: 0=Disabled, 1=Running, 2=Halted, 3=Stopped, 4=Error, 0=invalid slot/dci.
fn read_output_ep_state(state: &XhciState, slot_id: u8, dci: u8) -> u8 {
    if slot_id == 0 || dci == 0 {
        return 0;
    }
    let slot_idx = (slot_id - 1) as usize;
    let ctx_size = state.context_size;
    unsafe {
        let dev_ctx = &raw const DEVICE_CONTEXTS[slot_idx];
        dma_cache_invalidate((*dev_ctx).0.as_ptr(), 4096);
        let ep_base = (*dev_ctx).0.as_ptr().add(dci as usize * ctx_size);
        let ep_dw0 = core::ptr::read_volatile(ep_base as *const u32);
        (ep_dw0 & 0x7) as u8
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
            let _slot_dw0 = core::ptr::read_volatile((*dev_ctx).0.as_ptr() as *const u32);


            // Dump each endpoint DCI we care about (full 5 DWORDs)
            for &dci in &[state.kbd_endpoint, state.kbd_nkro_endpoint] {
                if dci == 0 { continue; }
                let ep_base = (*dev_ctx).0.as_ptr().add(dci as usize * ctx_size);
                let _ep_dw0 = core::ptr::read_volatile(ep_base as *const u32);
                let _ep_dw1 = core::ptr::read_volatile(ep_base.add(4) as *const u32);
                let _ep_dw2 = core::ptr::read_volatile(ep_base.add(8) as *const u32);
                let _ep_dw3 = core::ptr::read_volatile(ep_base.add(12) as *const u32);

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
            let _ep_dw0 = core::ptr::read_volatile(ep_base as *const u32);
            let _ep_dw1 = core::ptr::read_volatile(ep_base.add(4) as *const u32);
            let _ep_dw2 = core::ptr::read_volatile(ep_base.add(8) as *const u32);
            let _ep_dw3 = core::ptr::read_volatile(ep_base.add(12) as *const u32);
        }
    }
}

/// Wait for a Command Completion event, passing through Transfer Events and other
/// async events. Used during endpoint recovery in timer context — no logging.
///
/// Transfer Events consumed here are re-flagged via NEEDS_RESET_* so the poll
/// loop doesn't miss endpoint errors that arrive while waiting for commands.
///
/// Returns the completion code, or an error on timeout.
fn wait_for_command_completion(state: &XhciState) -> Result<u32, &'static str> {
    // 10K iterations × ~60ns = ~600μs max. Virtual xHC (Parallels) responds in
    // microseconds; real hardware would need more. Keeping this short is critical:
    // this function is called from poll_hid_events in the timer IRQ handler, so
    // blocking here starves the scheduler and prevents heartbeats.
    let mut timeout = 10_000u32;
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
                // For Transfer Events consumed while waiting: re-flag NEEDS_RESET_*
                // so the next poll_hid_events call handles them. Without this, error
                // completions for other endpoints are silently lost, leaving those
                // endpoints permanently halted with no pending TRBs.
                if trb_type_val == trb_type::TRANSFER_EVENT {
                    let slot = trb.slot_id();
                    let endpoint = ((trb.control >> 16) & 0x1F) as u8;
                    let cc = trb.completion_code();

                    // GET_REPORT EP0 response: handle it here since the event arrives
                    // while we're spinning for the interrupt endpoint Reset Endpoint
                    // command completion. Without PENDING check, late responses that
                    // arrive after the 200-tick stale-clear are also caught here.
                    // Post-enumeration, the only EP0 events from mouse_slot are GET_REPORT.
                    if slot == state.mouse_slot
                        && endpoint == 1
                    {
                        MOUSE_GET_REPORT_PENDING.store(false, Ordering::Release);
                        // Record last CC seen (fd= heartbeat field, 0xFF = no event yet).
                        DIAG_FIRST_DB.store(cc, Ordering::Relaxed);
                        if cc == completion_code::SUCCESS || cc == completion_code::SHORT_PACKET {
                            GET_REPORT_OK.fetch_add(1, Ordering::Relaxed);
                            let buf = &raw const CTRL_DATA_BUF;
                            dma_cache_invalidate((*buf).0.as_ptr(), 8);
                            let report = &(&(*buf).0)[..8];
                            if report.iter().any(|&b| b != 0) {
                                GET_REPORT_NONZERO.fetch_add(1, Ordering::Relaxed);
                                super::hid::process_mouse_report(report);
                            }
                        }
                    } else if cc != completion_code::SUCCESS && cc != completion_code::SHORT_PACKET {
                        // CC=12 during a command wait: endpoint is halted but re-flagging
                        // NEEDS_RESET_* here causes cascading resets (each reset's command
                        // wait sees the other endpoint's CC=12, triggering another reset).
                        // Use MSI_*_NEEDS_REQUEUE to defer: the next timer tick's state
                        // check will set NEEDS_RESET_* if the endpoint is still Halted.
                        // Other error CCs (CC=4, CC=6) are genuine errors: reset directly.
                        if cc == completion_code::ENDPOINT_NOT_ENABLED {
                            if slot == state.kbd_slot && endpoint == state.kbd_endpoint {
                                MSI_KBD_NEEDS_REQUEUE.store(true, Ordering::Release);
                            } else if slot == state.kbd_slot
                                && state.kbd_nkro_endpoint != 0
                                && endpoint == state.kbd_nkro_endpoint
                            {
                                MSI_NKRO_NEEDS_REQUEUE.store(true, Ordering::Release);
                            } else if slot == state.mouse_slot && endpoint == state.mouse_endpoint {
                                MSI_MOUSE_NEEDS_REQUEUE.store(true, Ordering::Release);
                            } else if slot == state.mouse_slot
                                && state.mouse_nkro_endpoint != 0
                                && endpoint == state.mouse_nkro_endpoint
                            {
                                MSI_MOUSE2_NEEDS_REQUEUE.store(true, Ordering::Release);
                            }
                        } else {
                            if slot == state.kbd_slot && endpoint == state.kbd_endpoint {
                                NEEDS_RESET_KBD_BOOT.store(true, Ordering::Release);
                            } else if slot == state.kbd_slot
                                && state.kbd_nkro_endpoint != 0
                                && endpoint == state.kbd_nkro_endpoint
                            {
                                NEEDS_RESET_KBD_NKRO.store(true, Ordering::Release);
                            } else if slot == state.mouse_slot && endpoint == state.mouse_endpoint {
                                NEEDS_RESET_MOUSE.store(true, Ordering::Release);
                            } else if slot == state.mouse_slot
                                && state.mouse_nkro_endpoint != 0
                                && endpoint == state.mouse_nkro_endpoint
                            {
                                NEEDS_RESET_MOUSE2.store(true, Ordering::Release);
                            }
                        }
                    }
                }
                // Consumed non-command event — fall through to timeout check.
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
/// Two paths based on current endpoint state (inferred from Reset Endpoint CC):
///
///   Halted  → Reset EP (CC=1) → Stopped → zero ring → Set TR Deq → queue TRB
///   Running/Stopped → Reset EP fails (CC=9) → skip ring reset → queue TRB at
///                     current TRANSFER_ENQUEUE (HC dequeue already valid)
///
/// The "skip ring reset" path is correct because the HC's dequeue pointer is
/// already positioned at the slot where the failed TRB was processed. Writing a
/// new TRB there and ringing the doorbell resumes the endpoint without disrupting
/// the HC's ring state.
///
/// Called from poll_hid_events (timer context). Uses wait_for_command_completion (no logging).
fn reset_halted_endpoint(
    state: &XhciState,
    slot_id: u8,
    dci: u8,
    hid_idx: usize,
) -> Result<(), &'static str> {
    let ring_idx = HID_RING_BASE + hid_idx;

    // Step 1: Reset Endpoint Command (valid only for Halted endpoints).
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
        ENDPOINT_RESET_FAIL_COUNT.fetch_add(1, Ordering::Relaxed);
        // Endpoint is Running or Stopped (not Halted). Reset Endpoint is only valid
        // for Halted endpoints — CC=9 (Context State Error) is expected here.
        // Skip ring zero and Set TR Dequeue Pointer: the HC's dequeue is still valid
        // (it processed the last TRB and advanced naturally). Just requeue at the
        // current TRANSFER_ENQUEUE position and ring the doorbell.
        let _ = DIAG_FIRST_QUEUE_SOURCE.compare_exchange(0, 3, Ordering::AcqRel, Ordering::Relaxed);
        let result = queue_hid_transfer(state, hid_idx, slot_id, dci);
        ENDPOINT_RESET_COUNT.fetch_add(1, Ordering::Relaxed);
        return result;
    }

    // Step 2: Zero transfer ring, add Link TRB, and reset state to beginning
    unsafe {
        let ring = &raw mut TRANSFER_RINGS[ring_idx];
        core::ptr::write_bytes(ring as *mut u8, 0, TRANSFER_RING_SIZE * 16);

        // Re-initialize Link TRB at end of ring (matching Linux ring structure)
        let ring_phys_link = virt_to_phys(&raw const TRANSFER_RINGS[ring_idx] as u64);
        let link = Trb {
            param: ring_phys_link,
            status: 0,
            control: (trb_type::LINK << 10) | (1 << 1), // Link, TC, cycle=0 (matches Linux)
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
        ENDPOINT_RESET_FAIL_COUNT.fetch_add(1, Ordering::Relaxed);
        // Don't abort — fall through to requeue anyway so the endpoint has a fresh TRB.
    }

    // Read endpoint state from output context after reset (NEC quirk + SetTRDeq).
    // Tells us if the endpoint is Running (1) or still Stopped/Halted.
    unsafe {
        if DIAG_EP_STATE_AFTER_RESET.load(Ordering::Relaxed) == 0xFF {
            let slot_idx = (slot_id - 1) as usize;
            let ep_out = DEVICE_CONTEXTS[slot_idx]
                .0
                .as_ptr()
                .add(dci as usize * state.context_size);
            dma_cache_invalidate(ep_out, 4);
            let dw0 = core::ptr::read_volatile(ep_out as *const u32);
            let ep_state = dw0 & 0x7;
            DIAG_EP_STATE_AFTER_RESET.store(
                ((slot_id as u32) << 16) | ((dci as u32) << 8) | ep_state,
                Ordering::Relaxed,
            );
        }
    }

    // Step 4: Requeue a HID transfer TRB
    let _ = DIAG_FIRST_QUEUE_SOURCE.compare_exchange(0, 3, Ordering::AcqRel, Ordering::Relaxed);
    queue_hid_transfer(state, hid_idx, slot_id, dci)?;

    ENDPOINT_RESET_COUNT.fetch_add(1, Ordering::Relaxed);

    Ok(())
}

/// Post-enumeration setup: drain stale events, mark HID polling as active.
///
/// Interrupt TRBs are queued post-init (after XHCI_INITIALIZED). This function
/// only drains stale events and marks polling as active.
fn start_hid_polling(state: &XhciState) {
    // Drain any stale Transfer Events that may have been generated during
    // port scanning or previous enumeration attempts.
    drain_stale_events(state);

    // TRBs are now queued inline during enumeration (matching Linux's flow:
    // queue immediately after each interface's HID setup). Set flags so the
    // deferred path and heartbeat know TRBs have been queued.
    KBD_TRB_FIRST_QUEUED.store(true, Ordering::Release);
    HID_TRBS_QUEUED.store(true, Ordering::Release);
}

// =============================================================================
// Port Scanning and Device Enumeration
// =============================================================================

/// Scan all root hub ports for connected devices, enumerate, and configure HID devices.
fn scan_ports(state: &mut XhciState) -> Result<(), &'static str> {

    // Dump PORTSC of all ports (especially USB 2.0 ports 12-13)
    for port in 0..state.max_ports as u64 {
        let portsc_addr = state.op_base + 0x400 + port * 0x10;
        let portsc = read32(portsc_addr);
        let ccs = portsc & 1;
        if ccs != 0 || port >= 12 {
        }
    }

    let mut slots_used: u8 = 0;
    // MOUSE_ONLY: enumerate only 1 device (mouse on port 0), skip keyboard/composite.
    let max_enumerate: u8 = if MOUSE_ONLY { 1 } else { 4 };

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



        // Check if port is enabled (PED, bit 1)
        if portsc & (1 << 1) == 0 {
            // Port not enabled - perform a port reset

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
                continue;
            }

            // Clear PRC (W1C) and check that port is now enabled
            let portsc_after = read32(portsc_addr);
            write32(portsc_addr, (portsc_after & preserve_mask) | (1 << 21));

            let portsc_final = read32(portsc_addr);
            if portsc_final & (1 << 1) == 0 {
                continue;
            }

        }

        // Enable Slot for this device
        let slot_id = match enable_slot(state) {
            Ok(id) => id,
            Err(_) => {
                continue;
            }
        };
        if slot_id == 0 {
            continue;
        }

        slots_used += 1;

        // Address Device (port numbers are 1-based)
        if let Err(_) = address_device(state, slot_id, port as u8 + 1) {
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
        if let Err(_) = get_device_descriptor_short(state, slot_id) {
            continue;
        }

        // Step 2: SET_ISOCH_DELAY (between the two descriptor reads)
        if let Err(_) = set_isoch_delay(state, slot_id) {
        }

        // Step 3: Full device descriptor (18 bytes)
        let mut desc_buf = [0u8; 18];
        if let Err(_) = get_device_descriptor(state, slot_id, &mut desc_buf) {
            continue;
        }

        // Step 4: BOS descriptor
        if let Err(_) = get_bos_descriptor(state, slot_id) {
        }

        // Get Configuration Descriptor
        let mut config_buf = [0u8; 256];
        let config_len = match get_config_descriptor(state, slot_id, &mut config_buf) {
            Ok(len) => len,
            Err(_) => {
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
        if let Err(_) = configure_hid(state, slot_id, &config_buf, config_len) {
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
            offset
        }
        None => {
            xhci_trace_note(0, "no_msi_cap");
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
        xhci_trace_note(0, "err:gicv2m");
        return 0;
    };


    if spi_count == 0 {
        xhci_trace_note(0, "err:no_spis");
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


    // Step 5: Configure GIC for this SPI (edge-triggered).
    //
    // The SPI is NOT enabled here — init() enables it after disabling IMAN.IE
    // to prevent an interrupt storm. With IMAN.IE=0, the XHCI won't write MSI
    // doorbell writes, so the SPI won't fire even though it's enabled.
    gic::configure_spi_edge_triggered(intid);


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
    XHCI_TRACE_ACTIVE.store(true, Ordering::Relaxed);
    xhci_trace_note(0, "init_start");

    // 1. Enable bus mastering + memory space
    pci_dev.enable_bus_master();
    pci_dev.enable_memory_space();

    // 2. Map BAR0 via HHDM
    let bar = pci_dev.get_mmio_bar().ok_or("XHCI: no MMIO BAR found")?;
    let base = HHDM_BASE + bar.address;

    // 3. Read capability registers
    let cap_word = read32(base);
    let cap_length = (cap_word & 0xFF) as u8;

    let hcsparams1 = read32(base + 0x04);
    let hcsparams2 = read32(base + 0x08);
    let hccparams1 = read32(base + 0x10);
    DIAG_HCCPARAMS1.store(hccparams1, Ordering::Relaxed);
    DIAG_HCSPARAMS2.store(hcsparams2, Ordering::Relaxed);
    let db_offset = read32(base + 0x14) & !0x3u32;
    let rts_offset = read32(base + 0x18) & !0x1Fu32;

    let max_slots = (hcsparams1 & 0xFF) as u8;
    let max_ports = ((hcsparams1 >> 24) & 0xFF) as u8;
    let context_size = if hccparams1 & (1 << 2) != 0 { 64 } else { 32 };

    // Check for scratchpad buffers (Linux confirms 0 needed on Parallels vxHC)
    let num_sp = ((hcsparams2 >> 16) & 0x3e0) | ((hcsparams2 >> 27) & 0x1f);
    xhci_trace_note(0, if num_sp > 0 { "scratchpad_needed" } else { "scratchpad_none" });

    let op_base = base + cap_length as u64;
    let rt_base = base + rts_offset as u64;
    let db_base = base + db_offset as u64;


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
                // DW1: Name String (ASCII, e.g., "USB ")
                let _name = read32(ecap_addr + 4);
                // DW2: compatible_port_offset(7:0), compatible_port_count(15:8),
                //       protocol_defined(27:16), protocol_speed_id_count(31:28)
                let _dw2 = read32(ecap_addr + 8);
                // DW3: protocol slot type (3:0)
                let _dw3 = read32(ecap_addr + 12);

            } else if cap_id != 0 {
            }

            if next_ptr == 0 {
                break;
            }
            ecap_addr += next_ptr as u64 * 4;
        }
    } else {
    }

    // 3c. EHCI BIOS handoff — claim the companion EHCI controller.
    //
    // Parallels exposes Intel 82801FB EHCI (0x8086:0x265c) alongside the NEC xHC.
    // Internal Parallels logs show EHC controller resets ~30ms after our
    // ConfigureEndpoint commands, followed by "DisableEndpoint while io_cnt is
    // not zero!" — which kills interrupt endpoints and causes CC=12.
    //
    // By claiming OS ownership of EHCI (USBLEGSUP handoff) and halting it BEFORE
    // our HCRST, we may prevent Parallels' internal EHC reset cascade.
    if let Some(ehci_dev) = crate::drivers::pci::find_device(0x8086, 0x265c) {
        xhci_trace_note(0, "ehci_claim");

        // EHCI USBLEGSUP is at PCI config offset 0x60 for Intel controllers.
        // Bits: [7:0]=cap_id(0x01), [15:8]=next_cap, [16]=BIOS_owned, [24]=OS_owned
        let usblegsup = crate::drivers::pci::pci_read_config_dword(
            ehci_dev.bus, ehci_dev.device, ehci_dev.function, 0x60);

        if usblegsup & 0xFF == 0x01 {
            // Valid USBLEGSUP capability — claim OS ownership
            crate::drivers::pci::pci_write_config_dword(
                ehci_dev.bus, ehci_dev.device, ehci_dev.function, 0x60,
                usblegsup | (1 << 24));

            // Wait for BIOS Owned (bit 16) to clear — up to 100ms
            for _ in 0..100_000u32 {
                let val = crate::drivers::pci::pci_read_config_dword(
                    ehci_dev.bus, ehci_dev.device, ehci_dev.function, 0x60);
                if val & (1 << 16) == 0 {
                    break;
                }
                core::hint::spin_loop();
            }
            xhci_trace_note(0, "ehci_legsup");
        }

        // Map EHCI BAR0 and halt the controller
        if let Some(ehci_bar) = ehci_dev.get_mmio_bar() {
            let ehci_base = HHDM_BASE + ehci_bar.address;
            let ehci_cap_length = read32(ehci_base) & 0xFF;
            let ehci_op_base = ehci_base + ehci_cap_length as u64;

            // USBCMD at op_base + 0x00: clear RS (bit 0) to halt
            let ehci_cmd = read32(ehci_op_base);
            write32(ehci_op_base, ehci_cmd & !1);

            // Wait for HCHalted (USBSTS bit 12) — up to ~10ms
            for _ in 0..100_000u32 {
                let sts = read32(ehci_op_base + 0x04);
                if sts & (1 << 12) != 0 {
                    break;
                }
                core::hint::spin_loop();
            }

            // Also write EHCI USBCMD = 0 to disable all functionality
            write32(ehci_op_base, 0);
            xhci_trace_note(0, "ehci_halted");
        }

        // Disable EHCI bus mastering at PCI level to prevent any DMA activity
        let cmd = crate::drivers::pci::pci_read_config_word(
            ehci_dev.bus, ehci_dev.device, ehci_dev.function, 0x04);
        crate::drivers::pci::pci_write_config_word(
            ehci_dev.bus, ehci_dev.device, ehci_dev.function, 0x04,
            cmd & !0x06); // Clear bus master (bit 2) + memory space (bit 1)
        xhci_trace_note(0, "ehci_pci_off");
    }

    // 4. Stop controller: clear USBCMD.RS, wait for USBSTS.HCH
    let usbcmd = read32(op_base);
    if usbcmd & 1 != 0 {
        // Controller is running, stop it
        write32(op_base, usbcmd & !1);
        wait_for(|| read32(op_base + 0x04) & 1 != 0, 100_000)
            .map_err(|_| "XHCI: timeout waiting for HCH")?;
        xhci_trace_note(0, "ctrl_stopped");
    }

    // 5. Reset: set USBCMD.HCRST, wait for clear
    write32(op_base, read32(op_base) | 2);
    wait_for(|| read32(op_base) & 2 == 0, 100_000)
        .map_err(|_| "XHCI: timeout waiting for HCRST clear")?;
    // Wait for CNR (Controller Not Ready, bit 11 of USBSTS) to clear
    wait_for(|| read32(op_base + 0x04) & (1 << 11) == 0, 100_000)
        .map_err(|_| "XHCI: timeout waiting for CNR clear")?;
    xhci_trace_note(0, "ctrl_reset");

    // 6. Set MaxSlotsEn
    let slots_en = max_slots.min(MAX_SLOTS as u8);
    write32(op_base + 0x38, slots_en as u32); // CONFIG register

    // 6b. Set DNCTRL (Device Notification Control) — match Linux (0x02)
    // Bit 1 (N1) enables Function Wake device notifications.
    write32(op_base + 0x14, 0x02);

    // 7. Set DCBAAP (Device Context Base Address Array Pointer)
    let dcbaa_phys = virt_to_phys((&raw const DCBAA) as u64);
    unsafe {
        // Zero the DCBAA (256 u64 entries)
        let dcbaa = &raw mut DCBAA;
        core::ptr::write_bytes((*dcbaa).0.as_mut_ptr(), 0, 256);
        dma_cache_clean((*dcbaa).0.as_ptr() as *const u8, 256 * core::mem::size_of::<u64>());
    }
    write64(op_base + 0x30, dcbaa_phys);

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


    // 10. Configure MSI for event delivery (restored — hypothesis #24 disproved).
    let irq = setup_xhci_msi(pci_dev);
    XHCI_IRQ.store(irq, Ordering::Release);

    // 11. Enable interrupts on Interrupter 0
    // Set IMOD (Interrupt Moderation) — match Linux (0xa0 = 160 * 250ns = 40µs)
    write32(ir0 + 0x04, 0x000000a0);
    let iman = read32(ir0);
    write32(ir0, iman | 2); // IMAN.IE = 1

    // 12. Start controller: USBCMD.RS=1, INTE=1
    let usbcmd = read32(op_base);
    write32(op_base, usbcmd | 1 | (1 << 2)); // RS=1, INTE=1

    // Wait a bit for ports to detect connections
    for _ in 0..100_000 {
        core::hint::spin_loop();
    }

    // Verify controller is running
    let usbsts = read32(op_base + 0x04);
    if usbsts & 1 != 0 {
        xhci_trace_note(0, "err:ctrl_halted");
    }

    // 12b. NEC vendor command: GET_FW (matches Linux xhci_run() for NEC hosts).
    // Linux sends TRB type 49 (NEC_GET_FW) immediately after RS=1 when vendor=0x1033.
    // The Parallels vxHC emulates NEC uPD720200 and may use this as an init signal.
    {
        let nec_trb = Trb {
            param: 0,
            status: 0,
            control: (trb_type::NEC_GET_FW << 10),
        };
        xhci_trace_trb(XhciTraceOp::CommandSubmit, 0, 0, &nec_trb);
        enqueue_command(nec_trb);
        // Ring host controller doorbell (slot 0, target 0) — state not yet created.
        write32(db_base, 0);
        unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)); }

        // Poll for the vendor command completion (type 48 or standard type 33).
        // Timeout after ~10ms — the NEC FW query is optional.
        let mut got_response = false;
        for _ in 0..100_000u32 {
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
                    let ttype = trb.trb_type();
                    xhci_trace_trb(XhciTraceOp::CommandComplete, 0, 0, &trb);
                    // Advance dequeue
                    EVENT_RING_DEQUEUE = (idx + 1) % EVENT_RING_SIZE;
                    if EVENT_RING_DEQUEUE == 0 { EVENT_RING_CYCLE = !cycle; }
                    write64(ir0 + 0x18,
                        virt_to_phys(&raw const EVENT_RING as u64)
                            + (EVENT_RING_DEQUEUE as u64) * 16
                            | (1 << 3));
                    if ttype == trb_type::COMMAND_COMPLETION
                        || ttype == trb_type::NEC_CMD_COMP
                    {
                        let cc = trb.completion_code();
                        if cc == completion_code::SUCCESS {
                            xhci_trace_note(0, "nec_fw_ok");
                        } else {
                            xhci_trace_note(0, "nec_fw_fail");
                        }
                        got_response = true;
                        break;
                    }
                    // If it's not a command completion, continue (may be port status change)
                }
            }
            core::hint::spin_loop();
        }
        if !got_response {
            xhci_trace_note(0, "nec_fw_timeout");
        }
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
        mouse_nkro_endpoint: 0,
    };

    // 14. Scan ports and configure HID devices.
    //
    // MSI is configured at PCI level (address/data written to xHC before RS=1,
    // matching Linux's pci_alloc_irq_vectors). GIC SPI is NOT yet enabled —
    // enumeration uses direct event ring polling via wait_for_event/wait_for_command.
    if let Err(_) = scan_ports(&mut xhci_state) {
        xhci_trace_note(0, "err:port_scan");
    }


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
    let xhci_state_ref = unsafe {
        (*(&raw const XHCI_STATE)).as_ref().unwrap()
    };

    // Diagnostic: dump endpoint context states AFTER all init (Phase 1-3 complete).
    // This verifies SET_CONFIGURATION and Phase 3 didn't reset the endpoint states.
    dump_endpoint_contexts(xhci_state_ref);

    // Post-init Halted endpoint recovery.
    //
    // Keyboard interrupt IN endpoints (DCI=3, DCI=5) may be Halted due to CC=12 stale
    // events from enumeration. We do NOT reset or requeue them — keyboard now uses EP0
    // GET_REPORT polling exclusively. Let the keyboard interrupt endpoints stay Halted.
    //
    // Mouse interrupt IN endpoints are reset/requeued if Halted (they may also be used).
    {
        let s = xhci_state_ref;
        // Keyboard: just log state, do NOT reset (EP0 GET_REPORT polling handles input).
        if s.kbd_slot != 0 {
            let _kbd_boot_state = read_output_ep_state(s, s.kbd_slot, s.kbd_endpoint);
            if s.kbd_nkro_endpoint != 0 {
                let _kbd_nkro_state = read_output_ep_state(s, s.kbd_slot, s.kbd_nkro_endpoint);
            }
        }
        // CLEAN EXPERIMENT: Do NOT reset mouse endpoints during init.
        // This prevents any TRBs from being queued before poll=300.
        let _mouse_state = read_output_ep_state(s, s.mouse_slot, s.mouse_endpoint);
        let _mouse2_state = read_output_ep_state(s, s.mouse_slot, s.mouse_nkro_endpoint);
    }

    // Drain stale events from enumeration BEFORE queuing interrupt TRBs.
    // This prevents drain_stale_events from consuming CC=12 Transfer Events
    // generated by the interrupt TRBs we're about to queue.
    start_hid_polling(xhci_state_ref);
    HID_POLLING_STARTED.store(true, Ordering::Release);

    // Clear all PORTSC change bits BEFORE queuing interrupt TRBs.
    // Linux explicitly acknowledges port status changes via PORTSC W1C writes.
    // The Parallels vxHC may refuse to service interrupt endpoints (CC=12)
    // if port change conditions haven't been acknowledged.
    let ports_cleared = clear_all_port_changes(xhci_state_ref);
    DIAG_PORTSC_CLEARED.store(ports_cleared, Ordering::Relaxed);

    // NO immediate TRB queue. The Parallels vxHC fires an internal XHC reset
    // ~400ms after HCRST that destroys endpoint state (despite output context
    // still reading "Running"). TRBs are queued in the deferred poll=600 path
    // (~3s after timer starts) which re-issues ConfigureEndpoint first to
    // re-create endpoints after the internal reset settles.

    xhci_trace_note(0, "init_complete");
    xhci_trace_dump();
    XHCI_TRACE_ACTIVE.store(false, Ordering::Relaxed);

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
    if state.irq != 0 {
        crate::arch_impl::aarch64::gic::disable_spi(state.irq);
        crate::arch_impl::aarch64::gic::clear_spi_pending(state.irq);
    }

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
                    xhci_trace_trb(XhciTraceOp::TransferEvent, slot, endpoint, &trb);

                    // Record first Transfer Event diagnostics (MSI handler often
                    // processes events before poll_hid_events sees them).
                    let _ = DIAG_FIRST_XFER_CC.compare_exchange(
                        0xFF, cc, Ordering::AcqRel, Ordering::Relaxed,
                    );
                    let _ = DIAG_FIRST_XFER_PTR.compare_exchange(
                        0, trb.param, Ordering::AcqRel, Ordering::Relaxed,
                    );
                    let _ = DIAG_FIRST_XFER_SLEP.compare_exchange(
                        0, ((slot as u32) << 8) | (endpoint as u32),
                        Ordering::AcqRel, Ordering::Relaxed,
                    );

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
                        } else if slot == state.mouse_slot
                            && state.mouse_nkro_endpoint != 0
                            && endpoint == state.mouse_nkro_endpoint
                        {
                            // Mouse2 (second mouse interface, DCI 5)
                            let report_buf = &raw const MOUSE2_REPORT_BUF;
                            dma_cache_invalidate((*report_buf).0.as_ptr(), 9);
                            let report = &(*report_buf).0;
                            super::hid::process_mouse_report(report);
                            MSI_MOUSE2_NEEDS_REQUEUE.store(true, Ordering::Release);
                        } else if slot == state.mouse_slot
                            && endpoint == 1
                            && HID_TRBS_QUEUED.load(Ordering::Relaxed)
                        {
                            // EP0 GET_REPORT response consumed by MSI handler.
                            // The MSI fires before the timer event loop runs, so all
                            // GET_REPORT Transfer Events arrive here, not in poll_hid_events.
                            MOUSE_GET_REPORT_PENDING.store(false, Ordering::Release);
                            DIAG_FIRST_DB.store(cc, Ordering::Relaxed);
                            GET_REPORT_OK.fetch_add(1, Ordering::Relaxed);
                            let buf = &raw const CTRL_DATA_BUF;
                            dma_cache_invalidate((*buf).0.as_ptr(), 8);
                            let report = &(&(*buf).0)[..8];
                            let snap = u64::from_le_bytes([
                                report[0], report[1], report[2], report[3],
                                report[4], report[5], report[6], report[7],
                            ]);
                            LAST_GET_REPORT_U64.store(snap, Ordering::Relaxed);
                            if report.iter().any(|&b| b != 0) {
                                GET_REPORT_NONZERO.fetch_add(1, Ordering::Relaxed);
                                super::hid::process_mouse_report(report);
                            }
                        }
                    } else {
                        // Error CC on HID interrupt endpoint. All error CCs (including
                        // CC=12) halt the endpoint on Parallels virtual xHC. Reset
                        // Endpoint is required to recover. The rate limiter in
                        // poll_hid_events caps reset rate to RESET_INTERVAL_TICKS to
                        // prevent command ring saturation.
                        XO_ERR_COUNT.fetch_add(1, Ordering::Relaxed);
                        XO_LAST_INFO.store(
                            ((slot as u64) << 16) | ((endpoint as u64) << 8) | (cc as u64),
                            Ordering::Relaxed,
                        );
                        if slot == state.kbd_slot && endpoint == state.kbd_endpoint {
                            NEEDS_RESET_KBD_BOOT.store(true, Ordering::Release);
                        } else if slot == state.kbd_slot
                            && state.kbd_nkro_endpoint != 0
                            && endpoint == state.kbd_nkro_endpoint
                        {
                            NEEDS_RESET_KBD_NKRO.store(true, Ordering::Release);
                        } else if slot == state.mouse_slot && endpoint == state.mouse_endpoint {
                            NEEDS_RESET_MOUSE.store(true, Ordering::Release);
                        } else if slot == state.mouse_slot
                            && state.mouse_nkro_endpoint != 0
                            && endpoint == state.mouse_nkro_endpoint
                        {
                            NEEDS_RESET_MOUSE2.store(true, Ordering::Release);
                        }
                    }
                }
                trb_type::COMMAND_COMPLETION => {
                    // Command completions during enumeration are handled by wait_for_event.
                    // Any stray completions during interrupt handling are ignored.
                }
                trb_type::PORT_STATUS_CHANGE => {
                    // Acknowledge PORTSC change bits for this port.
                    // The xHC generates PSCEs when change bits transition 0→1.
                    // Clearing them tells the xHC we've processed the change.
                    let port_id = ((trb.param >> 24) & 0xFF) as u8;
                    if port_id > 0 {
                        acknowledge_port_changes(state.op_base, port_id);
                    }
                    PSC_COUNT.fetch_add(1, Ordering::Relaxed);
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

}

// =============================================================================
// Polling Mode (fallback for systems without interrupt support)
// =============================================================================

/// Poll for HID events without relying on interrupts.
/// Deferred re-configuration + TRB queue.
///
/// Called from poll_hid_events at poll=600 (~3s after timer start).
/// Re-issues ConfigureEndpoint + BW dance for both slots to re-create
/// interrupt endpoints destroyed by the Parallels vxHC internal reset,
/// then queues interrupt TRBs and rings doorbells.
/// Phase 1 of deferred init: re-issue ConfigureEndpoint + BW dance for both
/// slots, but do NOT queue TRBs yet. This is called at poll=600 (~3s after
/// timer start) to re-create endpoints after the Parallels internal XHC reset.
fn deferred_reconfigure_only(state: &XhciState) {
    // Drain any stale events from the event ring first.
    drain_stale_events(state);

    // Re-configure mouse slot (slot 1) if it has interrupt endpoints.
    if state.mouse_slot != 0 && state.mouse_endpoint != 0 {
        let slot_id = state.mouse_slot;

        let mut pending_eps: [Option<PendingEp>; 4] = [None, None, None, None];
        let mut ep_count = 0usize;

        pending_eps[ep_count] = Some(PendingEp {
            dci: state.mouse_endpoint,
            hid_idx: 1,
            max_pkt: 64,
            b_interval: 4,
            ss_max_burst: 0,
            ss_bytes_per_interval: 64,
            ss_mult: 0,
        });
        ep_count += 1;

        if state.mouse_nkro_endpoint != 0 {
            pending_eps[ep_count] = Some(PendingEp {
                dci: state.mouse_nkro_endpoint,
                hid_idx: 3,
                max_pkt: 64,
                b_interval: 4,
                ss_max_burst: 0,
                ss_bytes_per_interval: 64,
                ss_mult: 0,
            });
            ep_count += 1;
        }

        // Reinitialize the transfer rings (zero + Link TRB) before ConfigEP.
        for i in 0..ep_count {
            if let Some(ref ep) = pending_eps[i] {
                let ring_idx = HID_RING_BASE + ep.hid_idx;
                unsafe {
                    let ring = &raw mut TRANSFER_RINGS[ring_idx];
                    core::ptr::write_bytes(ring as *mut u8, 0, TRANSFER_RING_SIZE * 16);
                    TRANSFER_ENQUEUE[ring_idx] = 0;
                    TRANSFER_CYCLE[ring_idx] = true;
                    let ring_phys = virt_to_phys(&raw const TRANSFER_RINGS[ring_idx] as u64);
                    let link_trb = Trb {
                        param: ring_phys,
                        status: 0,
                        control: (trb_type::LINK << 10) | (1 << 1) | 1,
                    };
                    core::ptr::write_volatile(
                        &mut (*ring)[TRANSFER_RING_SIZE - 1] as *mut Trb,
                        link_trb,
                    );
                    dma_cache_clean(
                        &TRANSFER_RINGS[ring_idx] as *const _ as *const u8,
                        TRANSFER_RING_SIZE * 16,
                    );
                }
            }
        }

        // Deconfigure (DC=1) REMOVED: Parallels vxHC hangs on this command.
        // The virtual xHC does not implement ConfigureEndpoint with DC=1,
        // causing wait_for_command to block forever (no Command Completion event).

        match configure_endpoints_batch(state, slot_id, &pending_eps, ep_count) {
            Ok(()) => crate::serial_println!("[xhci] deferred mouse slot {} cfg_ep OK (ep_count={})", slot_id, ep_count),
            Err(e) => crate::serial_println!("[xhci] deferred mouse slot {} cfg_ep FAIL: {} (ep_count={})", slot_id, e, ep_count),
        }
    }

    // Re-configure keyboard slot (slot 2) if it has interrupt endpoints.
    if state.kbd_slot != 0 && state.kbd_endpoint != 0 {
        let slot_id = state.kbd_slot;

        let mut pending_eps: [Option<PendingEp>; 4] = [None, None, None, None];
        let mut ep_count = 0usize;

        pending_eps[ep_count] = Some(PendingEp {
            dci: state.kbd_endpoint,
            hid_idx: 0,
            max_pkt: 64,
            b_interval: 4,
            ss_max_burst: 0,
            ss_bytes_per_interval: 64,
            ss_mult: 0,
        });
        ep_count += 1;

        if state.kbd_nkro_endpoint != 0 {
            pending_eps[ep_count] = Some(PendingEp {
                dci: state.kbd_nkro_endpoint,
                hid_idx: 2,
                max_pkt: 64,
                b_interval: 4,
                ss_max_burst: 0,
                ss_bytes_per_interval: 64,
                ss_mult: 0,
            });
            ep_count += 1;
        }

        for i in 0..ep_count {
            if let Some(ref ep) = pending_eps[i] {
                let ring_idx = HID_RING_BASE + ep.hid_idx;
                unsafe {
                    let ring = &raw mut TRANSFER_RINGS[ring_idx];
                    core::ptr::write_bytes(ring as *mut u8, 0, TRANSFER_RING_SIZE * 16);
                    TRANSFER_ENQUEUE[ring_idx] = 0;
                    TRANSFER_CYCLE[ring_idx] = true;
                    let ring_phys = virt_to_phys(&raw const TRANSFER_RINGS[ring_idx] as u64);
                    let link_trb = Trb {
                        param: ring_phys,
                        status: 0,
                        control: (trb_type::LINK << 10) | (1 << 1) | 1,
                    };
                    core::ptr::write_volatile(
                        &mut (*ring)[TRANSFER_RING_SIZE - 1] as *mut Trb,
                        link_trb,
                    );
                    dma_cache_clean(
                        &TRANSFER_RINGS[ring_idx] as *const _ as *const u8,
                        TRANSFER_RING_SIZE * 16,
                    );
                }
            }
        }

        // Deconfigure (DC=1) REMOVED: Parallels vxHC hangs on this command.

        match configure_endpoints_batch(state, slot_id, &pending_eps, ep_count) {
            Ok(()) => crate::serial_println!("[xhci] deferred kbd slot {} cfg_ep OK (ep_count={})", slot_id, ep_count),
            Err(e) => crate::serial_println!("[xhci] deferred kbd slot {} cfg_ep FAIL: {} (ep_count={})", slot_id, e, ep_count),
        }
    }

    // Dump EP states from output context to verify Running after deferred cfg_ep.
    unsafe {
        for &(slot_id, label) in &[(state.mouse_slot, "mouse"), (state.kbd_slot, "kbd")] {
            if slot_id != 0 {
                let slot_idx = (slot_id - 1) as usize;
                let dev_ctx = &raw const DEVICE_CONTEXTS[slot_idx];
                dma_cache_invalidate((*dev_ctx).0.as_ptr(), 4096);
                let ctx_size = state.context_size;
                for dci in [3u8, 5u8] {
                    let ep_dw0 = core::ptr::read_volatile(
                        (*dev_ctx).0.as_ptr().add(dci as usize * ctx_size) as *const u32,
                    );
                    let ep_state = ep_dw0 & 0x7;
                    crate::serial_println!(
                        "[xhci] deferred {} slot {} DCI {} EP_state={} (0=Disabled,1=Running)",
                        label, slot_id, dci, ep_state
                    );
                }
            }
        }
    }
}

/// Phase 2 of deferred init: queue TRBs and ring doorbells for all HID
/// endpoints. Called at poll=1200 (~6s after timer start), 3 seconds after
/// the deferred ConfigureEndpoint to allow any secondary internal reset to settle.
fn deferred_queue_trbs(state: &XhciState) {
    // Drain any events that appeared between ConfigEP and now.
    drain_stale_events(state);

    // Dump EP states right before queuing to verify they're still Running.
    unsafe {
        for &(slot_id, label) in &[(state.mouse_slot, "mouse"), (state.kbd_slot, "kbd")] {
            if slot_id != 0 {
                let slot_idx = (slot_id - 1) as usize;
                let dev_ctx = &raw const DEVICE_CONTEXTS[slot_idx];
                dma_cache_invalidate((*dev_ctx).0.as_ptr(), 4096);
                let ctx_size = state.context_size;
                for dci in [3u8, 5u8] {
                    let ep_dw0 = core::ptr::read_volatile(
                        (*dev_ctx).0.as_ptr().add(dci as usize * ctx_size) as *const u32,
                    );
                    let ep_state = ep_dw0 & 0x7;
                    crate::serial_println!(
                        "[xhci] pre-queue {} slot {} DCI {} EP_state={} (0=Disabled,1=Running)",
                        label, slot_id, dci, ep_state
                    );
                }
            }
        }
    }

    KBD_TRB_FIRST_QUEUED.store(true, Ordering::Release);
    HID_TRBS_QUEUED.store(true, Ordering::Release);

    // Keyboard boot
    if state.kbd_slot != 0 && state.kbd_endpoint != 0 {
        let _ = queue_hid_transfer(state, 0, state.kbd_slot, state.kbd_endpoint);
    }
    // Keyboard NKRO
    if state.kbd_slot != 0 && state.kbd_nkro_endpoint != 0 {
        let _ = queue_hid_transfer(state, 2, state.kbd_slot, state.kbd_nkro_endpoint);
    }
    // Mouse boot
    if state.mouse_slot != 0 && state.mouse_endpoint != 0 {
        let _ = queue_hid_transfer(state, 1, state.mouse_slot, state.mouse_endpoint);
    }
    // Mouse2
    if state.mouse_slot != 0 && state.mouse_nkro_endpoint != 0 {
        let _ = queue_hid_transfer(state, 3, state.mouse_slot, state.mouse_nkro_endpoint);
    }
}

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

                    // EP0 GET_REPORT response (non-blocking async path).
                    // Primary path: PENDING=true means we're expecting a response right now.
                    // Secondary path: PENDING=false (stale-cleared) but event still arrived —
                    //   late responses are valid and must update gk/fd.
                    // Post-enumeration, the only EP0 events for mouse_slot are GET_REPORT.
                    if slot == state.mouse_slot
                        && endpoint == 1
                        && MOUSE_GET_REPORT_PENDING.load(Ordering::Acquire)
                    {
                        MOUSE_GET_REPORT_PENDING.store(false, Ordering::Release);
                        // Record last CC seen (fd= heartbeat field, 0xFF = no event yet).
                        DIAG_FIRST_DB.store(cc, Ordering::Relaxed);
                        if cc == completion_code::SUCCESS || cc == completion_code::SHORT_PACKET {
                            GET_REPORT_OK.fetch_add(1, Ordering::Relaxed);
                            let buf = &raw const CTRL_DATA_BUF;
                            dma_cache_invalidate((*buf).0.as_ptr(), 8);
                            let report = &(&(*buf).0)[..8];
                            if report.iter().any(|&b| b != 0) {
                                GET_REPORT_NONZERO.fetch_add(1, Ordering::Relaxed);
                                super::hid::process_mouse_report(report);
                            }
                        }
                        // Event consumed — advance dequeue and continue event loop
                    } else if slot == state.mouse_slot && endpoint == 1 {
                        // Late response: PENDING was stale-cleared but Transfer Event arrived
                        // anyway. Catch it here so gk and fd reflect these successes.
                        DIAG_FIRST_DB.store(cc, Ordering::Relaxed);
                        if cc == completion_code::SUCCESS || cc == completion_code::SHORT_PACKET {
                            GET_REPORT_OK.fetch_add(1, Ordering::Relaxed);
                            let buf = &raw const CTRL_DATA_BUF;
                            dma_cache_invalidate((*buf).0.as_ptr(), 8);
                            let report = &(&(*buf).0)[..8];
                            if report.iter().any(|&b| b != 0) {
                                GET_REPORT_NONZERO.fetch_add(1, Ordering::Relaxed);
                                super::hid::process_mouse_report(report);
                            }
                        }
                    } else if cc == completion_code::SUCCESS || cc == completion_code::SHORT_PACKET {
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
                        // Mouse interrupt endpoint event (DCI 3)
                        else if slot == state.mouse_slot && endpoint == state.mouse_endpoint {
                            let report_buf = &raw const MOUSE_REPORT_BUF;
                            dma_cache_invalidate((*report_buf).0.as_ptr(), 8);
                            let report = &(*report_buf).0;
                            super::hid::process_mouse_report(report);
                            let _ = queue_hid_transfer(state, 1, state.mouse_slot, state.mouse_endpoint);
                        }
                        // Mouse2 interrupt endpoint event (DCI 5)
                        else if slot == state.mouse_slot
                            && state.mouse_nkro_endpoint != 0
                            && endpoint == state.mouse_nkro_endpoint
                        {
                            let report_buf = &raw const MOUSE2_REPORT_BUF;
                            dma_cache_invalidate((*report_buf).0.as_ptr(), 9);
                            let report = &(*report_buf).0;
                            super::hid::process_mouse_report(report);
                            let _ = queue_hid_transfer(state, 3, state.mouse_slot, state.mouse_nkro_endpoint);
                        } else {
                            XFER_OTHER_COUNT.fetch_add(1, Ordering::Relaxed);
                            XO_LAST_INFO.store(
                                ((slot as u64) << 16) | ((endpoint as u64) << 8) | (cc as u64),
                                Ordering::Relaxed,
                            );
                        }
                    } else {
                        // Error CC on HID interrupt endpoint. All error CCs (including
                        // CC=12) halt the endpoint on Parallels virtual xHC. Reset
                        // Endpoint is required to recover. The rate limiter in the
                        // NEEDS_RESET_* block below caps reset rate to RESET_INTERVAL_TICKS
                        // to prevent command ring saturation.
                        XFER_OTHER_COUNT.fetch_add(1, Ordering::Relaxed);
                        XO_ERR_COUNT.fetch_add(1, Ordering::Relaxed);
                        XO_LAST_INFO.store(
                            ((slot as u64) << 16) | ((endpoint as u64) << 8) | (cc as u64),
                            Ordering::Relaxed,
                        );
                        // Read endpoint state from output context on first error CC.
                        // Diagnostic: tells us the state after CC=12 (Running=1, Halted=2, etc.)
                        if DIAG_EP_STATE_AFTER_CC12.load(Ordering::Relaxed) == 0xFF
                            && slot > 0
                            && (slot as usize) <= MAX_SLOTS
                        {
                            let slot_idx = (slot - 1) as usize;
                            let ep_out = DEVICE_CONTEXTS[slot_idx]
                                .0
                                .as_ptr()
                                .add(endpoint as usize * state.context_size);
                            dma_cache_invalidate(ep_out, 4);
                            let dw0 = core::ptr::read_volatile(ep_out as *const u32);
                            let ep_state = dw0 & 0x7;
                            DIAG_EP_STATE_AFTER_CC12.store(
                                ((slot as u32) << 16) | ((endpoint as u32) << 8) | ep_state,
                                Ordering::Relaxed,
                            );
                        }
                        if slot == state.kbd_slot && endpoint == state.kbd_endpoint {
                            NEEDS_RESET_KBD_BOOT.store(true, Ordering::Release);
                        } else if slot == state.kbd_slot
                            && state.kbd_nkro_endpoint != 0
                            && endpoint == state.kbd_nkro_endpoint
                        {
                            NEEDS_RESET_KBD_NKRO.store(true, Ordering::Release);
                        } else if slot == state.mouse_slot && endpoint == state.mouse_endpoint {
                            NEEDS_RESET_MOUSE.store(true, Ordering::Release);
                        } else if slot == state.mouse_slot
                            && state.mouse_nkro_endpoint != 0
                            && endpoint == state.mouse_nkro_endpoint
                        {
                            NEEDS_RESET_MOUSE2.store(true, Ordering::Release);
                        }
                    }
                }
                trb_type::COMMAND_COMPLETION => {
                    // Stray command completions — may arrive from recovery commands.
                    // Ignore safely; wait_for_command_completion handles expected ones.
                }
                trb_type::PORT_STATUS_CHANGE => {
                    PSC_COUNT.fetch_add(1, Ordering::Relaxed);
                    let port_id = ((trb.param >> 24) & 0xFF) as u8;
                    if port_id > 0 {
                        acknowledge_port_changes(state.op_base, port_id);
                    }
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

    // MSI requeue flags: clear without requeuing. The reset state machine
    // below already requeues after resetting, and poll_hid_events requeues
    // after successful transfers. Double-requeuing would cause duplicate TRBs.
    let _ = MSI_KBD_NEEDS_REQUEUE.swap(false, Ordering::AcqRel);
    let _ = MSI_NKRO_NEEDS_REQUEUE.swap(false, Ordering::AcqRel);
    let _ = MSI_MOUSE_NEEDS_REQUEUE.swap(false, Ordering::AcqRel);
    let _ = MSI_MOUSE2_NEEDS_REQUEUE.swap(false, Ordering::AcqRel);

    let poll = POLL_COUNT.load(Ordering::Relaxed);

    // Reset state machine: on CC=12 (or any error CC), the endpoint may be
    // Halted internally even if the output context says Running. Issue Reset
    // Endpoint → Set TR Dequeue → requeue to recover. Rate-limited to avoid
    // command ring saturation.
    if NEEDS_RESET_KBD_BOOT.load(Ordering::Acquire) {
        let last = KBD_BOOT_RESET_POLL.load(Ordering::Relaxed);
        if poll >= last + RESET_INTERVAL_TICKS {
            NEEDS_RESET_KBD_BOOT.store(false, Ordering::Release);
            KBD_BOOT_RESET_POLL.store(poll, Ordering::Relaxed);
            if state.kbd_slot != 0 && state.kbd_endpoint != 0 {
                let _ = reset_halted_endpoint(state, state.kbd_slot, state.kbd_endpoint, 0);
            }
        }
    }
    if NEEDS_RESET_KBD_NKRO.load(Ordering::Acquire) {
        let last = KBD_NKRO_RESET_POLL.load(Ordering::Relaxed);
        if poll >= last + RESET_INTERVAL_TICKS {
            NEEDS_RESET_KBD_NKRO.store(false, Ordering::Release);
            KBD_NKRO_RESET_POLL.store(poll, Ordering::Relaxed);
            if state.kbd_slot != 0 && state.kbd_nkro_endpoint != 0 {
                let _ = reset_halted_endpoint(state, state.kbd_slot, state.kbd_nkro_endpoint, 2);
            }
        }
    }
    if NEEDS_RESET_MOUSE.load(Ordering::Acquire) {
        let last = MOUSE_RESET_POLL.load(Ordering::Relaxed);
        if poll >= last + RESET_INTERVAL_TICKS {
            NEEDS_RESET_MOUSE.store(false, Ordering::Release);
            MOUSE_RESET_POLL.store(poll, Ordering::Relaxed);
            if state.mouse_slot != 0 && state.mouse_endpoint != 0 {
                let _ = reset_halted_endpoint(state, state.mouse_slot, state.mouse_endpoint, 1);
            }
        }
    }
    if NEEDS_RESET_MOUSE2.load(Ordering::Acquire) {
        let last = MOUSE2_RESET_POLL.load(Ordering::Relaxed);
        if poll >= last + RESET_INTERVAL_TICKS {
            NEEDS_RESET_MOUSE2.store(false, Ordering::Release);
            MOUSE2_RESET_POLL.store(poll, Ordering::Relaxed);
            if state.mouse_slot != 0 && state.mouse_nkro_endpoint != 0 {
                let _ = reset_halted_endpoint(state, state.mouse_slot, state.mouse_nkro_endpoint, 3);
            }
        }
    }

    // Deferred MSI activation.
    // SPI is enabled after a stabilization period (200 polls = 1 second)
    // to avoid interfering with init.
    if state.irq != 0 && poll >= 200 {
        // Enable SPI for MSI delivery (handle_interrupt disables on each fire)
        crate::arch_impl::aarch64::gic::clear_spi_pending(state.irq);
        crate::arch_impl::aarch64::gic::enable_spi(state.irq);
        DIAG_SPI_ENABLE_COUNT.fetch_add(1, Ordering::Relaxed);
    }

    // Ensure HID_TRBS_QUEUED is set after initialization completes.
    if poll >= 250 && !HID_TRBS_QUEUED.load(Ordering::Acquire) {
        HID_TRBS_QUEUED.store(true, Ordering::Release);
    }

    // Re-ring doorbells after SPI activation (poll=300, ~1.5s after timer starts).
    //
    // The Parallels vxHC may not process interrupt endpoint TRBs until the MSI/SPI
    // interrupt path is active. TRBs were queued and doorbells rung during init
    // (before the timer started), but the SPI wasn't enabled until poll=200.
    // Re-ringing doorbells after SPI activation tells the xHC to re-check the
    // transfer rings now that the interrupt delivery path is ready.
    static DOORBELLS_RE_RUNG: AtomicBool = AtomicBool::new(false);
    if poll == 300 && !DOORBELLS_RE_RUNG.load(Ordering::Acquire) {
        DOORBELLS_RE_RUNG.store(true, Ordering::Release);
        // Mouse EP3
        if state.mouse_slot != 0 && state.mouse_endpoint != 0 {
            ring_doorbell(state, state.mouse_slot, state.mouse_endpoint);
        }
        // Mouse EP5 (mouse2)
        if state.mouse_slot != 0 && state.mouse_nkro_endpoint != 0 {
            ring_doorbell(state, state.mouse_slot, state.mouse_nkro_endpoint);
        }
        // Keyboard EP3
        if state.kbd_slot != 0 && state.kbd_endpoint != 0 {
            ring_doorbell(state, state.kbd_slot, state.kbd_endpoint);
        }
        // Keyboard EP5 (NKRO)
        if state.kbd_slot != 0 && state.kbd_nkro_endpoint != 0 {
            ring_doorbell(state, state.kbd_slot, state.kbd_nkro_endpoint);
        }
    }

    // Deferred re-enumerate + TRB queue.
    //
    // The Parallels vxHC fires an internal XHC reset ~400ms after HCRST that
    // destroys interrupt endpoint state (while output context still reads
    // "Running"). The C harness avoids CC=12 by waiting 2s then re-enumerating.
    //
    // At poll=600 (~3s after timer start), we re-issue ConfigureEndpoint + BW
    // dance to re-create endpoints after the internal reset has settled, then
    // queue interrupt TRBs.
    // Deferred re-configure + TRB queue DISABLED.
    //
    // Breakthrough: queuing TRBs during init (inline, matching Linux's flow) produces
    // xe=0, fc=255 (NO CC=12 errors). The CC=12 was caused by the deferred re-
    // ConfigureEndpoint at poll=600 which destroyed and rebuilt transfer rings.
    // The Parallels vxHC returns CC=12 when endpoints are re-configured AFTER the
    // initial enumeration — the initial ConfigureEndpoint + BW dance creates proper
    // internal state, but any subsequent re-ConfigureEndpoint breaks it.
    //
    // TRBs are now queued inline during enumeration (Phase 3), matching Linux's flow.
    // The deferred path is no longer needed.

    // NOTE: Mouse EP0 GET_REPORT polling is disabled.
    // The Parallels virtual xHC echoes the 8-byte setup packet back as the "data"
    // response (LAST_GET_REPORT_U64 = 0x00080000010001a1 = GET_REPORT setup bytes),
    // causing phantom mouse clicks (buttons=0xA1) and cursor drift (deltaX=1).
    // Mouse input will be handled via interrupt IN endpoints when that path is working.

    // Periodic diagnostic: dump controller + endpoint state every 2000 polls (~10s)
    if poll > 0 && poll % 2000 == 0 {
        unsafe {
            // USBCMD (bit 0 = RS, bit 2 = INTE)
            let usbcmd = read32(state.op_base);
            DIAG_USBCMD.store(usbcmd, Ordering::Relaxed);

            // USBSTS (bit 0 = HCH halted, bit 3 = EINT)
            let usbsts = read32(state.op_base + 0x04);
            DIAG_USBSTS.store(usbsts, Ordering::Relaxed);

            // IMAN for Interrupter 0 (bit 0 = IP, bit 1 = IE)
            let ir0 = state.rt_base + 0x20;
            let iman = read32(ir0);
            DIAG_IMAN.store(iman, Ordering::Relaxed);

            // ERDP readback — verify our last ERDP write took effect
            let erdp_readback = read64(ir0 + 0x18);
            DIAG_ERDP_READBACK.store(erdp_readback, Ordering::Relaxed);

            // Event ring state: dequeue index + cycle bit
            let er_idx = EVENT_RING_DEQUEUE;
            let er_cycle = EVENT_RING_CYCLE;
            DIAG_ER_STATE.store(((er_idx as u32) << 1) | if er_cycle { 1 } else { 0 }, Ordering::Relaxed);

            // Raw event ring TRB at current dequeue index
            {
                let ring = &raw const EVENT_RING;
                dma_cache_invalidate(
                    &(*ring).0[er_idx] as *const Trb as *const u8,
                    core::mem::size_of::<Trb>(),
                );
                let trb = core::ptr::read_volatile(&(*ring).0[er_idx]);
                DIAG_ER_TRB_CONTROL.store(trb.control, Ordering::Relaxed);
            }

            // Runtime output context TRDP for mouse EP3
            if state.mouse_slot != 0 && state.mouse_endpoint != 0 {
                let slot_idx = (state.mouse_slot - 1) as usize;
                let ctx_size = state.context_size;
                let dev_ctx = &raw const DEVICE_CONTEXTS[slot_idx];
                dma_cache_invalidate((*dev_ctx).0.as_ptr(), 4096);
                let ep_base = (*dev_ctx).0.as_ptr().add(state.mouse_endpoint as usize * ctx_size);
                let trdp_lo = core::ptr::read_volatile(ep_base.add(8) as *const u32);
                let trdp_hi = core::ptr::read_volatile(ep_base.add(12) as *const u32);
                DIAG_RUNTIME_TRDP.store(((trdp_hi as u64) << 32) | (trdp_lo as u64), Ordering::Relaxed);
            }

            // Raw transfer ring TRB at position 0 for mouse ring (hid_idx=1)
            {
                let ring_idx = HID_RING_BASE + 1; // mouse
                let trb = core::ptr::read_volatile(&TRANSFER_RINGS[ring_idx][0]);
                DIAG_TR_TRB_CONTROL.store(trb.control, Ordering::Relaxed);
            }

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
