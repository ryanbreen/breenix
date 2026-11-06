//! Driver clock-event sequencing; timestamp units are monotonic nanoseconds.
#[derive(Clone, Copy)]
pub struct ClockEvent { active: bool, tick: u64, wake: u64, last: u64 }
impl ClockEvent {
    pub const fn new() -> Self { Self { active: false, tick: 0, wake: u64::MAX, last: 0 } }
    pub fn wake_deadline(&self) -> u64 { self.wake }
    pub fn request(&mut self, now: u64, tick_remaining: u64, wake: u64) -> u64 {
        if !self.active { self.tick = now.saturating_add(tick_remaining); self.active = true; }
        self.wake = self.wake.min(wake);
        self.last = self.last.max(wake);
        self.tick.min(self.wake)
    }
    /// Return regular-tick due, fine-wake due, and the next one-shot deadline.
    /// A completed regular tick with no fine request restores periodic mode.
    pub fn interrupt(&mut self, now: u64, period: u64, poll: u64) -> (bool, bool, Option<u64>) {
        let tick = now >= self.tick;
        let wake = now >= self.wake;
        if tick { self.tick = self.tick.saturating_add((now.saturating_sub(self.tick) / period + 1).saturating_mul(period)); }
        if wake {
            self.wake = if now < self.last { now.saturating_add(poll).min(self.last) } else { u64::MAX };
            if self.wake == u64::MAX { self.last = 0; }
        }
        if tick && self.wake == u64::MAX { self.active = false; (true, wake, None) }
        else { (tick, wake, Some(self.tick.min(self.wake))) }
    }
}
