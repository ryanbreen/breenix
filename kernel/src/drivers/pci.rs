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
use x86_64::instructions::port::Port;

/// PCI configuration address port
const CONFIG_ADDRESS: u16 = 0xCF8;
/// PCI configuration data port
const CONFIG_DATA: u16 = 0xCFC;

/// Maximum number of PCI buses to scan
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
/// VirtIO network device ID (legacy)
pub const VIRTIO_NET_DEVICE_ID_LEGACY: u16 = 0x1000;
/// VirtIO network device ID (modern)
pub const VIRTIO_NET_DEVICE_ID_MODERN: u16 = 0x1041;

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
            .field("location", &format_args!("{:02x}:{:02x}.{}", self.bus, self.device, self.function))
            .field("vendor_id", &format_args!("{:#06x}", self.vendor_id))
            .field("device_id", &format_args!("{:#06x}", self.device_id))
            .field("class", &self.class)
            .field("subclass", &format_args!("{:#04x}", self.subclass))
            .field("irq", &self.interrupt_line)
            .finish()
    }
}

/// Read a 32-bit value from PCI configuration space
fn pci_read_config_dword(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
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

/// Write a 32-bit value to PCI configuration space
fn pci_write_config_dword(bus: u8, device: u8, function: u8, offset: u8, value: u32) {
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

/// Read a 16-bit value from PCI configuration space
#[allow(dead_code)] // Used by Device methods, which are part of public API
fn pci_read_config_word(bus: u8, device: u8, function: u8, offset: u8) -> u16 {
    let dword = pci_read_config_dword(bus, device, function, offset & 0xFC);
    let shift = ((offset & 2) * 8) as u32;
    ((dword >> shift) & 0xFFFF) as u16
}

/// Write a 16-bit value to PCI configuration space
#[allow(dead_code)] // Used by Device methods, which are part of public API
fn pci_write_config_word(bus: u8, device: u8, function: u8, offset: u8, value: u16) {
    let dword_offset = offset & 0xFC;
    let mut dword = pci_read_config_dword(bus, device, function, dword_offset);
    let shift = ((offset & 2) * 8) as u32;
    let mask = !(0xFFFF << shift);
    dword = (dword & mask) | ((value as u32) << shift);
    pci_write_config_dword(bus, device, function, dword_offset, dword);
}

/// Read an 8-bit value from PCI configuration space
#[allow(dead_code)] // Part of low-level API, will be used by VirtIO driver
fn pci_read_config_byte(bus: u8, device: u8, function: u8, offset: u8) -> u8 {
    let dword = pci_read_config_dword(bus, device, function, offset & 0xFC);
    let shift = ((offset & 3) * 8) as u32;
    ((dword >> shift) & 0xFF) as u8
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
    let mut bars = [Bar::empty(); 6];
    let mut bar_index = 0;
    while bar_index < 6 {
        let (bar, skip_next) = decode_bar(bus, device, function, bar_index);
        bars[bar_index as usize] = bar;
        bar_index += 1;
        if skip_next && bar_index < 6 {
            bar_index += 1; // Skip the next BAR slot for 64-bit BARs
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

    for bus in 0..=MAX_BUS {
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
                        if dev.is_virtio_net() { " (VirtIO-net)" } else { "" }
                    );
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
                                    if func_dev.is_virtio_net() { " (VirtIO-net)" } else { "" }
                                );
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

/// Get a copy of all discovered PCI devices
#[allow(dead_code)] // Part of public API, will be used by VirtIO driver
pub fn get_devices() -> Option<Vec<Device>> {
    PCI_DEVICES.lock().clone()
}

/// Find a specific device by vendor and device ID
#[allow(dead_code)] // Part of public API, will be used by VirtIO driver
pub fn find_device(vendor_id: u16, device_id: u16) -> Option<Device> {
    let devices = PCI_DEVICES.lock();
    devices.as_ref()?.iter().find(|d| d.vendor_id == vendor_id && d.device_id == device_id).cloned()
}

/// Find all VirtIO block devices
#[allow(dead_code)] // Part of public API, will be used by VirtIO driver
pub fn find_virtio_block_devices() -> Vec<Device> {
    let devices = PCI_DEVICES.lock();
    match devices.as_ref() {
        Some(devs) => devs.iter().filter(|d| d.is_virtio_block()).cloned().collect(),
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
