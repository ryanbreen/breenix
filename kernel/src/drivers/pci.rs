//! PCI Bus Enumeration and Device Discovery
//!
//! This module provides PCI configuration space access and device enumeration
//! for discovering and initializing PCI devices.
//!
//! # Architecture
//!
//! PCI uses two I/O ports for configuration space access:
//! - CONFIG_ADDRESS (0xCF8): Write the address of the config register to read/write
//! - CONFIG_DATA (0xCFC): Read/write the configuration data
//!
//! The address format is:
//! ```text
//! Bit 31    : Enable bit (must be 1)
//! Bits 23-16: Bus number (0-255)
//! Bits 15-11: Device number (0-31)
//! Bits 10-8 : Function number (0-7)
//! Bits 7-2  : Register offset (32-bit aligned)
//! Bits 1-0  : Must be 0
//! ```

use alloc::vec::Vec;
use core::{fmt, sync::atomic::AtomicBool};
use spin::Mutex;
#[cfg(target_arch = "x86_64")]
use x86_64::instructions::port::Port;

/// PCI configuration address port (x86 only - ARM64 uses ECAM)
#[cfg(target_arch = "x86_64")]
const CONFIG_ADDRESS: u16 = 0xCF8;
/// PCI configuration data port (x86 only - ARM64 uses ECAM)
#[cfg(target_arch = "x86_64")]
const CONFIG_DATA: u16 = 0xCFC;

/// Maximum number of PCI buses to scan (x86 only; ARM64 uses platform_config bus range)
#[cfg(not(target_arch = "aarch64"))]
const MAX_BUS: u8 = 255;
/// Maximum number of devices per bus
const MAX_DEVICE: u8 = 32;
/// Maximum number of functions per device
const MAX_FUNCTION: u8 = 8;

/// VirtIO vendor ID
pub const VIRTIO_VENDOR_ID: u16 = 0x1AF4;
/// VirtIO block device ID (legacy)
pub const VIRTIO_BLOCK_DEVICE_ID_LEGACY: u16 = 0x1001;
/// VirtIO block device ID (modern)
pub const VIRTIO_BLOCK_DEVICE_ID_MODERN: u16 = 0x1042;
/// VirtIO sound device ID (legacy)
pub const VIRTIO_SOUND_DEVICE_ID_LEGACY: u16 = 0x1019;
/// VirtIO sound device ID (modern)
pub const VIRTIO_SOUND_DEVICE_ID_MODERN: u16 = 0x1059;
/// VirtIO network device ID (legacy)
pub const VIRTIO_NET_DEVICE_ID_LEGACY: u16 = 0x1000;
/// VirtIO network device ID (modern)
pub const VIRTIO_NET_DEVICE_ID_MODERN: u16 = 0x1041;
/// VirtIO GPU device ID (modern only, no legacy transitional)
pub const VIRTIO_GPU_DEVICE_ID_MODERN: u16 = 0x1050;

/// PCI Capability ID for MSI
pub const PCI_CAP_ID_MSI: u8 = 0x05;
/// PCI Capability ID for MSI-X
pub const PCI_CAP_ID_MSIX: u8 = 0x11;

/// Intel vendor ID (for reference - common in QEMU)
pub const INTEL_VENDOR_ID: u16 = 0x8086;
/// Red Hat / QEMU vendor ID
pub const QEMU_VENDOR_ID: u16 = 0x1B36;

/// PCI device class codes
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[repr(u8)]
pub enum DeviceClass {
    Legacy = 0x00,
    MassStorage = 0x01,
    Network = 0x02,
    Display = 0x03,
    Multimedia = 0x04,
    Memory = 0x05,
    Bridge = 0x06,
    SimpleCommunication = 0x07,
    BaseSystemPeripheral = 0x08,
    InputDevice = 0x09,
    DockingStation = 0x0A,
    Processor = 0x0B,
    SerialBus = 0x0C,
    Wireless = 0x0D,
    IntelligentIO = 0x0E,
    SatelliteCommunication = 0x0F,
    Encryption = 0x10,
    SignalProcessing = 0x11,
    Unknown = 0xFF,
}

impl DeviceClass {
    fn from_u8(value: u8) -> Self {
        match value {
            0x00 => DeviceClass::Legacy,
            0x01 => DeviceClass::MassStorage,
            0x02 => DeviceClass::Network,
            0x03 => DeviceClass::Display,
            0x04 => DeviceClass::Multimedia,
            0x05 => DeviceClass::Memory,
            0x06 => DeviceClass::Bridge,
            0x07 => DeviceClass::SimpleCommunication,
            0x08 => DeviceClass::BaseSystemPeripheral,
            0x09 => DeviceClass::InputDevice,
            0x0A => DeviceClass::DockingStation,
            0x0B => DeviceClass::Processor,
            0x0C => DeviceClass::SerialBus,
            0x0D => DeviceClass::Wireless,
            0x0E => DeviceClass::IntelligentIO,
            0x0F => DeviceClass::SatelliteCommunication,
            0x10 => DeviceClass::Encryption,
            0x11 => DeviceClass::SignalProcessing,
            _ => DeviceClass::Unknown,
        }
    }
}

/// Base Address Register (BAR) information
#[derive(Debug, Copy, Clone)]
pub struct Bar {
    /// Physical address of the BAR
    pub address: u64,
    /// Size of the BAR region in bytes
    pub size: u64,
    /// Whether this is an I/O port BAR (vs memory-mapped)
    pub is_io: bool,
    /// Whether this is a 64-bit BAR (occupies two BAR slots)
    #[allow(dead_code)] // Part of public API, will be used by VirtIO driver
    pub is_64bit: bool,
    /// Whether the memory is prefetchable
    #[allow(dead_code)] // Part of public API, will be used by VirtIO driver
    pub prefetchable: bool,
}

impl Bar {
    /// Create an empty/invalid BAR
    const fn empty() -> Self {
        Bar {
            address: 0,
            size: 0,
            is_io: false,
            is_64bit: false,
            prefetchable: false,
        }
    }

    /// Check if this BAR is valid (has non-zero size)
    pub fn is_valid(&self) -> bool {
        self.size > 0
    }
}

/// Represents a PCI device
#[derive(Clone)]
pub struct Device {
    /// Bus number (0-255)
    pub bus: u8,
    /// Device/slot number (0-31)
    pub device: u8,
    /// Function number (0-7)
    pub function: u8,
    /// Vendor ID
    pub vendor_id: u16,
    /// Device ID
    pub device_id: u16,
    /// Revision ID
    #[allow(dead_code)] // Part of public API, will be used by VirtIO driver
    pub revision_id: u8,
    /// Device class
    pub class: DeviceClass,
    /// Device subclass
    pub subclass: u8,
    /// Programming interface
    #[allow(dead_code)] // Part of public API, will be used by VirtIO driver
    pub prog_if: u8,
    /// Interrupt line
    pub interrupt_line: u8,
    /// Interrupt pin
    #[allow(dead_code)] // Part of public API, will be used by VirtIO driver
    pub interrupt_pin: u8,
    /// Whether this is a multifunction device
    pub multifunction: bool,
    /// Base Address Registers (up to 6 for standard devices)
    pub bars: [Bar; 6],
}

impl Device {
    /// Check if this is a VirtIO device
    pub fn is_virtio(&self) -> bool {
        self.vendor_id == VIRTIO_VENDOR_ID
    }

    /// Check if this is a VirtIO block device
    pub fn is_virtio_block(&self) -> bool {
        self.is_virtio()
            && (self.device_id == VIRTIO_BLOCK_DEVICE_ID_LEGACY
                || self.device_id == VIRTIO_BLOCK_DEVICE_ID_MODERN)
    }

    /// Check if this is a VirtIO network device
    pub fn is_virtio_net(&self) -> bool {
        self.is_virtio()
            && (self.device_id == VIRTIO_NET_DEVICE_ID_LEGACY
                || self.device_id == VIRTIO_NET_DEVICE_ID_MODERN)
    }

    /// Check if this is any network controller
    pub fn is_virtio_sound(&self) -> bool {
        self.is_virtio()
            && (self.device_id == VIRTIO_SOUND_DEVICE_ID_LEGACY
                || self.device_id == VIRTIO_SOUND_DEVICE_ID_MODERN)
    }

    pub fn is_network(&self) -> bool {
        self.class == DeviceClass::Network
    }

    /// Get the first valid memory-mapped BAR
    #[allow(dead_code)] // Part of public API, will be used by VirtIO driver
    pub fn get_mmio_bar(&self) -> Option<&Bar> {
        self.bars.iter().find(|bar| bar.is_valid() && !bar.is_io)
    }

    /// Get the first valid I/O port BAR
    #[allow(dead_code)] // Part of public API, will be used by VirtIO driver
    pub fn get_io_bar(&self) -> Option<&Bar> {
        self.bars.iter().find(|bar| bar.is_valid() && bar.is_io)
    }

    /// Enable bus mastering for DMA
    #[allow(dead_code)] // Part of public API, will be used by VirtIO driver
    pub fn enable_bus_master(&self) {
        let command = pci_read_config_word(self.bus, self.device, self.function, 0x04);
        // Set bit 2 (Bus Master Enable)
        pci_write_config_word(self.bus, self.device, self.function, 0x04, command | 0x04);
    }

    /// Enable memory space access
    #[allow(dead_code)] // Part of public API, will be used by VirtIO driver
    pub fn enable_memory_space(&self) {
        let command = pci_read_config_word(self.bus, self.device, self.function, 0x04);
        // Set bit 1 (Memory Space Enable)
        pci_write_config_word(self.bus, self.device, self.function, 0x04, command | 0x02);
    }

    /// Enable I/O space access
    #[allow(dead_code)] // Part of public API, will be used by VirtIO driver
    pub fn enable_io_space(&self) {
        let command = pci_read_config_word(self.bus, self.device, self.function, 0x04);
        // Set bit 0 (I/O Space Enable)
        pci_write_config_word(self.bus, self.device, self.function, 0x04, command | 0x01);
    }

    /// Disable legacy INTx interrupts (set DisINTx bit in PCI Command register).
    pub fn disable_intx(&self) {
        let command = pci_read_config_word(self.bus, self.device, self.function, 0x04);
        // Bit 10: Interrupt Disable
        pci_write_config_word(
            self.bus,
            self.device,
            self.function,
            0x04,
            command | (1 << 10),
        );
    }

    /// Enable legacy INTx interrupts (clear DisINTx bit in PCI Command register).
    pub fn enable_intx(&self) {
        let command = pci_read_config_word(self.bus, self.device, self.function, 0x04);
        pci_write_config_word(
            self.bus,
            self.device,
            self.function,
            0x04,
            command & !(1 << 10),
        );
    }

    /// Disable PCI MSI. Clears the MSI Enable bit in the MSI Message Control register.
    /// Returns true if MSI was found and disabled, false if no MSI capability exists.
    pub fn disable_msi(&self) -> bool {
        if let Some(cap_offset) = self.find_msi_capability() {
            let msg_ctrl =
                pci_read_config_word(self.bus, self.device, self.function, cap_offset + 2);
            // Clear bit 0 (MSI Enable)
            pci_write_config_word(
                self.bus,
                self.device,
                self.function,
                cap_offset + 2,
                msg_ctrl & !0x0001,
            );
            true
        } else {
            false
        }
    }

    /// Find the MSI capability in the PCI capability list.
    ///
    /// Returns the config space offset of the MSI capability, or None if not found.
    pub fn find_msi_capability(&self) -> Option<u8> {
        // Check PCI Status register bit 4: Capabilities List exists
        let status = pci_read_config_word(self.bus, self.device, self.function, 0x06);
        if (status & (1 << 4)) == 0 {
            return None;
        }

        // Capabilities pointer at offset 0x34
        let mut cap_ptr = pci_read_config_byte(self.bus, self.device, self.function, 0x34);

        while cap_ptr != 0 {
            let cap_id = pci_read_config_byte(self.bus, self.device, self.function, cap_ptr);
            if cap_id == PCI_CAP_ID_MSI {
                return Some(cap_ptr);
            }
            cap_ptr = pci_read_config_byte(self.bus, self.device, self.function, cap_ptr + 1);
        }
        None
    }

    /// Configure and enable PCI MSI with a 32-bit message address.
    ///
    /// `cap_offset`: config space offset of the MSI capability
    /// `address`: MSI target address (e.g., GICv2m doorbell register)
    /// `data`: MSI data value (e.g., SPI number)
    pub fn configure_msi(&self, cap_offset: u8, address: u32, data: u16) {
        // Read Message Control to determine capability layout
        let msg_ctrl = pci_read_config_word(self.bus, self.device, self.function, cap_offset + 2);
        let is_64bit = (msg_ctrl & (1 << 7)) != 0;
        let has_mask = (msg_ctrl & (1 << 8)) != 0;

        // Write Message Address (always at cap+4)
        pci_write_config_dword(
            self.bus,
            self.device,
            self.function,
            cap_offset + 4,
            address,
        );

        // Write Message Data
        let data_offset = if is_64bit {
            // 64-bit: upper address at cap+8, data at cap+12
            pci_write_config_dword(self.bus, self.device, self.function, cap_offset + 8, 0);
            cap_offset + 12
        } else {
            // 32-bit: data at cap+8
            cap_offset + 8
        };
        pci_write_config_word(self.bus, self.device, self.function, data_offset, data);

        // Clear mask bits if per-vector masking is supported
        if has_mask {
            let mask_offset = if is_64bit {
                cap_offset + 16
            } else {
                cap_offset + 12
            };
            pci_write_config_dword(self.bus, self.device, self.function, mask_offset, 0);
        }

        // Enable MSI (bit 0 of Message Control), single message (bits 6:4 = 000)
        let new_ctrl = (msg_ctrl & !0x0070) | 0x0001; // Clear MME, set Enable
        pci_write_config_word(
            self.bus,
            self.device,
            self.function,
            cap_offset + 2,
            new_ctrl,
        );
    }

    /// Find the MSI-X capability in the PCI capability list.
    ///
    /// Returns the config space offset of the MSI-X capability, or None if not found.
    pub fn find_msix_capability(&self) -> Option<u8> {
        self.find_capability(PCI_CAP_ID_MSIX)
    }

    /// Read MSI-X table size from the capability.
    /// Returns the number of MSI-X vectors (Table Size + 1).
    pub fn msix_table_size(&self, cap_offset: u8) -> u16 {
        let msg_ctrl = pci_read_config_word(self.bus, self.device, self.function, cap_offset + 2);
        (msg_ctrl & 0x07FF) + 1 // Bits 10:0 = Table Size (N-1)
    }

    /// Read MSI-X Table BAR index and offset.
    /// Returns (bar_index, offset_within_bar).
    pub fn msix_table_location(&self, cap_offset: u8) -> (u8, u32) {
        let table_offset_bir =
            pci_read_config_dword(self.bus, self.device, self.function, cap_offset + 4);
        let bar_index = (table_offset_bir & 0x07) as u8;
        let offset = table_offset_bir & !0x07;
        (bar_index, offset)
    }

    /// Enable MSI-X (set Enable bit in Message Control, clear Function Mask).
    pub fn enable_msix(&self, cap_offset: u8) {
        let msg_ctrl = pci_read_config_word(self.bus, self.device, self.function, cap_offset + 2);
        // Bit 15: MSI-X Enable, Bit 14: Function Mask (clear to unmask)
        let new_ctrl = (msg_ctrl | (1 << 15)) & !(1 << 14);
        pci_write_config_word(
            self.bus,
            self.device,
            self.function,
            cap_offset + 2,
            new_ctrl,
        );
    }

    /// Disable MSI-X (clear Enable bit in Message Control).
    pub fn disable_msix(&self, cap_offset: u8) {
        let msg_ctrl = pci_read_config_word(self.bus, self.device, self.function, cap_offset + 2);
        let new_ctrl = msg_ctrl & !(1 << 15);
        pci_write_config_word(
            self.bus,
            self.device,
            self.function,
            cap_offset + 2,
            new_ctrl,
        );
    }

    /// Configure a single MSI-X table entry.
    ///
    /// `cap_offset`: config space offset of the MSI-X capability
    /// `vector_index`: which MSI-X vector to program (0-based)
    /// `address`: MSI target address (e.g. GICv2m doorbell)
    /// `data`: MSI data value (e.g. SPI number)
    ///
    /// The MSI-X table is memory-mapped in the BAR indicated by the capability.
    /// Each entry is 16 bytes: addr_lo(4) + addr_hi(4) + data(4) + vector_ctrl(4).
    pub fn configure_msix_entry(&self, cap_offset: u8, vector_index: u16, address: u64, data: u32) {
        let (bar_index, table_offset) = self.msix_table_location(cap_offset);
        if bar_index as usize >= 6 || !self.bars[bar_index as usize].is_valid() {
            return;
        }
        let bar_base = self.bars[bar_index as usize].address;
        const HHDM_BASE: u64 = 0xFFFF_0000_0000_0000;
        let virt_base = if bar_base >= HHDM_BASE {
            bar_base
        } else {
            HHDM_BASE + bar_base
        };
        let entry_addr = virt_base + table_offset as u64 + (vector_index as u64 * 16);

        unsafe {
            // Address low (offset 0)
            core::ptr::write_volatile(entry_addr as *mut u32, address as u32);
            // Address high (offset 4)
            core::ptr::write_volatile((entry_addr + 4) as *mut u32, (address >> 32) as u32);
            // Data (offset 8)
            core::ptr::write_volatile((entry_addr + 8) as *mut u32, data);
            // Vector Control (offset 12): 0 = unmasked
            core::ptr::write_volatile((entry_addr + 12) as *mut u32, 0);
        }
    }

    /// Find any PCI capability by ID. Returns the config space offset, or None.
    pub fn find_capability(&self, cap_id: u8) -> Option<u8> {
        let status = pci_read_config_word(self.bus, self.device, self.function, 0x06);
        if (status & (1 << 4)) == 0 {
            return None;
        }
        let mut cap_ptr = pci_read_config_byte(self.bus, self.device, self.function, 0x34);
        while cap_ptr != 0 {
            let id = pci_read_config_byte(self.bus, self.device, self.function, cap_ptr);
            if id == cap_id {
                return Some(cap_ptr);
            }
            cap_ptr = pci_read_config_byte(self.bus, self.device, self.function, cap_ptr + 1);
        }
        None
    }

    /// Transition device to D0 power state via PM capability (cap ID 0x01).
    /// This is what Linux's pci_enable_device() does internally via pci_set_power_state().
    /// Returns the previous power state (0=D0, 1=D1, 2=D2, 3=D3hot), or None if no PM cap.
    pub fn set_power_state_d0(&self) -> Option<u8> {
        let pm_cap = self.find_capability(0x01)?; // PCI_CAP_ID_PM = 0x01
                                                  // PMCSR (Power Management Control/Status Register) is at PM_cap + 4
        let pmcsr = pci_read_config_word(self.bus, self.device, self.function, pm_cap + 4);
        let current_state = (pmcsr & 0x03) as u8; // Bits [1:0] = power state
        if current_state != 0 {
            // Not in D0 — transition to D0 by clearing bits [1:0]
            let new_pmcsr = pmcsr & !0x03;
            pci_write_config_word(self.bus, self.device, self.function, pm_cap + 4, new_pmcsr);
            // PCI spec requires 10ms delay after D3hot->D0 transition
            // (We always wait this since it's safe)
            for _ in 0..10_000_000u64 {
                core::hint::spin_loop();
            }
        }
        Some(current_state)
    }

    /// Set Cache Line Size register (offset 0x0C).
    /// Linux sets this based on the CPU's cache line size (typically 64 bytes = 16 DWORDs).
    pub fn set_cache_line_size(&self, size_dwords: u8) {
        pci_write_config_byte(self.bus, self.device, self.function, 0x0C, size_dwords);
    }

    /// Set Latency Timer register (offset 0x0D).
    /// Linux's pci_set_master() sets this to 64 on conventional PCI if it's < 16.
    pub fn set_latency_timer(&self, timer: u8) {
        pci_write_config_byte(self.bus, self.device, self.function, 0x0D, timer);
    }

    /// Dump all PCI capabilities for diagnostics (prints to serial).
    pub fn dump_capabilities(&self) {
        let status = pci_read_config_word(self.bus, self.device, self.function, 0x06);
        if (status & (1 << 4)) == 0 {
            crate::serial_println!(
                "[pci] {:02x}:{:02x}.{}: no capabilities list",
                self.bus,
                self.device,
                self.function
            );
            return;
        }
        let mut cap_ptr = pci_read_config_byte(self.bus, self.device, self.function, 0x34);
        crate::serial_println!(
            "[pci] {:02x}:{:02x}.{}: capabilities:",
            self.bus,
            self.device,
            self.function
        );
        while cap_ptr != 0 {
            let id = pci_read_config_byte(self.bus, self.device, self.function, cap_ptr);
            let cap_name = match id {
                0x01 => "PM",
                0x05 => "MSI",
                0x10 => "PCIe",
                0x11 => "MSI-X",
                0x12 => "SATA",
                _ => "?",
            };
            // Read the full dword at cap_ptr for extra context
            let dw0 = pci_read_config_dword(self.bus, self.device, self.function, cap_ptr);
            let dw1 = pci_read_config_dword(self.bus, self.device, self.function, cap_ptr + 4);
            crate::serial_println!(
                "  cap 0x{:02x} ({}) @ 0x{:02x}: dw0=0x{:08x} dw1=0x{:08x}",
                id,
                cap_name,
                cap_ptr,
                dw0,
                dw1
            );
            cap_ptr = pci_read_config_byte(self.bus, self.device, self.function, cap_ptr + 1);
        }
    }

    /// Dump full 256-byte PCI config space in milestone format for byte-for-byte comparison.
    /// Output format: `[M1] label +offset: XXXXXXXX XXXXXXXX XXXXXXXX XXXXXXXX`
    pub fn dump_config_space_256(&self, label: &str) {
        for offset in (0u8..=240).step_by(16) {
            let dw0 = pci_read_config_dword(self.bus, self.device, self.function, offset);
            let dw1 = pci_read_config_dword(self.bus, self.device, self.function, offset + 4);
            let dw2 = pci_read_config_dword(self.bus, self.device, self.function, offset + 8);
            let dw3 = pci_read_config_dword(self.bus, self.device, self.function, offset + 12);
            crate::serial_println!(
                "[M1] {} +{:03x}: {:08x} {:08x} {:08x} {:08x}",
                label,
                offset,
                dw0,
                dw1,
                dw2,
                dw3
            );
        }
    }

    /// Full Linux-style PCI device enable: D0 transition + bus master + memory space + INTx disable.
    /// This replicates what Linux's pci_enable_device() + pci_set_master() does.
    pub fn linux_style_enable(&self) {
        // 1. Transition to D0 power state (like pci_set_power_state(dev, PCI_D0))
        if let Some(prev_state) = self.set_power_state_d0() {
            crate::serial_println!(
                "[pci] {:02x}:{:02x}.{}: PM D{} -> D0",
                self.bus,
                self.device,
                self.function,
                prev_state
            );
        } else {
            crate::serial_println!(
                "[pci] {:02x}:{:02x}.{}: no PM capability",
                self.bus,
                self.device,
                self.function
            );
        }

        // 2. Set Cache Line Size (64 bytes = 16 DWORDs, standard for ARM64)
        self.set_cache_line_size(16);

        // 3. Set Latency Timer (Linux uses 64 for conventional PCI)
        self.set_latency_timer(64);

        // 4. Enable Memory Space + Bus Master + Disable INTx (all in one write)
        let command = pci_read_config_word(self.bus, self.device, self.function, 0x04);
        // Bit 1: Memory Space, Bit 2: Bus Master, Bit 10: INTx Disable
        let new_command = command | 0x0406;
        pci_write_config_word(self.bus, self.device, self.function, 0x04, new_command);
        crate::serial_println!(
            "[pci] {:02x}:{:02x}.{}: cmd 0x{:04x} -> 0x{:04x}",
            self.bus,
            self.device,
            self.function,
            command,
            new_command
        );

        // 5. Clear any error bits in Status register (write-1-to-clear)
        let status = pci_read_config_word(self.bus, self.device, self.function, 0x06);
        if status & 0xF900 != 0 {
            // Clear error bits: SERR (14), Parity (15), Sig Target Abort (11),
            // Rcvd Target Abort (12), Rcvd Master Abort (13), Sig System Error (14), Parity (15)
            pci_write_config_word(self.bus, self.device, self.function, 0x06, status);
            crate::serial_println!(
                "[pci] {:02x}:{:02x}.{}: cleared status errors 0x{:04x}",
                self.bus,
                self.device,
                self.function,
                status & 0xF900
            );
        }
    }
}

impl fmt::Display for Device {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:02x}:{:02x}.{} {:04x}:{:04x} {:?}/{:02x}",
            self.bus,
            self.device,
            self.function,
            self.vendor_id,
            self.device_id,
            self.class,
            self.subclass
        )
    }
}

impl fmt::Debug for Device {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PciDevice")
            .field(
                "location",
                &format_args!("{:02x}:{:02x}.{}", self.bus, self.device, self.function),
            )
            .field("vendor_id", &format_args!("{:#06x}", self.vendor_id))
            .field("device_id", &format_args!("{:#06x}", self.device_id))
            .field("class", &self.class)
            .field("subclass", &format_args!("{:#04x}", self.subclass))
            .field("irq", &self.interrupt_line)
            .finish()
    }
}

/// Read a 32-bit value from PCI configuration space
#[cfg(target_arch = "x86_64")]
pub(crate) fn pci_read_config_dword(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    // Build the configuration address
    let address: u32 = 0x8000_0000
        | ((bus as u32) << 16)
        | ((device as u32) << 11)
        | ((function as u32) << 8)
        | ((offset & 0xFC) as u32);

    unsafe {
        let mut addr_port = Port::<u32>::new(CONFIG_ADDRESS);
        let mut data_port = Port::<u32>::new(CONFIG_DATA);

        addr_port.write(address);
        data_port.read()
    }
}

/// Read a 32-bit value from PCI configuration space via ECAM.
///
/// ECAM maps each device's 4KB config space into contiguous physical memory:
///   address = ECAM_BASE + (bus << 20) | (device << 15) | (function << 12) | offset
///
/// Returns 0xFFFFFFFF if no PCI ECAM is configured (no PCI bus available).
#[cfg(target_arch = "aarch64")]
pub(crate) fn pci_read_config_dword(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    let ecam_base = crate::platform_config::pci_ecam_base();
    if ecam_base == 0 {
        return 0xFFFF_FFFF; // No PCI
    }

    let addr = ecam_base
        + (((bus as u64) << 20)
            | ((device as u64) << 15)
            | ((function as u64) << 12)
            | ((offset & 0xFC) as u64));

    const HHDM_BASE: u64 = 0xFFFF_0000_0000_0000;
    let virt = (HHDM_BASE + addr) as *const u32;
    unsafe { core::ptr::read_volatile(virt) }
}

/// Write a 32-bit value to PCI configuration space
#[cfg(target_arch = "x86_64")]
pub(crate) fn pci_write_config_dword(bus: u8, device: u8, function: u8, offset: u8, value: u32) {
    let address: u32 = 0x8000_0000
        | ((bus as u32) << 16)
        | ((device as u32) << 11)
        | ((function as u32) << 8)
        | ((offset & 0xFC) as u32);

    unsafe {
        let mut addr_port = Port::<u32>::new(CONFIG_ADDRESS);
        let mut data_port = Port::<u32>::new(CONFIG_DATA);

        addr_port.write(address);
        data_port.write(value);
    }
}

/// Write a 32-bit value to PCI configuration space via ECAM (ARM64).
#[cfg(target_arch = "aarch64")]
pub(crate) fn pci_write_config_dword(bus: u8, device: u8, function: u8, offset: u8, value: u32) {
    let ecam_base = crate::platform_config::pci_ecam_base();
    if ecam_base == 0 {
        return; // No PCI
    }

    let addr = ecam_base
        + (((bus as u64) << 20)
            | ((device as u64) << 15)
            | ((function as u64) << 12)
            | ((offset & 0xFC) as u64));

    const HHDM_BASE: u64 = 0xFFFF_0000_0000_0000;
    let virt = (HHDM_BASE + addr) as *mut u32;
    unsafe { core::ptr::write_volatile(virt, value) }
}

/// Read a 16-bit value from PCI configuration space
#[allow(dead_code)] // Used by Device methods, which are part of public API
pub(crate) fn pci_read_config_word(bus: u8, device: u8, function: u8, offset: u8) -> u16 {
    let dword = pci_read_config_dword(bus, device, function, offset & 0xFC);
    let shift = ((offset & 2) * 8) as u32;
    ((dword >> shift) & 0xFFFF) as u16
}

/// Write a 16-bit value to PCI configuration space
#[allow(dead_code)] // Used by Device methods, which are part of public API
pub(crate) fn pci_write_config_word(bus: u8, device: u8, function: u8, offset: u8, value: u16) {
    let dword_offset = offset & 0xFC;
    let mut dword = pci_read_config_dword(bus, device, function, dword_offset);
    let shift = ((offset & 2) * 8) as u32;
    let mask = !(0xFFFF << shift);
    dword = (dword & mask) | ((value as u32) << shift);
    pci_write_config_dword(bus, device, function, dword_offset, dword);
}

/// Read an 8-bit value from PCI configuration space
#[allow(dead_code)] // Part of low-level API, will be used by VirtIO driver
pub(crate) fn pci_read_config_byte(bus: u8, device: u8, function: u8, offset: u8) -> u8 {
    let dword = pci_read_config_dword(bus, device, function, offset & 0xFC);
    let shift = ((offset & 3) * 8) as u32;
    ((dword >> shift) & 0xFF) as u8
}

/// Write an 8-bit value to PCI configuration space
#[allow(dead_code)] // Part of low-level API
pub(crate) fn pci_write_config_byte(bus: u8, device: u8, function: u8, offset: u8, value: u8) {
    let dword_offset = offset & 0xFC;
    let mut dword = pci_read_config_dword(bus, device, function, dword_offset);
    let shift = ((offset & 3) * 8) as u32;
    let mask = !(0xFF << shift);
    dword = (dword & mask) | ((value as u32) << shift);
    pci_write_config_dword(bus, device, function, dword_offset, dword);
}

/// Read a BAR address without writing 0xFFFFFFFF for sizing.
/// Used for devices where BAR sizing disrupts the device's internal state
/// (e.g., Parallels vxHC). Sets size to 0x1000 (4KB minimum) since we
/// can't determine the actual size without the destructive write.
fn read_bar_no_sizing(bus: u8, device: u8, function: u8, bar_index: u8) -> (Bar, bool) {
    let offset = 0x10 + (bar_index * 4);
    let bar_low = pci_read_config_dword(bus, device, function, offset);

    if bar_low & 0x01 != 0 {
        // I/O space BAR
        let address = (bar_low & 0xFFFF_FFFC) as u64;
        (
            Bar {
                address,
                size: 0x100,
                is_io: true,
                is_64bit: false,
                prefetchable: false,
            },
            false,
        )
    } else {
        let bar_type = (bar_low >> 1) & 0x03;
        let prefetchable = (bar_low & 0x08) != 0;
        if bar_type == 0x02 {
            // 64-bit BAR
            let bar_high = pci_read_config_dword(bus, device, function, offset + 4);
            let address = ((bar_high as u64) << 32) | ((bar_low & 0xFFFF_FFF0) as u64);
            (
                Bar {
                    address,
                    size: 0x1000,
                    is_io: false,
                    is_64bit: true,
                    prefetchable,
                },
                true,
            )
        } else {
            // 32-bit BAR
            let address = (bar_low & 0xFFFF_FFF0) as u64;
            (
                Bar {
                    address,
                    size: 0x1000,
                    is_io: false,
                    is_64bit: false,
                    prefetchable,
                },
                false,
            )
        }
    }
}

/// Decode a BAR from PCI configuration space
fn decode_bar(bus: u8, device: u8, function: u8, bar_index: u8) -> (Bar, bool) {
    let offset = 0x10 + (bar_index * 4);

    // Read current BAR value
    let bar_low = pci_read_config_dword(bus, device, function, offset);

    // Check for I/O space BAR
    if bar_low & 0x01 != 0 {
        // I/O space BAR
        // Write all 1s to get size
        pci_write_config_dword(bus, device, function, offset, 0xFFFF_FFFF);
        let size_mask = pci_read_config_dword(bus, device, function, offset);
        // Restore original value
        pci_write_config_dword(bus, device, function, offset, bar_low);

        let address = (bar_low & 0xFFFF_FFFC) as u64;
        let size = if size_mask == 0 || size_mask == 0xFFFF_FFFF {
            0
        } else {
            (!(size_mask & 0xFFFF_FFFC)).wrapping_add(1) as u64
        };

        (
            Bar {
                address,
                size,
                is_io: true,
                is_64bit: false,
                prefetchable: false,
            },
            false, // Not a 64-bit BAR, don't skip next
        )
    } else {
        // Memory space BAR
        let bar_type = (bar_low >> 1) & 0x03;
        let prefetchable = (bar_low & 0x08) != 0;

        if bar_type == 0x02 {
            // 64-bit BAR
            let bar_high = pci_read_config_dword(bus, device, function, offset + 4);

            // Write all 1s to get size
            pci_write_config_dword(bus, device, function, offset, 0xFFFF_FFFF);
            pci_write_config_dword(bus, device, function, offset + 4, 0xFFFF_FFFF);
            let size_low = pci_read_config_dword(bus, device, function, offset);
            let size_high = pci_read_config_dword(bus, device, function, offset + 4);
            // Restore original values
            pci_write_config_dword(bus, device, function, offset, bar_low);
            pci_write_config_dword(bus, device, function, offset + 4, bar_high);

            let address = ((bar_high as u64) << 32) | ((bar_low & 0xFFFF_FFF0) as u64);
            let size_mask = ((size_high as u64) << 32) | ((size_low & 0xFFFF_FFF0) as u64);
            let size = if size_mask == 0 {
                0
            } else {
                (!size_mask).wrapping_add(1)
            };

            (
                Bar {
                    address,
                    size,
                    is_io: false,
                    is_64bit: true,
                    prefetchable,
                },
                true, // 64-bit BAR, skip next BAR slot
            )
        } else {
            // 32-bit BAR
            // Write all 1s to get size
            pci_write_config_dword(bus, device, function, offset, 0xFFFF_FFFF);
            let size_mask = pci_read_config_dword(bus, device, function, offset);
            // Restore original value
            pci_write_config_dword(bus, device, function, offset, bar_low);

            let address = (bar_low & 0xFFFF_FFF0) as u64;
            let size = if size_mask == 0 || size_mask == 0xFFFF_FFFF {
                0
            } else {
                (!(size_mask & 0xFFFF_FFF0)).wrapping_add(1) as u64
            };

            (
                Bar {
                    address,
                    size,
                    is_io: false,
                    is_64bit: false,
                    prefetchable,
                },
                false,
            )
        }
    }
}

/// Probe for a device at the given bus/device/function
fn probe_device(bus: u8, device: u8, function: u8) -> Option<Device> {
    let vendor_device = pci_read_config_dword(bus, device, function, 0x00);

    // 0xFFFFFFFF indicates no device present
    if vendor_device == 0xFFFF_FFFF {
        return None;
    }

    let vendor_id = vendor_device as u16;
    let device_id = (vendor_device >> 16) as u16;

    // Read class/subclass/prog_if/revision
    let class_reg = pci_read_config_dword(bus, device, function, 0x08);
    let revision_id = class_reg as u8;
    let prog_if = (class_reg >> 8) as u8;
    let subclass = (class_reg >> 16) as u8;
    let class_code = (class_reg >> 24) as u8;

    // Read header type (to check multifunction)
    let header_reg = pci_read_config_dword(bus, device, function, 0x0C);
    let header_type = (header_reg >> 16) as u8;
    let multifunction = (header_type & 0x80) != 0;

    // Read interrupt info
    let int_reg = pci_read_config_dword(bus, device, function, 0x3C);
    let interrupt_line = int_reg as u8;
    let interrupt_pin = (int_reg >> 8) as u8;

    // Decode BARs
    // Skip destructive BAR sizing (write 0xFFFFFFFF) for USB controllers
    // (class=0x0C, subclass=0x03). On Parallels, the BAR disable/re-enable
    // from sizing corrupts the vxHC's internal USB emulation state.
    let skip_xhci_sizing = class_code == 0x0C && subclass == 0x03;
    let mut bars = [Bar::empty(); 6];
    let mut bar_index = 0;
    while bar_index < 6 {
        if skip_xhci_sizing {
            let (bar, skip_next) = read_bar_no_sizing(bus, device, function, bar_index);
            bars[bar_index as usize] = bar;
            bar_index += 1;
            if skip_next && bar_index < 6 {
                bar_index += 1;
            }
        } else {
            let (bar, skip_next) = decode_bar(bus, device, function, bar_index);
            bars[bar_index as usize] = bar;
            bar_index += 1;
            if skip_next && bar_index < 6 {
                bar_index += 1; // Skip the next BAR slot for 64-bit BARs
            }
        }
    }

    Some(Device {
        bus,
        device,
        function,
        vendor_id,
        device_id,
        revision_id,
        class: DeviceClass::from_u8(class_code),
        subclass,
        prog_if,
        interrupt_line,
        interrupt_pin,
        multifunction,
        bars,
    })
}

/// Global list of discovered PCI devices
static PCI_DEVICES: Mutex<Option<Vec<Device>>> = Mutex::new(None);
/// Track whether PCI enumeration has completed
static PCI_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Get a human-readable vendor name for common vendors
fn vendor_name(vendor_id: u16) -> &'static str {
    match vendor_id {
        VIRTIO_VENDOR_ID => "VirtIO",
        INTEL_VENDOR_ID => "Intel",
        QEMU_VENDOR_ID => "QEMU/RedHat",
        0x1022 => "AMD",
        0x10DE => "NVIDIA",
        0x14E4 => "Broadcom",
        0x10EC => "Realtek",
        _ => "Unknown",
    }
}

/// Enumerate all PCI devices on the bus
///
/// Returns the total number of devices found.
pub fn enumerate() -> usize {
    log::info!("PCI: Starting bus enumeration...");

    let mut devices = Vec::new();
    let mut virtio_block_count = 0;
    let mut network_count = 0;

    // Use platform-specific bus range on ARM64 (Parallels faults on out-of-range buses)
    #[cfg(target_arch = "aarch64")]
    let (bus_start, bus_end) = (
        crate::platform_config::pci_bus_start(),
        crate::platform_config::pci_bus_end(),
    );
    #[cfg(not(target_arch = "aarch64"))]
    let (bus_start, bus_end) = (0u8, MAX_BUS);

    for bus in bus_start..=bus_end {
        for device in 0..MAX_DEVICE {
            // First check function 0
            if let Some(dev) = probe_device(bus, device, 0) {
                let is_multifunction = dev.multifunction;

                // Log ALL discovered devices for visibility
                log::info!(
                    "PCI: {:02x}:{:02x}.{} [{:04x}:{:04x}] {} {:?}/0x{:02x} IRQ={}",
                    dev.bus,
                    dev.device,
                    dev.function,
                    dev.vendor_id,
                    dev.device_id,
                    vendor_name(dev.vendor_id),
                    dev.class,
                    dev.subclass,
                    dev.interrupt_line
                );

                // Log BAR info for all devices with valid BARs
                for (i, bar) in dev.bars.iter().enumerate() {
                    if bar.is_valid() {
                        log::debug!(
                            "PCI:   BAR{}: addr={:#x} size={:#x} {}",
                            i,
                            bar.address,
                            bar.size,
                            if bar.is_io { "I/O" } else { "MMIO" }
                        );
                    }
                }

                // Track specific device types
                if dev.is_virtio_block() {
                    virtio_block_count += 1;
                }
                if dev.is_network() {
                    network_count += 1;
                    log::info!(
                        "PCI:   -> Network controller detected!{}",
                        if dev.is_virtio_net() {
                            " (VirtIO-net)"
                        } else {
                            ""
                        }
                    );
                    // Boot stage marker for E1000 detection
                    if dev.vendor_id == 0x8086 && dev.device_id == 0x100e {
                        log::info!("E1000 network device found");
                    }
                }

                devices.push(dev);

                // If multifunction, check other functions
                if is_multifunction {
                    for function in 1..MAX_FUNCTION {
                        if let Some(func_dev) = probe_device(bus, device, function) {
                            log::info!(
                                "PCI: {:02x}:{:02x}.{} [{:04x}:{:04x}] {} {:?}/0x{:02x} IRQ={}",
                                func_dev.bus,
                                func_dev.device,
                                func_dev.function,
                                func_dev.vendor_id,
                                func_dev.device_id,
                                vendor_name(func_dev.vendor_id),
                                func_dev.class,
                                func_dev.subclass,
                                func_dev.interrupt_line
                            );

                            if func_dev.is_virtio_block() {
                                virtio_block_count += 1;
                            }
                            if func_dev.is_network() {
                                network_count += 1;
                                log::info!(
                                    "PCI:   -> Network controller detected!{}",
                                    if func_dev.is_virtio_net() {
                                        " (VirtIO-net)"
                                    } else {
                                        ""
                                    }
                                );
                                // Boot stage marker for E1000 detection
                                if func_dev.vendor_id == 0x8086 && func_dev.device_id == 0x100e {
                                    log::info!("E1000 network device found");
                                }
                            }
                            devices.push(func_dev);
                        }
                    }
                }
            }
        }
    }

    let device_count = devices.len();
    log::info!(
        "PCI: Enumeration complete. Found {} devices ({} VirtIO block, {} network)",
        device_count,
        virtio_block_count,
        network_count
    );

    // Store devices globally
    *PCI_DEVICES.lock() = Some(devices);
    PCI_INITIALIZED.store(true, core::sync::atomic::Ordering::Release);

    device_count
}

/// Assign MMIO addresses to PCI BARs that have no address yet.
///
/// On QEMU virt (ARM64), there is no firmware to assign BAR addresses.
/// This function walks all discovered devices and assigns addresses from
/// the PCI MMIO window for any BAR with address==0 but size>0.
///
/// Must be called after `enumerate()`.
#[cfg(target_arch = "aarch64")]
pub fn assign_bars() {
    let mmio_base = crate::platform_config::pci_mmio_base();
    let mmio_size = crate::platform_config::pci_mmio_size();
    if mmio_base == 0 || mmio_size == 0 {
        return;
    }

    // Start allocating from 0x10000000 + 0x01000000 to avoid conflicts with
    // VirtIO MMIO devices that QEMU places at 0x0a000000-0x0a003fff.
    // The PCI MMIO window is 0x10000000 - 0x3efeffff.
    let mut next_addr = mmio_base + 0x01000000; // 0x11000000

    let mut devices = PCI_DEVICES.lock();
    let Some(ref mut devs) = *devices else { return };

    for dev in devs.iter_mut() {
        for bar_idx in 0..6u8 {
            let bar = &mut dev.bars[bar_idx as usize];
            if bar.size > 0 && !bar.is_io && bar.address == 0 {
                // Align the allocation to the BAR size (PCI spec requirement)
                let align = bar.size;
                next_addr = (next_addr + align - 1) & !(align - 1);

                if next_addr + bar.size > mmio_base + mmio_size {
                    log::warn!("PCI: MMIO window exhausted, cannot assign BAR{} for {:02x}:{:02x}.{}",
                        bar_idx, dev.bus, dev.device, dev.function);
                    continue;
                }

                // Write the BAR address to PCI config space
                let offset = 0x10 + bar_idx * 4;
                let bar_value = next_addr as u32 | if bar.prefetchable { 0x08 } else { 0x00 }
                    | if bar.is_64bit { 0x04 } else { 0x00 };
                pci_write_config_dword(dev.bus, dev.device, dev.function, offset, bar_value);
                if bar.is_64bit {
                    pci_write_config_dword(dev.bus, dev.device, dev.function, offset + 4, (next_addr >> 32) as u32);
                }

                log::info!("PCI: Assigned BAR{} for {:02x}:{:02x}.{} -> {:#x} (size {:#x})",
                    bar_idx, dev.bus, dev.device, dev.function, next_addr, bar.size);

                bar.address = next_addr;
                next_addr += bar.size;
            }
        }

        // Enable memory space and bus mastering for devices with assigned BARs
        let has_mmio_bar = dev.bars.iter().any(|b| b.is_valid() && !b.is_io && b.address != 0);
        if has_mmio_bar {
            dev.enable_memory_space();
            dev.enable_bus_master();
        }
    }
}

/// Get a copy of all discovered PCI devices
#[allow(dead_code)] // Part of public API, will be used by VirtIO driver
pub fn get_devices() -> Option<Vec<Device>> {
    PCI_DEVICES.lock().clone()
}

/// Find a specific device by vendor and device ID
#[allow(dead_code)] // Part of public API, will be used by VirtIO driver
pub fn find_device(vendor_id: u16, device_id: u16) -> Option<Device> {
    let devices = PCI_DEVICES.lock();
    devices
        .as_ref()?
        .iter()
        .find(|d| d.vendor_id == vendor_id && d.device_id == device_id)
        .cloned()
}

/// Find a device by PCI class code, subclass, and programming interface.
pub fn find_by_class(class: DeviceClass, subclass: u8, prog_if: u8) -> Option<Device> {
    let devices = PCI_DEVICES.lock();
    devices
        .as_ref()?
        .iter()
        .find(|d| d.class == class && d.subclass == subclass && d.prog_if == prog_if)
        .cloned()
}

/// Find all VirtIO block devices
#[allow(dead_code)] // Part of public API, will be used by VirtIO driver
pub fn find_virtio_block_devices() -> Vec<Device> {
    let devices = PCI_DEVICES.lock();
    match devices.as_ref() {
        Some(devs) => devs
            .iter()
            .filter(|d| d.is_virtio_block())
            .cloned()
            .collect(),
        None => Vec::new(),
    }
}

/// Find all network controller devices
#[allow(dead_code)] // Part of public API, will be used by network driver
pub fn find_network_devices() -> Vec<Device> {
    let devices = PCI_DEVICES.lock();
    match devices.as_ref() {
        Some(devs) => devs.iter().filter(|d| d.is_network()).cloned().collect(),
        None => Vec::new(),
    }
}

/// Find all VirtIO network devices
#[allow(dead_code)] // Part of public API, will be used by VirtIO-net driver
pub fn find_virtio_net_devices() -> Vec<Device> {
    let devices = PCI_DEVICES.lock();
    match devices.as_ref() {
        Some(devs) => devs.iter().filter(|d| d.is_virtio_net()).cloned().collect(),
        None => Vec::new(),
    }
}

/// Find all VirtIO sound devices
#[allow(dead_code)] // Part of public API, will be used by VirtIO sound driver
pub fn find_virtio_sound_devices() -> Vec<Device> {
    let devices = PCI_DEVICES.lock();
    match devices.as_ref() {
        Some(devs) => devs
            .iter()
            .filter(|d| d.is_virtio_sound())
            .cloned()
            .collect(),
        None => Vec::new(),
    }
}
