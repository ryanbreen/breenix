//! EHCI (USB 2.0) Host Controller Driver
//!
//! Implements a minimal EHCI driver targeting the Intel 82801FB EHCI controller
//! at PCI slot 00:02.0 (vendor 0x8086, device 0x265c) on Parallels ARM64.
//!
//! This driver provides an alternative USB input path when the xHCI controller
//! has emulation bugs (CC=12 on interrupt endpoints, GET_REPORT echoes setup packet).
//!
//! # Architecture
//!
//! EHCI uses memory-mapped registers from BAR0:
//! - Capability Registers (base + 0x00): CAPLENGTH, HCSPARAMS, HCCPARAMS
//! - Operational Registers (base + CAPLENGTH): USBCMD, USBSTS, schedules, ports
//!
//! Two schedules:
//! - Async Schedule: circular linked list of QHs for control/bulk transfers
//! - Periodic Schedule: frame list (1024 entries, 1 per ms) for interrupt transfers
//!
//! Data structures: Queue Heads (QH) and Queue Transfer Descriptors (qTD)
//! are linked lists traversed by the HC hardware.

#![cfg(target_arch = "aarch64")]
// DMA buffers must be static mut for hardware access. The EHCI driver ensures
// single-threaded access through INITIALIZED flag and CPU0-only polling.
#![allow(static_mut_refs)]

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};

use super::descriptors::{
    class_code, descriptor_type, hid_protocol, hid_request, hid_subclass, request,
    ConfigDescriptor, DeviceDescriptor, EndpointDescriptor, InterfaceDescriptor, SetupPacket,
};

// =============================================================================
// Constants
// =============================================================================

const HHDM_BASE: u64 = 0xFFFF_0000_0000_0000;

/// Intel EHCI vendor/device for Parallels.
pub const INTEL_EHCI_VENDOR: u16 = 0x8086;
pub const INTEL_EHCI_DEVICE: u16 = 0x265c;

/// EHCI Operational Register offsets (from op_base = bar_base + CAPLENGTH).
#[allow(dead_code)]
mod op_reg {
    pub const USBCMD: u64 = 0x00;
    pub const USBSTS: u64 = 0x04;
    pub const USBINTR: u64 = 0x08;
    pub const FRINDEX: u64 = 0x0C;
    pub const CTRLDSSEGMENT: u64 = 0x10;
    pub const PERIODICLISTBASE: u64 = 0x14;
    pub const ASYNCLISTADDR: u64 = 0x18;
    pub const CONFIGFLAG: u64 = 0x40;
    pub const PORTSC_BASE: u64 = 0x44;
}

/// USBCMD bit definitions
mod usbcmd_bits {
    pub const RS: u32 = 1 << 0; // Run/Stop
    pub const HCRESET: u32 = 1 << 1; // Host Controller Reset
    pub const PSE: u32 = 1 << 4; // Periodic Schedule Enable
    pub const ASE: u32 = 1 << 5; // Async Schedule Enable
    pub const ITC_1MS: u32 = 0x08 << 16; // Interrupt Threshold = 1ms
}

/// USBSTS bit definitions
#[allow(dead_code)]
mod usbsts_bits {
    pub const USBINT: u32 = 1 << 0; // USB Interrupt (transfer complete)
    pub const USBERRINT: u32 = 1 << 1; // USB Error Interrupt
    pub const PCD: u32 = 1 << 2; // Port Change Detect
    pub const FLR: u32 = 1 << 3; // Frame List Rollover
    pub const HSE: u32 = 1 << 4; // Host System Error
    pub const IAA: u32 = 1 << 5; // Interrupt on Async Advance
    pub const HCHALTED: u32 = 1 << 12; // HC Halted
}

/// PORTSC bit definitions
#[allow(dead_code)]
mod portsc_bits {
    pub const CCS: u32 = 1 << 0; // Current Connect Status
    pub const CSC: u32 = 1 << 1; // Connect Status Change
    pub const PE: u32 = 1 << 2; // Port Enabled
    pub const PEC: u32 = 1 << 3; // Port Enable Change
    pub const PR: u32 = 1 << 8; // Port Reset
    pub const LS_MASK: u32 = 3 << 10; // Line Status
    pub const LS_K_STATE: u32 = 1 << 10; // K-state (low-speed)
    pub const PP: u32 = 1 << 12; // Port Power
    pub const PO: u32 = 1 << 13; // Port Owner (1 = companion)
}

/// QH/qTD PID codes
mod pid {
    pub const OUT: u32 = 0;
    pub const IN: u32 = 1;
    pub const SETUP: u32 = 2;
}

/// Size of the periodic frame list (1024 entries = 1024ms = ~1 second cycle).
const FRAME_LIST_SIZE: usize = 1024;

// =============================================================================
// Data Structures - QH and qTD
// =============================================================================

/// Queue Head (QH) - 48 bytes hardware, padded to 64 for alignment.
///
/// Must be 32-byte aligned. The HC traverses linked lists of QHs.
#[repr(C, align(64))]
#[derive(Clone, Copy)]
struct Qh {
    /// Horizontal Link Pointer: next QH/iTD physical address
    /// Bits 31:5 = address, 2:1 = type (01=QH), 0 = T (terminate)
    qhlp: u32,
    /// Endpoint Characteristics (DWord 1)
    characteristics: u32,
    /// Endpoint Capabilities (DWord 2)
    capabilities: u32,
    /// Current qTD Link Pointer (HC-managed)
    current_qtd: u32,
    // --- Transfer Overlay (matches qTD layout) ---
    /// Next qTD Pointer
    next_qtd: u32,
    /// Alternate Next qTD Pointer
    alt_qtd: u32,
    /// Token (status/control)
    token: u32,
    /// Buffer pointers [0..4]
    buffer: [u32; 5],
    // Pad to 64 bytes (12 DWORDs = 48 bytes, need 4 more)
    _pad: [u32; 4],
}

/// Queue Transfer Descriptor (qTD) - 32 bytes.
///
/// Must be 32-byte aligned. Linked lists of qTDs describe transfers.
#[repr(C, align(32))]
#[derive(Clone, Copy)]
struct Qtd {
    /// Next qTD Pointer (bits 31:5 = address, bit 0 = T)
    next: u32,
    /// Alternate Next qTD Pointer
    alt_next: u32,
    /// Token: status, PID, error count, transfer length, IOC, data toggle
    token: u32,
    /// Buffer page pointers [0..4]
    /// buffer[0] bits 11:0 = current byte offset in first page
    buffer: [u32; 5],
    // Pad to 32 bytes (7 DWORDs = 28 bytes, need 1 more)
    _pad: u32,
}

const QH_TYPE: u32 = 0b01 << 1; // Type field = QH
const T_BIT: u32 = 1; // Terminate bit

// =============================================================================
// Static DMA Buffers
// =============================================================================

/// Periodic frame list: 1024 u32 entries, page-aligned.
#[repr(C, align(4096))]
struct FrameList([u32; FRAME_LIST_SIZE]);

/// Async schedule head QH (circular, self-referencing).
static mut ASYNC_HEAD_QH: Qh = zero_qh();

/// Control transfer QH (linked into async schedule).
static mut CONTROL_QH: Qh = zero_qh();

/// Control transfer qTDs (setup + data + status).
static mut CONTROL_QTDS: [Qtd; 3] = [zero_qtd(); 3];

/// Interrupt QH for keyboard (linked into periodic schedule).
static mut INTERRUPT_QH: Qh = zero_qh();

/// Interrupt qTD for keyboard.
static mut INTERRUPT_QTD: Qtd = zero_qtd();

/// Periodic frame list.
static mut FRAME_LIST: FrameList = FrameList([T_BIT; FRAME_LIST_SIZE]);

/// Setup packet buffer for control transfers.
#[repr(C, align(64))]
struct SetupBuf([u8; 8]);
static mut SETUP_BUF: SetupBuf = SetupBuf([0; 8]);

/// Data buffer for control transfers (256 bytes should cover all descriptors).
#[repr(C, align(64))]
struct DataBuf([u8; 256]);
static mut DATA_BUF: DataBuf = DataBuf([0; 256]);

/// Keyboard report buffer (8 bytes for boot protocol).
#[repr(C, align(64))]
struct ReportBuf([u8; 8]);
static mut REPORT_BUF: ReportBuf = ReportBuf([0; 8]);

// =============================================================================
// Driver State
// =============================================================================

/// EHCI driver state.
#[allow(dead_code)] // bar_base and addr64 retained for future use
struct EhciState {
    /// MMIO base (virtual) = HHDM_BASE + BAR0 physical.
    bar_base: u64,
    /// Operational registers base = bar_base + caplength.
    op_base: u64,
    /// Number of physical downstream ports.
    n_ports: u8,
    /// Whether 64-bit addressing is supported.
    addr64: bool,
    /// Device address assigned to keyboard device (0 = none).
    kbd_addr: u8,
    /// Keyboard endpoint number (from endpoint descriptor).
    kbd_ep: u8,
    /// Keyboard max packet size.
    kbd_max_pkt: u16,
    /// Keyboard polling interval in frames (ms).
    kbd_interval: u8,
    /// Whether keyboard interrupt polling is active.
    kbd_polling: bool,
}

static INITIALIZED: AtomicBool = AtomicBool::new(false);
static mut EHCI_STATE: Option<EhciState> = None;

/// Next device address to assign (1-127).
static NEXT_ADDR: AtomicU8 = AtomicU8::new(1);

/// Counter: interrupt qTD completions.
pub static EHCI_INT_COMPLETIONS: AtomicU32 = AtomicU32::new(0);
/// Counter: control transfer completions.
pub static EHCI_CTL_COMPLETIONS: AtomicU32 = AtomicU32::new(0);
/// Counter: control transfer errors.
pub static EHCI_CTL_ERRORS: AtomicU32 = AtomicU32::new(0);

// =============================================================================
// Const initializers
// =============================================================================

const fn zero_qh() -> Qh {
    Qh {
        qhlp: T_BIT,
        characteristics: 0,
        capabilities: 0,
        current_qtd: 0,
        next_qtd: T_BIT,
        alt_qtd: T_BIT,
        token: 0,
        buffer: [0; 5],
        _pad: [0; 4],
    }
}

const fn zero_qtd() -> Qtd {
    Qtd {
        next: T_BIT,
        alt_next: T_BIT,
        token: 0,
        buffer: [0; 5],
        _pad: 0,
    }
}

// =============================================================================
// Memory Helpers (same as xhci.rs)
// =============================================================================

#[inline]
fn virt_to_phys(virt: u64) -> u64 {
    if virt >= HHDM_BASE {
        virt - HHDM_BASE
    } else {
        virt
    }
}

#[inline]
fn dma_cache_clean(ptr: *const u8, len: usize) {
    const CL: usize = 64;
    let start = ptr as usize & !(CL - 1);
    let end = (ptr as usize + len + CL - 1) & !(CL - 1);
    for addr in (start..end).step_by(CL) {
        unsafe {
            core::arch::asm!("dc cvac, {}", in(reg) addr, options(nostack));
        }
    }
    unsafe {
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }
}

#[inline]
fn dma_cache_invalidate(ptr: *const u8, len: usize) {
    const CL: usize = 64;
    let start = ptr as usize & !(CL - 1);
    let end = (ptr as usize + len + CL - 1) & !(CL - 1);
    for addr in (start..end).step_by(CL) {
        unsafe {
            core::arch::asm!("dc civac, {}", in(reg) addr, options(nostack));
        }
    }
    unsafe {
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }
}

#[inline]
fn read32(addr: u64) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

#[inline]
fn write32(addr: u64, val: u32) {
    unsafe { core::ptr::write_volatile(addr as *mut u32, val) }
}

/// Small busy-wait delay (~1ms per count at ~1GHz).
#[inline]
fn delay_ms(ms: u32) {
    for _ in 0..ms {
        for _ in 0..200_000u32 {
            core::hint::spin_loop();
        }
    }
}

// =============================================================================
// Initialization
// =============================================================================

/// Initialize the EHCI controller from a PCI device reference.
///
/// Performs the full EHCI initialization sequence:
/// 1. Enable PCI bus master + memory space
/// 2. Map BAR0
/// 3. Read capabilities
/// 4. BIOS handoff (USBLEGSUP)
/// 5. Reset controller
/// 6. Set up async + periodic schedules
/// 7. Start controller
/// 8. Enumerate ports and devices
pub fn init(pci_dev: &crate::drivers::pci::Device) -> Result<(), &'static str> {
    use crate::serial_println;

    serial_println!("[ehci] Initializing EHCI USB 2.0 controller...");
    serial_println!(
        "[ehci] PCI {:02x}:{:02x}.{} [{:04x}:{:04x}]",
        pci_dev.bus,
        pci_dev.device,
        pci_dev.function,
        pci_dev.vendor_id,
        pci_dev.device_id,
    );

    // 1. Enable PCI
    pci_dev.enable_bus_master();
    pci_dev.enable_memory_space();

    // 2. Map BAR0
    let bar = pci_dev.get_mmio_bar().ok_or("EHCI: no MMIO BAR")?;
    serial_println!(
        "[ehci] BAR0: phys={:#010x} size={:#x}",
        bar.address,
        bar.size
    );
    let bar_base = HHDM_BASE + bar.address;

    // 3. Read capability registers
    let caplength = read32(bar_base) & 0xFF;
    let hciversion = (read32(bar_base) >> 16) & 0xFFFF;
    let hcsparams = read32(bar_base + 0x04);
    let hccparams = read32(bar_base + 0x08);

    let n_ports = (hcsparams & 0xF) as u8;
    let n_cc = ((hcsparams >> 8) & 0xF) as u8;
    let n_pcc = ((hcsparams >> 12) & 0xF) as u8;
    let addr64 = (hccparams & 1) != 0;
    let eecp = ((hccparams >> 8) & 0xFF) as u8;

    let op_base = bar_base + caplength as u64;

    serial_println!(
        "[ehci] Capabilities: version={:#06x} caplength={} ports={} cc={} pcc={} 64bit={} eecp={:#x}",
        hciversion, caplength, n_ports, n_cc, n_pcc, addr64, eecp,
    );

    // 4. BIOS handoff via EECP/USBLEGSUP
    if eecp != 0 {
        bios_handoff(pci_dev, eecp);
    }

    // 5. Reset controller
    // Stop first
    let cmd = read32(op_base + op_reg::USBCMD);
    if cmd & usbcmd_bits::RS != 0 {
        write32(op_base + op_reg::USBCMD, cmd & !usbcmd_bits::RS);
        // Wait for halted
        for _ in 0..100 {
            if read32(op_base + op_reg::USBSTS) & usbsts_bits::HCHALTED != 0 {
                break;
            }
            delay_ms(1);
        }
    }

    // Issue reset
    write32(op_base + op_reg::USBCMD, usbcmd_bits::HCRESET);
    for _ in 0..100 {
        if read32(op_base + op_reg::USBCMD) & usbcmd_bits::HCRESET == 0 {
            break;
        }
        delay_ms(1);
    }
    if read32(op_base + op_reg::USBCMD) & usbcmd_bits::HCRESET != 0 {
        serial_println!("[ehci] WARNING: reset did not complete");
    }
    serial_println!("[ehci] Controller reset complete");

    // 6. Set up data structures
    // 6a. 4GB segment selector = 0 (all DMA in low 4GB)
    if addr64 {
        write32(op_base + op_reg::CTRLDSSEGMENT, 0);
    }

    // 6b. Initialize periodic frame list (all terminate initially)
    unsafe {
        for entry in FRAME_LIST.0.iter_mut() {
            *entry = T_BIT;
        }
        dma_cache_clean(FRAME_LIST.0.as_ptr() as *const u8, FRAME_LIST_SIZE * 4);
    }

    // 6c. Set periodic list base
    let frame_list_phys = virt_to_phys(unsafe { FRAME_LIST.0.as_ptr() as u64 });
    write32(op_base + op_reg::PERIODICLISTBASE, frame_list_phys as u32);

    // 6d. Set up async schedule head QH (self-referencing circular list)
    let head_qh_phys = virt_to_phys(&raw const ASYNC_HEAD_QH as u64);
    unsafe {
        ASYNC_HEAD_QH.qhlp = (head_qh_phys as u32 & !0x1F) | QH_TYPE; // Point to self
        ASYNC_HEAD_QH.characteristics = (1 << 15)   // H = Head of Reclamation List
            | (2 << 12)                               // EPS = High-Speed
            | (64 << 16)                              // MaxPacketLen = 64
            | (1 << 14); // DTC = 1
        ASYNC_HEAD_QH.capabilities = 1 << 30; // Mult = 1
        ASYNC_HEAD_QH.next_qtd = T_BIT;
        ASYNC_HEAD_QH.alt_qtd = T_BIT;
        ASYNC_HEAD_QH.token = 0; // Not active
        dma_cache_clean(
            &raw const ASYNC_HEAD_QH as *const u8,
            core::mem::size_of::<Qh>(),
        );
    }

    write32(op_base + op_reg::ASYNCLISTADDR, head_qh_phys as u32);

    // 7. Clear all status bits, configure interrupts
    write32(op_base + op_reg::USBSTS, 0x3F); // Write-clear all status bits
                                             // We'll use polling, so disable all interrupts
    write32(op_base + op_reg::USBINTR, 0);

    // 8. Start controller: enable both schedules, run
    let cmd = usbcmd_bits::ITC_1MS | usbcmd_bits::PSE | usbcmd_bits::ASE | usbcmd_bits::RS;
    write32(op_base + op_reg::USBCMD, cmd);

    // Wait for not halted
    for _ in 0..100 {
        if read32(op_base + op_reg::USBSTS) & usbsts_bits::HCHALTED == 0 {
            break;
        }
        delay_ms(1);
    }
    if read32(op_base + op_reg::USBSTS) & usbsts_bits::HCHALTED != 0 {
        serial_println!("[ehci] WARNING: controller did not start (still halted)");
    }

    // 9. Set Configure Flag (routes all ports to EHCI)
    write32(op_base + op_reg::CONFIGFLAG, 1);
    delay_ms(10); // Wait for port routing

    serial_println!("[ehci] Controller started, scanning ports...");

    // 10. Store state
    let mut state = EhciState {
        bar_base,
        op_base,
        n_ports,
        addr64,
        kbd_addr: 0,
        kbd_ep: 0,
        kbd_max_pkt: 0,
        kbd_interval: 0,
        kbd_polling: false,
    };

    // 11. Scan and enumerate ports
    scan_ports(&mut state);

    // 12. If keyboard found, set up periodic polling
    if state.kbd_addr != 0 {
        setup_keyboard_polling(&mut state);
    }

    serial_println!(
        "[ehci] Init complete: kbd_addr={} kbd_ep={} polling={}",
        state.kbd_addr,
        state.kbd_ep,
        state.kbd_polling,
    );

    unsafe {
        EHCI_STATE = Some(state);
    }
    INITIALIZED.store(true, Ordering::Release);

    Ok(())
}

/// BIOS handoff: claim ownership from BIOS via USBLEGSUP extended capability.
fn bios_handoff(pci_dev: &crate::drivers::pci::Device, eecp: u8) {
    use crate::drivers::pci::{pci_read_config_dword, pci_write_config_dword};
    use crate::serial_println;

    let mut offset = eecp;
    while offset != 0 {
        let cap = pci_read_config_dword(pci_dev.bus, pci_dev.device, pci_dev.function, offset);
        let cap_id = cap & 0xFF;
        let next = ((cap >> 8) & 0xFF) as u8;

        if cap_id == 0x01 {
            // USBLEGSUP: legacy support capability
            serial_println!("[ehci] USBLEGSUP at offset {:#x}: {:#010x}", offset, cap);

            // Set HC OS Owned Semaphore (bit 24)
            let val = cap | (1 << 24);
            pci_write_config_dword(pci_dev.bus, pci_dev.device, pci_dev.function, offset, val);

            // Wait for BIOS to release (bit 16 = BIOS Owned should clear)
            for _ in 0..100 {
                let cur =
                    pci_read_config_dword(pci_dev.bus, pci_dev.device, pci_dev.function, offset);
                if cur & (1 << 16) == 0 {
                    serial_println!("[ehci] BIOS handoff complete");
                    break;
                }
                delay_ms(10);
            }

            // Also clear legacy support control/status (offset+4)
            pci_write_config_dword(pci_dev.bus, pci_dev.device, pci_dev.function, offset + 4, 0);
            return;
        }
        offset = next;
    }
    serial_println!("[ehci] No USBLEGSUP capability found");
}

// =============================================================================
// Port Scanning and Device Enumeration
// =============================================================================

/// Scan all EHCI ports for connected devices.
fn scan_ports(state: &mut EhciState) {
    use crate::serial_println;

    for port in 0..state.n_ports as u64 {
        let portsc_addr = state.op_base + op_reg::PORTSC_BASE + port * 4;
        let portsc = read32(portsc_addr);

        serial_println!(
            "[ehci] Port {}: PORTSC={:#010x} CCS={} PE={} PR={} LS={} PP={} PO={}",
            port,
            portsc,
            (portsc & portsc_bits::CCS) >> 0,
            (portsc & portsc_bits::PE) >> 2,
            (portsc & portsc_bits::PR) >> 8,
            (portsc & portsc_bits::LS_MASK) >> 10,
            (portsc & portsc_bits::PP) >> 12,
            (portsc & portsc_bits::PO) >> 13,
        );

        if portsc & portsc_bits::CCS == 0 {
            continue; // No device connected
        }

        // Check line status for low-speed device
        let ls = (portsc & portsc_bits::LS_MASK) >> 10;
        if ls == 1 {
            // K-state = low-speed device, hand off to companion controller
            serial_println!("[ehci] Port {}: low-speed device, skipping", port);
            continue;
        }

        // Port reset
        serial_println!("[ehci] Port {}: resetting...", port);

        // Clear PE (write-1-to-clear status change bits, preserve power)
        let clear_bits = portsc_bits::CSC | portsc_bits::PEC;
        write32(portsc_addr, (portsc & !portsc_bits::PE) | clear_bits);

        // Assert reset
        let portsc = read32(portsc_addr);
        write32(portsc_addr, portsc | portsc_bits::PR);
        delay_ms(50); // USB spec: reset for at least 50ms

        // De-assert reset
        let portsc = read32(portsc_addr);
        write32(portsc_addr, portsc & !portsc_bits::PR);

        // Wait for reset complete (PE should become set for high-speed)
        delay_ms(10);
        let portsc = read32(portsc_addr);
        serial_println!(
            "[ehci] Port {}: after reset PORTSC={:#010x} PE={} CCS={}",
            port,
            portsc,
            (portsc & portsc_bits::PE) >> 2,
            portsc & portsc_bits::CCS,
        );

        if portsc & portsc_bits::PE == 0 {
            serial_println!(
                "[ehci] Port {}: not enabled after reset (full-speed?)",
                port
            );
            continue;
        }

        serial_println!("[ehci] Port {}: high-speed device enabled", port);

        // Enumerate the device
        match enumerate_device(state, port as u8) {
            Ok(true) => {
                serial_println!("[ehci] Port {}: keyboard found!", port);
                break; // We found what we need
            }
            Ok(false) => {
                serial_println!("[ehci] Port {}: device enumerated but no keyboard", port);
            }
            Err(e) => {
                serial_println!("[ehci] Port {}: enumeration failed: {}", port, e);
            }
        }
    }
}

/// Enumerate a device on the given port. Returns Ok(true) if a keyboard was found.
fn enumerate_device(state: &mut EhciState, _port: u8) -> Result<bool, &'static str> {
    use crate::serial_println;

    // Assign a device address
    let addr = NEXT_ADDR.fetch_add(1, Ordering::SeqCst);
    if addr > 127 {
        return Err("no more USB addresses available");
    }

    // GET_DESCRIPTOR (device, 8 bytes) to address 0 to learn max packet size
    let mut max_pkt0: u16 = 8; // Default for address 0

    serial_println!("[ehci] Getting device descriptor (8 bytes) from addr 0...");
    let setup = SetupPacket {
        bm_request_type: 0x80,
        b_request: request::GET_DESCRIPTOR,
        w_value: (descriptor_type::DEVICE as u16) << 8,
        w_index: 0,
        w_length: 8,
    };

    match control_transfer(state, 0, 0, max_pkt0, &setup, Some(8), true) {
        Ok(n) => {
            if n >= 8 {
                let data = unsafe { &DATA_BUF.0 };
                let pkt_size = data[7];
                if pkt_size > 0 {
                    max_pkt0 = pkt_size as u16;
                }
                serial_println!("[ehci] Device at addr 0: maxPacketSize0={}", max_pkt0);
            }
        }
        Err(e) => {
            serial_println!("[ehci] GET_DESCRIPTOR(8) failed: {}", e);
            return Err("GET_DESCRIPTOR(8) failed");
        }
    }

    // SET_ADDRESS
    serial_println!("[ehci] SET_ADDRESS to {}...", addr);
    let setup = SetupPacket {
        bm_request_type: 0x00,
        b_request: 0x05, // SET_ADDRESS
        w_value: addr as u16,
        w_index: 0,
        w_length: 0,
    };

    control_transfer(state, 0, 0, max_pkt0, &setup, None, false)?;
    delay_ms(10); // Post-SET_ADDRESS recovery time

    serial_println!("[ehci] Device address set to {}", addr);

    // GET_DESCRIPTOR (device, 18 bytes) at new address
    let setup = SetupPacket {
        bm_request_type: 0x80,
        b_request: request::GET_DESCRIPTOR,
        w_value: (descriptor_type::DEVICE as u16) << 8,
        w_index: 0,
        w_length: 18,
    };

    let n = control_transfer(state, addr, 0, max_pkt0, &setup, Some(18), true)?;
    if n < 18 {
        return Err("short device descriptor");
    }

    let dev_desc: DeviceDescriptor =
        unsafe { core::ptr::read_unaligned(DATA_BUF.0.as_ptr() as *const DeviceDescriptor) };

    serial_println!(
        "[ehci] Device: USB{}.{} class={:#04x} sub={:#04x} proto={:#04x} vendor={:#06x} product={:#06x}",
        { dev_desc.bcd_usb } >> 8,
        ({ dev_desc.bcd_usb } >> 4) & 0xF,
        dev_desc.b_device_class,
        dev_desc.b_device_sub_class,
        dev_desc.b_device_protocol,
        { dev_desc.id_vendor },
        { dev_desc.id_product },
    );

    // GET_DESCRIPTOR (configuration, 9 bytes first to learn total length)
    let setup = SetupPacket {
        bm_request_type: 0x80,
        b_request: request::GET_DESCRIPTOR,
        w_value: (descriptor_type::CONFIGURATION as u16) << 8,
        w_index: 0,
        w_length: 9,
    };

    let n = control_transfer(state, addr, 0, max_pkt0, &setup, Some(9), true)?;
    if n < 9 {
        return Err("short config descriptor");
    }

    let config_desc: ConfigDescriptor =
        unsafe { core::ptr::read_unaligned(DATA_BUF.0.as_ptr() as *const ConfigDescriptor) };
    let total_len = { config_desc.w_total_length } as usize;
    let config_value = config_desc.b_configuration_value;

    serial_println!(
        "[ehci] Config: totalLen={} numInterfaces={} configValue={}",
        total_len,
        config_desc.b_num_interfaces,
        config_value,
    );

    // GET_DESCRIPTOR (full configuration)
    let fetch_len = total_len.min(256);
    let setup = SetupPacket {
        bm_request_type: 0x80,
        b_request: request::GET_DESCRIPTOR,
        w_value: (descriptor_type::CONFIGURATION as u16) << 8,
        w_index: 0,
        w_length: fetch_len as u16,
    };

    let n = control_transfer(
        state,
        addr,
        0,
        max_pkt0,
        &setup,
        Some(fetch_len as u32),
        true,
    )?;

    // Parse configuration for HID keyboard interface
    let config_data = unsafe { &DATA_BUF.0[..n as usize] };
    let mut found_keyboard = false;
    let mut kbd_ep_addr: u8 = 0;
    let mut kbd_max_pkt: u16 = 8;
    let mut kbd_interval: u8 = 10;
    let mut kbd_iface: u8 = 0;

    // Walk descriptors
    let mut offset = 0;
    while offset + 2 <= config_data.len() {
        let d_len = config_data[offset] as usize;
        let d_type = config_data[offset + 1];

        if d_len < 2 || offset + d_len > config_data.len() {
            break;
        }

        if d_type == descriptor_type::INTERFACE && d_len >= 9 {
            let iface: InterfaceDescriptor = unsafe {
                core::ptr::read_unaligned(
                    config_data.as_ptr().add(offset) as *const InterfaceDescriptor
                )
            };
            serial_println!(
                "[ehci] Interface {}: class={:#04x} sub={:#04x} proto={:#04x} eps={}",
                iface.b_interface_number,
                iface.b_interface_class,
                iface.b_interface_sub_class,
                iface.b_interface_protocol,
                iface.b_num_endpoints,
            );

            if iface.b_interface_class == class_code::HID
                && iface.b_interface_sub_class == hid_subclass::BOOT
                && iface.b_interface_protocol == hid_protocol::KEYBOARD
            {
                serial_println!(
                    "[ehci] Found HID boot keyboard on interface {}",
                    iface.b_interface_number
                );
                found_keyboard = true;
                kbd_iface = iface.b_interface_number;
            }
        }

        if d_type == descriptor_type::ENDPOINT && d_len >= 7 && found_keyboard && kbd_ep_addr == 0 {
            let ep: EndpointDescriptor = unsafe {
                core::ptr::read_unaligned(
                    config_data.as_ptr().add(offset) as *const EndpointDescriptor
                )
            };

            if ep.is_interrupt() && ep.is_in() {
                kbd_ep_addr = ep.b_endpoint_address;
                kbd_max_pkt = { ep.w_max_packet_size } & 0x7FF;
                kbd_interval = ep.b_interval;
                serial_println!(
                    "[ehci] Keyboard endpoint: addr={:#04x} maxPkt={} interval={}ms",
                    kbd_ep_addr,
                    kbd_max_pkt,
                    kbd_interval,
                );
            }
        }

        offset += d_len;
    }

    if !found_keyboard {
        return Ok(false);
    }

    // SET_CONFIGURATION
    serial_println!("[ehci] SET_CONFIGURATION({})", config_value);
    let setup = SetupPacket {
        bm_request_type: 0x00,
        b_request: request::SET_CONFIGURATION,
        w_value: config_value as u16,
        w_index: 0,
        w_length: 0,
    };
    control_transfer(state, addr, 0, max_pkt0, &setup, None, false)?;

    // SET_IDLE (HID class request) - silence idle reports
    serial_println!("[ehci] SET_IDLE on interface {}", kbd_iface);
    let setup = SetupPacket {
        bm_request_type: 0x21, // Class, Interface, Host-to-Device
        b_request: hid_request::SET_IDLE,
        w_value: 0, // Indefinite idle
        w_index: kbd_iface as u16,
        w_length: 0,
    };
    let _ = control_transfer(state, addr, 0, max_pkt0, &setup, None, false);

    // SET_PROTOCOL (boot protocol)
    serial_println!("[ehci] SET_PROTOCOL(boot) on interface {}", kbd_iface);
    let setup = SetupPacket {
        bm_request_type: 0x21,
        b_request: hid_request::SET_PROTOCOL,
        w_value: 0, // 0 = boot protocol
        w_index: kbd_iface as u16,
        w_length: 0,
    };
    control_transfer(state, addr, 0, max_pkt0, &setup, None, false)?;

    // Store keyboard info
    state.kbd_addr = addr;
    state.kbd_ep = kbd_ep_addr & 0x0F;
    state.kbd_max_pkt = kbd_max_pkt;
    state.kbd_interval = kbd_interval;

    serial_println!(
        "[ehci] Keyboard configured: addr={} ep={} maxPkt={} interval={}ms",
        addr,
        state.kbd_ep,
        kbd_max_pkt,
        kbd_interval,
    );

    Ok(true)
}

// =============================================================================
// Control Transfers (Async Schedule)
// =============================================================================

/// Execute a USB control transfer via the async schedule.
///
/// Returns the number of bytes transferred in the data stage (0 if no data stage).
fn control_transfer(
    _state: &EhciState,
    dev_addr: u8,
    endpoint: u8,
    max_pkt: u16,
    setup: &SetupPacket,
    data_len: Option<u32>,
    data_in: bool,
) -> Result<u32, &'static str> {
    // Build the 8-byte setup packet in DMA buffer
    unsafe {
        let setup_bytes = core::slice::from_raw_parts(setup as *const SetupPacket as *const u8, 8);
        SETUP_BUF.0.copy_from_slice(setup_bytes);
        dma_cache_clean(SETUP_BUF.0.as_ptr(), 8);
    }

    let setup_phys = virt_to_phys(unsafe { SETUP_BUF.0.as_ptr() as u64 });
    let data_phys = virt_to_phys(unsafe { DATA_BUF.0.as_ptr() as u64 });

    // Clear data buffer
    if data_len.is_some() {
        unsafe {
            DATA_BUF.0.fill(0);
            dma_cache_clean(DATA_BUF.0.as_ptr(), 256);
        }
    }

    let has_data = data_len.is_some() && data_len.unwrap() > 0;
    let xfer_len = data_len.unwrap_or(0);

    unsafe {
        // Build SETUP qTD
        CONTROL_QTDS[0] = zero_qtd();
        CONTROL_QTDS[0].token = (1 << 7)           // Active
            | (pid::SETUP << 8) // PID = SETUP
            | (3 << 10)        // CERR = 3
            | (8 << 16)        // Total Bytes = 8
            | (0 << 31); // dt = 0 (SETUP always data0)
        CONTROL_QTDS[0].buffer[0] = setup_phys as u32;

        // Build DATA qTD (if needed)
        if has_data {
            let data_pid = if data_in { pid::IN } else { pid::OUT };
            CONTROL_QTDS[1] = zero_qtd();
            CONTROL_QTDS[1].token = (1 << 7)              // Active
                | (data_pid << 8)     // PID
                | (3 << 10)           // CERR = 3
                | (xfer_len << 16)    // Total Bytes
                | (1 << 31); // dt = 1 (DATA starts at data1)
            CONTROL_QTDS[1].buffer[0] = data_phys as u32;
            // Additional buffer pointers for multi-page transfers
            if xfer_len > 4096 {
                CONTROL_QTDS[1].buffer[1] = ((data_phys + 4096) & !0xFFF) as u32;
            }
        }

        // Build STATUS qTD
        let status_idx = if has_data { 2 } else { 1 };
        let status_pid = if has_data && data_in {
            pid::OUT
        } else {
            pid::IN
        };
        CONTROL_QTDS[status_idx] = zero_qtd();
        CONTROL_QTDS[status_idx].token = (1 << 7)              // Active
            | (status_pid << 8)   // PID = opposite of data direction
            | (3 << 10)           // CERR = 3
            | (0 << 16)           // Total Bytes = 0 (ZLP)
            | (1 << 15)           // IOC = 1
            | (1 << 31); // dt = 1

        // Chain: SETUP -> [DATA ->] STATUS
        let qtd0_phys = virt_to_phys(&raw const CONTROL_QTDS[0] as u64);
        let qtd1_phys = virt_to_phys(&raw const CONTROL_QTDS[1] as u64);
        let qtd2_phys = virt_to_phys(&raw const CONTROL_QTDS[2] as u64);

        if has_data {
            CONTROL_QTDS[0].next = (qtd1_phys as u32) & !0x1F;
            CONTROL_QTDS[0].alt_next = T_BIT;
            CONTROL_QTDS[1].next = (qtd2_phys as u32) & !0x1F;
            CONTROL_QTDS[1].alt_next = T_BIT;
            CONTROL_QTDS[2].next = T_BIT;
            CONTROL_QTDS[2].alt_next = T_BIT;
        } else {
            CONTROL_QTDS[0].next = (qtd1_phys as u32) & !0x1F;
            CONTROL_QTDS[0].alt_next = T_BIT;
            CONTROL_QTDS[1].next = T_BIT;
            CONTROL_QTDS[1].alt_next = T_BIT;
        }

        // Flush qTDs to memory
        dma_cache_clean(
            CONTROL_QTDS.as_ptr() as *const u8,
            3 * core::mem::size_of::<Qtd>(),
        );

        // Set up Control QH
        CONTROL_QH = zero_qh();
        let head_phys = virt_to_phys(&raw const ASYNC_HEAD_QH as u64);
        CONTROL_QH.qhlp = (head_phys as u32 & !0x1F) | QH_TYPE; // Link back to head

        CONTROL_QH.characteristics = (dev_addr as u32)          // Device Address
            | ((endpoint as u32) << 8) // Endpoint
            | (2 << 12)               // EPS = High-Speed
            | (1 << 14)               // DTC = 1 (data toggle from qTD)
            | ((max_pkt as u32) << 16); // Max Packet Length

        CONTROL_QH.capabilities = 1 << 30; // Mult = 1

        // Point overlay to first qTD
        CONTROL_QH.next_qtd = (qtd0_phys as u32) & !0x1F;
        CONTROL_QH.alt_qtd = T_BIT;
        CONTROL_QH.token = 0; // Clear overlay token

        dma_cache_clean(
            &raw const CONTROL_QH as *const u8,
            core::mem::size_of::<Qh>(),
        );

        // Insert Control QH into async schedule: HEAD -> CONTROL -> HEAD
        let control_phys = virt_to_phys(&raw const CONTROL_QH as u64);
        ASYNC_HEAD_QH.qhlp = (control_phys as u32 & !0x1F) | QH_TYPE;
        dma_cache_clean(
            &raw const ASYNC_HEAD_QH as *const u8,
            core::mem::size_of::<Qh>(),
        );
    }

    // Wait for completion (poll status qTD)
    let status_idx = if has_data { 2usize } else { 1usize };
    let timeout_ms = 500;
    let mut completed = false;

    for _ in 0..timeout_ms {
        unsafe {
            dma_cache_invalidate(
                &raw const CONTROL_QTDS[status_idx] as *const u8,
                core::mem::size_of::<Qtd>(),
            );
        }
        let token = unsafe { core::ptr::read_volatile(&raw const CONTROL_QTDS[status_idx].token) };

        if token & (1 << 7) == 0 {
            // Active bit cleared - transfer complete
            completed = true;

            // Check for errors
            let status = token & 0x7E; // Halted, DBE, Babble, Xact Err, Missed uF, STS
            if status != 0 {
                EHCI_CTL_ERRORS.fetch_add(1, Ordering::Relaxed);
                // Remove control QH from async schedule
                unsafe {
                    let head_phys = virt_to_phys(&raw const ASYNC_HEAD_QH as u64);
                    ASYNC_HEAD_QH.qhlp = (head_phys as u32 & !0x1F) | QH_TYPE;
                    dma_cache_clean(
                        &raw const ASYNC_HEAD_QH as *const u8,
                        core::mem::size_of::<Qh>(),
                    );
                }
                if token & (1 << 6) != 0 {
                    return Err("EHCI: qTD halted");
                }
                if token & (1 << 5) != 0 {
                    return Err("EHCI: data buffer error");
                }
                if token & (1 << 4) != 0 {
                    return Err("EHCI: babble detected");
                }
                if token & (1 << 3) != 0 {
                    return Err("EHCI: transaction error");
                }
                return Err("EHCI: unknown qTD error");
            }
            break;
        }
        delay_ms(1);
    }

    // Remove control QH from async schedule
    unsafe {
        let head_phys = virt_to_phys(&raw const ASYNC_HEAD_QH as u64);
        ASYNC_HEAD_QH.qhlp = (head_phys as u32 & !0x1F) | QH_TYPE;
        dma_cache_clean(
            &raw const ASYNC_HEAD_QH as *const u8,
            core::mem::size_of::<Qh>(),
        );
    }

    if !completed {
        EHCI_CTL_ERRORS.fetch_add(1, Ordering::Relaxed);
        return Err("EHCI: control transfer timeout");
    }

    EHCI_CTL_COMPLETIONS.fetch_add(1, Ordering::Relaxed);

    // Read back data if data stage was IN
    let bytes_transferred = if has_data && data_in {
        unsafe {
            dma_cache_invalidate(DATA_BUF.0.as_ptr(), 256);
            dma_cache_invalidate(
                &raw const CONTROL_QTDS[1] as *const u8,
                core::mem::size_of::<Qtd>(),
            );
            let data_token = core::ptr::read_volatile(&raw const CONTROL_QTDS[1].token);
            let remaining = (data_token >> 16) & 0x7FFF;
            xfer_len - remaining
        }
    } else {
        0
    };

    Ok(bytes_transferred)
}

// =============================================================================
// Periodic Schedule - Keyboard Interrupt Polling
// =============================================================================

/// Set up the interrupt QH and qTD for keyboard polling via the periodic schedule.
fn setup_keyboard_polling(state: &mut EhciState) {
    use crate::serial_println;

    if state.kbd_addr == 0 || state.kbd_ep == 0 {
        return;
    }

    let interval = (state.kbd_interval as usize).max(1).min(FRAME_LIST_SIZE);

    unsafe {
        // Set up interrupt QH
        INTERRUPT_QH = zero_qh();
        INTERRUPT_QH.qhlp = T_BIT; // Terminate (not circular in periodic schedule)

        INTERRUPT_QH.characteristics = (state.kbd_addr as u32)       // Device Address
            | ((state.kbd_ep as u32) << 8) // Endpoint Number
            | (2 << 12)                    // EPS = High-Speed
            | (0 << 14)                    // DTC = 0 (HC manages data toggle!)
            | ((state.kbd_max_pkt as u32) << 16); // Max Packet Length
                                                  // RL = 0 (must be 0 for periodic QHs)

        INTERRUPT_QH.capabilities = (1 << 30)    // Mult = 1
            | 0x01; // S-Mask = 0x01 (poll in microframe 0)

        // Set up interrupt qTD
        INTERRUPT_QTD = zero_qtd();
        let report_phys = virt_to_phys(REPORT_BUF.0.as_ptr() as u64);

        REPORT_BUF.0.fill(0);
        dma_cache_clean(REPORT_BUF.0.as_ptr(), 8);

        INTERRUPT_QTD.token = (1 << 7)               // Active
            | (pid::IN << 8)       // PID = IN
            | (3 << 10)            // CERR = 3
            | (8 << 16)            // Total Bytes = 8 (keyboard boot report)
            | (1 << 15); // IOC = 1

        INTERRUPT_QTD.buffer[0] = report_phys as u32;
        INTERRUPT_QTD.next = T_BIT;
        INTERRUPT_QTD.alt_next = T_BIT;

        dma_cache_clean(
            &raw const INTERRUPT_QTD as *const u8,
            core::mem::size_of::<Qtd>(),
        );

        // Point QH overlay to our qTD
        let qtd_phys = virt_to_phys(&raw const INTERRUPT_QTD as u64);
        INTERRUPT_QH.next_qtd = (qtd_phys as u32) & !0x1F;
        INTERRUPT_QH.alt_qtd = T_BIT;
        INTERRUPT_QH.token = 0; // Not active yet (overlay inactive)

        dma_cache_clean(
            &raw const INTERRUPT_QH as *const u8,
            core::mem::size_of::<Qh>(),
        );

        // Link QH into periodic frame list at every `interval` entries
        let qh_phys = virt_to_phys(&raw const INTERRUPT_QH as u64);
        let frame_entry = (qh_phys as u32 & !0x1F) | QH_TYPE; // Type = QH

        for i in (0..FRAME_LIST_SIZE).step_by(interval) {
            FRAME_LIST.0[i] = frame_entry;
        }
        dma_cache_clean(FRAME_LIST.0.as_ptr() as *const u8, FRAME_LIST_SIZE * 4);
    }

    state.kbd_polling = true;

    serial_println!(
        "[ehci] Keyboard polling: QH linked every {}ms, endpoint IN{}",
        interval,
        state.kbd_ep,
    );
}

/// Poll for keyboard events. Called from the timer interrupt handler.
///
/// Checks if the interrupt qTD completed. If so, processes the keyboard report
/// and resubmits the qTD.
pub fn poll_keyboard() {
    if !INITIALIZED.load(Ordering::Acquire) {
        return;
    }

    let state = unsafe {
        match &EHCI_STATE {
            Some(s) => s,
            None => return,
        }
    };

    if !state.kbd_polling {
        return;
    }

    // Check interrupt qTD token
    dma_cache_invalidate(
        &raw const INTERRUPT_QTD as *const u8,
        core::mem::size_of::<Qtd>(),
    );

    let token = unsafe { core::ptr::read_volatile(&raw const INTERRUPT_QTD.token) };

    // Still active? Nothing to do.
    if token & (1 << 7) != 0 {
        return;
    }

    // qTD completed. Check status.
    let status = token & 0x7E;
    let remaining = (token >> 16) & 0x7FFF;
    let transferred = 8u32.saturating_sub(remaining);

    EHCI_INT_COMPLETIONS.fetch_add(1, Ordering::Relaxed);

    if status == 0 && transferred > 0 {
        // Read keyboard report
        unsafe {
            dma_cache_invalidate(REPORT_BUF.0.as_ptr(), 8);
        }

        let report = unsafe { &REPORT_BUF.0 };

        // Update diagnostic counters
        let any_nonzero = report.iter().any(|&b| b != 0);
        if any_nonzero {
            super::hid::NONZERO_KBD_COUNT.fetch_add(1, Ordering::Relaxed);
        }

        // Pack report into u64 for heartbeat display
        let mut packed: u64 = 0;
        for i in 0..8 {
            packed |= (report[i] as u64) << (i * 8);
        }
        super::hid::LAST_KBD_REPORT_U64.store(packed, Ordering::Relaxed);

        // Route to HID keyboard processing
        super::hid::process_keyboard_report(report);
    }

    // Resubmit: reinitialize qTD and re-link into QH
    unsafe {
        REPORT_BUF.0.fill(0);
        dma_cache_clean(REPORT_BUF.0.as_ptr(), 8);

        let report_phys = virt_to_phys(REPORT_BUF.0.as_ptr() as u64);

        INTERRUPT_QTD.next = T_BIT;
        INTERRUPT_QTD.alt_next = T_BIT;
        INTERRUPT_QTD.token = (1 << 7)               // Active
            | (pid::IN << 8)       // PID = IN
            | (3 << 10)            // CERR = 3
            | (8 << 16)            // Total Bytes = 8
            | (1 << 15); // IOC = 1
        INTERRUPT_QTD.buffer[0] = report_phys as u32;

        dma_cache_clean(
            &raw const INTERRUPT_QTD as *const u8,
            core::mem::size_of::<Qtd>(),
        );

        // Re-link qTD into QH overlay
        let qtd_phys = virt_to_phys(&raw const INTERRUPT_QTD as u64);
        INTERRUPT_QH.next_qtd = (qtd_phys as u32) & !0x1F;
        INTERRUPT_QH.alt_qtd = T_BIT;
        INTERRUPT_QH.token = 0; // Clear overlay (inactive, HC will pick up next_qtd)

        dma_cache_clean(
            &raw const INTERRUPT_QH as *const u8,
            core::mem::size_of::<Qh>(),
        );
    }
}

/// Check if EHCI keyboard polling is active.
pub fn is_keyboard_active() -> bool {
    INITIALIZED.load(Ordering::Acquire)
        && unsafe { EHCI_STATE.as_ref().map_or(false, |s| s.kbd_polling) }
}
