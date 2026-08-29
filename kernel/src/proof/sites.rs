//! The site census — the harness's list of labelled protocol seams.
//!
//! Naming the window is the whole leverage argument. Ambient timer ticks sample
//! the instruction stream uniformly, and the windows this campaign's defects
//! live in are a handful of instructions wide; a seeded perturbation placed at a
//! *semantically named* point turns a needle-in-a-haystack into a
//! coupon-collector problem over a dozen sites.
//!
//! Two numbers come out of this module and both are DERIVED: `DECLARED` is
//! `ALL.len()`, and `visited_count()` is the population count of a bitmap. The
//! gate compares those two and never a literal list of site names — pinning a
//! name list in a ratchet is the mistake this campaign has made three times
//! (#549, #551, #527-r1). `tests/coreproof_sites_structure.rs` keeps the enum,
//! `ALL`, `name()`, `class()` and the actual `proof_point!` placements one set,
//! so a site cannot be declared without being placed or placed without being
//! declared.
//!
//! ## Classes are a safety rule, not a label
//!
//! A `Masked` site's seam sits inside a critical section that holds the
//! scheduler lock with interrupts masked. A yield or a forced reschedule from
//! inside that window is a deadlock the harness authored, not a finding, so
//! `stimulus::apply` downgrades an inadmissible action to `None` and counts the
//! downgrade. `Open` sites are ordinary kthread context and admit everything.

use core::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SiteId {
    BlockEntry,
    BlockAfterStateStore,
    BlockBeforeDeparture,
    BlockAfterDeparture,
    UnblockEntry,
    UnblockAfterSetReady,
    UnblockBeforeEnqueue,
    UnblockAfterEnqueue,
    DeferredRequeueClaim,
    DriverPreCycle,
    DriverPostCycle,
    DriverPreQuiesce,
}

pub const ALL: &[SiteId] = &[
    SiteId::BlockEntry,
    SiteId::BlockAfterStateStore,
    SiteId::BlockBeforeDeparture,
    SiteId::BlockAfterDeparture,
    SiteId::UnblockEntry,
    SiteId::UnblockAfterSetReady,
    SiteId::UnblockBeforeEnqueue,
    SiteId::UnblockAfterEnqueue,
    SiteId::DeferredRequeueClaim,
    SiteId::DriverPreCycle,
    SiteId::DriverPostCycle,
    SiteId::DriverPreQuiesce,
];

pub const DECLARED: usize = ALL.len();
const _: () = assert!(DECLARED <= 64);

static VISITED: AtomicU64 = AtomicU64::new(0);

/// Which actions a site admits. See the module header — this is a safety rule.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SiteClass {
    /// Inside a scheduler-lock critical section with interrupts masked.
    Masked,
    /// Ordinary kthread context.
    Open,
}

impl SiteId {
    pub fn name(self) -> &'static str {
        match self {
            Self::BlockEntry => "BlockEntry",
            Self::BlockAfterStateStore => "BlockAfterStateStore",
            Self::BlockBeforeDeparture => "BlockBeforeDeparture",
            Self::BlockAfterDeparture => "BlockAfterDeparture",
            Self::UnblockEntry => "UnblockEntry",
            Self::UnblockAfterSetReady => "UnblockAfterSetReady",
            Self::UnblockBeforeEnqueue => "UnblockBeforeEnqueue",
            Self::UnblockAfterEnqueue => "UnblockAfterEnqueue",
            Self::DeferredRequeueClaim => "DeferredRequeueClaim",
            Self::DriverPreCycle => "DriverPreCycle",
            Self::DriverPostCycle => "DriverPostCycle",
            Self::DriverPreQuiesce => "DriverPreQuiesce",
        }
    }

    /// Which side of the protocol's commit point this seam sits on.
    ///
    /// This is a PROPERTY of the site, not a separate draw. Drawing it
    /// independently would let a violation record say
    /// `site=BlockAfterDeparture:order=before`, which is a contradiction the
    /// reader would have to resolve against the source. Most of this campaign's
    /// defects were an operation on the wrong side of a commit point, so the
    /// field earns its place in the record — but only if it cannot disagree with
    /// the site it accompanies.
    pub fn order(self) -> super::rng::Order {
        use super::rng::Order;
        match self {
            Self::BlockEntry
            | Self::BlockBeforeDeparture
            | Self::UnblockEntry
            | Self::UnblockBeforeEnqueue
            | Self::DeferredRequeueClaim
            | Self::DriverPreCycle
            | Self::DriverPreQuiesce => Order::Before,
            Self::BlockAfterStateStore
            | Self::BlockAfterDeparture
            | Self::UnblockAfterSetReady
            | Self::UnblockAfterEnqueue
            | Self::DriverPostCycle => Order::After,
        }
    }

    pub fn class(self) -> SiteClass {
        match self {
            Self::DriverPreCycle | Self::DriverPostCycle | Self::DriverPreQuiesce => {
                SiteClass::Open
            }
            Self::BlockEntry
            | Self::BlockAfterStateStore
            | Self::BlockBeforeDeparture
            | Self::BlockAfterDeparture
            | Self::UnblockEntry
            | Self::UnblockAfterSetReady
            | Self::UnblockBeforeEnqueue
            | Self::UnblockAfterEnqueue
            | Self::DeferredRequeueClaim => SiteClass::Masked,
        }
    }
}

/// Record that this seam was reached at least once.
///
/// Test-before-set: after the first visit this is a plain relaxed load and a
/// compare, not a read-modify-write. The seam executes inside masked scheduler
/// critical sections, and the census only needs "reached at least once" — the
/// RMW is worth paying twelve times, not on every one of tens of thousands of
/// iterations.
#[inline(always)]
pub fn mark_visited(site: SiteId) {
    let bit = 1u64 << site as u8;
    if VISITED.load(Ordering::Relaxed) & bit == 0 {
        VISITED.fetch_or(bit, Ordering::Relaxed);
    }
}

pub fn visited_count() -> u32 {
    VISITED.load(Ordering::Relaxed).count_ones()
}
