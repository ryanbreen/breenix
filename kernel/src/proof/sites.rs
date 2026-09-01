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
//! ## Component-scoped, not one global set
//!
//! Rung 1 shipped a single `SiteId` naming Component A's twelve seams. Rung 2
//! added Component C, whose own contract needs three seams: `ScheduleEntry` at
//! the broad pre-dispatch boundary, `PreDispatchMask` for a tighter aim point,
//! and `DriverPreCycle` to prove the driver itself ran. The first two live
//! inside `scheduler.rs` as `SiteClass::Open` — a file every one of Component
//! A's placed seams is `Masked` in. Merging the two
//! into one array would declare a site the OTHER component's build can never
//! visit, permanently reddening that build's own non-vacuity gate
//! (`sites_visited < sites_declared`). Rung 3 adds Component H's own three-site
//! set under the same rule. `SiteId` and `ALL` are therefore three mutually
//! exclusive definitions, selected by positive per-component features. This is
//! the same "mutually-exclusive compile-time driver selection" pattern
//! `MODE`/`WINDOW`/`SEED` already use. Everything below the definitions
//! (`SiteClass`, `mark_visited`, `visited_count`) is generic over whichever
//! concrete `SiteId` is in scope and needs no per-component copy.
//!
//! ## Classes are a safety rule, not a label
//!
//! A `Masked` site's seam sits inside a critical section that holds the
//! scheduler lock with interrupts masked. A yield or a forced reschedule from
//! inside that window is a deadlock the harness authored, not a finding, so
//! `stimulus::apply` downgrades an inadmissible action to `None` and counts the
//! downgrade. `Open` sites are ordinary kthread context and admit everything.

use core::sync::atomic::{AtomicU64, Ordering};

// ============================================================================
// Component A — the ready-queue departure protocol's twelve seams.
// ============================================================================

#[cfg(feature = "coreproof_component_a")]
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

#[cfg(feature = "coreproof_component_a")]
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

#[cfg(feature = "coreproof_component_a")]
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

// ============================================================================
// Component C — per-CPU identity + stack custody's three seams.
// ============================================================================
//
// `ScheduleEntry` sits at the top of `scheduler::schedule()`, before the call
// to `run_deferred_reclamation()`, while `PreDispatchMask` sits immediately
// after that drain and before `schedule_from_kernel()`. Every caller of
// `schedule()` reaches both with interrupts ENABLED — masking is
// `schedule_from_kernel`'s own job, further down a call chain this harness may
// never seam (`context_switch.rs` is permanently prohibited,
// `scripts/check-coreproof-seams.sh`) — so their class is `Open`, not `Masked`.
// Both existing `KernelSchedule` and `Steal` antagonist ops already reach them
// on every peer step, so no new call path is needed to exercise either one.
//
// `ALL` now has three variants. `ScheduleEntry` and `PreDispatchMask` are both
// visited trivially by ordinary boot traffic — `scheduler::schedule()` runs
// constantly regardless of which component is driving, so those two alone
// would make `sites_visited == sites_declared` satisfied by construction, not
// by the harness actually running (this was a real, disclosed gap in rung 2's
// first cut — the review that caught it is `rung2-review.md`, finding B1).
// `DriverPreCycle` closes that gap: it lives in `driver_c.rs`'s own loop, not in
// `scheduler.rs`, so it can only ever be visited if the coreproof driver thread
// itself dispatches and runs at least one iteration. `sites_visited ==
// sites_declared` is therefore now a genuine non-vacuity check for Component C,
// not a tautology. `docker/qemu/run-coreproof-gate.sh`'s `adjudicate()` also now
// fails a boot outright on `iters=0`, independent of which sites a future
// component happens to declare — belt and suspenders, not either/or.

#[cfg(feature = "coreproof_component_c")]
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SiteId {
    ScheduleEntry,
    PreDispatchMask,
    DriverPreCycle,
}

#[cfg(feature = "coreproof_component_c")]
pub const ALL: &[SiteId] = &[
    SiteId::ScheduleEntry,
    SiteId::PreDispatchMask,
    SiteId::DriverPreCycle,
];

#[cfg(feature = "coreproof_component_c")]
impl SiteId {
    pub fn name(self) -> &'static str {
        match self {
            Self::ScheduleEntry => "ScheduleEntry",
            Self::PreDispatchMask => "PreDispatchMask",
            Self::DriverPreCycle => "DriverPreCycle",
        }
    }

    /// Reconstruct a `SiteId` from its `as u8` discriminant, defensively (never panics —
    /// falls back to `ALL[0]` for an out-of-range value). Used to decode a site that was
    /// stored as a raw `u8` in an atomic. Derived from `ALL`, not a second literal list —
    /// see this module's own header on why a name list here would be the same mistake
    /// #549/#551/#527-r1 already made once.
    pub(crate) fn from_u8(value: u8) -> Self {
        ALL.iter()
            .copied()
            .find(|site| *site as u8 == value)
            .unwrap_or(ALL[0])
    }

    /// See the Component A impl's doc for why this is derived, never drawn.
    pub fn order(self) -> super::rng::Order {
        use super::rng::Order;
        match self {
            // Both scheduler seams precede `schedule_from_kernel`'s own pre-mask identity
            // read: `ScheduleEntry` also precedes `run_deferred_reclamation()`, while
            // `PreDispatchMask` follows that drain.
            Self::ScheduleEntry | Self::PreDispatchMask => Order::Before,
            // Driver-only census marker; `Before` by the convention shared with every
            // component's own `DriverPreCycle`.
            Self::DriverPreCycle => Order::Before,
        }
    }

    pub fn class(self) -> SiteClass {
        match self {
            Self::ScheduleEntry | Self::PreDispatchMask | Self::DriverPreCycle => SiteClass::Open,
        }
    }
}

// ============================================================================
// Component H — dispatch admission's three seams.
// ============================================================================
//
// `IncomingHandoffCommit` observes the `pending_next` publication immediately
// after commit, while `PendingNextResolveEntry` observes the resolver at its own
// entry before any resolution. Both live inside `impl Scheduler { .. }` and are
// therefore `Masked`. `DriverPreCycle` is H's driver-only `Open` non-vacuity
// census marker. See `docs/planning/coreproof/rung3/spec.md` sections 1 and 3.

#[cfg(feature = "coreproof_component_h")]
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SiteId {
    IncomingHandoffCommit,
    PendingNextResolveEntry,
    DriverPreCycle,
}

#[cfg(feature = "coreproof_component_h")]
pub const ALL: &[SiteId] = &[
    SiteId::IncomingHandoffCommit,
    SiteId::PendingNextResolveEntry,
    SiteId::DriverPreCycle,
];

#[cfg(feature = "coreproof_component_h")]
impl SiteId {
    pub fn name(self) -> &'static str {
        match self {
            Self::IncomingHandoffCommit => "IncomingHandoffCommit",
            Self::PendingNextResolveEntry => "PendingNextResolveEntry",
            Self::DriverPreCycle => "DriverPreCycle",
        }
    }

    /// Reconstruct a `SiteId` from its atomic `u8` representation without panicking.
    pub(crate) fn from_u8(value: u8) -> Self {
        ALL.iter()
            .copied()
            .find(|site| *site as u8 == value)
            .unwrap_or(ALL[0])
    }

    /// See the Component A impl's doc for why this is derived, never drawn.
    pub fn order(self) -> super::rng::Order {
        use super::rng::Order;
        match self {
            Self::IncomingHandoffCommit => Order::After,
            Self::PendingNextResolveEntry | Self::DriverPreCycle => Order::Before,
        }
    }

    pub fn class(self) -> SiteClass {
        match self {
            Self::IncomingHandoffCommit | Self::PendingNextResolveEntry => SiteClass::Masked,
            Self::DriverPreCycle => SiteClass::Open,
        }
    }
}

// ============================================================================
// Shared: generic over whichever `SiteId` this build compiled.
// ============================================================================

pub const DECLARED: usize = ALL.len();
const _: () = assert!(DECLARED <= 64);
const _: () = assert!(DECLARED >= 1);

static VISITED: AtomicU64 = AtomicU64::new(0);

/// Which actions a site admits. See the module header — this is a safety rule.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SiteClass {
    /// Inside a scheduler-lock critical section with interrupts masked.
    Masked,
    /// Ordinary kthread context.
    Open,
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
