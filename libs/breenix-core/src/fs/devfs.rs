//! Device Filesystem Types (devfs)
//!
//! Provides portable device type definitions and device I/O operations.
//! The global devfs state and initialization remain in the kernel;
//! this module contains only the types and stateless operations.
//!
//! # Supported Devices
//!
//! - `/dev/null` - Discards all writes, reads return EOF
//! - `/dev/zero` - Discards all writes, reads return zero bytes
//! - `/dev/console` - System console (output routed by caller)
//! - `/dev/tty` - Current process's controlling terminal (output routed by caller)

/// Device types supported by devfs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceType {
    /// /dev/null - discards writes, reads return EOF
    Null,
    /// /dev/zero - discards writes, reads return zero bytes
    Zero,
    /// /dev/console - system console (serial port)
    Console,
    /// /dev/tty - controlling terminal
    Tty,
}

impl DeviceType {
    /// Get the device name (without /dev/ prefix)
    pub fn name(&self) -> &'static str {
        match self {
            DeviceType::Null => "null",
            DeviceType::Zero => "zero",
            DeviceType::Console => "console",
            DeviceType::Tty => "tty",
        }
    }

    /// Get the inode number for this device
    /// Using fixed inode numbers for devices
    pub fn inode(&self) -> u64 {
        match self {
            DeviceType::Null => 1,
            DeviceType::Zero => 2,
            DeviceType::Console => 3,
            DeviceType::Tty => 4,
        }
    }

    /// Check if this device is readable
    pub fn is_readable(&self) -> bool {
        true // All devices are readable
    }

    /// Check if this device is writable
    pub fn is_writable(&self) -> bool {
        true // All devices are writable
    }
}

/// A devfs device node
#[derive(Debug, Clone)]
pub struct DeviceNode {
    /// Device type
    pub device_type: DeviceType,
    /// Device major number (for st_rdev)
    pub major: u32,
    /// Device minor number (for st_rdev)
    pub minor: u32,
}

impl DeviceNode {
    /// Create a new device node
    pub fn new(device_type: DeviceType, major: u32, minor: u32) -> Self {
        Self {
            device_type,
            major,
            minor,
        }
    }

    /// Get the combined device number (major << 8 | minor)
    pub fn rdev(&self) -> u64 {
        ((self.major as u64) << 8) | (self.minor as u64)
    }
}

/// Device file operations - read from device
///
/// For Console/Tty, returns EAGAIN — the caller is responsible for
/// routing actual terminal input (kernel uses keyboard buffer, WASM uses JS events).
pub fn device_read(device_type: DeviceType, buf: &mut [u8]) -> Result<usize, i32> {
    match device_type {
        DeviceType::Null => {
            // /dev/null always returns EOF (0 bytes read)
            Ok(0)
        }
        DeviceType::Zero => {
            // /dev/zero fills buffer with zeros
            for byte in buf.iter_mut() {
                *byte = 0;
            }
            Ok(buf.len())
        }
        DeviceType::Console | DeviceType::Tty => {
            // Console/TTY read - return EAGAIN, caller handles actual input
            Err(-11) // EAGAIN
        }
    }
}

/// Device file operations - write to device
///
/// For Console/Tty, returns Ok(len) — the caller is responsible for
/// routing actual output (kernel uses serial port, WASM uses terminal pane).
pub fn device_write(device_type: DeviceType, buf: &[u8]) -> Result<usize, i32> {
    match device_type {
        DeviceType::Null | DeviceType::Zero => {
            // /dev/null and /dev/zero discard all writes
            Ok(buf.len())
        }
        DeviceType::Console | DeviceType::Tty => {
            // Console/TTY write - just report success, caller routes output
            Ok(buf.len())
        }
    }
}
