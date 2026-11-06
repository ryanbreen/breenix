//! Driver clock-event sequencing; timestamp units are monotonic nanoseconds.
use crate::time::deadlines::Deadlines;
pub struct ClockEvent { active: bool, tick: u64, wakes: Deadlines }
impl ClockEvent {
    pub const fn new() -> Self { Self { active: false, tick: 0, wakes: Deadlines::new() } }
    pub fn wake_deadline(&self) -> u64 { self.wakes.next() }
    pub fn request(&mut self, now: u64, tick_remaining: u64, wake: u64) -> Option<u64> {
        if !self.wakes.insert(wake) { return None; }
        if !self.active { self.tick = now.saturating_add(tick_remaining); self.active = true; }
        Some(self.tick.min(self.wakes.next()))
    }
    /// Return regular-tick due, fine-wake due, and the next one-shot deadline.
    /// A completed regular tick with no fine request restores periodic mode.
    pub fn interrupt(&mut self, now: u64, period: u64) -> (bool, bool, Option<u64>) {
        let tick = now >= self.tick;
        let wake = now >= self.wakes.next();
        if tick { self.tick = self.tick.saturating_add((now.saturating_sub(self.tick) / period + 1).saturating_mul(period)); }
        if wake { self.wakes.expire(now); }
        if tick && self.wakes.next() == u64::MAX { self.active = false; (true, wake, None) }
        else { (tick, wake, Some(self.tick.min(self.wakes.next()))) }
    }
}
