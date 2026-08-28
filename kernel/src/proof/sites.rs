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

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SiteClass {
    Masked,
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

#[inline(always)]
pub fn mark_visited(site: SiteId) {
    VISITED.fetch_or(1u64 << site as u8, Ordering::Relaxed);
}

pub fn visited_count() -> u32 {
    VISITED.load(Ordering::Relaxed).count_ones()
}
