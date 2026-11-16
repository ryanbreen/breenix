//! Pure transaction-close policy from breenfs design section 2.5.
pub const COMMIT_INTERVAL_NS: u64 = 5_000_000_000;
pub const DIRTY_LIMIT: u64 = 64 * 1024 * 1024;

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
