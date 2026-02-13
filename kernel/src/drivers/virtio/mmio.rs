//! VirtIO MMIO Transport Layer
//!
//! Implements the VirtIO MMIO interface for device communication on ARM64.
//! QEMU virt machine provides VirtIO devices via memory-mapped I/O.
//!
//! # VirtIO MMIO v2 Register Layout (per spec v1.1)
//!
//! | Offset | Name | Direction |
//! |--------|------|-----------|
//! | 0x000 | MagicValue | R |
//! | 0x004 | Version | R |
//! | 0x008 | DeviceID | R |
//! | 0x00c | VendorID | R |
//! | 0x010 | DeviceFeatures | R |
//! | 0x014 | DeviceFeaturesSel | W |
//! | 0x020 | DriverFeatures | W |
//! | 0x024 | DriverFeaturesSel | W |
//! | 0x030 | QueueSel | W |
//! | 0x034 | QueueNumMax | R |
//! | 0x038 | QueueNum | W |
//! | 0x044 | QueueReady | RW |
//! | 0x050 | QueueNotify | W |
//! | 0x060 | InterruptStatus | R |
//! | 0x064 | InterruptACK | W |
//! | 0x070 | Status | RW |
//! | 0x0fc | ConfigGeneration | R |
//! | 0x100+ | Config | RW |

use core::ptr::{read_volatile, write_volatile};

/// VirtIO MMIO magic value ("virt" in little-endian)
pub const VIRTIO_MMIO_MAGIC: u32 = 0x74726976;

/// VirtIO MMIO version (v2 modern)
pub const VIRTIO_MMIO_VERSION_2: u32 = 2;
/// VirtIO MMIO version (v1 legacy)
pub const VIRTIO_MMIO_VERSION_1: u32 = 1;

/// VirtIO device status bits
pub mod status {
    pub const ACKNOWLEDGE: u32 = 1;
    pub const DRIVER: u32 = 2;
    pub const DRIVER_OK: u32 = 4;
    pub const FEATURES_OK: u32 = 8;
    pub const DEVICE_NEEDS_RESET: u32 = 64;
    pub const FAILED: u32 = 128;
}

/// VirtIO device IDs
pub mod device_id {
    pub const NETWORK: u32 = 1;
    pub const BLOCK: u32 = 2;
    pub const CONSOLE: u32 = 3;
    pub const ENTROPY: u32 = 4;
    pub const BALLOON: u32 = 5;
    pub const SCSI: u32 = 8;
    pub const GPU: u32 = 16;
    pub const INPUT: u32 = 18;
    pub const SOUND: u32 = 25;
}

/// VirtIO MMIO register offsets
mod regs {
    pub const MAGIC: usize = 0x000;
    pub const VERSION: usize = 0x004;
    pub const DEVICE_ID: usize = 0x008;
    pub const VENDOR_ID: usize = 0x00c;
    pub const DEVICE_FEATURES: usize = 0x010;
    pub const DEVICE_FEATURES_SEL: usize = 0x014;
    pub const DRIVER_FEATURES: usize = 0x020;
    pub const DRIVER_FEATURES_SEL: usize = 0x024;
    // Legacy (v1) registers
    pub const GUEST_PAGE_SIZE: usize = 0x028;  // v1 only
    pub const QUEUE_SEL: usize = 0x030;
    pub const QUEUE_NUM_MAX: usize = 0x034;
    pub const QUEUE_NUM: usize = 0x038;
    pub const QUEUE_ALIGN: usize = 0x03c;      // v1 only
    pub const QUEUE_PFN: usize = 0x040;        // v1 only - Page Frame Number
    pub const QUEUE_READY: usize = 0x044;      // v2 only
    pub const QUEUE_NOTIFY: usize = 0x050;
    pub const INTERRUPT_STATUS: usize = 0x060;
    pub const INTERRUPT_ACK: usize = 0x064;
    pub const STATUS: usize = 0x070;
    // v2 only registers
    pub const QUEUE_DESC_LOW: usize = 0x080;
    pub const QUEUE_DESC_HIGH: usize = 0x084;
    pub const QUEUE_AVAIL_LOW: usize = 0x090;
    pub const QUEUE_AVAIL_HIGH: usize = 0x094;
    pub const QUEUE_USED_LOW: usize = 0x0a0;
    pub const QUEUE_USED_HIGH: usize = 0x0a4;
    pub const CONFIG_GENERATION: usize = 0x0fc;
    pub const CONFIG: usize = 0x100;
}

/// QEMU virt machine VirtIO MMIO base addresses
/// Each device has a 0x200 byte region
pub const VIRTIO_MMIO_BASE: u64 = 0x0a00_0000;
pub const VIRTIO_MMIO_SIZE: u64 = 0x200;
pub const VIRTIO_MMIO_COUNT: usize = 32;  // QEMU virt has up to 32 virtio-mmio devices

/// VirtIO MMIO device abstraction
pub struct VirtioMmioDevice {
    /// Base MMIO address
    base: u64,
    /// Device ID (0 = not present)
    device_id: u32,
    /// Device version
    version: u32,
    /// Features offered by the device
    device_features: u64,
    /// Features selected by the driver
    driver_features: u64,
}

impl VirtioMmioDevice {
    /// Get the base (virtual) MMIO address for this device.
    pub fn base(&self) -> u64 {
        self.base
    }

    /// Read a 32-bit register
    #[inline]
    fn read32(&self, offset: usize) -> u32 {
        unsafe { read_volatile((self.base + offset as u64) as *const u32) }
    }

    /// Write a 32-bit register
    #[inline]
    fn write32(&self, offset: usize, value: u32) {
        unsafe { write_volatile((self.base + offset as u64) as *mut u32, value) }
    }

    /// Probe a VirtIO MMIO device at the given base address
    ///
    /// Returns None if no device is present (magic value mismatch or device_id == 0)
    pub fn probe(base: u64) -> Option<Self> {
        // Convert physical MMIO base to kernel virtual address
        let virt_base = crate::memory::physical_memory_offset().as_u64() + base;

        let device = VirtioMmioDevice {
            base: virt_base,
            device_id: 0,
            version: 0,
            device_features: 0,
            driver_features: 0,
        };

        // Check magic value
        let magic = device.read32(regs::MAGIC);
        if magic != VIRTIO_MMIO_MAGIC {
            return None;
        }

        // Check version
        let version = device.read32(regs::VERSION);
        if version != VIRTIO_MMIO_VERSION_2 && version != VIRTIO_MMIO_VERSION_1 {
            return None;
        }

        // Check device ID (0 = no device)
        let device_id = device.read32(regs::DEVICE_ID);
        if device_id == 0 {
            return None;
        }

        Some(VirtioMmioDevice {
            base: virt_base,
            device_id,
            version,
            device_features: 0,
            driver_features: 0,
        })
    }

    /// Get the device ID
    pub fn device_id(&self) -> u32 {
        self.device_id
    }

    /// Get the device version
    pub fn version(&self) -> u32 {
        self.version
    }

    /// Get vendor ID
    pub fn vendor_id(&self) -> u32 {
        self.read32(regs::VENDOR_ID)
    }

    /// Get device features (must be called after init())
    pub fn device_features(&self) -> u64 {
        self.device_features
    }

    /// Read device status
    pub fn read_status(&self) -> u32 {
        self.read32(regs::STATUS)
    }

    /// Write device status
    pub fn write_status(&self, status: u32) {
        self.write32(regs::STATUS, status);
    }

    /// Reset the device
    pub fn reset(&self) {
        self.write_status(0);
        // Memory barrier
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    }

    /// Read device features (both low and high 32 bits)
    pub fn read_device_features(&mut self) -> u64 {
        // Select feature bits 0-31
        self.write32(regs::DEVICE_FEATURES_SEL, 0);
        let low = self.read32(regs::DEVICE_FEATURES) as u64;

        // Select feature bits 32-63
        self.write32(regs::DEVICE_FEATURES_SEL, 1);
        let high = self.read32(regs::DEVICE_FEATURES) as u64;

        self.device_features = (high << 32) | low;
        self.device_features
    }

    /// Write driver features
    pub fn write_driver_features(&mut self, features: u64) {
        self.driver_features = features;

        // Write low 32 bits
        self.write32(regs::DRIVER_FEATURES_SEL, 0);
        self.write32(regs::DRIVER_FEATURES, features as u32);

        // Write high 32 bits
        self.write32(regs::DRIVER_FEATURES_SEL, 1);
        self.write32(regs::DRIVER_FEATURES, (features >> 32) as u32);
    }

    /// Select a virtqueue for configuration
    pub fn select_queue(&self, queue: u32) {
        self.write32(regs::QUEUE_SEL, queue);
    }

    /// Get the maximum size of the selected queue
    pub fn get_queue_num_max(&self) -> u32 {
        self.read32(regs::QUEUE_NUM_MAX)
    }

    /// Set the size of the selected queue
    pub fn set_queue_num(&self, num: u32) {
        self.write32(regs::QUEUE_NUM, num);
    }

    /// Set guest page size (v1 only) - must be called before queue setup
    pub fn set_guest_page_size(&self, size: u32) {
        self.write32(regs::GUEST_PAGE_SIZE, size);
    }

    /// Set queue alignment (v1 only)
    pub fn set_queue_align(&self, align: u32) {
        self.write32(regs::QUEUE_ALIGN, align);
    }

    /// Set queue PFN (Page Frame Number) - v1 only
    /// This is the physical address divided by guest page size
    pub fn set_queue_pfn(&self, pfn: u32) {
        self.write32(regs::QUEUE_PFN, pfn);
    }

    /// Set queue ready (v2 only)
    pub fn set_queue_ready(&self, ready: bool) {
        self.write32(regs::QUEUE_READY, if ready { 1 } else { 0 });
    }

    /// Check if queue is ready
    pub fn is_queue_ready(&self) -> bool {
        self.read32(regs::QUEUE_READY) != 0
    }

    /// Set queue descriptor table address (v2 only)
    pub fn set_queue_desc(&self, addr: u64) {
        self.write32(regs::QUEUE_DESC_LOW, addr as u32);
        self.write32(regs::QUEUE_DESC_HIGH, (addr >> 32) as u32);
    }

    /// Set queue available ring address (v2 only)
    pub fn set_queue_avail(&self, addr: u64) {
        self.write32(regs::QUEUE_AVAIL_LOW, addr as u32);
        self.write32(regs::QUEUE_AVAIL_HIGH, (addr >> 32) as u32);
    }

    /// Set queue used ring address (v2 only)
    pub fn set_queue_used(&self, addr: u64) {
        self.write32(regs::QUEUE_USED_LOW, addr as u32);
        self.write32(regs::QUEUE_USED_HIGH, (addr >> 32) as u32);
    }

    /// Notify the device about a queue
    pub fn notify_queue(&self, queue: u32) {
        self.write32(regs::QUEUE_NOTIFY, queue);
    }

    /// Read interrupt status
    pub fn read_interrupt_status(&self) -> u32 {
        self.read32(regs::INTERRUPT_STATUS)
    }

    /// Acknowledge interrupts
    pub fn ack_interrupt(&self, flags: u32) {
        self.write32(regs::INTERRUPT_ACK, flags);
    }

    /// Read config generation counter
    pub fn config_generation(&self) -> u32 {
        self.read32(regs::CONFIG_GENERATION)
    }

    /// Read a byte from device config space
    pub fn read_config_u8(&self, offset: usize) -> u8 {
        unsafe { read_volatile((self.base + regs::CONFIG as u64 + offset as u64) as *const u8) }
    }

    /// Read a u32 from device config space
    pub fn read_config_u32(&self, offset: usize) -> u32 {
        unsafe { read_volatile((self.base + regs::CONFIG as u64 + offset as u64) as *const u32) }
    }

    /// Read a u64 from device config space
    pub fn read_config_u64(&self, offset: usize) -> u64 {
        let low = self.read_config_u32(offset) as u64;
        let high = self.read_config_u32(offset + 4) as u64;
        (high << 32) | low
    }

    /// Initialize the device
    ///
    /// Performs the VirtIO initialization sequence:
    /// 1. Reset device
    /// 2. Set ACKNOWLEDGE status
    /// 3. Set DRIVER status
    /// 4. Read and negotiate features
    /// 5. Set FEATURES_OK
    /// 6. Verify FEATURES_OK
    pub fn init(&mut self, requested_features: u64) -> Result<(), &'static str> {
        // Step 1: Reset the device
        self.reset();

        // Wait for reset to complete
        for _ in 0..1000 {
            if self.read_status() == 0 {
                break;
            }
            for _ in 0..1000 {
                core::hint::spin_loop();
            }
        }

        // Step 2: Set ACKNOWLEDGE status bit
        self.write_status(status::ACKNOWLEDGE);

        // Step 3: Set DRIVER status bit
        self.write_status(status::ACKNOWLEDGE | status::DRIVER);

        // Step 4: Read device features and negotiate
        let device_features = self.read_device_features();
        let negotiated = device_features & requested_features;
        self.write_driver_features(negotiated);

        // Step 5: Set FEATURES_OK
        self.write_status(status::ACKNOWLEDGE | status::DRIVER | status::FEATURES_OK);

        // Step 6: Verify FEATURES_OK is still set
        if (self.read_status() & status::FEATURES_OK) == 0 {
            self.write_status(status::FAILED);
            return Err("Device did not accept features");
        }

        Ok(())
    }

    /// Mark the device as ready (set DRIVER_OK)
    pub fn driver_ok(&self) {
        let status = self.read_status();
        self.write_status(status | status::DRIVER_OK);
    }
}

/// Enumerate all VirtIO MMIO devices on QEMU virt machine
pub fn enumerate_devices() -> impl Iterator<Item = VirtioMmioDevice> {
    (0..VIRTIO_MMIO_COUNT).filter_map(|i| {
        let base = VIRTIO_MMIO_BASE + (i as u64) * VIRTIO_MMIO_SIZE;
        VirtioMmioDevice::probe(base)
    })
}

/// Get a human-readable device type name
pub fn device_type_name(device_id: u32) -> &'static str {
    match device_id {
        device_id::NETWORK => "network",
        device_id::BLOCK => "block",
        device_id::CONSOLE => "console",
        device_id::ENTROPY => "entropy",
        device_id::BALLOON => "balloon",
        device_id::SCSI => "SCSI",
        device_id::GPU => "GPU",
        device_id::INPUT => "input",
        device_id::SOUND => "sound",
        _ => "unknown",
    }
}
