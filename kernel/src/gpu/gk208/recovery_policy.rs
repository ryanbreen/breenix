//! Native watchdog decisions; clocks are microseconds since device publication.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Quarantine,
    Reset,
}
pub struct Watchdog {
    sent: u64,
    quarantined: Option<u64>,
    progress: u64,
}
impl Watchdog {
    pub const fn new(sent: u64) -> Self {
        Self {
            sent,
            quarantined: None,
            progress: sent,
        }
    }
    pub fn progress(&mut self, now: u64) {
        self.progress = self.progress.max(now);
    }
    pub fn scan(&mut self, now: u64) -> Action {
        if let Some(at) = self.quarantined {
            if now.saturating_sub(at.max(self.progress)) >= 2_000_000 {
                Action::Reset
            } else {
                Action::None
            }
        } else if now.saturating_sub(self.sent) >= 2_000_000 {
            self.quarantined = Some(now);
            Action::Quarantine
        } else {
            Action::None
        }
    }
}
