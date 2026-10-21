//! Device Filesystem (devfs)
//!
//! Provides a virtual filesystem mounted at /dev containing device nodes.
//! Unlike ext2, devfs doesn't use disk storage - all nodes are virtual.
//!
//! # Supported Devices
//!
//! - `/dev/null` - Discards all writes, reads return EOF
//! - `/dev/zero` - Discards all writes, reads return zero bytes
//! - `/dev/console` - System console: tty1 (serial and the screen)
//! - `/dev/tty` - Current process's controlling terminal
//! - `/dev/tty0` - The foreground virtual terminal
//! - `/dev/tty1` .. `/dev/tty6` - The virtual terminals
//! - `/dev/vcs`, `/dev/vcsa` and their numbered forms - The terminals' screens
//!   (`docs/console/contract.md` section 3)
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

pub mod entries;
pub use entries::{SHM_DIRECTORY_INO, SHM_DIRECTORY_MODE};

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
    /// /dev/rtc0 - read-only platform real-time clock
    Rtc,
    /// /dev/ptmx - pseudo-terminal multiplexor: each open makes a new master
    Ptmx,
    /// /dev/tty0 - the foreground virtual terminal at each operation
    Tty0,
    /// /dev/tty1 .. /dev/tty6 - a virtual terminal (1-based)
    Vt(u8),
    /// /dev/vcs, /dev/vcs1 .. 6 - a terminal's screen text (0: the foreground)
    Vcs(u8),
    /// /dev/vcsa, /dev/vcsa1 .. 6 - the screen with attributes (0: the foreground)
    Vcsa(u8),
}

/// The tty group, which owns the virtual terminals.
pub const TTY_GID: u32 = 5;

const VT_NAMES: [&str; 7] = ["tty0", "tty1", "tty2", "tty3", "tty4", "tty5", "tty6"];
const VCS_NAMES: [&str; 7] = ["vcs", "vcs1", "vcs2", "vcs3", "vcs4", "vcs5", "vcs6"];
const VCSA_NAMES: [&str; 7] = ["vcsa", "vcsa1", "vcsa2", "vcsa3", "vcsa4", "vcsa5", "vcsa6"];

impl DeviceType {
    /// Get the device name (without /dev/ prefix)
    pub fn name(&self) -> &'static str {
        match self {
            DeviceType::Null => "null",
            DeviceType::Zero => "zero",
            DeviceType::Console => "console",
            DeviceType::Tty => "tty",
            DeviceType::Rtc => "rtc0",
            DeviceType::Ptmx => "ptmx",
            DeviceType::Tty0 => VT_NAMES[0],
            DeviceType::Vt(n) => VT_NAMES[*n as usize],
            DeviceType::Vcs(n) => VCS_NAMES[*n as usize],
            DeviceType::Vcsa(n) => VCSA_NAMES[*n as usize],
        }
    }

    /// Get the inode number for this device
    /// Using fixed inode numbers for devices
    pub fn inode(&self) -> u64 {
        match self {
            DeviceType::Null => 2,
            DeviceType::Zero => 3,
            DeviceType::Console => 4,
            DeviceType::Tty => 5,
            DeviceType::Rtc => 6,
            DeviceType::Ptmx => 7,
            DeviceType::Tty0 => 8,
            DeviceType::Vt(n) => 10 + *n as u64,
            DeviceType::Vcs(n) => 20 + *n as u64,
            DeviceType::Vcsa(n) => 30 + *n as u64,
        }
    }

    /// The virtual terminal (1-based) a terminal device names now: tty1 for
    /// /dev/console, the foreground for /dev/tty0. No terminal for other devices.
    pub fn terminal(&self) -> Option<usize> {
        match self {
            DeviceType::Console => Some(1),
            DeviceType::Vt(n) => Some(*n as usize),
            DeviceType::Tty0 => Some(crate::console::vt::foreground()),
            _ => None,
        }
    }

    /// The node's type, permission bits, owner and group, which open, stat
    /// and access checks read (`docs/users/design.md`, section 2). The
    /// data sinks and sources, the controlling terminal and the multiplexor
    /// are for everyone; the console and the screens are root's; the virtual
    /// terminals are root's and writable by the tty group; the clock is
    /// read-only.
    pub fn attrs(&self) -> crate::fs::attrs::InodeAttrs {
        let (permissions, gid) = match self {
            DeviceType::Null | DeviceType::Zero | DeviceType::Tty | DeviceType::Ptmx => (0o666, 0),
            DeviceType::Console | DeviceType::Vcs(_) | DeviceType::Vcsa(_) => (0o600, 0),
            DeviceType::Tty0 | DeviceType::Vt(_) => (0o620, TTY_GID),
            DeviceType::Rtc => (0o444, 0),
        };
        crate::fs::attrs::InodeAttrs::new(0o020000 | permissions, 0, gid)
    }

    /// Check if this device is readable
    pub fn is_readable(&self) -> bool {
        true // All devices are readable
    }

    /// Check if this device is writable
    pub fn is_writable(&self) -> bool {
        *self != DeviceType::Rtc
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
    devfs.devices.push(DeviceNode::new(DeviceType::Null, 1, 3)); // /dev/null
    devfs.devices.push(DeviceNode::new(DeviceType::Zero, 1, 5)); // /dev/zero

    // Major 5 = TTY devices
    devfs
        .devices
        .push(DeviceNode::new(DeviceType::Console, 5, 1)); // /dev/console
    devfs.devices.push(DeviceNode::new(DeviceType::Tty, 5, 0)); // /dev/tty
    devfs.devices.push(DeviceNode::new(DeviceType::Ptmx, 5, 2)); // /dev/ptmx

    // Major 4 = virtual terminals, major 7 = their screens (`devices.txt`).
    devfs.devices.push(DeviceNode::new(DeviceType::Tty0, 4, 0));
    for n in 1..=6u8 {
        devfs.devices.push(DeviceNode::new(DeviceType::Vt(n), 4, u32::from(n)));
    }
    for n in 0..=6u8 {
        devfs.devices.push(DeviceNode::new(DeviceType::Vcs(n), 7, u32::from(n)));
        devfs.devices.push(DeviceNode::new(DeviceType::Vcsa(n), 7, 128 + u32::from(n)));
    }

    #[cfg(target_arch = "aarch64")]
    let has_rtc = crate::platform_config::is_qemu() || crate::time::firmware_rtc::available();
    #[cfg(not(target_arch = "aarch64"))]
    let has_rtc = true;
    if has_rtc {
        devfs.devices.push(DeviceNode::new(DeviceType::Rtc, 254, 0));
    }

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

/// Resolve a native device inode through the registered driver table.
pub fn lookup_by_rdev(rdev: u64) -> Option<DeviceNode> {
    DEVFS
        .lock()
        .devices
        .iter()
        .find(|device| device.rdev() == rdev)
        .cloned()
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
    let mut names: Vec<String> = devfs
        .devices
        .iter()
        .map(|d| String::from(d.device_type.name()))
        .collect();
    names.push(String::from("shm"));
    names.push(String::from("mqueue"));
    names
}

/// Snapshot directory metadata without allocating per-name strings.
pub fn directory_entries() -> Result<Vec<entries::DirectoryEntry>, u64> {
    let devfs = DEVFS.lock();
    entries::entries(
        devfs
            .devices
            .iter()
            .map(|d| (d.device_type.name(), d.device_type.inode())),
    )
}

/// Check if devfs is initialized
pub fn is_initialized() -> bool {
    DEVFS.lock().initialized
}

/// Device file operations - read from device
pub fn device_read(device_type: DeviceType, buf: &mut [u8]) -> Result<usize, i32> {
    match device_type {
        DeviceType::Rtc => Err(-22), // RTC data is read through RTC_RD_TIME.
        DeviceType::Ptmx => Err(-6), // Opening /dev/ptmx gives a master, never this device.
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
            // This device API is non-sleeping; sys_read owns the blocking adapter.
            crate::ipc::stdin::read_bytes(buf).map_err(|e| -e)
        }
        DeviceType::Tty0 | DeviceType::Vt(_) => match device_type.terminal() {
            Some(1) => crate::ipc::stdin::read_bytes(buf).map_err(|e| -e),
            Some(number) => match crate::ipc::stdin::vt_input(number - 1) {
                Some(input) => input.read_or_prepare(buf, true).map_err(|_| -11),
                None => Err(-6),
            },
            None => Err(-6),
        },
        // The screens are read through their own descriptors (`console::vcs`).
        DeviceType::Vcs(_) | DeviceType::Vcsa(_) => Err(-22),
    }
}

/// Device file operations - write to device
pub fn device_write(device_type: DeviceType, buf: &[u8]) -> Result<usize, i32> {
    match device_type {
        DeviceType::Rtc => Err(13), // EACCES: no clock writes through this device.
        DeviceType::Ptmx => Err(6), // ENXIO: opening /dev/ptmx gives a master.
        DeviceType::Null | DeviceType::Zero => {
            // /dev/null and /dev/zero discard all writes
            Ok(buf.len())
        }
        DeviceType::Console | DeviceType::Tty => {
            // Console/TTY write - output to serial port
            crate::tty::driver::write_unprocessed(buf);
            Ok(buf.len())
        }
        DeviceType::Tty0 | DeviceType::Vt(_) => match device_type.terminal().and_then(crate::tty::driver::vt) {
            Some(tty) => {
                tty.write_bytes(buf);
                Ok(buf.len())
            }
            None => Err(6),
        },
        DeviceType::Vcs(_) | DeviceType::Vcsa(_) => Err(22),
    }
}
