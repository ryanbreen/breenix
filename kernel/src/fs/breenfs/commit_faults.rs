//! Boot-only BreenFS publication faults for the disposable suite root.
//!
//! With the window feature enabled, `breenfs_commit_window=<window>` holds the
//! named window for two seconds after its marker. Without a selection, the
//! markers are emitted without a pause.
//!
//! With the corruption feature enabled, `breenfs_corrupt_inject=<block>:<offset>:<bit>`
//! selects a filesystem-relative block written by a commit. Use `first-data`
//! in place of a block number to select the first data block staged for writing.

#[cfg(any(test, feature = "breenfs_corrupt_inject"))]
use core::sync::atomic::{AtomicBool, Ordering};

fn option<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let mut values = line
        .split_ascii_whitespace()
        .filter_map(|word| word.strip_prefix(name));
    let value = values.next()?;
    values.next().is_none().then_some(value)
}

#[cfg(any(test, feature = "breenfs_commit_window"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Window {
    NodeWrites,
    FirstFlush,
    BetweenRings,
}

#[cfg(any(test, feature = "breenfs_commit_window"))]
impl Window {
    pub(super) fn from_command_line(line: &str) -> Option<Self> {
        match option(line, "breenfs_commit_window=")? {
            "node-writes" => Some(Self::NodeWrites),
            "first-flush" => Some(Self::FirstFlush),
            "between-rings" => Some(Self::BetweenRings),
            _ => None,
        }
    }

    pub(super) fn marker(self) -> &'static str {
        match self {
            Self::NodeWrites => "[breenfs] commit-window node-writes",
            Self::FirstFlush => "[breenfs] commit-window first-flush",
            Self::BetweenRings => "[breenfs] commit-window between-rings",
        }
    }
}

#[cfg(any(test, feature = "breenfs_corrupt_inject"))]
pub(super) struct Injection {
    // A missing address selects the first data block in publication order.
    block: Option<u64>,
    offset: usize,
    bit: u8,
    taken: AtomicBool,
}

#[cfg(all(not(test), feature = "breenfs_corrupt_inject"))]
use super::BLOCK_SIZE;
#[cfg(test)]
const BLOCK_SIZE: usize = 4096;

#[cfg(any(test, feature = "breenfs_corrupt_inject"))]
impl Injection {
    // breenfs_corrupt_inject=<block|first-data>:<byte offset in block>:<bit>
    pub(super) fn from_command_line(line: &str) -> Option<Self> {
        let (block, location) = option(line, "breenfs_corrupt_inject=")?.split_once(':')?;
        let (offset, bit) = location.split_once(':')?;
        let block = if block == "first-data" {
            None
        } else {
            let block = block.parse::<u64>().ok()?;
            // Blocks below 20 are reserved, including the primary ring.
            if block < 20 {
                return None;
            }
            Some(block)
        };
        let offset = offset.parse().ok()?;
        let bit = bit.parse().ok()?;
        if offset >= BLOCK_SIZE || bit >= 8 {
            return None;
        }
        Some(Self {
            block,
            offset,
            bit,
            taken: AtomicBool::new(false),
        })
    }

    pub(super) fn take(&self, sector: u64, sector_size: usize, is_data: bool) -> Option<Flip> {
        let per_block = (BLOCK_SIZE / sector_size) as u64;
        let block = sector / per_block;
        if !self.block.map_or(is_data, |wanted| wanted == block)
            || sector % per_block != (self.offset / sector_size) as u64
            || self.taken.swap(true, Ordering::Relaxed)
        {
            return None;
        }
        Some(Flip {
            block,
            offset: self.offset,
            bit: self.bit,
        })
    }
}

#[cfg(any(test, feature = "breenfs_corrupt_inject"))]
pub(super) struct Flip {
    pub(super) block: u64,
    pub(super) offset: usize,
    pub(super) bit: u8,
}

#[cfg(any(test, feature = "breenfs_corrupt_inject"))]
impl Flip {
    pub(super) fn apply(&self, sector: &mut [u8]) {
        let offset = self.offset % sector.len();
        sector[offset] ^= 1 << self.bit;
    }
}

#[cfg(not(test))]
pub struct Faults {
    #[cfg(feature = "breenfs_commit_window")]
    pause: Option<Window>,
    #[cfg(feature = "breenfs_corrupt_inject")]
    pub(super) injection: Option<Injection>,
}

#[cfg(not(test))]
impl Faults {
    pub(in crate::fs::breenfs) fn for_mount(boot_root: bool) -> Option<Self> {
        #[cfg(target_arch = "x86_64")]
        let disposable = crate::block::root_on_installed_disk();
        #[cfg(target_arch = "aarch64")]
        let disposable = crate::platform_config::is_qemu();
        if !boot_root || !disposable || !crate::fs::vfs::suite_boot_requested() {
            return None;
        }
        let line = crate::boot::intent::command_line().unwrap_or("");
        Some(Self {
            #[cfg(feature = "breenfs_commit_window")]
            pause: Window::from_command_line(line),
            #[cfg(feature = "breenfs_corrupt_inject")]
            injection: Injection::from_command_line(line),
        })
    }

    #[cfg(feature = "breenfs_commit_window")]
    pub(super) fn window(&self, window: Window) {
        crate::serial_println!("\n{}", window.marker());
        if self.pause == Some(window) {
            use crate::fs::breenfs::commit_signal::monotonic_ns;
            const PAUSE_NS: u64 = 2_000_000_000;
            let end = monotonic_ns().saturating_add(PAUSE_NS);
            while monotonic_ns() < end {
                super::preemption_point();
                core::hint::spin_loop();
            }
        }
    }
}
