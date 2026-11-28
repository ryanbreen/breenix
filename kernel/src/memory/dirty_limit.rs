//! The dirty limit and the charge taken before pages become dirty (E3 plan,
//! sections 11.4 and 11.5).
//!
//! ```text
//! dirtyable  = free frames + clean, unmapped, reclaimable cache frames
//! limit      = dirtyable / 5
//! background = dirtyable / 10
//! ```
//!
//! The ratios are tuning constants, not RAM ceilings. A write charges each
//! part's newly dirtied pages with one compare-and-add that fails if the
//! global count would pass `limit`, so N concurrent writers cannot pass it by
//! N writes. The one charge allowed past `limit` is a part admitted alone
//! while its mount holds no other charge, which keeps BreenFS's progress
//! rule; a part is at most `limit / 2`. Each mount's charge is a count of its
//! own ([`MountCharge`]) that the charge updates, and the lone part claims it
//! while it reads 0, so two writers overlapping on one mount cannot both take it.
//!
//! Units are frames. The module has no kernel dependency.

use core::sync::atomic::{AtomicU64, Ordering};

const LIMIT_DIVISOR: u64 = 5;
const BACKGROUND_DIVISOR: u64 = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub limit: u64,
    pub background: u64,
}

impl Limits {
    /// The limits for the frames that could hold dirty data.
    pub const fn from_dirtyable(free_frames: u64, clean_reclaimable: u64) -> Limits {
        let dirtyable = free_frames.saturating_add(clean_reclaimable);
        Limits {
            limit: dirtyable / LIMIT_DIVISOR,
            background: dirtyable / BACKGROUND_DIVISOR,
        }
    }

    /// The largest write part, at least one page so a write progresses.
    pub const fn part_pages(&self) -> u64 {
        let part = self.limit / 2;
        if part == 0 {
            1
        } else {
            part
        }
    }

    /// A mount's share of `background` while `writers` mounts hold dirty data.
    pub const fn mount_background(&self, writers: u64) -> u64 {
        let writers = if writers == 0 { 1 } else { writers };
        self.background / writers
    }
}

/// A charge did not fit: the writer requests its mount's commit and parks on
/// the writeback-progress channel, then retries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Throttle {
    pub charged: u64,
    pub limit: u64,
}

/// What one mount holds of the global dirty count. Its charges and releases
/// go through [`DirtyCount`] with it, which keeps the two counts together.
pub struct MountCharge {
    charged: AtomicU64,
}

impl MountCharge {
    pub const fn new() -> MountCharge {
        MountCharge {
            charged: AtomicU64::new(0),
        }
    }

    pub fn charged(&self) -> u64 {
        self.charged.load(Ordering::Acquire)
    }
}

impl Default for MountCharge {
    fn default() -> Self {
        MountCharge::new()
    }
}

/// The global count of frames dirty data holds: `DIRTY` and `MAPPED_DIRTY`
/// pages, `WRITEBACK` frames a batch still holds, and mapped bounce copies.
pub struct DirtyCount {
    charged: AtomicU64,
}

impl DirtyCount {
    pub const fn new() -> DirtyCount {
        DirtyCount {
            charged: AtomicU64::new(0),
        }
    }

    pub fn charged(&self) -> u64 {
        self.charged.load(Ordering::Acquire)
    }

    /// Charge `pages` of `mount` before they become dirty. A part that would
    /// pass `limit` is admitted only as the lone part of a mount that holds
    /// no charge: it claims the mount's count while it reads 0, so of writers that
    /// overlap on one mount at most one is admitted past `limit`.
    pub fn try_charge(&self, mount: &MountCharge, pages: u64, limit: u64) -> Result<(), Throttle> {
        // The mount's count first, so a lone part's claim below sees the
        // mount's charges that are in progress.
        mount.charged.fetch_add(pages, Ordering::AcqRel);
        let mut current = self.charged.load(Ordering::Acquire);
        loop {
            let next = current.saturating_add(pages);
            if next > limit {
                break;
            }
            match self
                .charged
                .compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => return Ok(()),
                Err(seen) => current = seen,
            }
        }
        // Over the limit: withdraw from the mount, then claim the lone part's
        // exception, which needs the mount's count to read 0.
        mount.charged.fetch_sub(pages, Ordering::AcqRel);
        if mount
            .charged
            .compare_exchange(0, pages, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            self.charged.fetch_add(pages, Ordering::AcqRel);
            return Ok(());
        }
        Err(Throttle {
            charged: current,
            limit,
        })
    }

    /// Charge a store fault's page without the check (before E2.9 a store
    /// fault cannot park; mapped admission and the background freeze catch up).
    pub fn charge_unchecked(&self, mount: &MountCharge, pages: u64) {
        mount.charged.fetch_add(pages, Ordering::AcqRel);
        self.charged.fetch_add(pages, Ordering::AcqRel);
    }

    /// Release `pages` of `mount` whose frames became clean, were truncated,
    /// or were freed by their batch. Returns the global count after the
    /// release.
    pub fn release(&self, mount: &MountCharge, pages: u64) -> u64 {
        let held = mount.charged.fetch_sub(pages, Ordering::AcqRel);
        debug_assert!(held >= pages, "mount dirty count released below zero");
        let previous = self.charged.fetch_sub(pages, Ordering::AcqRel);
        debug_assert!(previous >= pages, "dirty count released below zero");
        previous - pages
    }

    /// Whether the count has passed `background`, so the page daemon asks each
    /// mount with dirty pages to freeze.
    pub fn over_background(&self, limits: &Limits) -> bool {
        self.charged() > limits.background
    }
}

impl Default for DirtyCount {
    fn default() -> Self {
        DirtyCount::new()
    }
}
