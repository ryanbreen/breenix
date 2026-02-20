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

use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering, fence};
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

/// NEC XHCI vendor ID.
pub const NEC_VENDOR_ID: u16 = 0x1033;
/// NEC uPD720200 XHCI device ID.
pub const NEC_XHCI_DEVICE_ID: u16 = 0x0194;

/// Maximum device slots we support.
const MAX_SLOTS: usize = 8;
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
    pub const STOP_ENDPOINT: u32 = 15;
    pub const SET_TR_DEQUEUE_POINTER: u32 = 16;
    pub const NOOP: u32 = 23;
    pub const TRANSFER_EVENT: u32 = 32;
    pub const COMMAND_COMPLETION: u32 = 33;
    pub const PORT_STATUS_CHANGE: u32 = 34;
}

/// xHCI completion codes
mod completion_code {
    pub const SUCCESS: u32 = 1;
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

/// Transfer rings per device slot.
///
/// During enumeration: indexed by slot_idx (slot_id - 1) for EP0 control transfers.
/// After HID configuration: configure_interrupt_endpoint repurposes the hid_idx
/// entry (0=keyboard, 1=mouse) for interrupt IN transfers.
static mut TRANSFER_RINGS: [[Trb; TRANSFER_RING_SIZE]; MAX_SLOTS] =
    [[Trb::zeroed(); TRANSFER_RING_SIZE]; MAX_SLOTS];
/// Transfer ring enqueue indices per slot.
static mut TRANSFER_ENQUEUE: [usize; MAX_SLOTS] = [0; MAX_SLOTS];
/// Transfer ring cycle state per slot.
static mut TRANSFER_CYCLE: [bool; MAX_SLOTS] = [true; MAX_SLOTS];

/// Input Contexts for device setup (2048 bytes each for 64-byte contexts).
/// Used temporarily during AddressDevice and ConfigureEndpoint commands.
static mut INPUT_CONTEXTS: [AlignedPage<[u8; 4096]>; MAX_SLOTS] =
    [const { AlignedPage([0u8; 4096]) }; MAX_SLOTS];

/// Device Contexts (output contexts, 2048 bytes each).
/// Managed by the controller; we provide physical addresses via DCBAA.
static mut DEVICE_CONTEXTS: [AlignedPage<[u8; 4096]>; MAX_SLOTS] =
    [const { AlignedPage([0u8; 4096]) }; MAX_SLOTS];

/// HID report buffer for keyboard (8 bytes: modifier + reserved + 6 keycodes).
static mut KBD_REPORT_BUF: Aligned64<[u8; 8]> = Aligned64([0u8; 8]);

/// HID report buffer for mouse (8 bytes: buttons + X + Y + wheel + ...).
static mut MOUSE_REPORT_BUF: Aligned64<[u8; 8]> = Aligned64([0u8; 8]);

/// Scratch buffer for control transfer data stages (256 bytes).
static mut CTRL_DATA_BUF: Aligned64<[u8; 256]> = Aligned64([0u8; 256]);

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
    max_ports: u8,
    /// Context entry size (32 or 64 bytes)
    context_size: usize,
    /// GIC INTID for this controller
    irq: u32,
    /// Slot ID for keyboard device (0 = not found)
    kbd_slot: u8,
    /// Endpoint DCI for keyboard interrupt IN
    kbd_endpoint: u8,
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

/// Diagnostic counters for heartbeat visibility.
pub static POLL_COUNT: AtomicU64 = AtomicU64::new(0);
pub static EVENT_COUNT: AtomicU64 = AtomicU64::new(0);
pub static KBD_EVENT_COUNT: AtomicU64 = AtomicU64::new(0);
/// Counts transfer events that didn't match kbd/mouse slots or had error CC.
pub static XFER_OTHER_COUNT: AtomicU64 = AtomicU64::new(0);
/// Counts port status change events.
pub static PSC_COUNT: AtomicU64 = AtomicU64::new(0);

/// When true, use EP0 GET_REPORT control transfers instead of interrupt endpoint.
/// Set automatically when the first interrupt transfer returns CC=12 (Endpoint Not Enabled).
static EP0_POLLING_MODE: AtomicBool = AtomicBool::new(false);
/// Track how many EP0 polls to skip (rate-limit: 1 GET_REPORT per N poll cycles).
static EP0_POLL_SKIP: AtomicU64 = AtomicU64::new(0);

/// EP0 polling async state machine (non-blocking, no spin waits in timer handler).
///
/// States:
///   0 = IDLE: ready to submit a GET_REPORT or start a ring reset
///   1 = XFER_PENDING: GET_REPORT in-flight, waiting for SUCCESS Transfer Event
///   2 = WAIT_STOP_EP: Stop EP command issued, waiting for Command Completion
///   3 = WAIT_SET_DEQUEUE: Set TR Dequeue command issued, waiting for CC
mod ep0_state {
    pub const IDLE: u8 = 0;
    pub const XFER_PENDING: u8 = 1;
    pub const WAIT_STOP_EP: u8 = 2;
    pub const WAIT_SET_DEQUEUE: u8 = 3;
}
static EP0_STATE: AtomicU8 = AtomicU8::new(ep0_state::IDLE);

/// Tracks how many GET_REPORT submissions have been made since the last ring reset.
static EP0_POLL_SUBMISSIONS: AtomicU64 = AtomicU64::new(0);
/// Tracks how many successful ring resets have been performed.
pub static EP0_RESET_COUNT: AtomicU64 = AtomicU64::new(0);
/// Tracks how many ring reset failures have occurred (CC != SUCCESS for Stop EP or Set TR Deq).
pub static EP0_RESET_FAIL_COUNT: AtomicU64 = AtomicU64::new(0);
/// Counts how many times the stuck-detection force-cleared EP0 state back to IDLE.
pub static EP0_PENDING_STUCK_COUNT: AtomicU64 = AtomicU64::new(0);
/// Counts consecutive poll cycles in a non-IDLE state without progress.
static EP0_STALL_POLLS: AtomicU64 = AtomicU64::new(0);

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
fn wait_for_event(state: &XhciState) -> Result<Trb, &'static str> {
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
                if trb_type_val == trb_type::COMMAND_COMPLETION
                    || trb_type_val == trb_type::TRANSFER_EVENT
                {
                    return Ok(trb);
                }
                // Asynchronous event (Port Status Change, etc.) — skip and
                // keep waiting for the command/transfer completion we expect.
                continue;
            }
        }
        timeout -= 1;
        if timeout == 0 {
            return Err("XHCI event timeout");
        }
        core::hint::spin_loop();
    }
}

/// Issue a Stop Endpoint command for EP0 (non-blocking).
///
/// After issuing, transitions to WAIT_STOP_EP state. The Command Completion
/// event will be handled by the event loop in poll_hid_events.
fn issue_stop_ep0(state: &XhciState, slot_id: u8) {
    let dci: u32 = 1; // EP0
    let stop_trb = Trb {
        param: 0,
        status: 0,
        control: (trb_type::STOP_ENDPOINT << 10)
            | (dci << 16)
            | ((slot_id as u32) << 24),
    };
    enqueue_command(stop_trb);
    ring_doorbell(state, 0, 0);
    EP0_STATE.store(ep0_state::WAIT_STOP_EP, Ordering::Release);
}

/// Handle the Command Completion for Stop Endpoint: zero the ring,
/// then issue Set TR Dequeue Pointer (non-blocking).
fn handle_stop_ep_complete(state: &XhciState, slot_id: u8, cc: u32) {
    // SUCCESS (1) or CONTEXT_STATE_ERROR (19) both acceptable
    if cc != 1 && cc != 19 {
        EP0_RESET_FAIL_COUNT.fetch_add(1, Ordering::Relaxed);
        EP0_STATE.store(ep0_state::IDLE, Ordering::Release);
        return;
    }

    let slot_idx = (slot_id - 1) as usize;

    // Zero the transfer ring and reset enqueue pointer
    unsafe {
        core::ptr::write_bytes(
            TRANSFER_RINGS[slot_idx].as_mut_ptr() as *mut u8,
            0,
            TRANSFER_RING_SIZE * 16,
        );
        TRANSFER_ENQUEUE[slot_idx] = 0;
        TRANSFER_CYCLE[slot_idx] = true;
        dma_cache_clean(
            TRANSFER_RINGS[slot_idx].as_ptr() as *const u8,
            TRANSFER_RING_SIZE * 16,
        );
    }

    // Issue Set TR Dequeue Pointer command
    let ring_phys = virt_to_phys(unsafe { &raw const TRANSFER_RINGS[slot_idx] } as u64);
    let dci: u32 = 1;
    let set_deq_trb = Trb {
        param: ring_phys | 1, // DCS = 1 (matching reset TRANSFER_CYCLE)
        status: 0,
        control: (trb_type::SET_TR_DEQUEUE_POINTER << 10)
            | (dci << 16)
            | ((slot_id as u32) << 24),
    };
    enqueue_command(set_deq_trb);
    ring_doorbell(state, 0, 0);
    EP0_STATE.store(ep0_state::WAIT_SET_DEQUEUE, Ordering::Release);
}

/// Handle the Command Completion for Set TR Dequeue Pointer.
fn handle_set_dequeue_complete(cc: u32) {
    if cc == 1 {
        EP0_POLL_SUBMISSIONS.store(0, Ordering::Relaxed);
        EP0_RESET_COUNT.fetch_add(1, Ordering::Relaxed);
    } else {
        EP0_RESET_FAIL_COUNT.fetch_add(1, Ordering::Relaxed);
    }
    EP0_STATE.store(ep0_state::IDLE, Ordering::Release);
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

    let event = wait_for_event(state)?;
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

        // Zero the output (device) context
        let dev_ctx = &raw mut DEVICE_CONTEXTS[slot_idx];
        core::ptr::write_bytes((*dev_ctx).0.as_mut_ptr(), 0, 4096);

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

        // DW1: Root Hub Port Number (bits 23:16)
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

        // EP0 DW1: EP Type (bits 5:3) = 4 (Control Bidirectional), CErr (bits 2:1) = 3
        let ep0_dw1: u32 = (4u32 << 3) | (3u32 << 1);
        core::ptr::write_volatile(ep0_ctx.add(0x04) as *mut u32, ep0_dw1);

        // EP0 DW2-DW3: TR Dequeue Pointer
        // Each device slot uses its own transfer ring during enumeration.
        let ring_ptr = &raw mut TRANSFER_RINGS[slot_idx];
        core::ptr::write_bytes(ring_ptr as *mut u8, 0, TRANSFER_RING_SIZE * 16);
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

        // EP0 DW4: Max Packet Size (bits 31:16), Average TRB Length (bits 15:0)
        let ep0_dw4: u32 = ((max_packet_size as u32) << 16) | 8; // Avg TRB len = 8 for control
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

        // Build AddressDevice TRB
        let input_ctx_phys = virt_to_phys(&raw const INPUT_CONTEXTS[slot_idx] as u64);
        let trb = Trb {
            param: input_ctx_phys,
            status: 0,
            // AddressDevice type, Slot ID in bits 31:24
            control: (trb_type::ADDRESS_DEVICE << 10) | ((slot_id as u32) << 24),
        };
        enqueue_command(trb);
        ring_doorbell(state, 0, 0);

        let event = wait_for_event(state)?;
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
                    | (1 << 5), // TC bit
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

    // Wait for completion event
    let event = wait_for_event(state)?;
    let cc = event.completion_code();
    if cc != completion_code::SUCCESS && cc != completion_code::SHORT_PACKET {
        crate::serial_println!(
            "[xhci] Control transfer failed: slot={} cc={}",
            slot_id,
            cc
        );
        return Err("XHCI control transfer failed");
    }

    Ok(())
}

/// Get the device descriptor from a USB device.
fn get_device_descriptor(
    state: &XhciState,
    slot_id: u8,
    buf: &mut [u8; 18],
) -> Result<(), &'static str> {
    let setup = SetupPacket {
        bm_request_type: 0x80, // Device-to-host, standard, device
        b_request: request::GET_DESCRIPTOR,
        w_value: (descriptor_type::DEVICE as u16) << 8, // Descriptor type in high byte
        w_index: 0,
        w_length: 18,
    };

    unsafe {
        // Zero the data buffer
        let data_buf = &raw mut CTRL_DATA_BUF;
        core::ptr::write_bytes((*data_buf).0.as_mut_ptr(), 0, 18);
        dma_cache_clean((*data_buf).0.as_ptr(), 18);

        let data_phys = virt_to_phys(&raw const CTRL_DATA_BUF as u64);

        control_transfer(state, slot_id, &setup, data_phys, 18, true)?;

        // Invalidate cache to see device-written data
        dma_cache_invalidate((*data_buf).0.as_ptr(), 18);

        // Copy to caller's buffer
        buf.copy_from_slice(&(&(*data_buf).0)[..18]);
    }

    // Log basic info (copy packed fields to locals to avoid unaligned references)
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

/// Send SET_PROTOCOL request to set boot protocol on a HID interface.
fn set_boot_protocol(
    state: &XhciState,
    slot_id: u8,
    interface: u8,
) -> Result<(), &'static str> {
    let setup = SetupPacket {
        bm_request_type: 0x21, // Host-to-device, class, interface
        b_request: hid_request::SET_PROTOCOL,
        w_value: 0, // 0 = Boot Protocol
        w_index: interface as u16,
        w_length: 0,
    };

    control_transfer(state, slot_id, &setup, 0, 0, false)?;
    crate::serial_println!(
        "[xhci] Set boot protocol on slot {} interface {}",
        slot_id,
        interface
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

// =============================================================================
// Endpoint Configuration (Configure Endpoint Command)
// =============================================================================

/// Configure an interrupt IN endpoint for a HID device.
///
/// Builds an Input Context with the endpoint context and issues a
/// ConfigureEndpoint command to the controller.
fn configure_interrupt_endpoint(
    state: &XhciState,
    slot_id: u8,
    ep_desc: &EndpointDescriptor,
    hid_idx: usize, // 0 = keyboard, 1 = mouse
) -> Result<u8, &'static str> {
    let slot_idx = (slot_id - 1) as usize;
    let ctx_size = state.context_size;

    // Calculate the Device Context Index (DCI) for this endpoint.
    // DCI = 2 * endpoint_number + direction (0=OUT, 1=IN)
    let ep_num = ep_desc.endpoint_number();
    let dci = ep_num * 2 + if ep_desc.is_in() { 1 } else { 0 };

    let max_packet_size = ep_desc.w_max_packet_size;
    crate::serial_println!(
        "[xhci] Configuring interrupt EP: addr={:#04x} num={} DCI={} maxpkt={} interval={}",
        ep_desc.b_endpoint_address,
        ep_num,
        dci,
        max_packet_size,
        ep_desc.b_interval,
    );

    unsafe {
        // Zero and rebuild the input context
        let input_ctx = &raw mut INPUT_CONTEXTS[slot_idx];
        core::ptr::write_bytes((*input_ctx).0.as_mut_ptr(), 0, 4096);

        let input_base = (*input_ctx).0.as_mut_ptr();

        // Input Control Context: Add flags for Slot Context and the target endpoint
        // A0 = 1 (Slot), A[dci] = 1
        let add_flags: u32 = 1 | (1u32 << dci);
        core::ptr::write_volatile(input_base.add(0x04) as *mut u32, add_flags);

        // Slot Context: Update Context Entries to include the new endpoint
        let slot_ctx = input_base.add(ctx_size);
        // Read current slot context from device context
        let dev_ctx = &raw const DEVICE_CONTEXTS[slot_idx];
        dma_cache_invalidate((*dev_ctx).0.as_ptr(), 4096);

        // Copy current slot context DW0 and update Context Entries
        let current_slot_dw0 = core::ptr::read_volatile(
            (*dev_ctx).0.as_ptr().add(0) as *const u32,
        );
        // Context Entries = max(current, dci)
        let current_entries = (current_slot_dw0 >> 27) & 0x1F;
        let new_entries = current_entries.max(dci as u32);
        let new_slot_dw0 = (current_slot_dw0 & !(0x1F << 27)) | (new_entries << 27);
        core::ptr::write_volatile(slot_ctx as *mut u32, new_slot_dw0);

        // Copy DW1 (Root Hub Port Number, etc.)
        let slot_dw1 = core::ptr::read_volatile(
            (*dev_ctx).0.as_ptr().add(4) as *const u32,
        );
        core::ptr::write_volatile(slot_ctx.add(4) as *mut u32, slot_dw1);

        // Endpoint Context at offset (1 + dci) * ctx_size
        let ep_ctx = input_base.add((1 + dci as usize) * ctx_size);

        // EP DW0: Interval (bits 23:16)
        // xHCI interval = 2^(bInterval-1) for HS/SS, or bInterval for LS/FS in 125us units
        // For simplicity, use the interval value directly
        let interval = if ep_desc.b_interval == 0 { 1 } else { ep_desc.b_interval };
        let ep_dw0: u32 = (interval as u32) << 16;
        core::ptr::write_volatile(ep_ctx as *mut u32, ep_dw0);

        // EP DW1: EP Type (bits 5:3), CErr (bits 2:1)
        // EP Type for Interrupt IN = 7 (per xHCI spec: Isoch OUT=1, Bulk OUT=2, Int OUT=3,
        //   Control Bidir=4, Isoch IN=5, Bulk IN=6, Int IN=7)
        let ep_type: u32 = 7; // Interrupt IN
        let cerr: u32 = 3;    // Max error count
        let ep_dw1: u32 = (ep_type << 3) | (cerr << 1);
        core::ptr::write_volatile(ep_ctx.add(0x04) as *mut u32, ep_dw1);

        // Clear and set up the HID transfer ring for this device
        let ring = &raw mut TRANSFER_RINGS[hid_idx];
        core::ptr::write_bytes(ring as *mut u8, 0, TRANSFER_RING_SIZE * 16);
        TRANSFER_ENQUEUE[hid_idx] = 0;
        TRANSFER_CYCLE[hid_idx] = true;

        let ring_phys = virt_to_phys(&raw const TRANSFER_RINGS[hid_idx] as u64);

        // EP DW2-DW3: TR Dequeue Pointer with DCS = 1
        core::ptr::write_volatile(
            ep_ctx.add(0x08) as *mut u32,
            (ring_phys as u32) | 1, // DCS = 1
        );
        core::ptr::write_volatile(
            ep_ctx.add(0x0C) as *mut u32,
            (ring_phys >> 32) as u32,
        );

        // EP DW4: Max Packet Size (bits 31:16), Max Burst Size (bits 15:8)=0,
        //         Average TRB Length (bits 15:0)
        let max_pkt = ep_desc.w_max_packet_size & 0x07FF; // Bits 10:0
        let avg_trb_len = max_pkt; // For interrupt, average = max packet
        let ep_dw4: u32 = ((max_pkt as u32) << 16) | (avg_trb_len as u32);
        core::ptr::write_volatile(ep_ctx.add(0x10) as *mut u32, ep_dw4);

        // Cache-clean the input context
        dma_cache_clean(input_base, 4096);

        // Issue ConfigureEndpoint command
        let input_ctx_phys = virt_to_phys(&raw const INPUT_CONTEXTS[slot_idx] as u64);
        let trb = Trb {
            param: input_ctx_phys,
            status: 0,
            control: (trb_type::CONFIGURE_ENDPOINT << 10) | ((slot_id as u32) << 24),
        };
        enqueue_command(trb);
        ring_doorbell(state, 0, 0);

        let event = wait_for_event(state)?;
        let cc = event.completion_code();
        if cc != completion_code::SUCCESS {
            crate::serial_println!(
                "[xhci] ConfigureEndpoint failed: slot={} dci={} cc={}",
                slot_id,
                dci,
                cc
            );
            return Err("XHCI ConfigureEndpoint failed");
        }

        // Verify: read back device context to check endpoint AND slot state
        dma_cache_invalidate((*dev_ctx).0.as_ptr(), 4096);

        // Check Slot Context: Context Entries must include DCI
        let slot_out_dw0 = core::ptr::read_volatile(
            (*dev_ctx).0.as_ptr() as *const u32,
        );
        let ctx_entries = (slot_out_dw0 >> 27) & 0x1F;
        // Note: slot state is in DW3 bits 31:27, not DW0
        let slot_out_dw3 = core::ptr::read_volatile(
            (*dev_ctx).0.as_ptr().add(12) as *const u32,
        );
        let device_addr = slot_out_dw3 & 0xFF;
        let slot_st = (slot_out_dw3 >> 27) & 0x1F;
        crate::serial_println!(
            "[xhci] Slot {} context after ConfigureEndpoint: ctx_entries={} slot_state={} dev_addr={}",
            slot_id, ctx_entries, slot_st, device_addr,
        );

        if (dci as u32) > ctx_entries {
            crate::serial_println!(
                "[xhci] WARNING: DCI {} > Context Entries {}! Endpoint out of range!",
                dci, ctx_entries,
            );
        }

        // Check Endpoint Context
        let ep_out = (*dev_ctx).0.as_ptr().add((dci as usize) * ctx_size);
        let ep_out_dw0 = core::ptr::read_volatile(ep_out as *const u32);
        let ep_state = ep_out_dw0 & 0x7; // Bits [2:0] = EP State
        let ep_out_dw1 = core::ptr::read_volatile(ep_out.add(4) as *const u32);
        let ep_out_dw2 = core::ptr::read_volatile(ep_out.add(8) as *const u32);
        let ep_out_dw3 = core::ptr::read_volatile(ep_out.add(12) as *const u32);
        let tr_dequeue = (ep_out_dw2 as u64) | ((ep_out_dw3 as u64) << 32);

        let ring_phys_verify = virt_to_phys(&raw const TRANSFER_RINGS[hid_idx] as u64);

        crate::serial_println!(
            "[xhci] Configured endpoint DCI {} for slot {}: state={} type={} ring_phys={:#x} ctx_dequeue={:#x}",
            dci, slot_id,
            ep_state,
            (ep_out_dw1 >> 3) & 0x7,
            ring_phys_verify,
            tr_dequeue & !0xF, // Mask out DCS and reserved bits
        );

        // EP State: 0=Disabled, 1=Running, 2=Halted, 3=Stopped, 4=Error
        if ep_state == 0 {
            crate::serial_println!(
                "[xhci] WARNING: Endpoint DCI {} still Disabled after ConfigureEndpoint SUCCESS!",
                dci
            );
        }

        Ok(dci)
    }
}

// =============================================================================
// HID Configuration and Transfer Queueing
// =============================================================================

/// Parse configuration descriptor, find HID interfaces, configure endpoints,
/// and start polling for HID reports.
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

    // Walk the descriptor chain looking for HID interfaces
    let mut offset = config_desc.b_length as usize;
    let mut found_hid = false;

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

            if iface.b_interface_class == class_code::HID
                && iface.b_interface_sub_class == hid_subclass::BOOT
            {
                crate::serial_println!(
                    "[xhci] Found HID boot interface: number={} protocol={} endpoints={}",
                    iface.b_interface_number,
                    iface.b_interface_protocol,
                    iface.b_num_endpoints,
                );

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
                            // First, set configuration if we haven't yet
                            if !found_hid {
                                set_configuration(state, slot_id, config_value)?;
                                found_hid = true;
                            }

                            // Determine HID device type
                            let (hid_idx, is_keyboard) =
                                if iface.b_interface_protocol == hid_protocol::KEYBOARD {
                                    (0usize, true)
                                } else {
                                    (1usize, false)
                                };

                            // Set boot protocol and idle
                            let _ = set_boot_protocol(state, slot_id, iface.b_interface_number);
                            let _ = set_idle(state, slot_id, iface.b_interface_number);

                            // Configure the interrupt endpoint
                            let dci = configure_interrupt_endpoint(
                                state, slot_id, ep_desc, hid_idx,
                            )?;

                            // Record the slot/endpoint for interrupt handling
                            if is_keyboard {
                                state.kbd_slot = slot_id;
                                state.kbd_endpoint = dci;
                                crate::serial_println!(
                                    "[xhci] Keyboard configured: slot={} DCI={}",
                                    slot_id,
                                    dci
                                );
                            } else {
                                state.mouse_slot = slot_id;
                                state.mouse_endpoint = dci;
                                crate::serial_println!(
                                    "[xhci] Mouse configured: slot={} DCI={}",
                                    slot_id,
                                    dci
                                );
                            }
                            // NOTE: Initial HID transfers are queued in start_hid_polling()
                            // AFTER all port scanning is complete, to avoid transfer events
                            // being consumed by wait_for_event during subsequent port commands.

                            break; // Found the endpoint for this interface
                        }
                    }

                    ep_offset += ep_len;
                }
            }
        }

        offset += desc_len;
    }

    if !found_hid {
        crate::serial_println!("[xhci] No HID boot interfaces found on slot {}", slot_id);
    }

    Ok(())
}

/// Queue a Normal TRB on a HID transfer ring to receive an interrupt IN report.
fn queue_hid_transfer(
    state: &XhciState,
    hid_idx: usize,
    slot_id: u8,
    dci: u8,
) -> Result<(), &'static str> {
    // Determine the physical address of the report buffer
    let buf_phys = if hid_idx == 0 {
        virt_to_phys((&raw const KBD_REPORT_BUF) as u64)
    } else {
        virt_to_phys((&raw const MOUSE_REPORT_BUF) as u64)
    };

    // Clean the report buffer before giving it to the controller
    if hid_idx == 0 {
        dma_cache_clean((&raw const KBD_REPORT_BUF) as *const u8, 8);
    } else {
        dma_cache_clean((&raw const MOUSE_REPORT_BUF) as *const u8, 8);
    }

    // Log the transfer ring state before enqueue
    unsafe {
        let ring_phys = virt_to_phys(&raw const TRANSFER_RINGS[hid_idx] as u64);
        let enq_idx = TRANSFER_ENQUEUE[hid_idx];
        let trb_phys = ring_phys + (enq_idx as u64) * 16;
        crate::serial_println!(
            "[xhci] queue_hid_transfer: hid_idx={} ring_phys={:#x} enq_idx={} trb_phys={:#x} buf_phys={:#x}",
            hid_idx, ring_phys, enq_idx, trb_phys, buf_phys,
        );
    }

    // Normal TRB for interrupt IN transfer
    let trb = Trb {
        param: buf_phys,
        status: 8, // Transfer length = 8 bytes
        // Normal TRB type, IOC (bit 5), ISP (Interrupt on Short Packet, bit 2)
        control: (trb_type::NORMAL << 10) | (1 << 5) | (1 << 2),
    };
    enqueue_transfer(hid_idx, trb);

    // Ring the doorbell for this endpoint
    crate::serial_println!(
        "[xhci] Ringing doorbell: slot={} target_dci={}",
        slot_id, dci,
    );
    ring_doorbell(state, slot_id, dci);

    Ok(())
}

/// Submit a GET_REPORT control transfer on EP0 asynchronously (non-blocking).
///
/// Enqueues Setup + Data + Status stage TRBs on the device's EP0 transfer ring
/// and rings the doorbell. The caller must check the event ring for the completion
/// in a later poll cycle.
///
/// When the ring is nearly full, issues Stop Endpoint + Set TR Dequeue Pointer
/// commands to reset the ring back to entry 0. This is necessary because the
/// Parallels XHCI emulation does not follow Link TRBs in transfer rings.
///
/// Enqueue a HID GET_REPORT control transfer on EP0.
///
/// Uses KBD_REPORT_BUF as the data buffer for the 8-byte boot keyboard report.
/// Caller must ensure the ring has room (at least 3 entries).
fn submit_ep0_get_report(state: &XhciState, slot_id: u8) {
    let slot_idx = (slot_id - 1) as usize;

    let setup = SetupPacket {
        bm_request_type: 0xA1,
        b_request: 0x01, // GET_REPORT
        w_value: 0x0100, // Input report, ID 0
        w_index: 0,
        w_length: 8,
    };

    let setup_data: u64 = unsafe {
        core::ptr::read_unaligned(&setup as *const SetupPacket as *const u64)
    };

    // Clean the report buffer
    dma_cache_clean((&raw const KBD_REPORT_BUF) as *const u8, 8);

    let buf_phys = virt_to_phys((&raw const KBD_REPORT_BUF) as u64);

    // Setup Stage TRB: TRT = 3 (IN Data Stage)
    let setup_trb = Trb {
        param: setup_data,
        status: 8, // Setup packet = 8 bytes
        control: (trb_type::SETUP_STAGE << 10) | (1 << 6) | (3 << 16), // IDT=1, TRT=IN
    };
    enqueue_transfer(slot_idx, setup_trb);

    // Data Stage TRB: IN direction, 8 bytes
    let data_trb = Trb {
        param: buf_phys,
        status: 8,
        control: (trb_type::DATA_STAGE << 10) | (1 << 16), // DIR=IN
    };
    enqueue_transfer(slot_idx, data_trb);

    // Status Stage TRB: OUT direction (opposite of data), IOC
    let status_trb = Trb {
        param: 0,
        status: 0,
        control: (trb_type::STATUS_STAGE << 10) | (1 << 5), // IOC, DIR=OUT (bit 16 = 0)
    };
    enqueue_transfer(slot_idx, status_trb);

    EP0_POLL_SUBMISSIONS.fetch_add(1, Ordering::Relaxed);

    // Ring doorbell for EP0 (DCI = 1)
    ring_doorbell(state, slot_id, 1);
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

/// Queue initial HID transfers for all configured HID devices.
///
/// Must be called AFTER scan_ports completes, so that transfer events
/// don't interfere with wait_for_event during port enumeration commands.
fn start_hid_polling(state: &XhciState) {
    // First, drain any leftover events from enumeration
    drain_stale_events(state);

    // Verify controller state
    let usbcmd = read32(state.op_base);
    let usbsts = read32(state.op_base + 0x04);
    crate::serial_println!(
        "[xhci] Pre-poll state: USBCMD={:#010x} USBSTS={:#010x}",
        usbcmd, usbsts,
    );

    if state.kbd_slot != 0 {
        let slot_idx = (state.kbd_slot - 1) as usize;
        let dci = state.kbd_endpoint as usize;

        // Re-verify endpoint state right before queueing
        unsafe {
            let dev_ctx = &raw const DEVICE_CONTEXTS[slot_idx];
            dma_cache_invalidate((*dev_ctx).0.as_ptr(), 4096);
            let ep_out = (*dev_ctx).0.as_ptr().add(dci * state.context_size);
            let ep_dw0 = core::ptr::read_volatile(ep_out as *const u32);
            let ep_state = ep_dw0 & 0x7;
            let ep_dw2 = core::ptr::read_volatile(ep_out.add(8) as *const u32);
            let ep_dw3 = core::ptr::read_volatile(ep_out.add(12) as *const u32);
            let tr_dequeue = (ep_dw2 as u64) | ((ep_dw3 as u64) << 32);
            crate::serial_println!(
                "[xhci] Pre-poll kbd EP DCI {}: state={} tr_dequeue={:#x}",
                dci, ep_state, tr_dequeue & !0xF,
            );
        }

        crate::serial_println!(
            "[xhci] Starting keyboard polling: slot={} DCI={}",
            state.kbd_slot,
            state.kbd_endpoint,
        );
        if let Err(e) = queue_hid_transfer(state, 0, state.kbd_slot, state.kbd_endpoint) {
            crate::serial_println!("[xhci] Failed to queue keyboard transfer: {}", e);
        }
    }
    if state.mouse_slot != 0 {
        crate::serial_println!(
            "[xhci] Starting mouse polling: slot={} DCI={}",
            state.mouse_slot,
            state.mouse_endpoint,
        );
        if let Err(e) = queue_hid_transfer(state, 1, state.mouse_slot, state.mouse_endpoint) {
            crate::serial_println!("[xhci] Failed to queue mouse transfer: {}", e);
        }
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

    let mut slots_used: u8 = 0;
    let max_enumerate: u8 = 4; // Only enumerate first few connected devices

    for port in 0..state.max_ports as u64 {
        // Stop early if we've found both keyboard and mouse
        if state.kbd_slot != 0 && state.mouse_slot != 0 {
            break;
        }
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

        // Get Device Descriptor
        let mut desc_buf = [0u8; 18];
        if let Err(e) = get_device_descriptor(state, slot_id, &mut desc_buf) {
            crate::serial_println!("[xhci] Port {}: get_device_descriptor failed: {}", port, e);
            continue;
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
    let iman = read32(ir0);
    write32(ir0, iman | 2); // IMAN.IE = 1

    // 11. Start controller: USBCMD.RS=1, INTE=1
    let usbcmd = read32(op_base);
    write32(op_base, usbcmd | 1 | (1 << 2)); // RS=1, INTE=1
    crate::serial_println!("[xhci] Controller started (USBCMD={:#010x})", read32(op_base));

    // Wait a bit for ports to detect connections
    for _ in 0..100_000 {
        core::hint::spin_loop();
    }

    // Verify controller is running
    let usbsts = read32(op_base + 0x04);
    if usbsts & 1 != 0 {
        crate::serial_println!("[xhci] WARNING: Controller halted after start (USBSTS={:#010x})", usbsts);
    }

    // 12. Compute GIC INTID and enable interrupt
    // On ARM64, PCI interrupt_line maps to GIC SPI.
    // SPI INTID = interrupt_line + 32 (GIC SPI offset)
    // IRQ line 255 means "not assigned" — use polling fallback.
    let irq = pci_dev.interrupt_line as u32 + 32;
    if pci_dev.interrupt_line != 0 && pci_dev.interrupt_line != 0xFF {
        crate::serial_println!("[xhci] Enabling GIC IRQ {} (PCI interrupt_line={})", irq, pci_dev.interrupt_line);
        use crate::arch_impl::aarch64::gic;
        use crate::arch_impl::traits::InterruptController;
        gic::Gicv2::enable_irq(irq as u8);
    } else {
        crate::serial_println!("[xhci] PCI interrupt_line={} (unassigned), using polling mode", pci_dev.interrupt_line);
    }

    // 13. Store state
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
        mouse_slot: 0,
        mouse_endpoint: 0,
    };

    // 14. Scan ports and configure HID devices
    if let Err(e) = scan_ports(&mut xhci_state) {
        crate::serial_println!("[xhci] Port scanning error: {}", e);
    }

    // 15. Now that all port enumeration is complete, queue initial HID transfers.
    // This must happen AFTER scan_ports so that transfer completion events
    // don't get consumed by wait_for_event during command processing.
    start_hid_polling(&xhci_state);

    crate::serial_println!(
        "[xhci] Initialization complete: kbd_slot={} kbd_ep={} mouse_slot={} mouse_ep={}",
        xhci_state.kbd_slot,
        xhci_state.kbd_endpoint,
        xhci_state.mouse_slot,
        xhci_state.mouse_endpoint,
    );

    // Store the final state
    unsafe {
        *(&raw mut XHCI_STATE) = Some(xhci_state);
    }
    XHCI_INITIALIZED.store(true, Ordering::Release);

    Ok(())
}

// =============================================================================
// Interrupt Handling
// =============================================================================

/// Handle an XHCI interrupt.
///
/// Called from the GIC interrupt handler when the XHCI IRQ fires.
/// Processes all pending events on the event ring.
pub fn handle_interrupt() {
    if !XHCI_INITIALIZED.load(Ordering::Acquire) {
        return;
    }
    let _guard = XHCI_LOCK.lock();

    let state = unsafe {
        match (*(&raw const XHCI_STATE)).as_ref() {
            Some(s) => s,
            None => return,
        }
    };

    // Read and acknowledge IMAN (Interrupt Management Register)
    let ir0 = state.rt_base + 0x20;
    let iman = read32(ir0);
    if iman & 1 == 0 {
        return; // No interrupt pending on this interrupter
    }
    // Clear IP (Interrupt Pending) by writing 1 to bit 0 (W1C)
    write32(ir0, iman | 1);

    // Clear USBSTS.EINT (Event Interrupt, bit 3) - W1C
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

            let trb_type_val = trb.trb_type();
            match trb_type_val {
                trb_type::TRANSFER_EVENT => {
                    let slot = trb.slot_id();
                    let endpoint = ((trb.control >> 16) & 0x1F) as u8;
                    let cc = trb.completion_code();

                    if cc == completion_code::SUCCESS || cc == completion_code::SHORT_PACKET {
                        if slot == state.kbd_slot && endpoint == state.kbd_endpoint {
                            // Keyboard report received
                            let report_buf = &raw const KBD_REPORT_BUF;
                            dma_cache_invalidate(
                                (*report_buf).0.as_ptr(),
                                8,
                            );
                            let report = &(*report_buf).0;
                            super::hid::process_keyboard_report(report);
                            // Requeue transfer for next report
                            let _ = queue_hid_transfer(
                                state,
                                0,
                                state.kbd_slot,
                                state.kbd_endpoint,
                            );
                        } else if slot == state.mouse_slot && endpoint == state.mouse_endpoint {
                            // Mouse report received
                            let report_buf = &raw const MOUSE_REPORT_BUF;
                            dma_cache_invalidate(
                                (*report_buf).0.as_ptr(),
                                8,
                            );
                            let report = &(*report_buf).0;
                            super::hid::process_mouse_report(report);
                            // Requeue transfer for next report
                            let _ = queue_hid_transfer(
                                state,
                                1,
                                state.mouse_slot,
                                state.mouse_endpoint,
                            );
                        }
                    }
                }
                trb_type::COMMAND_COMPLETION => {
                    // Command completions during enumeration are handled by wait_for_event.
                    // Any stray completions during interrupt handling are ignored.
                }
                trb_type::PORT_STATUS_CHANGE => {
                    // Port status change - log but don't handle hot-plug for now.
                    let port_id = ((trb.control >> 24) & 0xFF) as u8;
                    crate::serial_println!(
                        "[xhci] Port status change: port={}",
                        port_id,
                    );
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

            let evt_num = EVENT_COUNT.fetch_add(1, Ordering::Relaxed);

            let trb_type_val = trb.trb_type();

            // Log the first 8 events in detail for debugging
            if evt_num < 8 {
                crate::serial_println!(
                    "[xhci-poll] event #{}: type={} slot={} ep={} cc={} param={:#x} status={:#x} ctrl={:#x}",
                    evt_num,
                    trb_type_val,
                    trb.slot_id(),
                    (trb.control >> 16) & 0x1F,
                    trb.completion_code(),
                    trb.param,
                    trb.status,
                    trb.control,
                );
            }

            match trb_type_val {
                trb_type::TRANSFER_EVENT => {
                    let slot = trb.slot_id();
                    let endpoint = ((trb.control >> 16) & 0x1F) as u8;
                    let cc = trb.completion_code();

                    // Check for interrupt endpoint failure → switch to EP0 polling
                    if slot == state.kbd_slot
                        && endpoint == state.kbd_endpoint
                        && cc == completion_code::ENDPOINT_NOT_ENABLED
                        && !EP0_POLLING_MODE.load(Ordering::Relaxed)
                    {
                        crate::serial_println!(
                            "[xhci] Interrupt EP CC=12 (Endpoint Not Enabled) — switching to EP0 GET_REPORT polling"
                        );
                        EP0_POLLING_MODE.store(true, Ordering::Release);
                    } else if cc == completion_code::SUCCESS || cc == completion_code::SHORT_PACKET {
                        // EP0 GET_REPORT completion (DCI=1) for keyboard
                        if EP0_POLLING_MODE.load(Ordering::Relaxed)
                            && slot == state.kbd_slot
                            && endpoint == 1
                        {
                            // SHORT_PACKET is for the Data Stage — data is in the buffer
                            // but we wait for the Status Stage SUCCESS to process it.
                            // SUCCESS is for the Status Stage — transfer is complete.
                            if cc == completion_code::SUCCESS {
                                KBD_EVENT_COUNT.fetch_add(1, Ordering::Relaxed);
                                EP0_STATE.store(ep0_state::IDLE, Ordering::Release);
                                EP0_STALL_POLLS.store(0, Ordering::Relaxed);

                                // Read the keyboard report from KBD_REPORT_BUF
                                let report_buf = &raw const KBD_REPORT_BUF;
                                dma_cache_invalidate((*report_buf).0.as_ptr(), 8);
                                let report = &(*report_buf).0;
                                super::hid::process_keyboard_report(report);
                            }
                            // SHORT_PACKET for Data Stage: just acknowledge, data stage done
                            // Status Stage event will follow.
                        }
                        // Interrupt endpoint keyboard event (original path)
                        else if !EP0_POLLING_MODE.load(Ordering::Relaxed)
                            && slot == state.kbd_slot
                            && endpoint == state.kbd_endpoint
                        {
                            KBD_EVENT_COUNT.fetch_add(1, Ordering::Relaxed);
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
                        } else {
                            XFER_OTHER_COUNT.fetch_add(1, Ordering::Relaxed);
                        }
                    } else {
                        XFER_OTHER_COUNT.fetch_add(1, Ordering::Relaxed);
                    }
                }
                trb_type::COMMAND_COMPLETION => {
                    // Route CC events to the EP0 reset state machine
                    let cc = trb.completion_code();
                    let current_state = EP0_STATE.load(Ordering::Relaxed);
                    match current_state {
                        ep0_state::WAIT_STOP_EP => {
                            handle_stop_ep_complete(state, state.kbd_slot, cc);
                            EP0_STALL_POLLS.store(0, Ordering::Relaxed);
                        }
                        ep0_state::WAIT_SET_DEQUEUE => {
                            handle_set_dequeue_complete(cc);
                            EP0_STALL_POLLS.store(0, Ordering::Relaxed);
                        }
                        _ => {
                            // Unexpected CC (e.g., from enumeration leftover) — ignore
                        }
                    }
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

    // EP0 GET_REPORT polling async state machine (non-blocking).
    //
    // States:
    //   IDLE → check ring space → submit GET_REPORT → XFER_PENDING
    //   IDLE → ring full → issue Stop EP → WAIT_STOP_EP
    //   XFER_PENDING → (wait for SUCCESS Transfer Event in event loop above)
    //   WAIT_STOP_EP → (CC handled in event loop) → WAIT_SET_DEQUEUE
    //   WAIT_SET_DEQUEUE → (CC handled in event loop) → IDLE
    if EP0_POLLING_MODE.load(Ordering::Relaxed) && state.kbd_slot != 0 {
        let current_state = EP0_STATE.load(Ordering::Relaxed);

        match current_state {
            ep0_state::IDLE => {
                EP0_STALL_POLLS.store(0, Ordering::Relaxed);

                // Rate-limit: only submit every 2 poll cycles (~100 Hz at 200 Hz timer)
                let skip = EP0_POLL_SKIP.fetch_add(1, Ordering::Relaxed);
                if skip % 2 == 0 {
                    let slot_idx = (state.kbd_slot - 1) as usize;
                    let enq = unsafe { TRANSFER_ENQUEUE[slot_idx] };

                    if enq + 4 >= TRANSFER_RING_SIZE {
                        // Ring nearly full — start async reset (non-blocking)
                        issue_stop_ep0(state, state.kbd_slot);
                    } else {
                        // Ring has room — submit GET_REPORT
                        submit_ep0_get_report(state, state.kbd_slot);
                        EP0_STATE.store(ep0_state::XFER_PENDING, Ordering::Release);
                    }
                }
            }
            ep0_state::XFER_PENDING | ep0_state::WAIT_STOP_EP | ep0_state::WAIT_SET_DEQUEUE => {
                // Waiting for an event — check for stuck condition.
                // If stuck for >400 polls (~2 seconds at 200Hz), force back to IDLE.
                let polls = EP0_STALL_POLLS.fetch_add(1, Ordering::Relaxed) + 1;
                if polls >= 400 {
                    EP0_STATE.store(ep0_state::IDLE, Ordering::Release);
                    EP0_STALL_POLLS.store(0, Ordering::Relaxed);
                    EP0_PENDING_STUCK_COUNT.fetch_add(1, Ordering::Relaxed);
                }
            }
            _ => {
                // Unknown state — reset to IDLE
                EP0_STATE.store(ep0_state::IDLE, Ordering::Release);
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
    if !XHCI_INITIALIZED.load(Ordering::Acquire) {
        return None;
    }
    unsafe { (*(&raw const XHCI_STATE)).as_ref().map(|s| s.irq) }
}
