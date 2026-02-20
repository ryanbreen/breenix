//! USB subsystem for Breenix
//!
//! Provides USB host controller drivers and class drivers:
//! - XHCI host controller driver (USB 3.0)
//! - HID class driver (keyboard + mouse via boot protocol)
//! - USB standard descriptor types

pub mod descriptors;
pub mod hid;
pub mod xhci;
