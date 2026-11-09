//! UI-clock sampling and compositor progress, adapted from lane 2A (6e0001a6).
use libbreenix::{ui_clock, window::abi};

pub(super) struct Clock {
    test: bool,
    epoch: u64,
    now: u64,
    wall: u64,
    drawn_sec: i64,
    presented: u64,
    participant: Option<ui_clock::Participant>,
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
            participant: None,
        };
        clock.begin_epoch();
        clock.drawn_sec = clock.wall_sec();
        clock
    }

    pub(super) fn participate(&mut self) -> Result<(), String> {
        self.participant = ui_clock::Participant::compositor_configured()?;
        Ok(())
    }

    /// Call only after COMPOSITOR_ACK has been accepted by the kernel.
    pub(super) fn acknowledged(&self) {
        if let Some(participant) = &self.participant {
            participant.accepted(ui_clock::Stamp { epoch: self.epoch, now_ns: self.now });
        }
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

    /// Absorb time into a scene composite that has already passed input pacing.
    /// A stepped epoch alone leaves scheduling to scene changes.
    pub(super) fn scene_clock(&mut self) -> Option<i64> {
        let sec = self.wall_sec();
        if sec == self.drawn_sec { return None; }
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
            flags: if self.test && self.presented == 0 { abi::compositor_ack_flags::NONE_NEEDED } else { 0 },
            reserved: 0,
        }
    }
}

/// US Eastern Time offset from UTC in seconds.
/// DST (EDT, UTC-4) runs from 2nd Sunday of March to 1st Sunday of November.
/// Standard (EST, UTC-5) applies the rest of the year.
pub(super) fn eastern_offset_secs(utc_secs: i64) -> i64 {
    // Compute day-of-year, month, day-of-week from Unix timestamp
    let days = (utc_secs / 86400) as i64;

    // Compute year/month/day
    let mut y = 1970i64;
    let mut rem = days;
    loop {
        let ydays = if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) { 366 } else { 365 };
        if rem < ydays { break; }
        rem -= ydays;
        y += 1;
    }
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let mdays: [i64; 12] = [31, if leap {29} else {28}, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut month = 0u8;
    for i in 0..12 {
        if rem < mdays[i] { month = i as u8 + 1; break; }
        rem -= mdays[i];
    }
    let day = rem as u8 + 1;

    // 2nd Sunday of March: find first Sunday in March, add 7
    // 1st Sunday of November: find first Sunday in November
    // Check month boundaries for DST transitions
    // DST starts: 2nd Sunday of March at 2:00 AM EST (7:00 AM UTC)
    // DST ends: 1st Sunday of November at 2:00 AM EDT (6:00 AM UTC)
    let hour_utc = ((utc_secs % 86400) / 3600) as u8;

    // March: find day-of-week of March 1
    let jan1_dow = {
        let mut d = 0i64;
        for yr in 1970..y {
            d += if yr % 4 == 0 && (yr % 100 != 0 || yr % 400 == 0) { 366 } else { 365 };
        }
        ((d % 7 + 4) % 7) as u8 // 0=Sun
    };
    // Day of week of March 1 = (jan1_dow + 31 + feb_days) % 7
    let feb = if leap { 29 } else { 28 };
    let mar1_dow = (jan1_dow + 31 + feb) % 7;
    let dst_start_day = if mar1_dow == 0 { 8u8 } else { (14 - mar1_dow + 1) as u8 }; // 2nd Sunday
    // Day of week of Nov 1
    let days_to_nov1: u16 = [31u16, feb as u16, 31, 30, 31, 30, 31, 31, 30, 31].iter().sum();
    let nov1_dow = (jan1_dow as u16 + days_to_nov1) % 7;
    let dst_end_day = if nov1_dow == 0 { 1u8 } else { 7 - nov1_dow as u8 + 1 };

    let is_dst = match month {
        1..=2 => false,
        4..=10 => true,
        3 => day > dst_start_day || (day == dst_start_day && hour_utc >= 7),
        11 => day < dst_end_day || (day == dst_end_day && hour_utc < 6),
        12 => false,
        _ => false,
    };

    if is_dst { -4 * 3600 } else { -5 * 3600 }
}

pub(super) fn format_clock(utc_secs: i64, buf: &mut [u8; 11]) {
    let offset = eastern_offset_secs(utc_secs);
    let local = utc_secs + offset;
    // Handle day wrap
    let t = ((local % 86400) + 86400) % 86400;
    let s = (t % 60) as u8;
    let m = ((t / 60) % 60) as u8;
    let h = ((t / 3600) % 24) as u8;
    buf[0] = b'0' + h / 10;
    buf[1] = b'0' + h % 10;
    buf[2] = b':';
    buf[3] = b'0' + m / 10;
    buf[4] = b'0' + m % 10;
    buf[5] = b':';
    buf[6] = b'0' + s / 10;
    buf[7] = b'0' + s % 10;
    buf[8] = b' ';
    buf[9] = b'E';
    buf[10] = b'T';
}
