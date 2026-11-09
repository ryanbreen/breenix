//! Pure UI-clock timer scheduling. Times and periods are in milliseconds.
#[derive(Clone, Debug)]
struct Timer { id: u64, deadline: u64, period: u64, order: u64 }

#[derive(Default)]
pub struct Timers { now: u64, next_order: u64, timers: Vec<Timer> }

impl Timers {
    pub fn new(now: u64) -> Self { Self { now, ..Self::default() } }
    pub fn now(&self) -> u64 { self.now }
    /// A zero period denotes a one-shot timer.
    pub fn register(&mut self, id: u64, deadline: u64, period: u64) {
        let order = self.next_order;
        self.next_order = self.next_order.checked_add(1).expect("timer registration overflow");
        self.timers.push(Timer { id, deadline, period, order });
    }
    /// Advance by milliseconds, firing each due timer once with its missed count.
    pub fn step(&mut self, elapsed: u64) -> Vec<(u64, u64)> {
        self.now = self.now.checked_add(elapsed).expect("UI timer overflow");
        self.timers.sort_by_key(|timer| (timer.deadline, timer.order));
        let mut fired = Vec::new();
        let now = self.now;
        self.timers.retain_mut(|timer| {
            if timer.deadline > now { return true; }
            let missed = if timer.period == 0 { 0 } else { (now - timer.deadline) / timer.period };
            fired.push((timer.id, missed));
            if timer.period == 0 { return false; }
            match missed.checked_add(1).and_then(|n| n.checked_mul(timer.period)).and_then(|n| timer.deadline.checked_add(n)) {
                Some(next) => { timer.deadline = next; true }
                None => false,
            }
        });
        fired
    }
}
