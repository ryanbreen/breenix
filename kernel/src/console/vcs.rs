//! `/dev/vcs*` and `/dev/vcsa*`: a terminal's live screen as `vcs(4)`
//! documents it, projected as `docs/console/contract.md` section 3 defines.
//!
//! `vcsN` is one character byte per cell; `vcsaN` is a four-byte header (rows,
//! columns, cursor column, cursor row) and then a little-endian word per cell,
//! the character byte low and the attribute byte (foreground and background
//! palette index as displayed) high. Each read copies from one snapshot of the
//! grid taken under the console's lock. The devices are read-only.

use super::grid::Cell;
use crate::syscall::errno::{EFAULT, EINVAL, ENOMEM};
use alloc::vec::Vec;
use spin::Mutex;

#[derive(Debug)]
pub struct VcsFile {
    /// The terminal (1-based), or 0 for the foreground at each read.
    vt: usize,
    attributes: bool,
    position: Mutex<u64>,
}

impl VcsFile {
    pub fn new(vt: usize, attributes: bool) -> Self {
        Self { vt, attributes, position: Mutex::new(0) }
    }

    /// The device's size for the terminal's grid.
    fn size(&self, cols: usize, rows: usize) -> u64 {
        if self.attributes { 4 + 2 * (rows * cols) as u64 } else { (rows * cols) as u64 }
    }

    fn device(&self) -> crate::fs::devfs::DeviceType {
        use crate::fs::devfs::DeviceType;
        if self.attributes { DeviceType::Vcsa(self.vt as u8) } else { DeviceType::Vcs(self.vt as u8) }
    }
}

/// The character byte: Latin-1 as itself, anything else as `?`.
fn character(cell: Cell) -> u8 {
    u8::try_from(cell.glyph).unwrap_or(b'?')
}

/// The attribute byte: the displayed foreground index low, background high.
fn attribute(cell: Cell) -> u8 {
    let (fg, bg) = cell.displayed_colours();
    (fg as u8) | ((bg as u8) << 4)
}

/// The whole projection of one snapshot.
fn project(attributes: bool, rows: usize, cols: usize, cursor_col: usize, cursor_row: usize, cells: &[Cell]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(if attributes { 4 + 2 * cells.len() } else { cells.len() });
    if attributes {
        bytes.extend_from_slice(&[rows as u8, cols as u8, cursor_col as u8, cursor_row as u8]);
        for &cell in cells {
            bytes.extend_from_slice(&[character(cell), attribute(cell)]);
        }
    } else {
        bytes.extend(cells.iter().map(|&cell| character(cell)));
    }
    bytes
}

impl crate::ipc::object::IpcFile for VcsFile {
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn fill_stat(&self, st: &mut crate::syscall::fs::Stat) {
        let device = self.device();
        st.st_dev = 0xffff0002;
        st.st_ino = device.inode();
        st.st_mode = crate::syscall::fs::S_IFCHR | 0o600;
        st.st_uid = 0;
        st.st_gid = 0;
        st.st_nlink = 1;
        st.st_rdev = crate::fs::devfs::lookup_by_inode(device.inode()).map_or(0, |node| node.rdev());
    }

    fn read(&self, buf: u64, count: usize) -> Result<u64, u64> {
        let (rows, cols, cursor_col, cursor_row, cells) =
            super::vt::screen(self.vt).ok_or(ENOMEM as u64)?;
        let bytes = project(self.attributes, rows, cols, cursor_col, cursor_row, &cells);
        let mut position = self.position.lock();
        let start = (*position).min(bytes.len() as u64) as usize;
        let n = count.min(bytes.len() - start);
        if n > 0 {
            crate::syscall::userptr::write_user_bytes(buf, bytes[start..].as_ptr(), n).map_err(|_| EFAULT as u64)?;
        }
        *position += n as u64;
        Ok(n as u64)
    }

    fn seek(&self, offset: i64, whence: i32) -> Result<u64, u64> {
        const SEEK_SET: i32 = 0;
        const SEEK_CUR: i32 = 1;
        const SEEK_END: i32 = 2;
        let vt = if self.vt == 0 { super::vt::foreground() } else { self.vt };
        let (cols, rows) = super::vt::size(vt);
        let mut position = self.position.lock();
        let base = match whence {
            SEEK_SET => 0,
            SEEK_CUR => *position as i64,
            SEEK_END => self.size(cols, rows) as i64,
            _ => return Err(EINVAL as u64),
        };
        let target = base.checked_add(offset).filter(|&target| target >= 0).ok_or(EINVAL as u64)?;
        *position = target as u64;
        Ok(target as u64)
    }
}
