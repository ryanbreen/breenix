//! UI-clock sampling and compositor progress, adapted from lane 2A (6e0001a6).
use libbreenix::{ui_clock, window::abi};

pub(super) struct Clock {
    test: bool,
    epoch: u64,
    now: u64,
    wall: u64,
    drawn_sec: i64,
    presented: u64,
}

impl Clock {
    pub(super) fn new(test: bool) -> Self {
        let mut clock = Self {
            test,
            epoch: 0,
            now: 0,
            wall: 0,
            drawn_sec: 0,
            presented: 0,
        };
        clock.begin_epoch();
        clock.drawn_sec = clock.wall_sec();
        clock
    }

    pub(super) fn begin_epoch(&mut self) {
        loop {
            let epoch = ui_clock::epoch();
            let now = ui_clock::now_ns();
            let wall = ui_clock::wall_ns();
            if epoch == ui_clock::epoch() {
                if epoch != self.epoch {
                    self.presented = 0;
                }
                self.epoch = epoch;
                self.now = now;
                self.wall = wall;
                return;
            }
        }
    }

    pub(super) fn wall_sec(&self) -> i64 {
        (self.wall / 1_000_000_000) as i64
    }

    /// Stepped sessions acknowledge time without scheduling chrome-only composites.
    pub(super) fn due(&self) -> bool {
        !self.test && self.wall_sec() != self.drawn_sec
    }

    pub(super) fn update_clock(&mut self) -> Option<i64> {
        let sec = self.wall_sec();
        if !self.due() {
            return None;
        }
        self.drawn_sec = sec;
        Some(sec)
    }

    pub(super) fn presented(&mut self, serial: u64) {
        self.presented = serial;
    }

    pub(super) fn acknowledgment(&self, claim_epoch: u64) -> abi::CompositorAck {
        abi::CompositorAck {
            claim_epoch,
            epoch: self.epoch,
            handled_serial: 0,
            presented: self.presented,
            next_deadline_ns: if self.test { 0 } else {
                self.now.saturating_add(1_000_000_000 - self.wall % 1_000_000_000)
            },
            flags: u32::from(self.test),
            reserved: 0,
        }
    }
}
