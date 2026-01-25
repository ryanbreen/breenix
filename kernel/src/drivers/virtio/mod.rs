//! VirtIO Transport Layer
//!
//! Provides VirtIO device communication via platform-specific transports:
//! - x86_64: Legacy I/O port interface (PCI-based)
//! - ARM64: MMIO interface (memory-mapped)
//!
//! # VirtIO Legacy Interface (x86_64)
//!
//! The legacy interface uses I/O ports for device configuration:
//! - Device features, guest features at offsets 0x00-0x07
//! - Queue management at offsets 0x08-0x11
//! - Device status at offset 0x12
//! - ISR status at offset 0x13
//! - Device-specific config at offset 0x14+
//!
//! # VirtIO MMIO Interface (ARM64)
//!
//! The MMIO interface uses memory-mapped registers at fixed addresses.
//! QEMU virt machine provides devices at 0x0a000000+.

#[cfg(target_arch = "x86_64")]
pub mod block;
#[cfg(target_arch = "x86_64")]
pub mod queue;

#[cfg(target_arch = "aarch64")]
pub mod mmio;

#[cfg(target_arch = "x86_64")]
use x86_64::instructions::port::Port;

/// VirtIO device status bits
pub mod status {
    /// Guest OS has found the device and recognized it as a VirtIO device
    pub const ACKNOWLEDGE: u8 = 1;
    /// Guest OS knows how to drive the device
    pub const DRIVER: u8 = 2;
    /// Driver is ready
    pub const DRIVER_OK: u8 = 4;
    /// Feature negotiation complete
    pub const FEATURES_OK: u8 = 8;
    /// Something went wrong in the guest
    pub const FAILED: u8 = 128;
}

/// VirtIO legacy register offsets (x86_64 I/O port interface)
#[cfg(target_arch = "x86_64")]
mod regs {
    pub const DEVICE_FEATURES: u16 = 0x00;
    pub const GUEST_FEATURES: u16 = 0x04;
    pub const QUEUE_ADDRESS: u16 = 0x08;
    pub const QUEUE_SIZE: u16 = 0x0C;
    pub const QUEUE_SELECT: u16 = 0x0E;
    pub const QUEUE_NOTIFY: u16 = 0x10;
    pub const DEVICE_STATUS: u16 = 0x12;
    pub const ISR_STATUS: u16 = 0x13;
    // Device-specific config starts at 0x14
    pub const DEVICE_CONFIG: u16 = 0x14;
}

/// VirtIO device abstraction (x86_64 legacy I/O port interface)
///
/// Provides access to the VirtIO legacy I/O port interface.
/// This is only available on x86_64. ARM64 uses VirtIO MMIO instead.
#[cfg(target_arch = "x86_64")]
pub struct VirtioDevice {
    /// Base I/O port address (from PCI BAR0)
    io_base: u16,
    /// Features offered by the device
    device_features: u32,
    /// Features selected by the driver
    driver_features: u32,
}

#[cfg(target_arch = "x86_64")]
impl VirtioDevice {
    /// Create a new VirtIO device from a PCI BAR0 I/O port address
    pub fn new(io_base: u16) -> Self {
        VirtioDevice {
            io_base,
            device_features: 0,
            driver_features: 0,
        }
    }

    /// Read a byte from device I/O port
    fn read_u8(&self, offset: u16) -> u8 {
        let mut port = Port::<u8>::new(self.io_base + offset);
        unsafe { port.read() }
    }

    /// Write a byte to device I/O port
    fn write_u8(&self, offset: u16, value: u8) {
        let mut port = Port::<u8>::new(self.io_base + offset);
        unsafe { port.write(value) }
    }

    /// Read a u16 from device I/O port
    fn read_u16(&self, offset: u16) -> u16 {
        let mut port = Port::<u16>::new(self.io_base + offset);
        unsafe { port.read() }
    }

    /// Write a u16 to device I/O port
    fn write_u16(&self, offset: u16, value: u16) {
        let mut port = Port::<u16>::new(self.io_base + offset);
        unsafe { port.write(value) }
    }

    /// Read a u32 from device I/O port
    fn read_u32(&self, offset: u16) -> u32 {
        let mut port = Port::<u32>::new(self.io_base + offset);
        unsafe { port.read() }
    }

    /// Write a u32 to device I/O port
    fn write_u32(&self, offset: u16, value: u32) {
        let mut port = Port::<u32>::new(self.io_base + offset);
        unsafe { port.write(value) }
    }

    /// Read device features
    pub fn read_device_features(&self) -> u32 {
        self.read_u32(regs::DEVICE_FEATURES)
    }

    /// Write guest/driver features
    pub fn write_driver_features(&self, features: u32) {
        self.write_u32(regs::GUEST_FEATURES, features);
    }

    /// Read device status
    pub fn read_status(&self) -> u8 {
        self.read_u8(regs::DEVICE_STATUS)
    }

    /// Write device status
    pub fn write_status(&self, status: u8) {
        self.write_u8(regs::DEVICE_STATUS, status);
    }

    /// Reset the device
    pub fn reset(&self) {
        self.write_status(0);
    }

    /// Read ISR status (acknowledges interrupt)
    pub fn read_isr(&self) -> u8 {
        self.read_u8(regs::ISR_STATUS)
    }

    /// Select a virtqueue for configuration
    pub fn select_queue(&self, queue: u16) {
        self.write_u16(regs::QUEUE_SELECT, queue);
    }

    /// Get the size of the currently selected queue
    pub fn get_queue_size(&self) -> u16 {
        self.read_u16(regs::QUEUE_SIZE)
    }
    /// Set the physical address of the currently selected queue
    /// Note: Address must be page-aligned and divided by 4096
    pub fn set_queue_address(&self, phys_addr: u64) {
        let page_num = (phys_addr / 4096) as u32;
        self.write_u32(regs::QUEUE_ADDRESS, page_num);
    }

    /// Get the physical address of the currently selected queue
    /// Returns the PFN (page frame number), multiply by 4096 for actual address
    pub fn get_queue_address(&self) -> u32 {
        self.read_u32(regs::QUEUE_ADDRESS)
    }

    /// Notify the device that there are buffers in a queue
    pub fn notify_queue(&self, queue: u16) {
        self.write_u16(regs::QUEUE_NOTIFY, queue);
    }

    /// Read a u32 from device-specific configuration
    fn read_config_u32(&self, offset: u16) -> u32 {
        self.read_u32(regs::DEVICE_CONFIG + offset)
    }

    /// Read a u64 from device-specific configuration (as two u32s)
    pub fn read_config_u64(&self, offset: u16) -> u64 {
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
    ///
    /// Returns Ok(()) on success, Err with description on failure.
    pub fn init(&mut self, requested_features: u32) -> Result<(), &'static str> {
        // Step 0: Deactivate all queues BEFORE reset
        // This ensures QEMU properly clears its internal queue state (last_avail_idx).
        // Without this, QEMU may retain stale state from the UEFI bootloader.
        for queue_idx in 0..16 {
            self.select_queue(queue_idx);
            // Writing 0 to queue address deactivates the queue
            self.write_u32(regs::QUEUE_ADDRESS, 0);
        }

        // Step 1: Reset the device
        // This is CRITICAL for QEMU when taking over from UEFI bootloader.
        self.reset();

        // Wait for reset to complete - poll status until it reads 0
        // VirtIO spec says device must set status to 0 to indicate reset complete
        let mut reset_attempts = 0;
        loop {
            // Small delay between polls
            for _ in 0..10000 {
                core::hint::spin_loop();
            }

            let status = self.read_status();
            if status == 0 {
                break;
            }

            reset_attempts += 1;
            if reset_attempts >= 100 {
                log::warn!("VirtIO: Device status still {:#x} after {} reset attempts", status, reset_attempts);
                break;
            }
        }

        log::debug!("VirtIO: Reset complete after {} attempts", reset_attempts);

        // Step 2: Set ACKNOWLEDGE status bit
        self.write_status(status::ACKNOWLEDGE);

        // Step 3: Set DRIVER status bit
        self.write_status(status::ACKNOWLEDGE | status::DRIVER);

        // Step 4: Read device features and negotiate
        self.device_features = self.read_device_features();
        self.driver_features = self.device_features & requested_features;
        log::debug!(
            "VirtIO: Device features={:#x}, requested={:#x}, negotiated={:#x}",
            self.device_features, requested_features, self.driver_features
        );
        self.write_driver_features(self.driver_features);

        // Step 5: Set FEATURES_OK
        self.write_status(status::ACKNOWLEDGE | status::DRIVER | status::FEATURES_OK);

        // Step 6: Re-read status to verify FEATURES_OK is still set
        let status = self.read_status();
        if (status & status::FEATURES_OK) == 0 {
            self.write_status(status::FAILED);
            return Err("Device did not accept features");
        }

        Ok(())
    }

    /// Mark the device as ready (set DRIVER_OK)
    ///
    /// Call this after configuring all virtqueues.
    pub fn driver_ok(&self) {
        let status = self.read_status();
        self.write_status(status | status::DRIVER_OK);
    }
}
