//! Device drivers subsystem
//!
//! This module provides the driver infrastructure for Breenix, including
//! PCI enumeration and device-specific drivers like VirtIO.

pub mod pci;

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

    log::info!("Driver subsystem initialized");
    device_count
}
