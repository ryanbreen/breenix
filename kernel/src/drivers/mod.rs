//! Device drivers subsystem
//!
//! This module provides the driver infrastructure for Breenix, including
//! PCI enumeration and device-specific drivers like VirtIO.

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

    // TEMPORARILY DISABLED: Initialize VirtIO block driver if device was found
    // Uncomment this block once boot issues are resolved
    /*
    match virtio::block::init() {
        Ok(()) => {
            log::info!("VirtIO block driver initialized successfully");

            // Run a quick test
            if let Err(e) = virtio::block::test_read() {
                log::warn!("VirtIO block test failed: {}", e);
            }
        }
        Err(e) => {
            log::warn!("VirtIO block driver initialization failed: {}", e);
        }
    }
    */
    log::info!("VirtIO block driver temporarily disabled for debugging");

    log::info!("Driver subsystem initialized");
    device_count
}
