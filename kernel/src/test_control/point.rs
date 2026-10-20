//! Reserve one-shot effects during preparation, count them only when queued.
use spin::Mutex;

pub struct Point(Mutex<State>);
struct State { armed: bool, target: u64, param: u64, fired: u64, generation: u64 }

impl Point {
    pub const fn new() -> Self {
        Self(Mutex::new(State { armed: false, target: 0, param: 0, fired: 0, generation: 0 }))
    }
    pub fn arm(&self, target: u64, param: u64) {
        let mut s = self.0.lock();
        s.generation += 1;
        s.target = target;
        s.param = param;
        s.armed = true;
    }
    pub fn disarm(&self) {
        let mut s = self.0.lock();
        s.generation += 1;
        s.armed = false;
    }
    pub fn fired(&self) -> u64 { self.0.lock().fired }
    pub fn reserve(&self, target: u64, wanted: impl Fn(u64) -> bool) -> Option<Reservation<'_>> {
        let mut s = self.0.lock();
        if !s.armed || (s.target != 0 && s.target != target) || !wanted(s.param) { return None; }
        s.armed = false;
        Some(Reservation { point: self, generation: s.generation, param: s.param, committed: false })
    }
}

/// No lock is held during preparation. A rollback restores this arm only if no newer
/// arm or disarm superseded it; a commit counts the reserved effect exactly once.
pub struct Reservation<'a> { point: &'a Point, generation: u64, param: u64, committed: bool }
impl Reservation<'_> {
    pub fn fire(mut self) -> u64 {
        self.point.0.lock().fired += 1;
        self.committed = true;
        self.param
    }
}
impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        if !self.committed {
            let mut s = self.point.0.lock();
            if s.generation == self.generation { s.armed = true; }
        }
    }
}
