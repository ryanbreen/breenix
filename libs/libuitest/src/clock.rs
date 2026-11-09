//! Session clock driver and recorded-metadata acknowledgment barrier (plan 6.3).
use libbreenix::ui_clock::{ClockPage, Stamp, ANIMATING, COMPOSITOR};

pub const STEP_BOUND_NS: u64 = 2_000_000_000;
pub struct Clock { page: ClockPage }
pub struct Step {
    stamp: Stamp,
    deadline: u64,
    waiting: Vec<u64>,
}
impl Clock {
    pub fn open(path: &std::path::Path) -> Result<Self, String> {
        Ok(Self { page: ClockPage::open(path)? })
    }
    /// Snapshot eligibility before waking participants, which may then change
    /// their next deadline. Exclusion is per session and per window.
    pub fn begin(&self, dt_ms: u64, real_now_ns: u64, excluded: impl Fn(u64) -> bool) -> Result<Step, String> {
        let participants = self.page.participants();
        let stamp = self.page.advance(dt_ms)?;
        let waiting = participants.into_iter().filter(|p| !excluded(p.id)
            && (p.next_deadline_ns <= stamp.now_ns || p.flags & (ANIMATING | COMPOSITOR) != 0))
            .map(|p| p.id).collect();
        Ok(Step { stamp, deadline: real_now_ns.saturating_add(STEP_BOUND_NS), waiting })
    }
}
impl Step {
    pub fn stamp(&self) -> Stamp { self.stamp }
    pub fn waiting(&self) -> &[u64] { &self.waiting }
    /// Supply the epoch from kernel READ_META (pixel or no-change commit).
    /// A registered window reading time is not an acknowledgment.
    pub fn observe(&mut self, window: u64, epoch: u64) {
        if epoch == self.stamp.epoch { self.waiting.retain(|id| *id != window); }
    }
    pub fn complete(&self) -> bool { self.waiting.is_empty() }
    pub fn check(&self, real_now_ns: u64) -> Result<(), String> {
        if real_now_ns >= self.deadline && !self.complete() {
            return Err(format!("clock-step-timeout epoch={} window={} waiting={:?}", self.stamp.epoch, self.waiting[0], self.waiting));
        }
        Ok(())
    }
}
