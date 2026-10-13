//! DevPTS Filesystem (devpts)
//!
//! Provides a virtual filesystem mounted at /dev/pts containing PTY slave devices.
//! Unlike ext2, devptsfs doesn't use disk storage - all nodes are virtual and
//! dynamically generated based on active PTY pairs.
//!
//! # Supported Entries
//!
//! - `/dev/pts/` - Directory containing active PTY slave devices
//! - `/dev/pts/0` - First PTY slave (if allocated and unlocked)
//! - `/dev/pts/1` - Second PTY slave, etc.
//!
//! # Architecture
//!
//! ```text
//! sys_open("/dev/pts/0")
//!         |
//!         v
//!     devpts_lookup("0")
//!         |
//!         v
//!     Check PTY 0 exists and is unlocked
//!         |
//!         v
//!     Return PtySlave file descriptor
//! ```

use spin::Mutex;

use crate::tty::pty;

/// Directory entry for /dev/pts listing
#[derive(Debug, Clone)]
pub struct PtsEntry {
    /// PTY number (0, 1, 2, ...)
    pub pty_num: u32,
    /// Inode number for stat
    pub inode: u64,
}

impl PtsEntry {
    /// The entry name (the number in decimal), written into `buf`.
    pub fn name<'a>(&self, buf: &'a mut [u8; 10]) -> &'a [u8] {
        let mut n = self.pty_num;
        let mut start = buf.len();
        loop {
            start -= 1;
            buf[start] = b'0' + (n % 10) as u8;
            n /= 10;
            if n == 0 {
                break;
            }
        }
        &buf[start..]
    }
}

/// Global devpts state
struct DevptsState {
    /// Whether devpts is initialized
    initialized: bool,
}

impl DevptsState {
    const fn new() -> Self {
        Self { initialized: false }
    }
}

static DEVPTS: Mutex<DevptsState> = Mutex::new(DevptsState::new());

/// Initialize devpts filesystem
pub fn init() {
    let mut devpts = DEVPTS.lock();
    if devpts.initialized {
        log::warn!("devpts already initialized");
        return;
    }

    devpts.initialized = true;
    log::info!("devpts: initialized at /dev/pts");

    // Register mount point
    crate::fs::vfs::mount::mount("/dev/pts", "devpts");
}

/// Whether `/dev/pts` shows pair `pty_num`. As on Linux, a pair is listed
/// from allocation, locked or not, until its master closes.
fn visible(pty_num: u32) -> bool {
    pty::get(pty_num).is_some_and(|pair| !pair.master_closed())
}

/// Look up a PTY slave device by name (the number as string, e.g., "0", "1")
///
/// Returns the PTY number if the name is a valid number and that pair is
/// visible. Opening a locked pair's slave fails with EIO (see
/// `pty::open_slave`), not ENOENT.
pub fn lookup(name: &str) -> Option<u32> {
    let pty_num: u32 = name.parse().ok()?;
    visible(pty_num).then_some(pty_num)
}

/// Look up a PTY slave by inode number
///
/// For devpts, the inode number is derived from the PTY number.
/// We use a base offset to avoid collision with other filesystem inodes.
pub fn lookup_by_inode(inode: u64) -> Option<u32> {
    // Inode = PTY_INODE_BASE + pty_num
    const PTY_INODE_BASE: u64 = 0x10000;

    if inode < PTY_INODE_BASE {
        return None;
    }

    let pty_num = (inode - PTY_INODE_BASE) as u32;
    visible(pty_num).then_some(pty_num)
}

/// Get inode number for a PTY slave
pub fn get_inode(pty_num: u32) -> u64 {
    const PTY_INODE_BASE: u64 = 0x10000;
    PTY_INODE_BASE + pty_num as u64
}

/// The first visible PTY slave entry numbered above `after` (or the lowest,
/// for None). Listing walks these without allocating.
pub fn next_entry(after: Option<u32>) -> Option<PtsEntry> {
    let mut after = after;
    while let Some(pty_num) = pty::next_active(after) {
        if visible(pty_num) {
            return Some(PtsEntry {
                pty_num,
                inode: get_inode(pty_num),
            });
        }
        after = Some(pty_num);
    }
    None
}

/// Check if devpts is initialized
pub fn is_initialized() -> bool {
    DEVPTS.lock().initialized
}

/// Get device numbers for a PTY slave
///
/// PTY slaves use major number 136 (standard Unix/Linux convention)
/// and minor number = PTY number
pub fn get_device_numbers(pty_num: u32) -> (u32, u32) {
    const PTY_SLAVE_MAJOR: u32 = 136;
    (PTY_SLAVE_MAJOR, pty_num)
}

/// Get the combined device number (major << 8 | minor) for stat st_rdev
pub fn get_rdev(pty_num: u32) -> u64 {
    let (major, minor) = get_device_numbers(pty_num);
    ((major as u64) << 8) | (minor as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_inode() {
        assert_eq!(get_inode(0), 0x10000);
        assert_eq!(get_inode(1), 0x10001);
        assert_eq!(get_inode(255), 0x100ff);
    }

    #[test]
    fn test_get_device_numbers() {
        let (major, minor) = get_device_numbers(0);
        assert_eq!(major, 136);
        assert_eq!(minor, 0);

        let (major, minor) = get_device_numbers(5);
        assert_eq!(major, 136);
        assert_eq!(minor, 5);
    }

    #[test]
    fn test_get_rdev() {
        // major=136 (0x88), minor=0: rdev = 0x8800
        assert_eq!(get_rdev(0), 0x8800);
        // major=136 (0x88), minor=5: rdev = 0x8805
        assert_eq!(get_rdev(5), 0x8805);
    }
}
