//! The readahead window (E3 plan, sections 8.1 and 8.2).
//!
//! Each open file description, and each file VMA, carries a [`Readahead`].
//! A miss at index i is sequential if i follows the last window or lies in it;
//! then the window doubles, up to `max`. Otherwise it
//! resets to the request, rounded up to 4 pages. The first read at offset 0
//! starts at twice the request, since files read from their start are usually
//! read through (McKusick et al. 2014, ch. 9; McVoy and Kleiman 1991).
//!
//! The window on each path is clipped to `max` by [`clip`], and a demanded
//! request larger than `max` is filled in successive windows of at most `max`.
//!
//! Units are pages. The module has no kernel dependency.

/// The readahead cap in pages: 2 MiB of 4 KiB pages. A tuning constant, not a
/// RAM ceiling.
pub const CAP_PAGES: u32 = 512;

/// The rounding of a reset window.
const RESET_ROUND: u32 = 4;

/// No asynchronous trigger in the window.
pub const NO_TRIGGER: u64 = u64::MAX;

/// The largest window: the device's maximum transfer times its queue depth,
/// at most [`CAP_PAGES`], halved while a zone's pressure flag is set, and at
/// least one page.
pub fn max_window(max_transfer_pages: u32, queue_depth: u32, pressure: bool) -> u32 {
    let mut max = max_transfer_pages
        .saturating_mul(queue_depth.max(1))
        .min(CAP_PAGES);
    if pressure {
        max /= 2;
    }
    max.max(1)
}

/// The one cap that windows pass through.
pub fn clip(pages: u64, max: u32) -> u32 {
    pages.min(u64::from(max.max(1))) as u32
}

/// An access-pattern hint (`POSIX_FADV_*`, `MADV_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hint {
    Normal,
    /// Start at `max`.
    Sequential,
    /// No growth: the window is the request.
    Random,
}

/// A window to claim and fill: pages `[start, start + size)`. `trigger` is
/// the first page past the demanded ones, which gets `READAHEAD`, or
/// [`NO_TRIGGER`] when the whole window is demanded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub start: u64,
    pub size: u32,
    pub trigger: u64,
}

impl Window {
    pub fn end(&self) -> u64 {
        self.start.saturating_add(u64::from(self.size))
    }
}

/// The readahead state of one description or VMA.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Readahead {
    prev: u64,
    start: u64,
    size: u32,
    async_at: u64,
    hint: Hint,
    fresh: bool,
}

impl Readahead {
    pub const fn new() -> Readahead {
        Readahead {
            prev: 0,
            start: 0,
            size: 0,
            async_at: NO_TRIGGER,
            hint: Hint::Normal,
            fresh: true,
        }
    }

    pub fn set_hint(&mut self, hint: Hint) {
        self.hint = hint;
    }

    /// The page whose hit queues the next window.
    pub const fn async_at(&self) -> u64 {
        self.async_at
    }

    fn sequential(&self, index: u64) -> bool {
        !self.fresh
            && (index == self.prev.wrapping_add(1)
                || (index >= self.start && index < self.start.saturating_add(u64::from(self.size))))
    }

    /// The window for a miss at `index` demanding `request` pages.
    pub fn on_miss(&mut self, index: u64, request: u64, max: u32) -> Window {
        let request = request.max(1);
        let raw = match self.hint {
            Hint::Random => request,
            Hint::Sequential => u64::from(max),
            Hint::Normal if self.fresh && index == 0 => request.saturating_mul(2),
            Hint::Normal if self.sequential(index) => {
                (u64::from(self.size).saturating_mul(2)).max(request)
            }
            Hint::Normal => request.div_ceil(u64::from(RESET_ROUND)) * u64::from(RESET_ROUND),
        };
        let size = clip(raw, max);
        let demanded = clip(request, size);
        let trigger = if demanded < size {
            index.saturating_add(u64::from(demanded))
        } else {
            NO_TRIGGER
        };
        self.fresh = false;
        self.start = index;
        self.size = size;
        // The last page of the window, not of the demand: hits inside the
        // window are not reported here, so a reader that consumes the window
        // and misses at its end is sequential.
        self.prev = index.saturating_add(u64::from(size) - 1);
        self.async_at = trigger;
        Window {
            start: index,
            size,
            trigger,
        }
    }

    /// A hit on the `READAHEAD` page: the next window, which follows the
    /// current one and doubles, up to `max`. Its first page is its trigger.
    /// No window when `index` is not the trigger.
    pub fn on_async_hit(&mut self, index: u64, max: u32) -> Option<Window> {
        if index != self.async_at || self.async_at == NO_TRIGGER {
            return None;
        }
        let start = self.start.saturating_add(u64::from(self.size));
        let grown = match self.hint {
            Hint::Random => u64::from(self.size),
            _ => u64::from(self.size).saturating_mul(2),
        };
        let size = clip(grown, max);
        self.prev = start.saturating_add(u64::from(size) - 1);
        self.start = start;
        self.size = size;
        self.async_at = start;
        Some(Window {
            start,
            size,
            trigger: start,
        })
    }
}

impl Default for Readahead {
    fn default() -> Self {
        Readahead::new()
    }
}

/// The successive windows of at most `max` pages that cover `count` pages
/// from `first`: the `WILLNEED` hints and `MAP_POPULATE`.
pub fn windows(first: u64, count: u64, max: u32) -> impl Iterator<Item = Window> {
    let mut next = first;
    let end = first.saturating_add(count);
    core::iter::from_fn(move || {
        if next >= end {
            return None;
        }
        let size = clip(end - next, max);
        let window = Window {
            start: next,
            size,
            trigger: NO_TRIGGER,
        };
        next += u64::from(size);
        Some(window)
    })
}
