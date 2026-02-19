//! VirtIO PCI Modern Transport (VirtIO 1.0+)
//!
//! Implements the VirtIO PCI transport using capability-based BAR access.
//! This module is used on platforms with PCI (e.g., Parallels on ARM64)
//! where VirtIO devices appear as PCI functions.
//!
//! The modern VirtIO PCI transport uses PCI capabilities to locate
//! memory-mapped regions in BARs for device configuration:
//! - Common Configuration (features, status, queue management)
//! - Notification (queue doorbell)
//! - ISR Status
//! - Device-specific Configuration
//!
//! For transitional (legacy) devices (PCI ID 0x1000-0x103F), the modern
//! interface is also available via capabilities alongside the legacy I/O port
//! interface. We always prefer the modern interface.

#![allow(dead_code)]

use crate::drivers::pci::{self, Device as PciDevice};

/// HHDM base for memory-mapped access.
const HHDM_BASE: u64 = 0xFFFF_0000_0000_0000;

// =============================================================================
// VirtIO PCI Capability Types (from VirtIO 1.0 spec, section 4.1.4)
// =============================================================================

/// PCI Capability ID for vendor-specific (VirtIO uses this)
const PCI_CAP_ID_VNDR: u8 = 0x09;

/// Common configuration
const VIRTIO_PCI_CAP_COMMON_CFG: u8 = 1;
/// Notifications
const VIRTIO_PCI_CAP_NOTIFY_CFG: u8 = 2;
/// ISR status
const VIRTIO_PCI_CAP_ISR_CFG: u8 = 3;
/// Device-specific configuration
const VIRTIO_PCI_CAP_DEVICE_CFG: u8 = 4;

// =============================================================================
// Common Configuration Register Offsets (VirtIO 1.0 spec, section 4.1.4.3)
// =============================================================================

/// Select device feature dword (0 = low 32, 1 = high 32)
const COMMON_DFSELECT: usize = 0x00;
/// Read device feature bits (dword selected by DFSELECT)
const COMMON_DF: usize = 0x04;
/// Select driver feature dword
const COMMON_GFSELECT: usize = 0x08;
/// Write driver feature bits
const COMMON_GF: usize = 0x0C;
/// MSI-X configuration vector
const COMMON_MSIX: usize = 0x10;
/// Number of virtqueues
const COMMON_NUMQ: usize = 0x12;
/// Device status
const COMMON_STATUS: usize = 0x14;
/// Configuration atomicity generation counter
const COMMON_CFGGEN: usize = 0x15;
/// Queue select
const COMMON_Q_SELECT: usize = 0x16;
/// Queue size (max)
const COMMON_Q_SIZE: usize = 0x18;
/// Queue MSI-X vector
const COMMON_Q_MSIX: usize = 0x1A;
/// Queue enable
const COMMON_Q_ENABLE: usize = 0x1C;
/// Queue notify offset (multiplied by notify_off_multiplier)
const COMMON_Q_NOFF: usize = 0x1E;
/// Queue descriptor table address (64-bit)
const COMMON_Q_DESCLO: usize = 0x20;
const COMMON_Q_DESCHI: usize = 0x24;
/// Queue available ring address (64-bit)
const COMMON_Q_AVAILLO: usize = 0x28;
const COMMON_Q_AVAILHI: usize = 0x2C;
/// Queue used ring address (64-bit)
const COMMON_Q_USEDLO: usize = 0x30;
const COMMON_Q_USEDHI: usize = 0x34;

// =============================================================================
// VirtIO Status Bits
// =============================================================================

const STATUS_ACKNOWLEDGE: u8 = 1;
const STATUS_DRIVER: u8 = 2;
const STATUS_DRIVER_OK: u8 = 4;
const STATUS_FEATURES_OK: u8 = 8;
const STATUS_FAILED: u8 = 128;

// =============================================================================
// VirtIO Device Types (from VirtIO spec)
// =============================================================================

/// VirtIO device type IDs (same as MMIO device_id values)
pub mod device_id {
    pub const NETWORK: u32 = 1;
    pub const BLOCK: u32 = 2;
    pub const CONSOLE: u32 = 3;
    pub const GPU: u32 = 16;
    pub const INPUT: u32 = 18;
    pub const SOUND: u32 = 25;
}

// =============================================================================
// Capability Region Descriptor
// =============================================================================

/// Describes a memory-mapped region found via PCI capabilities.
#[derive(Debug, Clone, Copy)]
struct CapRegion {
    /// Virtual address of the region (HHDM + BAR physical + offset)
    virt_base: u64,
    /// Length of the region
    length: u32,
}

impl CapRegion {
    const NONE: Self = CapRegion { virt_base: 0, length: 0 };

    fn is_valid(&self) -> bool {
        self.virt_base != 0 && self.length > 0
    }

    #[inline]
    fn read_u8(&self, offset: usize) -> u8 {
        assert!(offset < self.length as usize);
        unsafe { core::ptr::read_volatile((self.virt_base + offset as u64) as *const u8) }
    }

    #[inline]
    fn write_u8(&self, offset: usize, value: u8) {
        assert!(offset < self.length as usize);
        unsafe { core::ptr::write_volatile((self.virt_base + offset as u64) as *mut u8, value) }
    }

    #[inline]
    fn read_u16(&self, offset: usize) -> u16 {
        assert!(offset + 1 < self.length as usize);
        unsafe { core::ptr::read_volatile((self.virt_base + offset as u64) as *const u16) }
    }

    #[inline]
    fn write_u16(&self, offset: usize, value: u16) {
        assert!(offset + 1 < self.length as usize);
        unsafe { core::ptr::write_volatile((self.virt_base + offset as u64) as *mut u16, value) }
    }

    #[inline]
    fn read_u32(&self, offset: usize) -> u32 {
        assert!(offset + 3 < self.length as usize);
        unsafe { core::ptr::read_volatile((self.virt_base + offset as u64) as *const u32) }
    }

    #[inline]
    fn write_u32(&self, offset: usize, value: u32) {
        assert!(offset + 3 < self.length as usize);
        unsafe { core::ptr::write_volatile((self.virt_base + offset as u64) as *mut u32, value) }
    }
}

// =============================================================================
// VirtIO PCI Device
// =============================================================================

/// A VirtIO device accessed via PCI modern transport.
///
/// This provides the same interface as `VirtioMmioDevice` so that device-specific
/// drivers can work with either transport.
pub struct VirtioPciDevice {
    /// The underlying PCI device (for config space access)
    pci_dev: PciDevice,
    /// Common configuration region (features, status, queues)
    common: CapRegion,
    /// Notification region (queue doorbell writes)
    notify: CapRegion,
    /// Notify offset multiplier (from NOTIFY_CFG capability)
    notify_off_multiplier: u32,
    /// ISR status region
    isr: CapRegion,
    /// Device-specific configuration region
    device_cfg: CapRegion,
    /// Cached device features
    device_features: u64,
    /// VirtIO device type ID
    virtio_device_id: u32,
}

impl VirtioPciDevice {
    /// Probe a PCI device for VirtIO modern capabilities.
    ///
    /// Returns `Some(device)` if this PCI device supports the VirtIO modern interface
    /// (has the required PCI capabilities pointing to BAR regions).
    pub fn probe(pci_dev: PciDevice) -> Option<Self> {
        // Verify this is a VirtIO device
        if pci_dev.vendor_id != pci::VIRTIO_VENDOR_ID {
            return None;
        }

        // Determine VirtIO device type from PCI device ID
        let virtio_device_id = pci_device_id_to_virtio(&pci_dev);
        if virtio_device_id == 0 {
            return None;
        }

        // Enable memory space and bus mastering
        pci_dev.enable_memory_space();
        pci_dev.enable_bus_master();

        // Walk PCI capabilities to find VirtIO capability structures
        let mut common = CapRegion::NONE;
        let mut notify = CapRegion::NONE;
        let mut notify_off_multiplier = 0u32;
        let mut isr = CapRegion::NONE;
        let mut device_cfg = CapRegion::NONE;

        // PCI Status register bit 4 = Capabilities List exists
        let status = pci::pci_read_config_word(
            pci_dev.bus, pci_dev.device, pci_dev.function, 0x06,
        );
        if (status & (1 << 4)) == 0 {
            return None; // No capabilities
        }

        // Capabilities pointer is at offset 0x34
        let mut cap_ptr = pci::pci_read_config_byte(
            pci_dev.bus, pci_dev.device, pci_dev.function, 0x34,
        );

        while cap_ptr != 0 {
            let cap_id = pci::pci_read_config_byte(
                pci_dev.bus, pci_dev.device, pci_dev.function, cap_ptr,
            );
            let cap_next = pci::pci_read_config_byte(
                pci_dev.bus, pci_dev.device, pci_dev.function, cap_ptr + 1,
            );

            if cap_id == PCI_CAP_ID_VNDR {
                // VirtIO PCI capability structure:
                // +0: cap_vndr (0x09)
                // +1: cap_next
                // +2: cap_len
                // +3: cfg_type (COMMON, NOTIFY, ISR, DEVICE)
                // +4: bar (which BAR this maps to)
                // +8: offset (offset within the BAR)
                // +12: length (length of the region)
                let cfg_type = pci::pci_read_config_byte(
                    pci_dev.bus, pci_dev.device, pci_dev.function, cap_ptr + 3,
                );
                let bar_index = pci::pci_read_config_byte(
                    pci_dev.bus, pci_dev.device, pci_dev.function, cap_ptr + 4,
                ) as usize;

                // Read offset and length as dwords
                let offset = pci_read_cap_dword(&pci_dev, cap_ptr + 8);
                let length = pci_read_cap_dword(&pci_dev, cap_ptr + 12);

                // Resolve the BAR physical address
                if bar_index < 6 {
                    let bar = &pci_dev.bars[bar_index];
                    if bar.is_valid() && !bar.is_io {
                        let virt_base = HHDM_BASE + bar.address + offset as u64;
                        let region = CapRegion { virt_base, length };

                        match cfg_type {
                            VIRTIO_PCI_CAP_COMMON_CFG => common = region,
                            VIRTIO_PCI_CAP_NOTIFY_CFG => {
                                notify = region;
                                // NOTIFY_CFG has an extra dword: notify_off_multiplier at cap+16
                                notify_off_multiplier = pci_read_cap_dword(&pci_dev, cap_ptr + 16);
                            }
                            VIRTIO_PCI_CAP_ISR_CFG => isr = region,
                            VIRTIO_PCI_CAP_DEVICE_CFG => device_cfg = region,
                            _ => {} // Ignore unknown capability types
                        }
                    }
                }
            }

            cap_ptr = cap_next;
        }

        // We need at minimum the common config and notification regions
        if !common.is_valid() || !notify.is_valid() {
            return None;
        }

        Some(VirtioPciDevice {
            pci_dev,
            common,
            notify,
            notify_off_multiplier,
            isr,
            device_cfg,
            device_features: 0,
            virtio_device_id,
        })
    }

    // =========================================================================
    // Device Identity
    // =========================================================================

    /// Get the VirtIO device type ID.
    pub fn device_id(&self) -> u32 {
        self.virtio_device_id
    }

    /// Get the VirtIO device version (always 1 for modern PCI transport).
    pub fn version(&self) -> u32 {
        1
    }

    // =========================================================================
    // Status and Initialization
    // =========================================================================

    /// Read the device status register.
    pub fn read_status(&self) -> u8 {
        self.common.read_u8(COMMON_STATUS)
    }

    /// Write the device status register.
    pub fn write_status(&self, status: u8) {
        self.common.write_u8(COMMON_STATUS, status);
    }

    /// Reset the device.
    pub fn reset(&self) {
        self.write_status(0);
    }

    /// Initialize the device with feature negotiation.
    ///
    /// Performs the VirtIO 1.0 initialization sequence:
    /// 1. Reset device
    /// 2. Set ACKNOWLEDGE
    /// 3. Set DRIVER
    /// 4. Negotiate features
    /// 5. Set FEATURES_OK
    /// 6. Verify FEATURES_OK
    pub fn init(&mut self, requested_features: u64) -> Result<(), &'static str> {
        // Reset
        self.reset();

        // Wait for reset
        for _ in 0..10_000 {
            if self.read_status() == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        // ACKNOWLEDGE
        self.write_status(STATUS_ACKNOWLEDGE);

        // DRIVER
        self.write_status(STATUS_ACKNOWLEDGE | STATUS_DRIVER);

        // Read device features (64-bit: low 32 + high 32)
        self.device_features = self.read_device_features();
        let driver_features = self.device_features & requested_features;
        self.write_driver_features(driver_features);

        // FEATURES_OK
        self.write_status(STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK);

        // Verify FEATURES_OK
        if (self.read_status() & STATUS_FEATURES_OK) == 0 {
            self.write_status(STATUS_FAILED);
            return Err("Device did not accept features");
        }

        Ok(())
    }

    /// Mark the device as ready (DRIVER_OK).
    pub fn driver_ok(&self) {
        let status = self.read_status();
        self.write_status(status | STATUS_DRIVER_OK);
    }

    // =========================================================================
    // Feature Negotiation
    // =========================================================================

    /// Read 64-bit device features.
    pub fn read_device_features(&self) -> u64 {
        // Low 32 bits
        self.common.write_u32(COMMON_DFSELECT, 0);
        let low = self.common.read_u32(COMMON_DF) as u64;

        // High 32 bits
        self.common.write_u32(COMMON_DFSELECT, 1);
        let high = self.common.read_u32(COMMON_DF) as u64;

        (high << 32) | low
    }

    /// Write 64-bit driver features.
    pub fn write_driver_features(&self, features: u64) {
        // Low 32 bits
        self.common.write_u32(COMMON_GFSELECT, 0);
        self.common.write_u32(COMMON_GF, features as u32);

        // High 32 bits
        self.common.write_u32(COMMON_GFSELECT, 1);
        self.common.write_u32(COMMON_GF, (features >> 32) as u32);
    }

    /// Get the cached device features.
    pub fn device_features(&self) -> u64 {
        self.device_features
    }

    // =========================================================================
    // Queue Management
    // =========================================================================

    /// Select a virtqueue for configuration.
    pub fn select_queue(&self, queue: u32) {
        self.common.write_u16(COMMON_Q_SELECT, queue as u16);
    }

    /// Get the maximum size of the currently selected queue.
    pub fn get_queue_num_max(&self) -> u32 {
        self.common.read_u16(COMMON_Q_SIZE) as u32
    }

    /// Set the size of the currently selected queue.
    pub fn set_queue_num(&self, num: u32) {
        self.common.write_u16(COMMON_Q_SIZE, num as u16);
    }

    /// Get the number of virtqueues.
    pub fn num_queues(&self) -> u16 {
        self.common.read_u16(COMMON_NUMQ)
    }

    /// Set the descriptor table physical address for the current queue.
    pub fn set_queue_desc(&self, addr: u64) {
        self.common.write_u32(COMMON_Q_DESCLO, addr as u32);
        self.common.write_u32(COMMON_Q_DESCHI, (addr >> 32) as u32);
    }

    /// Set the available ring physical address for the current queue.
    pub fn set_queue_avail(&self, addr: u64) {
        self.common.write_u32(COMMON_Q_AVAILLO, addr as u32);
        self.common.write_u32(COMMON_Q_AVAILHI, (addr >> 32) as u32);
    }

    /// Set the used ring physical address for the current queue.
    pub fn set_queue_used(&self, addr: u64) {
        self.common.write_u32(COMMON_Q_USEDLO, addr as u32);
        self.common.write_u32(COMMON_Q_USEDHI, (addr >> 32) as u32);
    }

    /// Enable or disable the currently selected queue.
    pub fn set_queue_ready(&self, ready: bool) {
        self.common.write_u16(COMMON_Q_ENABLE, if ready { 1 } else { 0 });
    }

    /// Notify the device that there are new buffers in a queue.
    pub fn notify_queue(&self, queue: u32) {
        // Read the queue's notify offset from the common config
        self.select_queue(queue);
        let queue_notify_off = self.common.read_u16(COMMON_Q_NOFF) as u32;

        // The notification address is:
        //   notify_base + queue_notify_off * notify_off_multiplier
        let offset = (queue_notify_off * self.notify_off_multiplier) as usize;

        // Write the queue index to the notification address
        // For VirtIO 1.0, we write a u16 value of the queue index
        unsafe {
            let addr = (self.notify.virt_base + offset as u64) as *mut u16;
            core::ptr::write_volatile(addr, queue as u16);
        }
    }

    // =========================================================================
    // Interrupt Handling
    // =========================================================================

    /// Read the ISR status register.
    ///
    /// Bit 0: Queue interrupt
    /// Bit 1: Configuration change
    /// Reading this register clears it.
    pub fn read_interrupt_status(&self) -> u32 {
        if self.isr.is_valid() {
            self.isr.read_u8(0) as u32
        } else {
            0
        }
    }

    /// Acknowledge interrupts.
    pub fn ack_interrupt(&self, _flags: u32) {
        // For modern PCI transport, reading ISR auto-acknowledges.
        // This is a no-op but kept for interface compatibility with MMIO.
    }

    // =========================================================================
    // Device Configuration
    // =========================================================================

    /// Read the configuration generation counter.
    pub fn config_generation(&self) -> u32 {
        self.common.read_u8(COMMON_CFGGEN) as u32
    }

    /// Read a u8 from device-specific configuration.
    pub fn read_config_u8(&self, offset: usize) -> u8 {
        if !self.device_cfg.is_valid() {
            return 0;
        }
        self.device_cfg.read_u8(offset)
    }

    /// Read a u32 from device-specific configuration.
    pub fn read_config_u32(&self, offset: usize) -> u32 {
        if !self.device_cfg.is_valid() {
            return 0;
        }
        self.device_cfg.read_u32(offset)
    }

    /// Read a u64 from device-specific configuration (two u32 reads).
    pub fn read_config_u64(&self, offset: usize) -> u64 {
        let low = self.read_config_u32(offset) as u64;
        let high = self.read_config_u32(offset + 4) as u64;
        (high << 32) | low
    }

    /// Get a reference to the underlying PCI device.
    pub fn pci_device(&self) -> &PciDevice {
        &self.pci_dev
    }
}

// =============================================================================
// Helpers
// =============================================================================

/// Read a dword from PCI config space at an arbitrary offset (for capability walking).
fn pci_read_cap_dword(dev: &PciDevice, offset: u8) -> u32 {
    // pci_read_config_dword aligns to 4 bytes, so we can use the offset directly
    // if it's already dword-aligned. For capability fields, offsets are designed
    // to be properly aligned.
    let dword_offset = offset & 0xFC;
    let dword = pci::pci_read_config_byte(dev.bus, dev.device, dev.function, dword_offset) as u32
        | (pci::pci_read_config_byte(dev.bus, dev.device, dev.function, dword_offset + 1) as u32) << 8
        | (pci::pci_read_config_byte(dev.bus, dev.device, dev.function, dword_offset + 2) as u32) << 16
        | (pci::pci_read_config_byte(dev.bus, dev.device, dev.function, dword_offset + 3) as u32) << 24;
    dword
}

/// Convert PCI device ID to VirtIO device type ID.
///
/// Modern VirtIO devices: PCI device ID = 0x1040 + VirtIO device type
/// Transitional (legacy) devices: PCI device ID 0x1000-0x103F map to specific types
fn pci_device_id_to_virtio(dev: &PciDevice) -> u32 {
    let pci_id = dev.device_id;

    // Modern VirtIO 1.0+ devices: device_id = 0x1040 + virtio_type
    if pci_id >= 0x1040 && pci_id <= 0x107F {
        return (pci_id - 0x1040) as u32;
    }

    // Transitional (legacy) devices
    match pci_id {
        0x1000 => device_id::NETWORK,
        0x1001 => device_id::BLOCK,
        0x1003 => device_id::CONSOLE,
        0x1019 => device_id::SOUND,
        _ => 0, // Unknown
    }
}

/// Enumerate all VirtIO PCI devices found during PCI bus scan.
///
/// Returns a vector of initialized `VirtioPciDevice` wrappers for each
/// VirtIO device that supports the modern PCI transport.
pub fn enumerate_virtio_pci_devices() -> alloc::vec::Vec<VirtioPciDevice> {
    let mut devices = alloc::vec::Vec::new();

    if let Some(pci_devices) = pci::get_devices() {
        for pci_dev in pci_devices {
            if pci_dev.vendor_id == pci::VIRTIO_VENDOR_ID {
                if let Some(virtio_dev) = VirtioPciDevice::probe(pci_dev) {
                    devices.push(virtio_dev);
                }
            }
        }
    }

    devices
}

/// Get a human-readable name for a VirtIO device type.
pub fn device_type_name(device_id: u32) -> &'static str {
    match device_id {
        device_id::NETWORK => "network",
        device_id::BLOCK => "block",
        device_id::CONSOLE => "console",
        device_id::GPU => "GPU",
        device_id::INPUT => "input",
        device_id::SOUND => "sound",
        _ => "unknown",
    }
}
