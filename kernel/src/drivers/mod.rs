//! Device drivers subsystem
//!
//! This module provides the driver infrastructure for Breenix, including
//! PCI enumeration and device-specific drivers.

pub mod e1000;
pub mod pci;
pub mod virtio;

/// Initialize the driver subsystem
///
/// This enumerates PCI devices and initializes any detected devices
/// that have drivers available.
///
/// Returns the number of PCI devices found.
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
            // TODO: Enable E1000 IRQ when interrupt handler is wired up
        }
        Err(e) => {
            log::warn!("E1000 network driver initialization failed: {}", e);
        }
    }

    log::info!("Driver subsystem initialized");
    device_count
}
