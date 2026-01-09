//! Device Filesystem (devfs)
//!
//! Provides a virtual filesystem mounted at /dev containing device nodes.
//! Unlike ext2, devfs doesn't use disk storage - all nodes are virtual.
//!
//! # Supported Devices
//!
//! - `/dev/null` - Discards all writes, reads return EOF
//! - `/dev/zero` - Discards all writes, reads return zero bytes
//! - `/dev/console` - System console (serial output)
//! - `/dev/tty` - Current process's controlling terminal
//!
//! # Architecture
//!
//! ```text
//! sys_open("/dev/null")
//!         |
//!         v
//!     devfs_open()
//!         |
//!         v
//!     DeviceFile { device_type: DevNull }
//!         |
//!         v
//!     sys_read/sys_write dispatch to device handlers
//! ```

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

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

/// Global devfs state
struct DevfsState {
    /// Registered devices
    devices: Vec<DeviceNode>,
    /// Whether devfs is initialized
    initialized: bool,
}

impl DevfsState {
    const fn new() -> Self {
        Self {
            devices: Vec::new(),
            initialized: false,
        }
    }
}

static DEVFS: Mutex<DevfsState> = Mutex::new(DevfsState::new());

/// Initialize devfs with standard devices
pub fn init() {
    let mut devfs = DEVFS.lock();
    if devfs.initialized {
        log::warn!("devfs already initialized");
        return;
    }

    // Register standard devices
    // Major 1 = memory devices (null, zero, etc.)
    devfs.devices.push(DeviceNode::new(DeviceType::Null, 1, 3));    // /dev/null
    devfs.devices.push(DeviceNode::new(DeviceType::Zero, 1, 5));    // /dev/zero

    // Major 5 = TTY devices
    devfs.devices.push(DeviceNode::new(DeviceType::Console, 5, 1)); // /dev/console
    devfs.devices.push(DeviceNode::new(DeviceType::Tty, 5, 0));     // /dev/tty

    devfs.initialized = true;
    log::info!("devfs: initialized with {} devices", devfs.devices.len());

    // Register mount point
    crate::fs::vfs::mount::mount("/dev", "devfs");
}

/// Look up a device by path (without /dev/ prefix)
pub fn lookup(name: &str) -> Option<DeviceNode> {
    let devfs = DEVFS.lock();
    for device in &devfs.devices {
        if device.device_type.name() == name {
            return Some(device.clone());
        }
    }
    None
}

/// Look up a device by inode number
pub fn lookup_by_inode(inode: u64) -> Option<DeviceNode> {
    let devfs = DEVFS.lock();
    for device in &devfs.devices {
        if device.device_type.inode() == inode {
            return Some(device.clone());
        }
    }
    None
}

/// List all device names (for /dev directory listing)
pub fn list_devices() -> Vec<String> {
    let devfs = DEVFS.lock();
    devfs.devices.iter().map(|d| String::from(d.device_type.name())).collect()
}

/// Check if devfs is initialized
pub fn is_initialized() -> bool {
    DEVFS.lock().initialized
}

/// Device file operations - read from device
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
            // Console/TTY read - for now return EAGAIN (no input available)
            // In the future, this would read from keyboard buffer
            Err(-11) // EAGAIN
        }
    }
}

/// Device file operations - write to device
pub fn device_write(device_type: DeviceType, buf: &[u8]) -> Result<usize, i32> {
    match device_type {
        DeviceType::Null | DeviceType::Zero => {
            // /dev/null and /dev/zero discard all writes
            Ok(buf.len())
        }
        DeviceType::Console | DeviceType::Tty => {
            // Console/TTY write - output to serial port
            for &byte in buf {
                crate::serial::write_byte(byte);
            }
            Ok(buf.len())
        }
    }
}
