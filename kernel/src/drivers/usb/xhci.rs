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

use core::sync::atomic::{fence, AtomicBool, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

use super::descriptors::{
    class_code, descriptor_type, hid_protocol, hid_request, request, ConfigDescriptor,
    DeviceDescriptor, EndpointDescriptor, InterfaceDescriptor, SetupPacket,
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

/// Skip HCRST and use halt-resume instead. On Parallels, HCRST may destroy
/// the hypervisor's internal USB device model state. By just halting (RS=0),
/// swapping data structures, and resuming (RS=1), we preserve whatever
/// internal state the hypervisor built during UEFI firmware operation.
const SKIP_HCRST: bool = false;

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
/// Tested both with and without — CC=12 persists regardless.
// EXPERIMENT: Skip BW dance. The Parallels hypervisor flags StopEndpoint
// commands as "not supported" and after 3 rapid StopEndpoints, triggers a
// cascading 2nd XHC controller reset that destroys endpoint state from 1st HCRST.
const SKIP_BW_DANCE: bool = true;

/// UEFI-inherit mode: Instead of HCRST + full re-enumeration, inherit the
/// xHCI state left by UEFI firmware. We only swap out the command ring and
/// event ring, then use StopEndpoint + SetTRDequeuePointer to redirect
/// interrupt endpoints to our transfer rings.
///
/// This tests whether UEFI-created endpoints are still functional after
/// ExitBootServices. If CC=12 disappears, the root cause is that the
/// Parallels hypervisor only creates internal endpoint state from UEFI's
/// ConfigureEndpoint commands, not from post-EBS re-enumeration.
const INHERIT_UEFI: bool = false;

/// Experiment: after ConfigureEndpoint leaves EPs in Running state, explicitly
/// StopEndpoint + Set TR Dequeue Pointer before queueing the first Normal TRB.
/// This tests whether the xHC accepts the transfer ring address when set via
/// command (read from the command ring, which is proven to work) rather than
/// Deferred TRB queue: poll count at which to first queue interrupt TRBs.
/// 0 = queue immediately during init (current behavior).
/// 400 = defer to poll=400 (~2s after timer starts, matching Linux's msleep(2000)).
/// When > 0, start_hid_polling() is NOT called during init; instead,
/// poll_hid_events checks this value and queues TRBs on the first matching poll.
const DEFERRED_TRB_POLL: u64 = 0;

/// Post-enumeration delay in milliseconds before first doorbell ring.
/// The Linux kernel module has msleep(2000) between ConfigureEndpoint and
/// the first interrupt TRB doorbell. This gives the Parallels virtual xHC
/// time to stabilize internal endpoint state. 0 = no delay.
const POST_ENUM_DELAY_MS: u32 = 0;

/// Focus debug mode: only initialize the mouse device (slot=1), skip keyboard entirely.
/// Reduces from 4 interrupt endpoints to 2, isolating whether CC=12 is caused by
/// keyboard interference or is a fundamental per-endpoint issue.
const MOUSE_ONLY: bool = false;

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
/// Event ring size in TRBs (256 entries x 16 bytes = 4096 bytes, matching Linux).
const EVENT_RING_SIZE: usize = 256;
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
///
/// EXPERIMENT: Moved from .dma (NC at 0x50000000) to .bss (WB-cacheable, zeroed by boot.S).
/// Testing whether NC memory mapping causes CC=12 on Parallels.
static mut DCBAA: AlignedPage<[u64; 256]> = AlignedPage([0u64; 256]);

/// Command Ring: 64 TRBs x 16 bytes = 1KB.
static mut CMD_RING: Aligned64<[Trb; CMD_RING_SIZE]> = Aligned64([Trb::zeroed(); CMD_RING_SIZE]);
/// Command ring enqueue pointer index.
static mut CMD_RING_ENQUEUE: usize = 0;
/// Command ring producer cycle state.
static mut CMD_RING_CYCLE: bool = true;

/// Event Ring: 256 TRBs x 16 bytes = 4KB (matches Linux xhci-ring.c).
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

/// Saved UEFI DCBAAP value, read before we overwrite it during init.
/// Used by INHERIT_UEFI to copy UEFI's device context data into our arrays.
static mut UEFI_DCBAAP_SAVED: u64 = 0;

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
/// Set once after the first deferred SPI activation; handle_interrupt
/// keeps SPI alive after that, so poll_hid_events doesn't re-enable.
static SPI_ACTIVATED: AtomicBool = AtomicBool::new(false);
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
/// Diagnostic: raw EP context DWORDs at first CC=12 event (captured once).
/// DW0: EP State[2:0], Mult[9:8], MaxPStreams[14:10], Interval[23:16], MaxESITHi[31:24]
/// DW1: CErr[2:1], EPType[5:3], MaxBurst[15:8], MaxPacketSize[31:16]
/// DW2-3: TR Dequeue Pointer (64-bit) with DCS[0]
/// DW4: AvgTRBLen[15:0], MaxESITLo[31:16]
static DIAG_CC12_EP_DW0: AtomicU32 = AtomicU32::new(0xDEAD);
static DIAG_CC12_EP_DW1: AtomicU32 = AtomicU32::new(0xDEAD);
static DIAG_CC12_EP_DW2: AtomicU32 = AtomicU32::new(0xDEAD);
static DIAG_CC12_EP_DW3: AtomicU32 = AtomicU32::new(0xDEAD);
static DIAG_CC12_EP_DW4: AtomicU32 = AtomicU32::new(0xDEAD);
/// Diagnostic: Slot Context DW0 at first CC=12 (Context Entries in bits 31:27).
static DIAG_CC12_SLOT_DW0: AtomicU32 = AtomicU32::new(0xDEAD);
/// Diagnostic: Slot Context DW3 at first CC=12 (Slot State in bits 31:27, USB Addr in 7:0).
static DIAG_CC12_SLOT_DW3: AtomicU32 = AtomicU32::new(0xDEAD);
/// Diagnostic: DCBAA entry for the slot at first CC=12 (physical address of device context).
static DIAG_CC12_DCBAA: AtomicU64 = AtomicU64::new(0xDEAD);
/// Diagnostic: MFINDEX register value (microframe index) for timing analysis.
pub static DIAG_MFINDEX: AtomicU32 = AtomicU32::new(0);
/// Diagnostic: counts Transfer Events silently consumed during enumeration
/// by wait_for_event_inner (command_only mode) and control_transfer (non-EP0 skip).
pub static CONSUMED_XFER_DURING_ENUM: AtomicU64 = AtomicU64::new(0);
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

/// Whether initial HID interrupt TRBs have been queued post-init.
/// TRBs are deferred until after XHCI_INITIALIZED and SPI enable so the full
/// MSI → GIC SPI → CPU ISR → IMAN.IP ack pathway is active when the xHC
/// processes the first interrupt endpoint transfer.
static HID_TRBS_QUEUED: AtomicBool = AtomicBool::new(false);

// =============================================================================
// Memory Helpers
// =============================================================================

/// Convert a kernel virtual address to the IPA (Intermediate Physical Address)
/// that DMA controllers need to access guest memory.
///
/// On QEMU/Parallels (RAM offset=0): VA 0xFFFF_0000_40xx → IPA 0x40xx.
/// On VMware (RAM offset=0x40000000): VA 0xFFFF_0000_80xx → IPA 0x80xx.
///
/// The kernel binary uses PC-relative (ADRP) addressing for statics. On VMware,
/// the kernel runs at VA 0xFFFF_0000_80XXXXXX (identity-mapped via L1[2]), so
/// BSS statics have flat addresses in the 0x80XXXXXX range — already valid IPAs.
/// Addresses in the linker-expected 0x40XXXXXX range (via L1[1] remapping) need
/// the RAM base offset added to get the actual IPA.
#[inline]
fn virt_to_phys(virt: u64) -> u64 {
    if virt >= HHDM_BASE {
        let flat = virt - HHDM_BASE;
        let rbo = crate::platform_config::ram_base_offset();
        let actual_ram_base = 0x4000_0000u64 + rbo;
        if flat >= actual_ram_base {
            // Already in the actual physical RAM range (identity-mapped on VMware
            // via L1[2], or direct on QEMU/Parallels where rbo=0).
            flat
        } else if flat >= 0x4000_0000 {
            // In the linker-expected range (L1[1] remapping on VMware).
            flat + rbo
        } else {
            // Device MMIO (< 0x40000000): identity-mapped on all platforms.
            flat
        }
    } else {
        // Already a physical address (identity-mapped kernel on Parallels)
        virt
    }
}

/// Clean (flush) a range of memory from CPU caches to the point of coherency.
///
/// Must be called after writing DMA descriptors/data and before issuing
/// DMA commands, so the device sees the updated data in physical memory.
///
/// RE-ENABLED: DMA structures are now in .bss (WB-cacheable memory).
/// CPU stores go to cache and must be flushed to memory before the
/// xHCI controller (via hypervisor DMA emulation) can see them.
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
        core::arch::asm!("dsb st", options(nostack, preserves_flags));
    }
}

/// Invalidate a range of memory in CPU caches after a device DMA write.
///
/// Must be called after the xHCI controller writes to DMA memory (e.g.,
/// output device contexts, event ring), before the CPU reads the data.
///
/// RE-ENABLED: DMA structures are now in .bss (WB-cacheable memory).
/// Uses dc civac (clean+invalidate) which first writes back any dirty
/// data then invalidates, ensuring the CPU reads fresh data from memory.
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
        core::arch::asm!("dsb ld", options(nostack, preserves_flags));
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

// =============================================================================
// MMIO Write Trace — captures every write32() from HCRST through first doorbell
// =============================================================================

/// Maximum entries in the MMIO write trace buffer.
const MMIO_TRACE_MAX: usize = 4096;

/// Whether MMIO write tracing is active.
static MMIO_TRACE_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Current write index into MMIO_TRACE_BUF.
static MMIO_TRACE_IDX: AtomicU32 = AtomicU32::new(0);

/// BAR base virtual address, set once during init so write32 can compute offsets.
static MMIO_TRACE_BAR_BASE: AtomicU64 = AtomicU64::new(0);

/// Trace buffer: (offset_from_BAR, value) pairs.
/// offset and value are each u32, packed into a u64 for atomic-free static init.
static mut MMIO_TRACE_BUF: [(u32, u32); MMIO_TRACE_MAX] = [(0u32, 0u32); MMIO_TRACE_MAX];

/// Whether tracing is active.
static XHCI_TRACE_ACTIVE: AtomicBool = AtomicBool::new(false);
/// Monotonic sequence number for trace records.
static XHCI_TRACE_SEQ: AtomicU32 = AtomicU32::new(0);
/// Data buffer write cursor.
static XHCI_TRACE_DATA_CURSOR: AtomicU32 = AtomicU32::new(0);

/// Trace record ring buffer.
static mut XHCI_TRACE_RECORDS: [XhciTraceRecord; XHCI_TRACE_MAX_RECORDS] = {
    const ZERO: XhciTraceRecord = XhciTraceRecord {
        seq: 0,
        op: 0,
        slot: 0,
        dci: 0,
        _pad: 0,
        timestamp: 0,
        data_offset: 0xFFFF_FFFF,
        data_len: 0,
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
            core::ptr::copy_nonoverlapping(data.as_ptr(), data_ptr.add(off), copy_len);
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
    let bytes: [u8; 16] = unsafe { core::mem::transmute_copy(trb) };
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

/// Format the xHCI trace buffer as a String for procfs consumption.
/// Same data as xhci_trace_dump() but writes to a String instead of serial.
pub fn format_trace_buffer() -> alloc::string::String {
    use alloc::string::String;
    use core::fmt::Write;

    let mut out = String::new();
    let total = XHCI_TRACE_SEQ.load(Ordering::Relaxed);
    if total == 0 {
        let _ = writeln!(out, "=== XHCI_TRACE_START ===");
        let _ = writeln!(out, "(no records)");
        let _ = writeln!(out, "=== XHCI_TRACE_END ===");
        return out;
    }

    let start = if total as usize <= XHCI_TRACE_MAX_RECORDS {
        0u32
    } else {
        total - XHCI_TRACE_MAX_RECORDS as u32
    };

    let _ = writeln!(out, "=== XHCI_TRACE_START total={} ===", total);

    for seq in start..total {
        let idx = seq as usize % XHCI_TRACE_MAX_RECORDS;
        let rec = unsafe {
            &*core::ptr::addr_of!(XHCI_TRACE_RECORDS)
                .cast::<XhciTraceRecord>()
                .add(idx)
        };

        let op_name = op_name_str(rec.op);

        let _ = writeln!(
            out,
            "T {:04} {:12} S={:02} E={:02} TS={:016X} LEN={:04X}",
            rec.seq, op_name, rec.slot, rec.dci, rec.timestamp, rec.data_len,
        );

        if rec.data_len > 0 && rec.data_offset != 0xFFFF_FFFF {
            let off = rec.data_offset as usize;
            let len = rec.data_len as usize;
            if off + len <= XHCI_TRACE_DATA_SIZE {
                let data = unsafe {
                    core::slice::from_raw_parts(
                        core::ptr::addr_of!(XHCI_TRACE_DATA).cast::<u8>().add(off),
                        len,
                    )
                };

                if rec.op == 50 {
                    if let Ok(s) = core::str::from_utf8(data) {
                        let _ = writeln!(out, "  \"{}\"", s);
                    }
                    continue;
                }

                let mut i = 0;
                while i < len {
                    let row_end = (i + 16).min(len);
                    let _ = write!(out, "  ");
                    let mut j = i;
                    while j < row_end {
                        let dw_end = (j + 4).min(row_end);
                        let mut k = j;
                        while k < dw_end {
                            let byte = data[k];
                            let _ = write!(out, "{:02X}", byte);
                            k += 1;
                        }
                        if dw_end < row_end {
                            let _ = write!(out, " ");
                        }
                        j = dw_end;
                    }
                    let _ = writeln!(out);
                    i += 16;
                }
            }
        }
    }

    let _ = writeln!(out, "=== XHCI_TRACE_END ===");

    // Append diagnostic counters for btrace to parse
    let _ = writeln!(out, "=== XHCI_DIAG ===");
    let _ = writeln!(out, "poll_count {}", POLL_COUNT.load(Ordering::Relaxed));
    let _ = writeln!(out, "event_count {}", EVENT_COUNT.load(Ordering::Relaxed));
    let _ = writeln!(
        out,
        "consumed_xfer_enum {}",
        CONSUMED_XFER_DURING_ENUM.load(Ordering::Relaxed)
    );
    let _ = writeln!(
        out,
        "first_xfer_cc {}",
        DIAG_FIRST_XFER_CC.load(Ordering::Relaxed)
    );
    let _ = writeln!(
        out,
        "first_queue_src {}",
        DIAG_FIRST_QUEUE_SOURCE.load(Ordering::Relaxed)
    );
    let _ = writeln!(
        out,
        "ep_state_cc12 {}",
        DIAG_EP_STATE_AFTER_CC12.load(Ordering::Relaxed)
    );
    let _ = writeln!(
        out,
        "endpoint_resets {}",
        ENDPOINT_RESET_COUNT.load(Ordering::Relaxed)
    );
    let _ = writeln!(
        out,
        "xfer_other {}",
        XFER_OTHER_COUNT.load(Ordering::Relaxed)
    );
    let _ = writeln!(
        out,
        "ep_before_db 0x{:08x}",
        DIAG_EP_STATE_BEFORE_DB.load(Ordering::Relaxed)
    );
    let _ = writeln!(
        out,
        "slot_before_db 0x{:08x}",
        DIAG_SLOT_STATE_BEFORE_DB.load(Ordering::Relaxed)
    );
    let _ = writeln!(
        out,
        "trdp_output 0x{:016x}",
        DIAG_TRDP_FROM_OUTPUT.load(Ordering::Relaxed)
    );
    let _ = writeln!(
        out,
        "portsc_before_db 0x{:08x}",
        DIAG_PORTSC_BEFORE_DB.load(Ordering::Relaxed)
    );
    let _ = writeln!(
        out,
        "first_queued_phys 0x{:016x}",
        DIAG_FIRST_QUEUED_PHYS.load(Ordering::Relaxed)
    );
    let _ = writeln!(
        out,
        "first_xfer_ptr 0x{:016x}",
        DIAG_FIRST_XFER_PTR.load(Ordering::Relaxed)
    );
    let slep = DIAG_FIRST_XFER_SLEP.load(Ordering::Relaxed);
    let _ = writeln!(
        out,
        "first_xfer_slep slot={} ep={}",
        (slep >> 8) & 0xFF,
        slep & 0xFF
    );
    let _ = writeln!(
        out,
        "first_xfer_status 0x{:08x}",
        DIAG_FIRST_XFER_STATUS.load(Ordering::Relaxed)
    );
    let _ = writeln!(
        out,
        "first_xfer_control 0x{:08x}",
        DIAG_FIRST_XFER_CONTROL.load(Ordering::Relaxed)
    );
    let _ = writeln!(
        out,
        "ep_state_after_reset 0x{:08x}",
        DIAG_EP_STATE_AFTER_RESET.load(Ordering::Relaxed)
    );
    let _ = writeln!(
        out,
        "reset_fail_count {}",
        ENDPOINT_RESET_FAIL_COUNT.load(Ordering::Relaxed)
    );
    let _ = writeln!(out, "xo_err_count {}", XO_ERR_COUNT.load(Ordering::Relaxed));
    let _ = writeln!(
        out,
        "xo_last_info slot={} ep={} cc={}",
        (XO_LAST_INFO.load(Ordering::Relaxed) >> 16) & 0xFF,
        (XO_LAST_INFO.load(Ordering::Relaxed) >> 8) & 0xFF,
        XO_LAST_INFO.load(Ordering::Relaxed) & 0xFF
    );
    let _ = writeln!(out, "skip_bw_dance {}", SKIP_BW_DANCE);
    let _ = writeln!(out, "=== XHCI_DIAG_END ===");

    out
}

/// Map an operation byte to its human-readable name.
fn op_name_str(op: u8) -> &'static str {
    match op {
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
    }
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

        let op_name = op_name_str(rec.op);

        crate::serial_println!(
            "T {:04} {:12} S={:02} E={:02} TS={:016X} LEN={:04X}",
            rec.seq,
            op_name,
            rec.slot,
            rec.dci,
            rec.timestamp,
            rec.data_len,
        );

        // Dump payload in 16-byte hex lines
        if rec.data_len > 0 && rec.data_offset != 0xFFFF_FFFF {
            let off = rec.data_offset as usize;
            let len = rec.data_len as usize;
            if off + len <= XHCI_TRACE_DATA_SIZE {
                let data = unsafe {
                    core::slice::from_raw_parts(
                        core::ptr::addr_of!(XHCI_TRACE_DATA).cast::<u8>().add(off),
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
// Milestone-based initialization instrumentation (matches Linux module)
//
// M1:  CONTROLLER_DISCOVERY  — BAR mapped, capabilities read
// M2:  CONTROLLER_RESET      — HCRST done, CNR clear, HCH=1
// M3:  DATA_STRUCTURES       — DCBAA, CMD ring, EVT ring, ERST programmed
// M4:  CONTROLLER_RUNNING    — RS=1, INTE=1, IMAN.IE=1
// M5:  PORT_DETECTION        — Connected ports identified, speed known
// M6:  SLOT_ENABLE           — EnableSlot CC=1, slot ID allocated
// M7:  DEVICE_ADDRESS        — Input ctx built, AddressDevice CC=1
// M8:  ENDPOINT_CONFIG       — ConfigureEndpoint CC=1, BW dance done
// M9:  HID_CLASS_SETUP       — SET_CONFIGURATION, SET_IDLE, descriptors
// M10: INTERRUPT_TRANSFER    — Normal TRBs queued, doorbells rung
// M11: EVENT_DELIVERY        — First transfer event (HID data received)
// =============================================================================

// Milestone constants — intentionally kept for re-enablement of tracing macros.
#[allow(dead_code)]
const M_DISCOVERY: u8 = 1;
#[allow(dead_code)]
const M_RESET: u8 = 2;
#[allow(dead_code)]
const M_DATA_STRUC: u8 = 3;
#[allow(dead_code)]
const M_RUNNING: u8 = 4;
#[allow(dead_code)]
const M_PORT_DET: u8 = 5;
#[allow(dead_code)]
const M_SLOT_EN: u8 = 6;
#[allow(dead_code)]
const M_ADDR_DEV: u8 = 7;
#[allow(dead_code)]
const M_EP_CONFIG: u8 = 8;
#[allow(dead_code)]
const M_HID_SETUP: u8 = 9;
#[allow(dead_code)]
const M_INTR_XFER: u8 = 10;
#[allow(dead_code)]
const M_EVT_DELIV: u8 = 11;
#[allow(dead_code)]
const M_TOTAL: u8 = 11;

#[allow(dead_code)]
const MILESTONE_NAMES: [&str; 12] = [
    "UNUSED",
    "CONTROLLER_DISCOVERY",
    "CONTROLLER_RESET",
    "DATA_STRUCTURES",
    "CONTROLLER_RUNNING",
    "PORT_DETECTION",
    "SLOT_ENABLE",
    "DEVICE_ADDRESS",
    "ENDPOINT_CONFIG",
    "HID_CLASS_SETUP",
    "INTERRUPT_TRANSFER",
    "EVENT_DELIVERY",
];

macro_rules! ms_begin {
    ($m:expr) => {};
}
macro_rules! ms_pass {
    ($m:expr) => {};
}
macro_rules! ms_fail {
    ($m:expr, $reason:expr) => {};
}
macro_rules! ms_kv {
    ($m:expr, $fmt:literal $(, $arg:expr)*) => {};
}

/// Hex-dump a DMA buffer under a milestone (silenced).
#[allow(dead_code)]
fn ms_dump(_m: u8, _label: &str, _phys: u64, _buf: *const u8, _len: usize) {}

/// Dump a TRB under a milestone (silenced).
fn ms_trb(_m: u8, _label: &str, _trb: &Trb) {}

/// Dump all key xHCI registers under a milestone (silenced).
fn ms_regs(_m: u8, _op_base: u64, _ir0_base: u64) {}

// =============================================================================
// MMIO Register Access
// =============================================================================

#[inline]
fn read32(addr: u64) -> u32 {
    unsafe {
        let val = core::ptr::read_volatile(addr as *const u32);
        // DSB to ensure the read completes before subsequent operations,
        // matching Linux's readl() which includes a post-read barrier.
        core::arch::asm!("dsb ld", options(nostack, preserves_flags));
        val
    }
}

#[inline]
fn write32(addr: u64, val: u32) {
    // Record to MMIO trace buffer if tracing is active
    if MMIO_TRACE_ACTIVE.load(Ordering::Relaxed) {
        let bar_base = MMIO_TRACE_BAR_BASE.load(Ordering::Relaxed);
        if bar_base != 0 && addr >= bar_base {
            let offset = (addr - bar_base) as u32;
            let idx = MMIO_TRACE_IDX.fetch_add(1, Ordering::Relaxed) as usize;
            if idx < MMIO_TRACE_MAX {
                unsafe {
                    MMIO_TRACE_BUF[idx] = (offset, val);
                }
            }
        }
    }
    unsafe {
        // DSB before write ensures all prior Normal memory writes (e.g. TRB data)
        // are globally visible before this Device-nGnRnE MMIO write.
        // DSB after write ensures this MMIO write reaches the device before
        // subsequent operations. Matches Linux's writel() barrier semantics.
        core::arch::asm!("dsb st", options(nostack, preserves_flags));
        core::ptr::write_volatile(addr as *mut u32, val);
        core::arch::asm!("dsb st", options(nostack, preserves_flags));
    }
}

#[inline]
#[allow(dead_code)] // Part of MMIO register access API
fn read64(addr: u64) -> u64 {
    // xHCI spec: 64-bit registers must be accessed as two 32-bit reads (lo then hi).
    // Parallels' MMIO trap handler may not correctly handle a single 64-bit load.
    // This matches Linux's xhci_read64() which uses two readl() calls.
    let lo = read32(addr) as u64;
    let hi = read32(addr + 4) as u64;
    (hi << 32) | lo
}

#[inline]
fn write64(addr: u64, val: u64) {
    // xHCI spec: 64-bit registers must be accessed as two 32-bit writes (lo then hi).
    // Parallels' MMIO trap handler may not correctly handle a single 64-bit store.
    // This matches Linux's xhci_write64() which uses two writel() calls.
    write32(addr, val as u32);
    write32(addr + 4, (val >> 32) as u32);
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

/// Busy-wait for `ms` milliseconds using the ARM64 generic timer.
/// Falls back to a spin loop if the timer frequency is unavailable.
fn delay_ms(ms: u32) {
    let freq: u64;
    unsafe { core::arch::asm!("mrs {}, cntfrq_el0", out(reg) freq) };
    if freq == 0 {
        // Fallback: ~1ms per iteration at ~1GHz
        for _ in 0..ms {
            for _ in 0..200_000u32 {
                core::hint::spin_loop();
            }
        }
        return;
    }
    let start: u64;
    unsafe { core::arch::asm!("mrs {}, cntpct_el0", out(reg) start) };
    let ticks = (freq / 1000) * ms as u64;
    loop {
        let now: u64;
        unsafe { core::arch::asm!("mrs {}, cntpct_el0", out(reg) now) };
        if now.wrapping_sub(start) >= ticks {
            break;
        }
        core::hint::spin_loop();
    }
}

// =============================================================================
// EHCI Controller Reset (Parallels workaround)
// =============================================================================

/// Reset the EHCI controller at PCI 00:02.0 [8086:265c].
///
/// The Parallels hypervisor's virtual USB subsystem requires the EHCI controller
/// to be reset in close proximity to the xHCI reset for the virtual xHC to
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
                control: (trb_type::LINK << 10) | if cycle { 1 } else { 0 } | (1 << 1),
            };
            core::ptr::write_volatile(&mut (*ring).0[CMD_RING_SIZE - 1] as *mut Trb, link);
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
    // write32() now includes pre- and post-DSB barriers,
    // ensuring TRB data is globally visible before the doorbell
    // and the doorbell reaches the device before continuing.
    write32(state.db_base + (slot as u64) * 4, target as u32);
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
                let erdp_phys =
                    virt_to_phys(&raw const EVENT_RING as u64) + (EVENT_RING_DEQUEUE as u64) * 16;
                let ir0 = state.rt_base + 0x20; // Interrupter 0
                write64(ir0 + 0x18, erdp_phys | (1 << 3));

                // Unconditionally clear IMAN.IP (W1C bit 0) and USBSTS.EINT (W1C bit 3)
                // after every event, matching Linux xhci-ring.c ack_event().
                // Writing 1 to W1C bits clears them; writing 1 when already clear is a no-op.
                write32(ir0, read32(ir0) | 1); // W1C IMAN.IP
                write32(
                    state.op_base + 0x04,
                    read32(state.op_base + 0x04) | (1 << 3),
                ); // W1C EINT

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
                    CONSUMED_XFER_DURING_ENUM.fetch_add(1, Ordering::Relaxed);
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

/// Issue a No-Op command and wait for completion. Used to warm up the command
/// ring after RS=1, matching Linux xhci_hcd which sends a NEC vendor NOOP as
/// its first command before Enable Slot.
fn send_noop(state: &XhciState) -> Result<(), &'static str> {
    let trb = Trb {
        param: 0,
        status: 0,
        control: trb_type::NOOP << 10,
    };
    enqueue_command(trb);
    ring_doorbell(state, 0, 0);

    let event = wait_for_command(state)?;
    let cc = event.completion_code();
    if cc != completion_code::SUCCESS {
        xhci_trace_note(0, "err:noop");
        return Err("XHCI NOOP failed");
    }
    xhci_trace_note(0, "noop_ok");
    Ok(())
}

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
    let slot_id = event.slot_id();
    if cc != completion_code::SUCCESS {
        xhci_trace_note(0, "err:enable_slot");
        return Err("XHCI EnableSlot failed");
    }

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
            1 => 64,  // Full Speed
            2 => 8,   // Low Speed
            3 => 64,  // High Speed
            4 => 512, // SuperSpeed
            _ => 64,  // Default to Full Speed
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
        core::ptr::write_volatile(ep0_ctx.add(0x0C) as *mut u32, (ep0_ring_phys >> 32) as u32);

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
        dma_cache_clean(&(*dcbaa).0[slot_id as usize] as *const u64 as *const u8, 8);

        // Build AddressDevice TRB (BSR=0)
        // BSR=0: the xHC assigns an address and sends SET_ADDRESS to the device,
        // transitioning the slot to Addressed state. Required before ConfigureEndpoint.
        // Note: the ftrace agent misidentified the cycle bit (b:C) as the BSR bit —
        // BSR=1 causes CC=19 (Context State Error) on ConfigureEndpoint.
        let input_ctx_phys = virt_to_phys(&raw const INPUT_CONTEXTS[slot_idx] as u64);

        ms_kv!(
            M_ADDR_DEV,
            "slot={} port={} speed={}",
            slot_id,
            port_id,
            port_speed
        );
        ms_dump(
            M_ADDR_DEV,
            "input_ctx",
            input_ctx_phys,
            input_base,
            ctx_size * 3,
        );

        let trb = Trb {
            param: input_ctx_phys,
            status: 0,
            // AddressDevice type, Slot ID in bits 31:24
            control: (trb_type::ADDRESS_DEVICE << 10) | ((slot_id as u32) << 24),
        };
        ms_trb(M_ADDR_DEV, "AddressDevice_TRB", &trb);
        enqueue_command(trb);
        ring_doorbell(state, 0, 0);

        let event = wait_for_command(state)?;
        let cc = event.completion_code();
        ms_kv!(M_ADDR_DEV, "CC={} slot={}", cc, slot_id);
        if cc != completion_code::SUCCESS {
            return Err("XHCI AddressDevice failed");
        }

        // Dump output context after successful AddressDevice
        dma_cache_invalidate((*dev_ctx).0.as_ptr(), 4096);
        ms_dump(
            M_ADDR_DEV,
            "output_ctx",
            dev_ctx_phys,
            (*dev_ctx).0.as_ptr(),
            ctx_size * 2,
        );

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

        core::ptr::write_volatile(&mut TRANSFER_RINGS[hid_idx][idx] as *mut Trb, t);
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
                control: (trb_type::LINK << 10) | if cycle { 1 } else { 0 } | (1 << 1), // TC (Toggle Cycle) bit — xHCI spec bit 1, not bit 5
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
        control: (trb_type::RESET_ENDPOINT << 10) | ((slot_id as u32) << 24) | (1u32 << 16),
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
        control: (trb_type::SET_TR_DEQUEUE_POINTER << 10) | ((slot_id as u32) << 24) | (1u32 << 16),
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
    let setup_data: u64 =
        unsafe { core::ptr::read_unaligned(setup as *const SetupPacket as *const u64) };

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

    // Status Stage TRB — xHCI spec Section 4.11.2.2:
    //   No Data Stage (TRT=0):   Status direction = IN  (bit 16 = 1)
    //   IN Data Stage  (TRT=3):  Status direction = OUT (bit 16 = 0)
    //   OUT Data Stage (TRT=2):  Status direction = IN  (bit 16 = 1)
    // i.e. Status direction is always opposite of data direction, defaulting to IN.
    let status_dir: u32 = if data_len > 0 && direction_in {
        0 // IN data stage → Status direction OUT
    } else {
        1 << 16 // No data stage or OUT data stage → Status direction IN
    };
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
            CONSUMED_XFER_DURING_ENUM.fetch_add(1, Ordering::Relaxed);
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
fn get_device_descriptor_short(state: &XhciState, slot_id: u8) -> Result<(), &'static str> {
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
                Err(_) => {}
            }
        }
    }
}

/// Send SET_ISOCH_DELAY to a USB 3.0 device.
///
/// This is a standard USB 3.0 request (bRequest=0x1F) that sets the isochronous
/// delay to 40us (wValue=0x0028). Linux xhci_hcd sends this after the short
/// (8-byte) device descriptor read, before reading the full descriptor.
#[allow(dead_code)]
fn set_isoch_delay(state: &XhciState, slot_id: u8) -> Result<(), &'static str> {
    let setup = SetupPacket {
        bm_request_type: 0x00, // Host-to-device, Standard, Device
        b_request: 0x1F,       // SET_ISOCH_DELAY
        w_value: 0x0028,       // 40us delay
        w_index: 0,
        w_length: 0,
    };
    // No data stage — just Setup + Status
    control_transfer(state, slot_id, &setup, 0, 0, false)?;
    Ok(())
}

/// Read the BOS (Binary Object Store) descriptor from a USB 3.0 device.
///
/// Linux reads this after the full device descriptor and before the config descriptor.
/// The BOS descriptor contains USB 3.0 device capabilities.
fn get_bos_descriptor(state: &XhciState, slot_id: u8) -> Result<(), &'static str> {
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

            control_transfer(
                state,
                slot_id,
                &setup_full,
                data_phys,
                total_len as u16,
                true,
            )?;
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

        if total_len > 256 {}

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
fn set_configuration(state: &XhciState, slot_id: u8, config_value: u8) -> Result<(), &'static str> {
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
                0,
                portsc,
                Ordering::AcqRel,
                Ordering::Relaxed,
            );
        }
        if portsc & change_mask != 0 {
            total_cleared += 1;
        }
    }
    total_cleared
}

/// Send SET_IDLE request to a HID interface (duration=0 = indefinite).
fn set_idle(state: &XhciState, slot_id: u8, interface: u8) -> Result<(), &'static str> {
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

/// Send SET_PROTOCOL to a HID boot-class interface.
///
/// Boot-class HID devices (subclass=1) require SET_PROTOCOL to activate
/// their interrupt endpoints for the specified protocol mode. The Linux kernel
/// module sends this for all boot devices; omitting it may cause the Parallels
/// vxHC to not deliver interrupt transfers.
fn set_protocol(
    state: &XhciState,
    slot_id: u8,
    interface: u8,
    protocol: u8, // 0 = Boot, 1 = Report
) -> Result<(), &'static str> {
    let setup = SetupPacket {
        bm_request_type: 0x21, // Host-to-device, class, interface
        b_request: hid_request::SET_PROTOCOL,
        w_value: protocol as u16,
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
fn set_report_leds(state: &XhciState, slot_id: u8, interface: u8) -> Result<(), &'static str> {
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
    let req_len = if exact_len > 0 && exact_len <= 256 {
        exact_len
    } else {
        128
    };

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
    _ss_mult: u8,
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

    // --- M8: ENDPOINT_CONFIG ---
    ms_begin!(M_EP_CONFIG);
    ms_kv!(M_EP_CONFIG, "slot={} ep_count={}", slot_id, ep_count);

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
            let val = core::ptr::read_volatile((*dev_ctx).0.as_ptr().add(dw_offset) as *const u32);
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
                core::ptr::write_volatile(ep_ctx.add(0x08) as *mut u32, (ring_phys as u32) | 1);
                core::ptr::write_volatile(ep_ctx.add(0x0C) as *mut u32, (ring_phys >> 32) as u32);

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
        ms_kv!(
            M_EP_CONFIG,
            "slot={} ep_count={} add_flags=0x{:x} max_dci={}",
            slot_id,
            ep_count,
            add_flags,
            max_dci
        );
        ms_dump(
            M_EP_CONFIG,
            "input_ctx",
            input_ctx_phys,
            input_base,
            (1 + max_dci as usize + 1) * ctx_size,
        );
        {
            xhci_trace_input_ctx(slot_id, input_base, ctx_size, max_dci as u8);
            let trb = Trb {
                param: input_ctx_phys,
                status: 0,
                control: (trb_type::CONFIGURE_ENDPOINT << 10) | ((slot_id as u32) << 24),
            };
            ms_trb(M_EP_CONFIG, "ConfigureEndpoint_TRB", &trb);
            xhci_trace_trb(XhciTraceOp::CommandSubmit, slot_id, 0, &trb);
            enqueue_command(trb);
            ring_doorbell(state, 0, 0);

            let event = wait_for_command(state)?;
            xhci_trace_trb(XhciTraceOp::CommandComplete, slot_id, 0, &event);
            let cc = event.completion_code();
            ms_kv!(
                M_EP_CONFIG,
                "ConfigureEndpoint slot={} CC={} add_flags=0x{:x}",
                slot_id,
                cc,
                add_flags
            );
            if cc != completion_code::SUCCESS {
                ms_fail!(M_EP_CONFIG, "ConfigureEndpoint command failed");
                return Err("XHCI ConfigureEndpoint failed");
            }

            // Dump output device context after successful ConfigureEndpoint
            dma_cache_invalidate((*dev_ctx).0.as_ptr(), 4096);
            let out_ctx_len = (1 + max_dci as usize) * ctx_size;
            let dev_ctx_phys = virt_to_phys(&raw const DEVICE_CONTEXTS[slot_idx] as u64);
            ms_dump(
                M_EP_CONFIG,
                "output_ctx_post_cfgep",
                dev_ctx_phys,
                (*dev_ctx).0.as_ptr(),
                out_ctx_len,
            );
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
            ms_kv!(
                M_EP_CONFIG,
                "BW_dance: slot={} ep_count={}",
                slot_id,
                ep_count
            );
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

                    ms_kv!(M_EP_CONFIG, "BW_stop: slot={} dci={}", slot_id, dci);
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
                    ms_kv!(
                        M_EP_CONFIG,
                        "StopEndpoint slot={} dci={} CC={}",
                        slot_id,
                        dci,
                        _stop_cc
                    );
                    // Read output context EP state after StopEP (should be 3=Stopped)
                    dma_cache_invalidate((*dev_ctx).0.as_ptr(), 4096);
                    let _stop_ep_state = core::ptr::read_volatile(
                        (*dev_ctx).0.as_ptr().add((dci as usize) * ctx_size) as *const u32,
                    ) & 0x7;
                    let _stop_deq_lo = core::ptr::read_volatile(
                        (*dev_ctx).0.as_ptr().add((dci as usize) * ctx_size + 8) as *const u32,
                    );

                    // Step 2: Rebuild the shared reconfig context from Output Context,
                    // then Re-ConfigureEndpoint using that buffer.
                    core::ptr::write_bytes(reconfig_base as *mut u8, 0, 4096);

                    // ICC: Drop=0, Add=ALL endpoints (same as initial ConfigEP).
                    // Linux's BW dance uses the FULL add_flags (SLOT + all DCIs)
                    // for every re-ConfigureEndpoint, not just the stopped one.
                    // The Linux module code: rctrl->add_flags = add_flags;
                    core::ptr::write_volatile(reconfig_base as *mut u32, 0u32);
                    core::ptr::write_volatile(reconfig_base.add(4) as *mut u32, add_flags);

                    // Slot context (ctx_size offset): copy DW0-DW2 from output, zero DW3.
                    let rc_slot = reconfig_base.add(ctx_size);
                    for dw_offset in (0..12usize).step_by(4) {
                        let val = core::ptr::read_volatile(
                            (*dev_ctx).0.as_ptr().add(dw_offset) as *const u32
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

                    // Endpoint context: copy ALL endpoint contexts from Output Context.
                    // Linux's BW dance copies every endpoint (not just the stopped one),
                    // clearing EP state bits (DW0 bits 2:0) for each.
                    for j in 0..ep_count {
                        if let Some(ref ep_j) = endpoints[j] {
                            let ep_dci = ep_j.dci as usize;
                            let rc_ep = reconfig_base.add((1 + ep_dci) * ctx_size);
                            let src_ep = (*dev_ctx).0.as_ptr().add(ep_dci * ctx_size);
                            for dw_offset in (0..32usize).step_by(4) {
                                let val =
                                    core::ptr::read_volatile(src_ep.add(dw_offset) as *const u32);
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
                        control: (trb_type::CONFIGURE_ENDPOINT << 10) | ((slot_id as u32) << 24),
                    };
                    xhci_trace_trb(XhciTraceOp::CommandSubmit, slot_id, 0, &reconfig_trb);
                    enqueue_command(reconfig_trb);
                    ring_doorbell(state, 0, 0);
                    let reconfig_event = wait_for_command(state)?;
                    xhci_trace_trb(XhciTraceOp::CommandComplete, slot_id, 0, &reconfig_event);
                    let _reconfig_cc = reconfig_event.completion_code();
                    ms_kv!(
                        M_EP_CONFIG,
                        "BW_dance slot={} dci={} CC={}",
                        slot_id,
                        dci,
                        _reconfig_cc
                    );

                    // Diagnostic: verify TR Dequeue pointer in output context after re-ConfigEP.
                    dma_cache_invalidate((*dev_ctx).0.as_ptr(), 4096);
                    let ep_out_base = (*dev_ctx).0.as_ptr().add((dci as usize) * ctx_size);
                    let _post_dw0 = core::ptr::read_volatile(ep_out_base as *const u32);
                    let _post_dw2 = core::ptr::read_volatile(ep_out_base.add(8) as *const u32);
                    let _post_dw3 = core::ptr::read_volatile(ep_out_base.add(12) as *const u32);
                    let _ring_phys_check =
                        virt_to_phys(&raw const TRANSFER_RINGS[HID_RING_BASE + ep.hid_idx] as u64);
                    xhci_trace_output_ctx(slot_id, (*dev_ctx).0.as_ptr(), ctx_size, max_dci as u8);

                    // Dump output device context after BW dance StopEndpoint+Reconfigure for this DCI
                    let dev_ctx_phys_bw = virt_to_phys(&raw const DEVICE_CONTEXTS[slot_idx] as u64);
                    let out_ctx_len_bw = (1 + max_dci as usize) * ctx_size;
                    ms_dump(
                        M_EP_CONFIG,
                        "output_ctx_post_bw_dance",
                        dev_ctx_phys_bw,
                        (*dev_ctx).0.as_ptr(),
                        out_ctx_len_bw,
                    );
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
                let _ring_phys_chk =
                    virt_to_phys(&raw const TRANSFER_RINGS[HID_RING_BASE + ep.hid_idx] as u64);
                if ep_state == 0 {}
            }
        }
    }

    ms_pass!(M_EP_CONFIG);
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
        is_nkro: bool,  // Non-boot HID interface (subclass=0, Report ID protocol)
        is_boot: bool,  // Boot-class interface (subclass=1)
        hid_idx: usize, // Transfer ring index (0=kbd boot, 1=mouse, 2=kbd NKRO, 3=mouse2)
        dci: u8,
        hid_report_len: u16, // wDescriptorLength from HID descriptor (0 = unknown)
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
            let iface =
                unsafe { &*(config_buf.as_ptr().add(offset) as *const InterfaceDescriptor) };

            crate::serial_println!(
                "[xhci] slot{} iface{}: class={:#04x} sub={:#04x} proto={:#04x} numEP={}",
                slot_id,
                iface.b_interface_number,
                iface.b_interface_class,
                iface.b_interface_sub_class,
                iface.b_interface_protocol,
                iface.b_num_endpoints
            );

            if iface.b_interface_class == class_code::HID {
                // Parse HID descriptor (type 0x21) for wDescriptorLength.
                // The HID descriptor immediately follows the interface descriptor.
                let mut hid_report_len: u16 = 0;
                {
                    let mut hid_off = offset + desc_len;
                    while hid_off + 2 <= config_len {
                        let hd_len = config_buf[hid_off] as usize;
                        let hd_type = config_buf[hid_off + 1];
                        if hd_len == 0 || hid_off + hd_len > config_len {
                            break;
                        }
                        if hd_type == descriptor_type::INTERFACE
                            || hd_type == descriptor_type::ENDPOINT
                        {
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
                            let mut _ss_mult: u8 = 0;
                            let ss_offset = ep_offset + ep_len;
                            if ss_offset + 2 <= config_len {
                                let ss_len = config_buf[ss_offset] as usize;
                                let ss_type = config_buf[ss_offset + 1];
                                if ss_type == 0x30
                                    && ss_len >= 6
                                    && ss_offset + ss_len <= config_len
                                {
                                    ss_max_burst = config_buf[ss_offset + 2];
                                    // bmAttributes bits[1:0] = Mult (max burst multiplier)
                                    _ss_mult = config_buf[ss_offset + 3] & 0x3;
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
                                } else if found_mouse {
                                    // Second generic HID interface on same device
                                    // (VMware mouse has two proto=0 interfaces)
                                    if !found_mouse2 {
                                        found_mouse2 = true;
                                        (3usize, false, false)
                                    } else {
                                        break; // Only support two mouse interfaces
                                    }
                                } else {
                                    // First generic HID interface — treat as mouse
                                    found_mouse = true;
                                    (1usize, false, false)
                                };

                            crate::serial_println!(
                                "[xhci] HID iface: proto={} subclass={} -> {} (hid_idx={})",
                                iface.b_interface_protocol,
                                iface.b_interface_sub_class,
                                if is_keyboard {
                                    "keyboard"
                                } else if is_nkro {
                                    "NKRO"
                                } else {
                                    "mouse"
                                },
                                hid_idx
                            );

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
                                    _ss_mult,
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
                                    is_boot: iface.b_interface_sub_class == 1,
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
    // Phase 1b: ConfigureEndpoint BEFORE SET_CONFIGURATION (matching Linux module)
    //
    // Both spec-correct (SET_CONFIG first) and module order (ConfigureEndpoint first)
    // produce CC=12 on Parallels. Keeping module order for consistency with Linux module.
    // =========================================================================
    if ep_count > 0 {
        configure_endpoints_batch(state, slot_id, &pending_eps, ep_count)?;
    }

    // --- M9: HID_CLASS_SETUP ---
    ms_begin!(M_HID_SETUP);
    ms_kv!(
        M_HID_SETUP,
        "SET_CONFIGURATION slot={} config={}",
        slot_id,
        config_value
    );

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

            // Dump output device context after SET_CONFIGURATION
            let dev_ctx_phys = virt_to_phys(&raw const DEVICE_CONTEXTS[slot_idx] as u64);
            let out_ctx_len = (1 + max_dci as usize) * ctx_size;
            ms_dump(
                M_HID_SETUP,
                "output_ctx_post_set_config",
                dev_ctx_phys,
                (*dev_ctx).0.as_ptr(),
                out_ctx_len,
            );
        }
    }

    // =========================================================================
    // Phase 2+3 (merged): Per-interface atomic setup — matches Linux ordering.
    //
    // Linux does all setup for interface N before moving to interface N+1.
    // Previously Phase 2 (SET_INTERFACE loop) and Phase 3 (per-interface class
    // setup) were separate loops. Merging them ensures the vxHC sees the same
    // per-interface atomic ordering as the Linux kernel module.
    //
    // Per-interface sequence:
    //   1. SET_INTERFACE(alt=0)
    //   2. SET_PROTOCOL(boot=0)     — only if is_boot (subclass=1)
    //   3. SET_IDLE
    //   4. GET HID Report Descriptor
    //   5. GET_REPORT/SET_REPORT (Feature) — mouse only
    //   6. SET_REPORT (LED)         — keyboard only
    // =========================================================================
    ms_kv!(
        M_HID_SETUP,
        "hid_interfaces={} slot={}",
        iface_count,
        slot_id
    );

    for i in 0..iface_count {
        if let Some(ref info) = ifaces[i] {
            // Step 1: SET_INTERFACE(alt=0) — matches working Linux module.
            ms_kv!(
                M_HID_SETUP,
                "SET_INTERFACE slot={} iface={}",
                slot_id,
                info.interface_number
            );
            match set_interface(state, slot_id, info.interface_number, 0) {
                Ok(()) => {}
                Err(_) => {} // Non-fatal: some devices may STALL
            }

            if !MINIMAL_INIT {
                // Step 2: SET_PROTOCOL(boot=0) for boot-class interfaces (subclass=1).
                if info.is_boot {
                    ms_kv!(
                        M_HID_SETUP,
                        "SET_PROTOCOL slot={} iface={}",
                        slot_id,
                        info.interface_number
                    );
                    match set_protocol(state, slot_id, info.interface_number, 0) {
                        Ok(()) => {}
                        Err(_) => {} // Non-fatal: some devices may STALL
                    }
                }
            }

            if info.is_nkro {
                // NKRO keyboard: SET_IDLE + GET_HID_REPORT_DESC then ep2in TRB.
                if !MINIMAL_INIT {
                    ms_kv!(
                        M_HID_SETUP,
                        "SET_IDLE slot={} iface={}",
                        slot_id,
                        info.interface_number
                    );
                    match set_idle(state, slot_id, info.interface_number) {
                        Ok(()) => {}
                        Err(_) => {}
                    }
                    fetch_hid_report_descriptor(
                        state,
                        slot_id,
                        info.interface_number,
                        info.hid_report_len,
                    );
                }
                state.kbd_slot = slot_id;
                state.kbd_nkro_endpoint = info.dci;

                // TRB queueing deferred to post-enumeration in init() to prevent
                // Transfer Events from being silently consumed by subsequent
                // control_transfer() and wait_for_command() calls.
            } else if info.is_keyboard {
                // Boot keyboard: SET_IDLE + GET_HID_REPORT_DESC + SET_REPORT(LED=0).
                if !MINIMAL_INIT {
                    ms_kv!(
                        M_HID_SETUP,
                        "SET_IDLE slot={} iface={}",
                        slot_id,
                        info.interface_number
                    );
                    match set_idle(state, slot_id, info.interface_number) {
                        Ok(()) => {}
                        Err(_) => {}
                    }
                    fetch_hid_report_descriptor(
                        state,
                        slot_id,
                        info.interface_number,
                        info.hid_report_len,
                    );
                    match set_report_leds(state, slot_id, info.interface_number) {
                        Ok(()) => {}
                        Err(_) => {}
                    }
                }
                state.kbd_slot = slot_id;
                state.kbd_endpoint = info.dci;

                // TRB queueing deferred to post-enumeration in start_hid_polling()
                // to prevent Transfer Events from being silently consumed by
                // subsequent control_transfer() and wait_for_command() calls.
            } else {
                // Mouse: SET_IDLE + GET_HID_REPORT_DESC + GET_REPORT(Feature) + SET_REPORT(Feature).
                // Linux sends SET_IDLE and fetches the HID report descriptor for ALL
                // HID interfaces, including mouse — not just keyboards.
                if !MINIMAL_INIT {
                    ms_kv!(
                        M_HID_SETUP,
                        "SET_IDLE slot={} iface={} (mouse)",
                        slot_id,
                        info.interface_number
                    );
                    match set_idle(state, slot_id, info.interface_number) {
                        Ok(()) => {}
                        Err(_) => {}
                    }
                    fetch_hid_report_descriptor(
                        state,
                        slot_id,
                        info.interface_number,
                        info.hid_report_len,
                    );
                    let feature_id: u8 = if info.hid_idx == 3 { 0x12 } else { 0x11 };
                    match get_set_feature_report(state, slot_id, info.interface_number, feature_id)
                    {
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

                // TRB queueing deferred to post-enumeration in start_hid_polling()
                // to prevent Transfer Events from being silently consumed by
                // subsequent control_transfer() and wait_for_command() calls.
            }
        }
    }

    // Dump output device context after all per-interface HID setup
    if ep_count > 0 && max_dci != 0 {
        let slot_idx = (slot_id - 1) as usize;
        let ctx_size = state.context_size;
        unsafe {
            let dev_ctx = &raw const DEVICE_CONTEXTS[slot_idx];
            dma_cache_invalidate((*dev_ctx).0.as_ptr(), 4096);
            let dev_ctx_phys = virt_to_phys(&raw const DEVICE_CONTEXTS[slot_idx] as u64);
            let out_ctx_len = (1 + max_dci as usize) * ctx_size;
            ms_dump(
                M_HID_SETUP,
                "output_ctx_post_hid_setup",
                dev_ctx_phys,
                (*dev_ctx).0.as_ptr(),
                out_ctx_len,
            );
        }
    }

    ms_pass!(M_HID_SETUP);
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

    // Linux uses the endpoint's maxPacketSize (64) as the TRB transfer length,
    // not the HID report size (8 or 9). The xHC may write up to maxPacketSize
    // bytes and ISP (Interrupt on Short Packet) handles the common case where
    // the device sends fewer bytes than the buffer size.
    let (buf_phys, buf_len) = match hid_idx {
        0 => (virt_to_phys((&raw const KBD_REPORT_BUF) as u64), 64usize),
        2 => (virt_to_phys((&raw const NKRO_REPORT_BUF) as u64), 64usize),
        3 => (virt_to_phys((&raw const MOUSE2_REPORT_BUF) as u64), 64usize),
        _ => (virt_to_phys((&raw const MOUSE_REPORT_BUF) as u64), 64usize),
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
        let trb_phys =
            virt_to_phys(&raw const TRANSFER_RINGS[ring_idx] as u64) + (enq_idx as u64) * 16;
        let _ = DIAG_FIRST_QUEUED_PHYS.compare_exchange(
            0,
            trb_phys,
            Ordering::AcqRel,
            Ordering::Relaxed,
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
            let slot_dw3 = core::ptr::read_volatile((*dev_ctx).0.as_ptr().add(12) as *const u32);
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
            let slot_dw1 = core::ptr::read_volatile((*dev_ctx).0.as_ptr().add(4) as *const u32);
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
            let erdp_phys =
                virt_to_phys(&raw const EVENT_RING as u64) + (EVENT_RING_DEQUEUE as u64) * 16;
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
                if dci == 0 {
                    continue;
                }
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
                let erdp_phys =
                    virt_to_phys(&raw const EVENT_RING as u64) + (EVENT_RING_DEQUEUE as u64) * 16;
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
                    if slot == state.mouse_slot && endpoint == 1 {
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
                                super::hid::process_mouse_report(report, 0);
                            }
                        }
                    } else if cc != completion_code::SUCCESS && cc != completion_code::SHORT_PACKET
                    {
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
        control: (trb_type::RESET_ENDPOINT << 10) | ((slot_id as u32) << 24) | ((dci as u32) << 16),
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
        core::ptr::write_volatile(&mut (*ring)[TRANSFER_RING_SIZE - 1] as *mut Trb, link);

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
/// Called after all device enumeration is complete. Drains stale events, then
/// queues interrupt TRBs for all configured HID endpoints and rings doorbells.
///
/// TRBs are queued HERE instead of inline during Phase 3 of configure_hid()
/// because control_transfer() and wait_for_event_inner() silently consume
/// Transfer Events for non-EP0 endpoints. Queueing inline meant the Transfer
/// Events from earlier interfaces' TRBs were eaten by subsequent control
/// transfers for later interfaces, leaving no pending TRBs after init.
fn start_hid_polling(state: &XhciState) {
    // --- M10: INTERRUPT_TRANSFER ---
    ms_begin!(M_INTR_XFER);

    crate::serial_println!(
        "[xhci] start_hid_polling: kbd=slot{}/dci{} nkro=dci{} mouse=slot{}/dci{} mouse2=dci{}",
        state.kbd_slot,
        state.kbd_endpoint,
        state.kbd_nkro_endpoint,
        state.mouse_slot,
        state.mouse_endpoint,
        state.mouse_nkro_endpoint
    );

    // Drain any stale Transfer Events that may have been generated during
    // port scanning or previous enumeration attempts.
    drain_stale_events(state);

    // Set flags so the poll path and heartbeat know TRBs have been queued.
    KBD_TRB_FIRST_QUEUED.store(true, Ordering::Release);
    HID_TRBS_QUEUED.store(true, Ordering::Release);
    let _ = DIAG_FIRST_QUEUE_SOURCE.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Relaxed);

    // Queue interrupt TRBs for all configured HID endpoints.
    // Keyboard boot
    if state.kbd_slot != 0 && state.kbd_endpoint != 0 {
        ms_kv!(
            M_INTR_XFER,
            "queued+doorbell: slot={} dci={} hid_idx=0",
            state.kbd_slot,
            state.kbd_endpoint
        );
        let _ = queue_hid_transfer(state, 0, state.kbd_slot, state.kbd_endpoint);
    }
    // Keyboard NKRO
    if state.kbd_slot != 0 && state.kbd_nkro_endpoint != 0 {
        ms_kv!(
            M_INTR_XFER,
            "queued+doorbell: slot={} dci={} hid_idx=2",
            state.kbd_slot,
            state.kbd_nkro_endpoint
        );
        let _ = queue_hid_transfer(state, 2, state.kbd_slot, state.kbd_nkro_endpoint);
    }
    // Mouse boot
    if state.mouse_slot != 0 && state.mouse_endpoint != 0 {
        ms_kv!(
            M_INTR_XFER,
            "queued+doorbell: slot={} dci={} hid_idx=1",
            state.mouse_slot,
            state.mouse_endpoint
        );
        let _ = queue_hid_transfer(state, 1, state.mouse_slot, state.mouse_endpoint);
    }
    // Mouse2
    if state.mouse_slot != 0 && state.mouse_nkro_endpoint != 0 {
        ms_kv!(
            M_INTR_XFER,
            "queued+doorbell: slot={} dci={} hid_idx=3",
            state.mouse_slot,
            state.mouse_nkro_endpoint
        );
        let _ = queue_hid_transfer(state, 3, state.mouse_slot, state.mouse_nkro_endpoint);
    }

    // Stop MMIO write tracing (dump disabled — CC=12 investigation complete)
    MMIO_TRACE_ACTIVE.store(false, Ordering::Release);

    ms_pass!(M_INTR_XFER);
}

// =============================================================================
// INHERIT_UEFI: Endpoint Discovery and Redirection
// =============================================================================

/// Inherit UEFI's configured endpoints instead of re-enumerating from scratch.
///
/// Walks our DEVICE_CONTEXTS (which have UEFI's data copied in), discovers
/// interrupt IN endpoints, assigns them to xhci_state fields, and redirects
/// each endpoint's transfer ring to our memory via StopEndpoint + SetTRDequeuePointer.
fn inherit_uefi_endpoints(state: &mut XhciState) -> Result<(), &'static str> {
    let context_size = state.context_size;
    let mut discovered_slots: [(u8, u8, u8); 8] = [(0, 0, 0); 8]; // (slot_id, dci_boot, dci_nkro)
    let mut num_discovered = 0usize;

    // Walk all slots and find those with interrupt IN endpoints
    for slot_id in 1..=state.max_slots {
        let slot_idx = (slot_id as usize) - 1;
        let dcbaa_entry = unsafe {
            let dcbaa = &raw const DCBAA;
            (*dcbaa).0[slot_id as usize]
        };
        if dcbaa_entry == 0 {
            continue;
        }

        unsafe {
            let dev_ctx = &raw const DEVICE_CONTEXTS[slot_idx];
            dma_cache_invalidate((*dev_ctx).0.as_ptr(), 4096);

            // Slot Context DW0: bits 31:27 = Context Entries (num DCIs with valid data)
            let slot_dw0 = core::ptr::read_volatile((*dev_ctx).0.as_ptr() as *const u32);
            let num_entries = (slot_dw0 >> 27) & 0x1F;

            // Slot Context DW3: bits 31:27 = Slot State
            let slot_dw3 = core::ptr::read_volatile((*dev_ctx).0.as_ptr().add(12) as *const u32);
            let slot_state = (slot_dw3 >> 27) & 0x1F;

            // Slot states: 0=Disabled/Enabled, 1=Default, 2=Addressed, 3=Configured
            if slot_state < 2 {
                continue;
            }

            let mut boot_dci: u8 = 0;
            let mut nkro_dci: u8 = 0;

            for dci in 1..=num_entries {
                let ep_base = (*dev_ctx).0.as_ptr().add(dci as usize * context_size);
                let ep_dw0 = core::ptr::read_volatile(ep_base as *const u32);
                let ep_dw1 = core::ptr::read_volatile(ep_base.add(4) as *const u32);
                let ep_state = ep_dw0 & 0x7;
                let ep_type = (ep_dw1 >> 3) & 0x7;
                // EP Type 7 = Interrupt IN
                if ep_type == 7 && ep_state != 0 {
                    if boot_dci == 0 {
                        boot_dci = dci as u8;
                    } else if nkro_dci == 0 {
                        nkro_dci = dci as u8;
                    }
                }
            }

            if boot_dci != 0 && num_discovered < 8 {
                discovered_slots[num_discovered] = (slot_id, boot_dci, nkro_dci);
                num_discovered += 1;
            }
        }
    }

    if num_discovered == 0 {
        return Err("INHERIT_UEFI: no interrupt IN endpoints found");
    }

    // Assign discovered slots to mouse/keyboard.
    // Parallels typically: slot 1 = mouse, slot 2 = keyboard.
    // We assign first discovered to mouse, second to keyboard.
    if num_discovered >= 1 {
        let (slot_id, boot_dci, nkro_dci) = discovered_slots[0];
        state.mouse_slot = slot_id;
        state.mouse_endpoint = boot_dci;
        state.mouse_nkro_endpoint = nkro_dci;
    }
    if num_discovered >= 2 {
        let (slot_id, boot_dci, nkro_dci) = discovered_slots[1];
        state.kbd_slot = slot_id;
        state.kbd_endpoint = boot_dci;
        state.kbd_nkro_endpoint = nkro_dci;
    }

    // For each discovered endpoint, redirect its transfer ring to ours.
    // Sequence: StopEndpoint (if Running) → SetTRDequeuePointer → ready for Normal TRBs.
    let endpoints_to_redirect: [(u8, u8, usize); 4] = [
        // (slot_id, dci, hid_idx)
        (state.mouse_slot, state.mouse_endpoint, 1), // hid_idx 1 = mouse boot
        (state.mouse_slot, state.mouse_nkro_endpoint, 3), // hid_idx 3 = mouse2
        (state.kbd_slot, state.kbd_endpoint, 0),     // hid_idx 0 = kbd boot
        (state.kbd_slot, state.kbd_nkro_endpoint, 2), // hid_idx 2 = kbd NKRO
    ];

    for &(slot_id, dci, hid_idx) in &endpoints_to_redirect {
        if slot_id == 0 || dci == 0 {
            continue;
        }

        // Read current endpoint state from our copy of the output context
        let slot_idx = (slot_id as usize) - 1;
        let ep_state = unsafe {
            let dev_ctx = &raw const DEVICE_CONTEXTS[slot_idx];
            dma_cache_invalidate((*dev_ctx).0.as_ptr(), 4096);
            let ep_base = (*dev_ctx).0.as_ptr().add(dci as usize * context_size);
            core::ptr::read_volatile(ep_base as *const u32) & 0x7
        };

        // StopEndpoint if Running (state=1) or Halted (state=2)
        // State 0=Disabled, 1=Running, 2=Halted, 3=Stopped
        if ep_state == 1 || ep_state == 2 {
            if ep_state == 2 {
                // Halted → ResetEndpoint first to get to Stopped
                let reset_trb = Trb {
                    param: 0,
                    status: 0,
                    control: (trb_type::RESET_ENDPOINT << 10)
                        | ((slot_id as u32) << 24)
                        | ((dci as u32) << 16),
                };
                enqueue_command(reset_trb);
                ring_doorbell(state, 0, 0);
                let _ = wait_for_command(state);
            } else {
                // Running → StopEndpoint to get to Stopped
                let stop_trb = Trb {
                    param: 0,
                    status: 0,
                    control: (trb_type::STOP_ENDPOINT << 10)
                        | ((slot_id as u32) << 24)
                        | ((dci as u32) << 16),
                };
                enqueue_command(stop_trb);
                ring_doorbell(state, 0, 0);
                let _ = wait_for_command(state);
            }
        }

        // SetTRDequeuePointer: redirect to our transfer ring
        let ring_idx = HID_RING_BASE + hid_idx;
        // Zero the transfer ring and reset enqueue index
        unsafe {
            core::ptr::write_bytes(TRANSFER_RINGS[ring_idx].as_mut_ptr(), 0, TRANSFER_RING_SIZE);
            TRANSFER_ENQUEUE[ring_idx] = 0;
            TRANSFER_CYCLE[ring_idx] = true;
            dma_cache_clean(
                TRANSFER_RINGS[ring_idx].as_ptr() as *const u8,
                TRANSFER_RING_SIZE * core::mem::size_of::<Trb>(),
            );
        }

        let ring_phys = virt_to_phys(unsafe { (&raw const TRANSFER_RINGS[ring_idx]) as u64 });
        // DCS bit 0 = 1 (matches initial cycle state)
        let set_deq_trb = Trb {
            param: ring_phys | 1,
            status: 0,
            control: (trb_type::SET_TR_DEQUEUE_POINTER << 10)
                | ((slot_id as u32) << 24)
                | ((dci as u32) << 16),
        };
        enqueue_command(set_deq_trb);
        ring_doorbell(state, 0, 0);
        let _ = wait_for_command(state);
    }
    Ok(())
}

// =============================================================================
// Port Scanning and Device Enumeration
// =============================================================================

/// Scan all root hub ports for connected devices, enumerate, and configure HID devices.
fn scan_ports(state: &mut XhciState) -> Result<(), &'static str> {
    // CRITICAL: Clear ALL PORTSC change bits BEFORE any Enable Slot command.
    //
    // MMIO comparison shows Linux's hub driver clears CSC (Connection Status
    // Change, bit 17) before sending Enable Slot. Breenix was sending Enable
    // Slot with CSC still set. The Parallels virtual xHC may use CSC state to
    // track whether the driver properly completed the port status change
    // handshake before claiming the device.
    let _early_cleared = clear_all_port_changes(state);

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

        let port_id = port as u8 + 1; // 1-based port ID
        crate::serial_println!(
            "[xhci] port {} connected: PORTSC=0x{:08x} PED={} speed={}",
            port_id,
            portsc,
            (portsc >> 1) & 1,
            (portsc >> 10) & 0xF
        );

        // --- M5: PORT_DETECTION ---
        ms_begin!(M_PORT_DET);
        ms_kv!(
            M_PORT_DET,
            "port={} PORTSC=0x{:08x} CCS={} PED={}",
            port_id,
            portsc,
            portsc & 1,
            (portsc >> 1) & 1
        );
        ms_kv!(
            M_PORT_DET,
            "port={} speed_raw={} PRC={}",
            port_id,
            (portsc >> 10) & 0xF,
            (portsc >> 21) & 1
        );

        // Port reset: only when PED=0 (matching Linux kernel module behavior).
        //
        // After HCRST, SuperSpeed ports auto-enable (PED=1). The Linux kernel
        // module skips port reset when PED=1, and the Parallels hypervisor
        // handles this correctly — AddressDevice internally triggers a device
        // reset in the hypervisor's USB device model.
        //
        // Explicit port reset when PED=1 was previously added but may confuse
        // the hypervisor's internal state machine.
        if portsc & (1 << 1) == 0 {
            ms_kv!(M_PORT_DET, "port={} resetting (PED=0)", port_id);

            // Write PR (Port Reset, bit 4).
            //
            // PORTSC bit types (xHCI spec Table 5-27):
            //   PED (bit 1): W1CS — writing 1 DISABLES the port! Must write 0.
            //   PR  (bit 4): RW1S — writing 1 starts port reset
            //   Bits 17-23:  RW1C — change status bits, writing 1 clears them
            //
            // All W1C/W1CS bits must be written as 0 to avoid side effects.
            // This matches Linux's xhci_port_state_to_neutral().
            let preserve_mask: u32 = !((1 << 1) |  // PED - W1CS, writing 1 disables port!
                (1 << 17) | (1 << 18) | (1 << 19) | (1 << 20) | (1 << 21) | (1 << 22) | (1 << 23));
            write32(portsc_addr, (portsc & preserve_mask) | (1 << 4));

            // Wait for PRC (Port Reset Change, bit 21)
            if wait_for(|| read32(portsc_addr) & (1 << 21) != 0, 500_000).is_err() {
                crate::serial_println!("[xhci] port {} reset timeout", port_id);
                ms_kv!(M_PORT_DET, "port={} reset timeout", port_id);
                continue;
            }

            // Clear PRC (W1C) — write 1 to bit 21 to clear it.
            // Use the same preserve_mask to avoid disabling PED.
            let portsc_after = read32(portsc_addr);
            write32(portsc_addr, (portsc_after & preserve_mask) | (1 << 21));

            let portsc_final = read32(portsc_addr);
            crate::serial_println!(
                "[xhci] port {} post-reset: PORTSC=0x{:08x} PED={} speed={}",
                port_id,
                portsc_final,
                (portsc_final >> 1) & 1,
                (portsc_final >> 10) & 0xF
            );
            ms_kv!(
                M_PORT_DET,
                "port={} post_reset PORTSC=0x{:08x} PED={}",
                port_id,
                portsc_final,
                (portsc_final >> 1) & 1
            );
            if portsc_final & (1 << 1) == 0 {
                continue;
            }
        } else {
            ms_kv!(
                M_PORT_DET,
                "port={} PED=1, skipping port reset (matching Linux module)",
                port_id
            );
        }

        ms_pass!(M_PORT_DET);

        // --- M6: SLOT_ENABLE ---
        ms_begin!(M_SLOT_EN);
        // Enable Slot for this device
        let slot_id = match enable_slot(state) {
            Ok(id) => {
                crate::serial_println!("[xhci] port {} EnableSlot -> slot {}", port_id, id);
                id
            }
            Err(e) => {
                crate::serial_println!("[xhci] port {} EnableSlot failed: {}", port_id, e);
                ms_fail!(M_SLOT_EN, "EnableSlot returned error");
                continue;
            }
        };
        if slot_id == 0 {
            crate::serial_println!("[xhci] port {} EnableSlot returned 0", port_id);
            continue;
        }
        ms_pass!(M_SLOT_EN);

        slots_used += 1;

        // --- M7: DEVICE_ADDRESS ---
        ms_kv!(
            M_ADDR_DEV,
            "port={} speed={} slot={}",
            port_id,
            (read32(portsc_addr) >> 10) & 0xF,
            slot_id
        );
        ms_begin!(M_ADDR_DEV);
        // Address Device (port numbers are 1-based)
        if let Err(e) = address_device(state, slot_id, port_id) {
            crate::serial_println!("[xhci] port {} AddressDevice failed: {}", port_id, e);
            continue;
        }
        crate::serial_println!(
            "[xhci] port {} AddressDevice OK (slot {})",
            port_id,
            slot_id
        );
        ms_pass!(M_ADDR_DEV);

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

        // Step 2: SET_ISOCH_DELAY — SKIPPED.
        // Parallels' virtual xHCI STALLs this request, causing an EP0 reset.
        // The working Linux module also does NOT send SET_ISOCH_DELAY.

        // Step 3: Full device descriptor (18 bytes)
        let mut desc_buf = [0u8; 18];
        if let Err(_) = get_device_descriptor(state, slot_id, &mut desc_buf) {
            continue;
        }
        {
            let dd = unsafe { &*(desc_buf.as_ptr() as *const DeviceDescriptor) };
            let vid = dd.id_vendor;
            let pid = dd.id_product;
            crate::serial_println!(
                "[xhci] slot{}: vid={:#06x} pid={:#06x} class={:#04x}/{:#04x}/{:#04x}",
                slot_id,
                vid,
                pid,
                dd.b_device_class,
                dd.b_device_sub_class,
                dd.b_device_protocol
            );
        }

        // Step 4: BOS descriptor
        if let Err(_) = get_bos_descriptor(state, slot_id) {}

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
        if let Err(_) = configure_hid(state, slot_id, &config_buf, config_len) {}
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
        Some(offset) => offset,
        None => {
            xhci_trace_note(0, "no_msi_cap");
            return 0;
        }
    };

    // Step 2: Probe for GICv2m
    // On Parallels ARM64, GICv2m is at 0x02250000 (discovered from MADT).
    const PARALLELS_GICV2M_BASE: u64 = 0x0225_0000;
    let gicv2m_base = crate::platform_config::gicv2m_base_phys();
    let (base, _spi_base, spi_count) = if gicv2m_base != 0 {
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

    // Step 3: Allocate next available SPI for XHCI
    let spi = crate::platform_config::allocate_msi_spi();
    if spi == 0 {
        xhci_trace_note(0, "err:alloc_spi");
        return 0;
    }
    let intid = spi; // GIC INTID = SPI number for GICv2m

    // Step 4: Program PCI MSI registers
    // MSI address = GICv2m doorbell (MSI_SETSPI_NS at offset 0x40)
    let msi_address = (base + 0x40) as u32;
    let msi_data = spi as u16;
    pci_dev.configure_msi(msi_cap, msi_address, msi_data);

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

    // DMA structures are now in .bss (WB-cacheable, zeroed by boot.S).
    // No dc ivac needed — there are no stale NC cache lines to worry about.

    // Enable MemSpace + BusMaster immediately to keep the xHCI BAR mapped.
    // UEFI firmware disables MemSpace during ExitBootServices cleanup, which
    // triggers phymemrange_disable. We re-enable it right away to minimize
    // the BAR-unmapped gap (matching linux-probe's ~95ms gap).
    {
        let cmd_before = crate::drivers::pci::pci_read_config_word(
            pci_dev.bus,
            pci_dev.device,
            pci_dev.function,
            0x04,
        );
        let cmd_new = cmd_before | 0x0006; // MemSpace + BusMaster
        if cmd_new != cmd_before {
            crate::drivers::pci::pci_write_config_word(
                pci_dev.bus,
                pci_dev.device,
                pci_dev.function,
                0x04,
                cmd_new,
            );
        }
    }

    // Wait for Parallels' ASYNC OnExitBootServices handler to complete.
    //
    // CRITICAL FINDING: The Parallels hypervisor's OnExitBootServices() handler
    // runs ASYNCHRONOUSLY ~440ms after the EBS call, and triggers a second XHC
    // controller reset at ~685ms. If our HCRST happens BEFORE this async reset,
    // the async reset destroys all endpoints we just created → CC=12.
    //
    // By waiting 1000ms, we ensure the async handler's XHC reset has already
    // completed before we do our HCRST. Our HCRST becomes the LAST reset,
    // and endpoint state created after it will persist.
    //
    // VMware does not exhibit this behavior, so skip the delay there.
    if !crate::platform_config::is_vmware() {
        const EBS_SETTLE_MS: u32 = 1000;
        delay_ms(EBS_SETTLE_MS);
    }

    // Step 1: pci_enable_device() equivalent
    // 1a. Transition to D0 power state (pci_set_power_state → pci_raw_set_power_state)
    let _ = pci_dev.set_power_state_d0();
    // 1b. Enable Memory Space only (pci_enable_resources sets bit 1)
    //     Linux does NOT set Bus Master or INTx Disable here.
    pci_dev.enable_memory_space();
    // 1c. Clear Status register error bits (w1c)
    {
        let status = crate::drivers::pci::pci_read_config_word(
            pci_dev.bus,
            pci_dev.device,
            pci_dev.function,
            0x06,
        );
        if status & 0xF900 != 0 {
            crate::drivers::pci::pci_write_config_word(
                pci_dev.bus,
                pci_dev.device,
                pci_dev.function,
                0x06,
                status,
            );
        }
    }

    // Step 2: pci_set_master() equivalent — separate write for Bus Master
    pci_dev.enable_bus_master();

    // Note: Linux does NOT write Cache Line Size or Latency Timer for PCIe devices.
    // The firmware defaults are preserved. We no longer write these either.

    // Verify the result
    let pci_cmd_after = crate::drivers::pci::pci_read_config_word(
        pci_dev.bus,
        pci_dev.device,
        pci_dev.function,
        0x04,
    );
    if pci_cmd_after & 0x06 != 0x06 {
        crate::serial_println!("[xhci-pci] WARNING: bus master or mem space NOT set!");
    }

    // 2. Map BAR0 via HHDM
    //    Read BAR0 directly from PCI config (not cached self.bars) because
    //    Step 0 may have reassigned BAR0 to a new address.
    let bar0_raw = crate::drivers::pci::pci_read_config_dword(
        pci_dev.bus,
        pci_dev.device,
        pci_dev.function,
        0x10,
    );
    let bar0_phys = (bar0_raw & 0xFFFFF000) as u64;
    let base = HHDM_BASE + bar0_phys;

    // Store BAR base for MMIO write tracing (write32 computes offset = addr - base)
    MMIO_TRACE_BAR_BASE.store(base, Ordering::Relaxed);

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
    xhci_trace_note(
        0,
        if num_sp > 0 {
            "scratchpad_needed"
        } else {
            "scratchpad_none"
        },
    );

    let op_base = base + cap_length as u64;
    let rt_base = base + rts_offset as u64;
    let db_base = base + db_offset as u64;

    // --- M1: CONTROLLER_DISCOVERY ---
    ms_begin!(M_DISCOVERY);
    ms_kv!(M_DISCOVERY, "BAR0_phys=0x{:x}", bar0_phys);
    ms_kv!(
        M_DISCOVERY,
        "xHCI_version=0x{:04x}",
        (cap_word >> 16) & 0xFFFF
    );
    ms_kv!(
        M_DISCOVERY,
        "max_slots={} max_ports={} ctx_size={}",
        max_slots,
        max_ports,
        context_size
    );
    ms_kv!(
        M_DISCOVERY,
        "cap_len={} op_off=0x{:x} rt_off=0x{:x} db_off=0x{:x}",
        cap_length,
        cap_length as u32,
        rts_offset,
        db_offset
    );
    ms_kv!(
        M_DISCOVERY,
        "HCSPARAMS1=0x{:08x} HCCPARAMS1=0x{:08x}",
        hcsparams1,
        hccparams1
    );
    ms_pass!(M_DISCOVERY);

    // 3b. Walk Extended Capabilities list for Supported Protocol info.
    // HCCPARAMS1 bits 31:16 = xECP (xHCI Extended Capabilities Pointer) in DWORDs from base.
    let xecp_offset = ((hccparams1 >> 16) & 0xFFFF) as u64;
    if xecp_offset != 0 {
        let mut ecap_addr = base + xecp_offset * 4;
        for _ in 0..16 {
            let ecap_dw0 = read32(ecap_addr);
            let cap_id = ecap_dw0 & 0xFF;
            let next_ptr = (ecap_dw0 >> 8) & 0xFF;

            if cap_id == 1 {
                // USB Legacy Support Capability (USBLEGSUP, ID=1)
                // xHCI spec 7.1.1: BIOS/OS handoff to claim controller from UEFI.
                // DW0 bits: [16] HC BIOS Owned Semaphore, [24] HC OS Owned Semaphore
                let bios_owned = (ecap_dw0 >> 16) & 1;
                if bios_owned != 0 {
                    // Set OS Owned Semaphore (bit 24), wait for BIOS to release (bit 16 clears)
                    write32(ecap_addr, ecap_dw0 | (1 << 24));
                    for _ in 0..1_000_000u32 {
                        let v = read32(ecap_addr);
                        if (v >> 16) & 1 == 0 {
                            break;
                        }
                        core::hint::spin_loop();
                    }
                    xhci_trace_note(0, "usblegsup_claimed");
                } else {
                    // BIOS doesn't own it, just set OS owned
                    write32(ecap_addr, ecap_dw0 | (1 << 24));
                    xhci_trace_note(0, "usblegsup_no_bios");
                }
                // Also disable SMI on USB events (USBLEGCTLSTS at ecap_addr + 4)
                // Set bits 1, 4, 13, 14, 15 to disable SMI routing
                let legctl = read32(ecap_addr + 4);
                write32(
                    ecap_addr + 4,
                    legctl & !((1 << 0) | (1 << 1) | (1 << 4) | (1 << 13) | (1 << 14) | (1 << 15)),
                );
            } else if cap_id == 2 {
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

    // 3.5. PCI Bus Master disable→enable cycle
    //
    // On Linux, when xhci_hcd is unbound and our module binds, the PCI device goes through:
    //   pci_disable_device() → clears Bus Master (CMD bit 2)
    //   pci_enable_device()  → sets Memory Space (CMD bit 1)
    //   pci_set_master()     → sets Bus Master (CMD bit 2)
    //
    // This disable→enable cycle may signal the Parallels hypervisor to reinitialize
    // its internal USB device model. Breenix never goes through this cycle because
    // the UEFI firmware leaves Bus Master enabled and we just confirm it.
    {
        let cmd = crate::drivers::pci::pci_read_config_word(
            pci_dev.bus,
            pci_dev.device,
            pci_dev.function,
            0x04,
        );

        // Disable Bus Master (clear bit 2), keep Memory Space (bit 1)
        let cmd_no_bm = cmd & !0x0004;
        crate::drivers::pci::pci_write_config_word(
            pci_dev.bus,
            pci_dev.device,
            pci_dev.function,
            0x04,
            cmd_no_bm,
        );
        delay_ms(10); // Brief settle

        // Re-enable Bus Master
        crate::drivers::pci::pci_write_config_word(
            pci_dev.bus,
            pci_dev.device,
            pci_dev.function,
            0x04,
            cmd,
        );
        delay_ms(10);
    }

    // 4. Stop controller + HCRST

    // Enable MMIO write tracing: capture every write32() from halt/HCRST onward
    MMIO_TRACE_IDX.store(0, Ordering::Relaxed);
    MMIO_TRACE_ACTIVE.store(true, Ordering::Release);

    // 4a. Halt the controller (RS=0)
    let usbcmd = read32(op_base);
    if usbcmd & 1 != 0 {
        write32(op_base, usbcmd & !1);
        wait_for(|| read32(op_base + 0x04) & 1 != 0, 100_000)
            .map_err(|_| "XHCI: timeout waiting for HCH")?;
        xhci_trace_note(0, "ctrl_stopped");
    }

    if INHERIT_UEFI {
        // INHERIT_UEFI: Skip HCRST. Read UEFI's device context state, then
        // fall through to normal data structure setup (command ring, event ring).
        // After the controller is running, we'll redirect UEFI's interrupt
        // endpoints to our transfer rings instead of re-enumerating.
        xhci_trace_note(0, "inherit_uefi");

        // Read UEFI's DCBAAP before we overwrite it
        let uefi_dcbaap = read64(op_base + 0x30);
        unsafe {
            UEFI_DCBAAP_SAVED = uefi_dcbaap;
        }
    } else if SKIP_HCRST {
        xhci_trace_note(0, "ctrl_halt_no_reset");
    } else {
        // Full HCRST
        write32(op_base, read32(op_base) | (1 << 1));
        wait_for(|| read32(op_base) & (1 << 1) == 0, 500_000)
            .map_err(|_| "XHCI: timeout waiting for HCRST to clear")?;
        wait_for(|| read32(op_base + 0x04) & (1 << 11) == 0, 500_000)
            .map_err(|_| "XHCI: timeout waiting for CNR to clear after reset")?;
        xhci_trace_note(0, "ctrl_reset");

        // EHCI reset DISABLED: The Parallels hypervisor's EHC reset triggers a
        // CASCADING second XHC controller reset, which destroys the endpoint state
        // that was just created after our first HCRST. Hypervisor log evidence:
        //   45.571: "XHC controller reset" + "EHC controller reset" (same timestamp)
        //   45.571: "DisableEndpoint while io_cnt is not zero!" (endpoints destroyed)
        // After the cascading 2nd reset: NO ep creates → CC=12.
        // On linux-probe, the linux module does NOT reset the EHCI controller.
    }

    // --- M2: CONTROLLER_RESET ---
    ms_begin!(M_RESET);
    {
        let usbsts_post = read32(op_base + 0x04);
        ms_kv!(
            M_RESET,
            "USBSTS=0x{:08x} HCH={} CNR={}",
            usbsts_post,
            usbsts_post & 1,
            (usbsts_post >> 11) & 1
        );
        ms_regs(M_RESET, op_base, rt_base + 0x20);
        if (usbsts_post & 1) != 0 && (usbsts_post & (1 << 11)) == 0 {
            ms_pass!(M_RESET);
        } else {
            ms_fail!(M_RESET, "HCH or CNR unexpected");
        }
    }

    // --- M3: DATA_STRUCTURES ---
    ms_begin!(M_DATA_STRUC);

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
        dma_cache_clean(
            (*dcbaa).0.as_ptr() as *const u8,
            256 * core::mem::size_of::<u64>(),
        );
    }
    // INHERIT_UEFI: copy UEFI's device context data into our DEVICE_CONTEXTS
    // arrays BEFORE writing DCBAAP, so the xHC sees UEFI's endpoint state
    // in our memory.
    if INHERIT_UEFI {
        let uefi_dcbaap = unsafe { UEFI_DCBAAP_SAVED };
        if uefi_dcbaap != 0 {
            let uefi_dcbaa_virt = HHDM_BASE + uefi_dcbaap;
            dma_cache_invalidate(uefi_dcbaa_virt as *const u8, 256 * 8);
            for slot_id in 1..=slots_en {
                let ctx_ptr_phys = unsafe {
                    core::ptr::read_volatile((uefi_dcbaa_virt + slot_id as u64 * 8) as *const u64)
                };
                if ctx_ptr_phys == 0 {
                    continue;
                }
                let slot_idx = (slot_id as usize) - 1;
                let src_virt = HHDM_BASE + ctx_ptr_phys;
                unsafe {
                    dma_cache_invalidate(src_virt as *const u8, 4096);
                    // Copy UEFI's output context into our DEVICE_CONTEXTS
                    core::ptr::copy_nonoverlapping(
                        src_virt as *const u8,
                        DEVICE_CONTEXTS[slot_idx].0.as_mut_ptr(),
                        4096,
                    );
                    dma_cache_clean(DEVICE_CONTEXTS[slot_idx].0.as_ptr(), 4096);
                    // Set DCBAA entry to point to our copy
                    let our_ctx_phys = virt_to_phys((&raw const DEVICE_CONTEXTS[slot_idx]) as u64);
                    let dcbaa = &raw mut DCBAA;
                    (*dcbaa).0[slot_id as usize] = our_ctx_phys;
                }
            }
            // Re-clean DCBAA after copying entries
            unsafe {
                dma_cache_clean(
                    (*(&raw const DCBAA)).0.as_ptr() as *const u8,
                    256 * core::mem::size_of::<u64>(),
                );
            }
        }
    }
    write64(op_base + 0x30, dcbaa_phys);
    // Readback DCBAAP to verify write (volatile read ensures write posted)
    let _dcbaap_rb = read64(op_base + 0x30);
    ms_kv!(
        M_DATA_STRUC,
        "DCBAA: phys=0x{:x} written=0x{:x} readback=0x{:x}",
        dcbaa_phys,
        dcbaa_phys,
        _dcbaap_rb
    );

    // 8. Set Command Ring Control Register (CRCR)
    let cmd_ring_phys = virt_to_phys((&raw const CMD_RING) as u64);
    unsafe {
        // Zero the command ring (CMD_RING_SIZE Trb entries)
        let ring = &raw mut CMD_RING;
        core::ptr::write_bytes((*ring).0.as_mut_ptr(), 0, CMD_RING_SIZE);
        CMD_RING_ENQUEUE = 0;
        CMD_RING_CYCLE = true;
        dma_cache_clean(
            (*ring).0.as_ptr() as *const u8,
            CMD_RING_SIZE * core::mem::size_of::<Trb>(),
        );
    }
    // CRCR: physical address | RCS (Ring Cycle State) = 1
    let crcr_val = cmd_ring_phys | 1;
    write64(op_base + 0x18, crcr_val);
    ms_kv!(
        M_DATA_STRUC,
        "CMD_RING: phys=0x{:x} CRCR_written=0x{:x} readback=0x{:x}",
        cmd_ring_phys,
        crcr_val,
        read64(op_base + 0x18)
    );

    // 9. Set up Event Ring for Interrupter 0
    let event_ring_phys = virt_to_phys((&raw const EVENT_RING) as u64);
    let erst_phys = virt_to_phys((&raw const ERST) as u64);

    unsafe {
        // Zero the event ring (EVENT_RING_SIZE Trb entries)
        let ering = &raw mut EVENT_RING;
        core::ptr::write_bytes((*ering).0.as_mut_ptr(), 0, EVENT_RING_SIZE);
        EVENT_RING_DEQUEUE = 0;
        EVENT_RING_CYCLE = true;
        dma_cache_clean(
            (*ering).0.as_ptr() as *const u8,
            EVENT_RING_SIZE * core::mem::size_of::<Trb>(),
        );

        // Set up ERST entry
        let erst = &raw mut ERST;
        (*erst).0[0] = ErstEntry {
            base: event_ring_phys,
            size: EVENT_RING_SIZE as u32,
            _rsvd: 0,
        };
        dma_cache_clean(
            (*erst).0.as_ptr() as *const u8,
            core::mem::size_of::<ErstEntry>(),
        );
    }

    let ir0 = rt_base + 0x20; // Interrupter 0 register set

    // Match Linux module register write order: IMOD before ERSTSZ/ERDP/ERSTBA.
    // Linux's xhci_setup_rings() writes IMOD first (line 1155), then event ring regs.
    write32(ir0 + 0x04, 0x000000a0); // IMOD = 160 * 250ns = 40µs

    // ERSTSZ (Event Ring Segment Table Size) = 1 segment
    write32(ir0 + 0x08, 1);
    // ERDP (Event Ring Dequeue Pointer) = start of event ring
    write64(ir0 + 0x18, event_ring_phys);
    // ERSTBA (Event Ring Segment Table Base Address) - must be written AFTER ERSTSZ
    write64(ir0 + 0x10, erst_phys);
    ms_kv!(M_DATA_STRUC, "EVT_RING: seg_phys=0x{:x}", event_ring_phys);
    ms_kv!(
        M_DATA_STRUC,
        "ERST: phys=0x{:x} seg_addr=0x{:x} seg_size={}",
        erst_phys,
        event_ring_phys,
        EVENT_RING_SIZE
    );
    ms_kv!(
        M_DATA_STRUC,
        "ERDP: written=0x{:x} readback=0x{:x}",
        event_ring_phys,
        read64(ir0 + 0x18)
    );
    ms_kv!(
        M_DATA_STRUC,
        "ERSTBA: written=0x{:x} readback=0x{:x}",
        erst_phys,
        read64(ir0 + 0x10)
    );
    ms_regs(M_DATA_STRUC, op_base, ir0);
    ms_pass!(M_DATA_STRUC);

    // 10. MSI is NOT configured here — matching Linux module order.
    // The Linux module (breenix_xhci_probe.c) calls pci_alloc_irq_vectors()
    // AFTER all enumeration, TRB queuing, and doorbell ringing (line 2229).
    // During enumeration, Linux has: no MSI configured, INTx enabled.
    // MSI will be configured after start_hid_polling() below.

    // 11. Enable interrupts on Interrupter 0 (needed for event ring operation).
    // IMOD already written above (matching Linux's order: IMOD before event ring regs).
    let iman = read32(ir0);
    write32(ir0, iman | 2); // IMAN.IE = 1

    // 12. Start controller: USBCMD.RS=1, INTE=1
    // INTE is needed for the controller to write events to the event ring.
    let usbcmd = read32(op_base);
    write32(op_base, usbcmd | 1 | (1 << 2)); // RS=1, INTE=1

    // --- M4: CONTROLLER_RUNNING ---
    ms_begin!(M_RUNNING);
    {
        let cmd = read32(op_base);
        let iman_val = read32(ir0);
        if (cmd & 1) != 0 && (cmd & (1 << 2)) != 0 && (iman_val & 2) != 0 {
            ms_pass!(M_RUNNING);
        } else {
            ms_fail!(M_RUNNING, "RS/INTE/IE not set");
        }
    }

    // MATCH LINUX MODULE: enumerate IMMEDIATELY after RS=1.
    // The working Linux module (breenix_xhci_probe.c) does NOT:
    //   - Send NEC vendor command (GET_FW type 49)
    //   - Clear PORTSC change bits before enumeration
    //   - Wait 2000ms before first enumeration
    // It enumerates immediately, then msleep(2000), then enumerates again.
    //
    // Previously, Breenix had all of these extras and still got CC=12.
    // Stripping them to match the working Linux module exactly.

    // Brief delay after RS=1 for periodic schedule to start
    delay_ms(20);

    // Verify controller is running
    let usbsts = read32(op_base + 0x04);
    if usbsts & 1 != 0 {
        xhci_trace_note(0, "err:ctrl_halted");
    }

    // NOTE: 3s pre-enumeration delay was tested here and did NOT fix CC=12.
    // The internal reset theory has been debunked. Removed to speed boot.

    // Re-verify controller is still running after delay
    let usbsts2 = read32(op_base + 0x04);
    if usbsts2 & 1 != 0 {
        xhci_trace_note(0, "err:halted_after_delay");
    }

    // EXPERIMENT: Configure MSI BEFORE enumeration.
    // Theory: Parallels hypervisor needs MSI configured before it will create
    // internal endpoint state. On Linux, xhci_hcd always configures MSI during
    // init (even if later unbound). On Breenix, UEFI never configured MSI.
    let early_irq = setup_xhci_msi(pci_dev);

    // 14. Create state with IRQ already set.
    // F32t: Now that configure_msi() uses Linux ordering (disable → mask →
    // write → flush → INTx off → enable → unmask) no MSI storm is expected,
    // so the deferred SPI activation path can enable this irq (was irq: 0
    // as a workaround since commit 488d2fc2).
    let mut xhci_state = XhciState {
        base,
        cap_length,
        op_base,
        rt_base,
        db_base,
        max_slots: slots_en,
        max_ports,
        context_size,
        irq: early_irq,
        kbd_slot: 0,
        kbd_endpoint: 0,
        kbd_nkro_endpoint: 0,
        mouse_slot: 0,
        mouse_endpoint: 0,
        mouse_nkro_endpoint: 0,
    };

    // Diagnostic: print DMA addresses used for XHCI data structures.
    // On VMware (ram_base_offset=0x40000000), these must be IPAs in the 0x80XXXXXX range.
    crate::serial_println!(
        "[xhci] DMA addrs: DCBAA=0x{:x} CMD_RING=0x{:x} EVT_RING=0x{:x} ERST=0x{:x}",
        dcbaa_phys,
        cmd_ring_phys,
        event_ring_phys,
        erst_phys
    );
    crate::serial_println!(
        "[xhci] Regs: USBCMD=0x{:x} USBSTS=0x{:x} CRCR=0x{:x} DCBAAP=0x{:x}",
        read32(op_base),
        read32(op_base + 0x04),
        read64(op_base + 0x18),
        read64(op_base + 0x30)
    );
    crate::serial_println!(
        "[xhci] IR0: IMAN=0x{:x} ERDP=0x{:x} ERSTBA=0x{:x}",
        read32(ir0),
        read64(ir0 + 0x18),
        read64(ir0 + 0x10)
    );

    // NOOP command: warm up the command ring before real commands.
    // Linux xhci_hcd sends a NEC vendor NOOP as its first command after RS=1.
    // This completes a full command ring cycle (queue TRB → ring doorbell →
    // receive completion event → update ERDP) before Enable Slot.
    if let Err(e) = send_noop(&xhci_state) {
        crate::serial_println!("[xhci] NOOP command failed: {}", e);
        // Dump post-NOOP state for debugging
        crate::serial_println!(
            "[xhci] Post-NOOP: USBSTS=0x{:x} CRCR=0x{:x}",
            read32(op_base + 0x04),
            read64(op_base + 0x18)
        );
        crate::serial_println!(
            "[xhci] Post-NOOP IR0: IMAN=0x{:x} ERDP=0x{:x}",
            read32(ir0),
            read64(ir0 + 0x18)
        );
        // Read back first event ring entry to see if controller wrote anything
        unsafe {
            let ring = &raw const EVENT_RING;
            dma_cache_invalidate(
                &(*ring).0[0] as *const Trb as *const u8,
                core::mem::size_of::<Trb>(),
            );
            let trb = core::ptr::read_volatile(&(*ring).0[0]);
            crate::serial_println!(
                "[xhci] EVT[0]: param=0x{:x} status=0x{:x} control=0x{:x}",
                trb.param,
                trb.status,
                trb.control
            );
        }
    }

    // Dump all port PORTSC values for diagnostic (VMware may not have devices at boot).
    {
        let max_p = xhci_state.max_ports;
        crate::serial_println!("[xhci] Port scan: {} ports", max_p);
        for p in 0..max_p.min(16) as u64 {
            let psc = read32(op_base + 0x400 + p * 0x10);
            if psc != 0 {
                let ccs = psc & 1;
                let ped = (psc >> 1) & 1;
                let speed = (psc >> 10) & 0xF;
                crate::serial_println!(
                    "[xhci]   port {}: PORTSC=0x{:08x} CCS={} PED={} speed={}",
                    p + 1,
                    psc,
                    ccs,
                    ped,
                    speed
                );
            }
        }
    }

    // 15. Scan ports and configure HID devices.
    if INHERIT_UEFI {
        if let Err(e) = inherit_uefi_endpoints(&mut xhci_state) {
            crate::serial_println!(
                "[xhci] INHERIT_UEFI failed: {}, falling back to scan_ports",
                e
            );
            if let Err(_) = scan_ports(&mut xhci_state) {
                xhci_trace_note(0, "err:port_scan");
            }
        }
    } else if let Err(_) = scan_ports(&mut xhci_state) {
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
    let xhci_state_ref = unsafe { (*(&raw const XHCI_STATE)).as_ref().unwrap() };

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

    // NOTE: 2s delay was tested here (matching Linux's msleep(2000)) and did
    // NOT fix CC=12. The issue is not timing-related.

    // CRITICAL: Clear ALL PORTSC change bits IMMEDIATELY before queuing TRBs.
    //
    // Discovery: the Parallels vxHC returns CC=12 (Endpoint Not Enabled) when
    // there are unacknowledged PORTSC change bits (especially CSC, bit 17).
    //
    // The Linux module's port reset code writes `portsc | PR` which ACCIDENTALLY
    // clears all pending change bits (they're W1C — Write-1-to-Clear). Breenix's
    // port reset code carefully preserves change bits with a mask, so they
    // accumulate and are never cleared.
    //
    // Previously, PORTSC clearing was done AFTER start_hid_polling() which is
    // too late — CC=12 occurs immediately when the doorbell is rung. The clearing
    // must happen BEFORE any doorbell ring for interrupt endpoints.
    let ports_cleared = clear_all_port_changes(xhci_state_ref);
    DIAG_PORTSC_CLEARED.store(ports_cleared, Ordering::Relaxed);
    xhci_trace_note(0, "portsc_cleared");

    // Delay before queueing TRBs: the Linux kernel module has a 2-second
    // msleep(2000) between ConfigureEndpoint completion and first doorbell ring.
    // This may allow the Parallels virtual xHC to stabilize internal state.
    // POST_ENUM_DELAY_MS controls this delay (0 = no delay).
    if POST_ENUM_DELAY_MS > 0 {
        delay_ms(POST_ENUM_DELAY_MS);
    }

    // Queue TRBs immediately after PORTSC clearing + delay.
    if DEFERRED_TRB_POLL == 0 {
        start_hid_polling(xhci_state_ref);
        HID_POLLING_STARTED.store(true, Ordering::Release);
    }
    // else: TRBs will be queued at poll == DEFERRED_TRB_POLL in poll_hid_events.

    // 16. MSI was already configured BEFORE enumeration (experiment).
    // Store the IRQ from the early setup.
    XHCI_IRQ.store(early_irq, Ordering::Release);

    // F32t Phase 5a: enable the GIC SPI inline now that TRBs are queued and
    // IMAN.IE + USBCMD.RS are set. This replaces the deferred activation that
    // lived at poll=50 inside poll_hid_events(). Linux parity:
    // /tmp/linux-v6.8/drivers/usb/host/xhci.c::xhci_run_finished_at enables
    // interrupts at the end of controller bring-up.
    if early_irq != 0 && !SPI_ACTIVATED.swap(true, Ordering::AcqRel) {
        crate::arch_impl::aarch64::gic::clear_spi_pending(early_irq);
        crate::arch_impl::aarch64::gic::enable_spi(early_irq);
        DIAG_SPI_ENABLE_COUNT.fetch_add(1, Ordering::Relaxed);
        crate::serial_println!("[xhci] SPI {} enabled at init complete", early_irq);
    }

    xhci_trace_note(0, "init_complete");
    // Trace data available via GDB (call trace_dump()) or /proc/xhci/trace

    // --- M11: EVENT_DELIVERY ---
    ms_begin!(M_EVT_DELIV);
    // M11 is left open — PASS/FAIL is determined by the first transfer event
    // which arrives asynchronously after init() returns. The poll path will
    // report M11 completion when a Transfer Event is received.

    // Keep XHCI_TRACE_ACTIVE enabled so btrace (/proc/xhci/trace) shows
    // post-init events (Transfer Events, doorbell rings, etc.)

    // Single summary line for successful init
    crate::serial_println!(
        "[xhci] Initialized: {} slots, MSI irq={}",
        slots_en,
        early_irq
    );

    Ok(())
}

// =============================================================================
// Interrupt Handling
// =============================================================================

/// Handle an XHCI interrupt.
///
/// Called from the GIC interrupt handler when the XHCI IRQ fires.
/// Disables the GIC SPI while processing to prevent re-delivery during
/// IMAN/ERDP acknowledgment, then re-enables it before returning so the
/// next event gets a real interrupt with no polling delay.
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
                        0xFF,
                        cc,
                        Ordering::AcqRel,
                        Ordering::Relaxed,
                    );
                    let _ = DIAG_FIRST_XFER_PTR.compare_exchange(
                        0,
                        trb.param,
                        Ordering::AcqRel,
                        Ordering::Relaxed,
                    );
                    let _ = DIAG_FIRST_XFER_SLEP.compare_exchange(
                        0,
                        ((slot as u32) << 8) | (endpoint as u32),
                        Ordering::AcqRel,
                        Ordering::Relaxed,
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
                                nkro[0], nkro[1], nkro[2], nkro[3], nkro[4], nkro[5], nkro[6],
                                nkro[7],
                            ]);
                            LAST_NKRO_REPORT_U64.store(nkro_snap, Ordering::Relaxed);
                            if nkro[0] == 1 {
                                KBD_EVENT_COUNT.fetch_add(1, Ordering::Relaxed);
                                // NKRO report (after report ID) is:
                                //   [modifier, key1, key2, ..., key7] — NO reserved byte
                                // Boot keyboard format expects:
                                //   [modifier, reserved=0, key1, ..., key6]
                                // Reformat by inserting a zero reserved byte.
                                let boot_fmt: [u8; 8] = [
                                    nkro[1], 0, // modifier, reserved
                                    nkro[2], nkro[3], nkro[4], nkro[5], nkro[6], nkro[7],
                                ];
                                super::hid::process_keyboard_report(&boot_fmt);
                            }
                            let _ = queue_hid_transfer(state, 2, slot, endpoint);
                        }
                        // Boot keyboard (DCI 3, interface 0)
                        else if slot == state.kbd_slot && endpoint == state.kbd_endpoint {
                            KBD_EVENT_COUNT.fetch_add(1, Ordering::Relaxed);
                            let report_buf = &raw const KBD_REPORT_BUF;
                            dma_cache_invalidate((*report_buf).0.as_ptr(), 8);
                            let report = &(*report_buf).0;
                            super::hid::process_keyboard_report(report);
                            // Requeue immediately — SPI is disabled at the top
                            // of handle_interrupt so no MSI storm is possible.
                            let _ = queue_hid_transfer(state, 0, slot, endpoint);
                        } else if slot == state.mouse_slot && endpoint == state.mouse_endpoint {
                            let report_buf = &raw const MOUSE_REPORT_BUF;
                            dma_cache_invalidate((*report_buf).0.as_ptr(), 8);
                            let report = &(*report_buf).0;
                            super::hid::process_mouse_report(report, 0);
                            let _ = queue_hid_transfer(state, 1, slot, endpoint);
                        } else if slot == state.mouse_slot
                            && state.mouse_nkro_endpoint != 0
                            && endpoint == state.mouse_nkro_endpoint
                        {
                            // Mouse2 (second mouse interface, DCI 5)
                            let report_buf = &raw const MOUSE2_REPORT_BUF;
                            dma_cache_invalidate((*report_buf).0.as_ptr(), 9);
                            let report = &(*report_buf).0;
                            super::hid::process_mouse_report(report, 1);
                            let _ = queue_hid_transfer(state, 3, slot, endpoint);
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
                                report[0], report[1], report[2], report[3], report[4], report[5],
                                report[6], report[7],
                            ]);
                            LAST_GET_REPORT_U64.store(snap, Ordering::Relaxed);
                            if report.iter().any(|&b| b != 0) {
                                GET_REPORT_NONZERO.fetch_add(1, Ordering::Relaxed);
                                super::hid::process_mouse_report(report, 0);
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
            let erdp_phys =
                virt_to_phys(&raw const EVENT_RING as u64) + (EVENT_RING_DEQUEUE as u64) * 16;
            write64(ir0 + 0x18, erdp_phys | (1 << 3));
        }
    }

    // Re-enable the GIC SPI now that we've drained the event ring.
    // Any MSI generated by the IMAN/ERDP writes above will fire as a
    // new interrupt after we return.  That second invocation will find
    // an empty ring (IP=0, no cycle-bit match) and return quickly —
    // no storm, because we only write IMAN/USBSTS when their bits are
    // actually set.
    if state.irq != 0 {
        crate::arch_impl::aarch64::gic::clear_spi_pending(state.irq);
        crate::arch_impl::aarch64::gic::enable_spi(state.irq);
    }
}

// =============================================================================
// Polling Mode (fallback for systems without interrupt support)
// =============================================================================

/// Phase 1 of deferred init (DISABLED — see comment in poll_hid_events).
#[allow(dead_code)]
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
            _ss_mult: 0,
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
                _ss_mult: 0,
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

        if let Err(e) = configure_endpoints_batch(state, slot_id, &pending_eps, ep_count) {
            crate::serial_println!(
                "[xhci] deferred mouse slot {} cfg_ep FAIL: {} (ep_count={})",
                slot_id,
                e,
                ep_count
            );
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
            _ss_mult: 0,
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
                _ss_mult: 0,
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

        if let Err(e) = configure_endpoints_batch(state, slot_id, &pending_eps, ep_count) {
            crate::serial_println!(
                "[xhci] deferred kbd slot {} cfg_ep FAIL: {} (ep_count={})",
                slot_id,
                e,
                ep_count
            );
        }
    }
}

/// Phase 2 of deferred init: queue TRBs and ring doorbells for all HID
/// endpoints. Called at poll=1200 (~6s after timer start), 3 seconds after
/// the deferred ConfigureEndpoint to allow any secondary internal reset to settle.
#[allow(dead_code)]
fn deferred_queue_trbs(state: &XhciState) {
    // Drain any events that appeared between ConfigEP and now.
    drain_stale_events(state);

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

/// Timer-tick housekeeping for xHCI.
///
/// Called from the timer interrupt at 200 Hz (every 5ms). Handles:
/// - One-time deferred SPI activation (first 250ms after init)
/// - Endpoint reset recovery for CC=12 errors
/// - Doorbell re-ring after SPI activation
/// - Draining any events the MSI handler missed (safety net only;
///   the primary event path is handle_interrupt)
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

    // Deferred TRB queue: queue interrupt TRBs after a delay (matching Linux's msleep).
    let poll = POLL_COUNT.load(Ordering::Relaxed);
    if DEFERRED_TRB_POLL > 0
        && poll == DEFERRED_TRB_POLL
        && !HID_POLLING_STARTED.load(Ordering::Acquire)
    {
        start_hid_polling(state);
        HID_POLLING_STARTED.store(true, Ordering::Release);
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
                        0,
                        trb.param,
                        Ordering::AcqRel,
                        Ordering::Relaxed,
                    );
                    let _ = DIAG_FIRST_XFER_STATUS.compare_exchange(
                        0,
                        trb.status | 1,
                        Ordering::AcqRel,
                        Ordering::Relaxed,
                    );
                    let _ = DIAG_FIRST_XFER_CONTROL.compare_exchange(
                        0,
                        trb.control | 1,
                        Ordering::AcqRel,
                        Ordering::Relaxed,
                    );
                    let _ = DIAG_FIRST_XFER_SLEP.compare_exchange(
                        0,
                        ((slot as u32) << 8) | (endpoint as u32),
                        Ordering::AcqRel,
                        Ordering::Relaxed,
                    );
                    // Record first Transfer Event CC (0xFF = unset sentinel)
                    let _ = DIAG_FIRST_XFER_CC.compare_exchange(
                        0xFF,
                        cc,
                        Ordering::AcqRel,
                        Ordering::Relaxed,
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
                                super::hid::process_mouse_report(report, 0);
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
                                super::hid::process_mouse_report(report, 0);
                            }
                        }
                    } else if cc == completion_code::SUCCESS || cc == completion_code::SHORT_PACKET
                    {
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
                                nkro[0], nkro[1], nkro[2], nkro[3], nkro[4], nkro[5], nkro[6],
                                nkro[7],
                            ]);
                            LAST_NKRO_REPORT_U64.store(nkro_snap, Ordering::Relaxed);

                            // NKRO report (after report ID) has NO reserved byte:
                            //   [report_id=1, modifier, key1, key2, ..., key7] = 9 bytes
                            // Reformat to boot keyboard layout for process_keyboard_report:
                            //   [modifier, reserved=0, key1, ..., key6]
                            if nkro[0] == 1 {
                                KBD_EVENT_COUNT.fetch_add(1, Ordering::Relaxed);
                                let boot_fmt: [u8; 8] = [
                                    nkro[1], 0, // modifier, reserved
                                    nkro[2], nkro[3], nkro[4], nkro[5], nkro[6], nkro[7],
                                ];
                                super::hid::process_keyboard_report(&boot_fmt);
                            }

                            let _ = queue_hid_transfer(
                                state,
                                2,
                                state.kbd_slot,
                                state.kbd_nkro_endpoint,
                            );
                        }
                        // Boot keyboard interrupt endpoint (DCI 3, interface 0)
                        else if slot == state.kbd_slot && endpoint == state.kbd_endpoint {
                            KBD_EVENT_COUNT.fetch_add(1, Ordering::Relaxed);
                            DMA_SENTINEL_REPLACED.fetch_add(1, Ordering::SeqCst);

                            let report_buf = &raw const KBD_REPORT_BUF;
                            dma_cache_invalidate((*report_buf).0.as_ptr(), 8);
                            let report = &(*report_buf).0;

                            super::hid::process_keyboard_report(report);
                            let _ =
                                queue_hid_transfer(state, 0, state.kbd_slot, state.kbd_endpoint);
                        }
                        // Mouse interrupt endpoint event (DCI 3)
                        else if slot == state.mouse_slot && endpoint == state.mouse_endpoint {
                            let report_buf = &raw const MOUSE_REPORT_BUF;
                            dma_cache_invalidate((*report_buf).0.as_ptr(), 8);
                            let report = &(*report_buf).0;
                            super::hid::process_mouse_report(report, 0);
                            let _ = queue_hid_transfer(
                                state,
                                1,
                                state.mouse_slot,
                                state.mouse_endpoint,
                            );
                        }
                        // Mouse2 interrupt endpoint event (DCI 5)
                        else if slot == state.mouse_slot
                            && state.mouse_nkro_endpoint != 0
                            && endpoint == state.mouse_nkro_endpoint
                        {
                            let report_buf = &raw const MOUSE2_REPORT_BUF;
                            dma_cache_invalidate((*report_buf).0.as_ptr(), 9);
                            let report = &(*report_buf).0;
                            super::hid::process_mouse_report(report, 1);
                            let _ = queue_hid_transfer(
                                state,
                                3,
                                state.mouse_slot,
                                state.mouse_nkro_endpoint,
                            );
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
                        // Read full endpoint + slot context on first error CC.
                        if DIAG_EP_STATE_AFTER_CC12.load(Ordering::Relaxed) == 0xFF
                            && slot > 0
                            && (slot as usize) <= MAX_SLOTS
                        {
                            let slot_idx = (slot - 1) as usize;
                            let ctx_base = DEVICE_CONTEXTS[slot_idx].0.as_ptr();
                            dma_cache_invalidate(ctx_base, 4096);
                            let ep_out = ctx_base.add(endpoint as usize * state.context_size);
                            let dw0 = core::ptr::read_volatile(ep_out as *const u32);
                            let ep_state = dw0 & 0x7;
                            DIAG_EP_STATE_AFTER_CC12.store(
                                ((slot as u32) << 16) | ((endpoint as u32) << 8) | ep_state,
                                Ordering::Relaxed,
                            );
                            // Capture full EP context DW0-DW4
                            DIAG_CC12_EP_DW0.store(dw0, Ordering::Relaxed);
                            DIAG_CC12_EP_DW1.store(
                                core::ptr::read_volatile(ep_out.add(4) as *const u32),
                                Ordering::Relaxed,
                            );
                            DIAG_CC12_EP_DW2.store(
                                core::ptr::read_volatile(ep_out.add(8) as *const u32),
                                Ordering::Relaxed,
                            );
                            DIAG_CC12_EP_DW3.store(
                                core::ptr::read_volatile(ep_out.add(12) as *const u32),
                                Ordering::Relaxed,
                            );
                            DIAG_CC12_EP_DW4.store(
                                core::ptr::read_volatile(ep_out.add(16) as *const u32),
                                Ordering::Relaxed,
                            );
                            // Capture Slot Context DW0 and DW3
                            DIAG_CC12_SLOT_DW0.store(
                                core::ptr::read_volatile(ctx_base as *const u32),
                                Ordering::Relaxed,
                            );
                            DIAG_CC12_SLOT_DW3.store(
                                core::ptr::read_volatile(ctx_base.add(12) as *const u32),
                                Ordering::Relaxed,
                            );
                            // Capture DCBAA entry for this slot
                            let dcbaa = &raw const DCBAA;
                            dma_cache_invalidate(
                                &(*dcbaa).0[slot as usize] as *const u64 as *const u8,
                                8,
                            );
                            DIAG_CC12_DCBAA.store((*dcbaa).0[slot as usize], Ordering::Relaxed);
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
            let erdp_phys =
                virt_to_phys(&raw const EVENT_RING as u64) + (EVENT_RING_DEQUEUE as u64) * 16;
            write64(ir0 + 0x18, erdp_phys | (1 << 3));
        }
    }

    // MSI requeue fallback: if the MSI handler failed to requeue (e.g., lock
    // contention caused the handler to bail early), requeue here as a safety net.
    if MSI_KBD_NEEDS_REQUEUE.swap(false, Ordering::AcqRel) {
        if state.kbd_slot != 0 && state.kbd_endpoint != 0 {
            let _ = queue_hid_transfer(state, 0, state.kbd_slot, state.kbd_endpoint);
        }
    }
    if MSI_NKRO_NEEDS_REQUEUE.swap(false, Ordering::AcqRel) {
        if state.kbd_slot != 0 && state.kbd_nkro_endpoint != 0 {
            let _ = queue_hid_transfer(state, 2, state.kbd_slot, state.kbd_nkro_endpoint);
        }
    }
    if MSI_MOUSE_NEEDS_REQUEUE.swap(false, Ordering::AcqRel) {
        if state.mouse_slot != 0 && state.mouse_endpoint != 0 {
            let _ = queue_hid_transfer(state, 1, state.mouse_slot, state.mouse_endpoint);
        }
    }
    if MSI_MOUSE2_NEEDS_REQUEUE.swap(false, Ordering::AcqRel) {
        if state.mouse_slot != 0 && state.mouse_nkro_endpoint != 0 {
            let _ = queue_hid_transfer(state, 3, state.mouse_slot, state.mouse_nkro_endpoint);
        }
    }

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
                let _ =
                    reset_halted_endpoint(state, state.mouse_slot, state.mouse_nkro_endpoint, 3);
            }
        }
    }

    // F32t Phase 5a: SPI activation moved to end of xhci::init() so the
    // deferred poll-counter path is no longer required to enable MSI.

    // Ensure HID_TRBS_QUEUED is set after initialization completes.
    if poll >= 100 && !HID_TRBS_QUEUED.load(Ordering::Acquire) {
        HID_TRBS_QUEUED.store(true, Ordering::Release);
    }

    // Re-ring doorbells shortly after SPI activation (poll=75, ~375ms).
    // Tells the xHC to re-check transfer rings now that the interrupt path is live.
    static DOORBELLS_RE_RUNG: AtomicBool = AtomicBool::new(false);
    if poll == 75 && !DOORBELLS_RE_RUNG.load(Ordering::Acquire) {
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
            DIAG_ER_STATE.store(
                ((er_idx as u32) << 1) | if er_cycle { 1 } else { 0 },
                Ordering::Relaxed,
            );

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
                let ep_base = (*dev_ctx)
                    .0
                    .as_ptr()
                    .add(state.mouse_endpoint as usize * ctx_size);
                let trdp_lo = core::ptr::read_volatile(ep_base.add(8) as *const u32);
                let trdp_hi = core::ptr::read_volatile(ep_base.add(12) as *const u32);
                DIAG_RUNTIME_TRDP.store(
                    ((trdp_hi as u64) << 32) | (trdp_lo as u64),
                    Ordering::Relaxed,
                );
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
                    let ep_base = (*dev_ctx)
                        .0
                        .as_ptr()
                        .add(state.kbd_endpoint as usize * ctx_size);
                    core::ptr::read_volatile(ep_base as *const u32) & 0x7
                } else {
                    0
                };
                let dci5_state = if state.kbd_nkro_endpoint != 0 {
                    let ep_base = (*dev_ctx)
                        .0
                        .as_ptr()
                        .add(state.kbd_nkro_endpoint as usize * ctx_size);
                    core::ptr::read_volatile(ep_base as *const u32) & 0x7
                } else {
                    0
                };
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
