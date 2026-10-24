//! Phase A UI-clock work and W16 acknowledgments; descriptor-only sleep is lane 2B.
use super::*;
use libbreenix::{
    ui_clock,
    window::{self, abi},
};

pub(super) struct Clock {
    frozen: bool,
    test: bool,
    epoch: u64,
    now: u64,
    wall: u64,
    presented: u64,
}
impl Clock {
    pub(super) fn new(frozen: bool, test: bool) -> Self {
        let mut clock = Self {
            frozen,
            test,
            epoch: 0,
            now: 0,
            wall: 0,
            presented: 0,
        };
        clock.begin();
        clock
    }
    fn begin(&mut self) {
        loop {
            let epoch = ui_clock::epoch();
            let now = ui_clock::now_ns();
            let wall = if self.frozen {
                1_767_225_600_000_000_000u64.saturating_add(now)
            } else {
                ui_clock::wall_ns()
            };
            if epoch == ui_clock::epoch() {
                if epoch != self.epoch {
                    self.presented = 0;
                }
                self.epoch = epoch;
                self.now = now;
                self.wall = wall;
                break;
            }
        }
    }
    pub(super) fn wall_sec(&self) -> i64 {
        (self.wall / 1_000_000_000) as i64
    }
    pub(super) fn format(&self, text: &mut [u8; 12]) {
        if self.frozen {
            let sec = self.wall_sec().rem_euclid(86400);
            let value = format!("{:02}:{:02}:{:02} UTC", sec / 3600, sec / 60 % 60, sec % 60);
            text.copy_from_slice(value.as_bytes());
        } else {
            let mut value = [0; 11];
            format_clock(self.wall_sec(), &mut value);
            text[..11].copy_from_slice(&value);
            text[11] = b' ';
        }
    }
    pub(super) fn presented(&mut self, serial: u64) {
        self.presented = serial;
    }
    pub(super) fn acknowledge(&self, claim_epoch: u64, handled_serial: u64, deadline: Option<u64>) {
        let mut ack = abi::CompositorAck {
            claim_epoch,
            epoch: self.epoch,
            // Lane 3 records and acknowledges the handled source serial.
            handled_serial,
            presented: self.presented,
            next_deadline_ns: deadline.map_or_else(|| self.now.saturating_add(1_000_000_000 - self.wall % 1_000_000_000),
                |deadline| deadline.min(self.now.saturating_add(1_000_000_000 - self.wall % 1_000_000_000))),
            flags: u32::from(self.test),
            reserved: 0,
        };
        if let Err(error) = window::call(abi::op::COMPOSITOR_ACK, &mut ack) {
            eprintln!("[bwm] compositor acknowledgment failed: {}", error);
        }
    }
}
impl State {
    pub(super) fn begin_epoch(&mut self) {
        self.clock.begin();
    }
    pub(super) fn acknowledge(&self) {
        if !self.full_redraw
            && !self.content_dirty
            && !self.windows_dirty
            && !self.display_lost
            && !self.frame_failed
        {
            self.clock.acknowledge(self.claim_epoch, handled_serial(self.stream.seen), self.stream.deadline());
        }
    }
}
