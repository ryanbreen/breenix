//! Pure transaction-close policy from breenfs design section 2.5.
pub const COMMIT_INTERVAL_NS: u64 = 5_000_000_000;
pub const DIRTY_LIMIT: u64 = 64 * 1024 * 1024;
/// Mutation admission bound for a mount's dirty pages plus the private data
/// of the generation being published: one eighth of the smallest kernel heap
/// (64 MiB, memory/heap.rs), the share design 1.2 gives the buffer cache.
/// Admission asks for a commit at half of it, long before DIRTY_LIMIT
/// (E24 coordinator ledger line 309, C3).
pub const DIRTY_BUDGET: u64 = 8 * 1024 * 1024;

pub struct CommitPolicy {
    opened_ns: u64,
}
impl CommitPolicy {
    pub fn new(opened_ns: u64) -> Self {
        Self { opened_ns }
    }
    pub fn deadline(&self) -> u64 {
        self.opened_ns.saturating_add(COMMIT_INTERVAL_NS)
    }
    pub fn due(&self, now_ns: u64, dirty_bytes: u64, fsync: bool, pressure: bool) -> bool {
        now_ns >= self.deadline() || dirty_bytes >= DIRTY_LIMIT || fsync || pressure
    }
}
