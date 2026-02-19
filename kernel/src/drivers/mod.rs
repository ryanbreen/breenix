//! Device drivers subsystem
//!
//! This module provides the driver infrastructure for Breenix, including
//! PCI enumeration and device-specific drivers.

#[cfg(target_arch = "aarch64")]
pub mod ahci;
#[cfg(target_arch = "x86_64")]
pub mod e1000;
pub mod fw_cfg;
pub mod pci;
pub mod virtio;  // Now available on both x86_64 and aarch64

/// Initialize the driver subsystem
///
/// This enumerates devices and initializes any detected devices
/// that have drivers available.
///
/// Returns the number of devices found.
#[cfg(target_arch = "x86_64")]
pub fn init() -> usize {
    log::info!("Initializing driver subsystem...");

    // Enumerate PCI bus and detect devices
    let device_count = pci::enumerate();

    // Initialize VirtIO block driver if device was found
    match virtio::block::init() {
        Ok(()) => {
            log::info!("VirtIO block driver initialized successfully");

            // Enable VirtIO IRQ now that driver is initialized
            // IMPORTANT: Must be done AFTER driver init, not during PIC init
            crate::interrupts::enable_virtio_irq();

            // Run a quick test
            if let Err(e) = virtio::block::test_read() {
                log::warn!("VirtIO block test failed: {}", e);
            }
        }
        Err(e) => {
            log::warn!("VirtIO block driver initialization failed: {}", e);
        }
    }

    // Initialize E1000 network driver if device was found
    match e1000::init() {
        Ok(()) => {
            log::info!("E1000 network driver initialized successfully");

            // Enable E1000 IRQ now that driver is initialized
            // E1000 uses IRQ 10 in QEMU's PCI configuration
            crate::interrupts::enable_irq10();
        }
        Err(e) => {
            log::warn!("E1000 network driver initialization failed: {}", e);
        }
    }

    // Initialize VirtIO sound driver
    match virtio::sound::init() {
        Ok(()) => {
            log::info!("VirtIO sound driver initialized successfully");
        }
        Err(e) => {
            log::warn!("VirtIO sound driver initialization failed: {}", e);
        }
    }

    log::info!("Driver subsystem initialized");
    device_count
}

/// Initialize the driver subsystem (ARM64 version)
///
/// Detects the platform at runtime:
/// - If PCI ECAM is configured (Parallels/UEFI boot): enumerate PCI bus
/// - Otherwise (QEMU virt): enumerate VirtIO MMIO devices
#[cfg(target_arch = "aarch64")]
pub fn init() -> usize {
    use crate::serial_println;

    serial_println!("[drivers] Initializing driver subsystem...");

    let ecam_base = crate::platform_config::pci_ecam_base();

    if ecam_base != 0 {
        // PCI-based platform (Parallels): enumerate PCI bus
        serial_println!("[drivers] PCI ECAM at {:#x}, enumerating PCI bus...", ecam_base);
        let device_count = pci::enumerate();
        serial_println!("[drivers] Found {} PCI devices", device_count);

        // Enumerate VirtIO PCI devices with modern transport
        let virtio_devices = virtio::pci_transport::enumerate_virtio_pci_devices();
        for dev in &virtio_devices {
            serial_println!(
                "[drivers] VirtIO PCI device: {} (type={})",
                virtio::pci_transport::device_type_name(dev.device_id()),
                dev.device_id()
            );
        }
        serial_println!("[drivers] Found {} VirtIO PCI devices", virtio_devices.len());

        // Initialize AHCI storage driver
        match ahci::init() {
            Ok(count) => {
                serial_println!("[drivers] AHCI initialized: {} SATA device(s)", count);
            }
            Err(e) => {
                serial_println!("[drivers] AHCI init skipped: {}", e);
            }
        }

        serial_println!("[drivers] Driver subsystem initialized (PCI)");
        device_count
    } else {
        // MMIO-based platform (QEMU virt): enumerate VirtIO MMIO
        init_virtio_mmio()
    }
}

/// Initialize VirtIO MMIO devices (QEMU virt platform).
#[cfg(target_arch = "aarch64")]
fn init_virtio_mmio() -> usize {
    use crate::serial_println;

    // Enumerate VirtIO MMIO devices
    let mut device_count = 0;
    for device in virtio::mmio::enumerate_devices() {
        let type_name = virtio::mmio::device_type_name(device.device_id());
        serial_println!(
            "[drivers] Found VirtIO MMIO device: {} (ID={}, version={})",
            type_name,
            device.device_id(),
            device.version()
        );
        device_count += 1;
    }

    serial_println!("[drivers] Found {} VirtIO MMIO devices", device_count);

    // Initialize VirtIO block driver
    match virtio::block_mmio::init() {
        Ok(()) => {
            serial_println!("[drivers] VirtIO block driver initialized");
            // Run a quick read test
            if let Err(e) = virtio::block_mmio::test_read() {
                serial_println!("[drivers] VirtIO block test failed: {}", e);
            }
        }
        Err(e) => {
            serial_println!("[drivers] VirtIO block driver init failed: {}", e);
        }
    }

    // Initialize VirtIO network driver
    match virtio::net_mmio::init() {
        Ok(()) => {
            serial_println!("[drivers] VirtIO network driver initialized");
            // Run a quick test
            if let Err(e) = virtio::net_mmio::test_device() {
                serial_println!("[drivers] VirtIO network test failed: {}", e);
            }
        }
        Err(e) => {
            serial_println!("[drivers] VirtIO network driver init failed: {}", e);
        }
    }

    // Initialize VirtIO GPU driver
    match virtio::gpu_mmio::init() {
        Ok(()) => {
            serial_println!("[drivers] VirtIO GPU driver initialized");
            if let Err(e) = virtio::gpu_mmio::test_device() {
                serial_println!("[drivers] VirtIO GPU test failed: {}", e);
            }
        }
        Err(e) => {
            serial_println!("[drivers] VirtIO GPU driver init failed: {}", e);
        }
    }

    // Initialize VirtIO sound driver
    match virtio::sound_mmio::init() {
        Ok(()) => {
            serial_println!("[drivers] VirtIO sound driver initialized");
        }
        Err(e) => {
            serial_println!("[drivers] VirtIO sound driver init failed: {}", e);
        }
    }

    serial_println!("[drivers] Driver subsystem initialized (MMIO)");
    device_count
}
