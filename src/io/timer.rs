use crate::constants::timer::{
    PIT_A, PIT_CONTROL, PIT_MASK, PIT_SCALE, PIT_SET, SUBTICKS_PER_TICK,
};

use x86::io::outb;

use crate::io::Port;
use crate::util::time::Time;

use core::sync::atomic::{AtomicU64, Ordering};
use lazy_static::lazy_static;

pub struct Timer {
    start: AtomicU64,
    ticks: AtomicU64,
    seconds: AtomicU64,
    millis: AtomicU64,
}

impl Timer {
    pub fn new() -> Timer {
        let divisor: u32 = PIT_SCALE / SUBTICKS_PER_TICK as u32;
        unsafe {
            outb(PIT_CONTROL, PIT_SET);
            outb(PIT_A, (divisor & (PIT_MASK as u32)) as u8);
            outb(PIT_A, ((divisor >> 8) & (PIT_MASK as u32)) as u8);
        }

        Timer {
            start: AtomicU64::new(RealTimeClock::new().time().secs),
            ticks: AtomicU64::new(0),
            seconds: AtomicU64::new(0),
            millis: AtomicU64::new(0),
        }
    }
}

lazy_static! {
    pub static ref TIMER: Timer = { Timer::new() };
}

pub fn timer_interrupt() {
    TIMER.ticks.fetch_add(1, Ordering::Relaxed);
    TIMER.millis.fetch_add(1, Ordering::Relaxed);
    if (TIMER.millis.load(Ordering::Relaxed) == SUBTICKS_PER_TICK) {
        TIMER.seconds.fetch_add(1, Ordering::Relaxed);
        TIMER.millis.store(0, Ordering::Relaxed);
    }
}

pub fn time_since_start() -> Time {
    Time::new(
        TIMER.seconds.load(Ordering::Relaxed),
        TIMER.millis.load(Ordering::Relaxed) as u32 * 1000,
        0,
    )
}

#[allow(dead_code)]
pub fn monotonic_clock() -> u64 {
    TIMER.ticks.load(Ordering::Relaxed)
}

pub fn real_time() -> u64 {
    TIMER.start.load(Ordering::Relaxed) + TIMER.seconds.load(Ordering::Relaxed)
}

fn cvt_bcd(value: usize) -> usize {
    (value & 0xF) + ((value / 16) * 10)
}

struct RealTimeClock {
    address: Port<u8>,
    data: Port<u8>,
}

impl RealTimeClock {
    pub fn new() -> RealTimeClock {
        unsafe {
            RealTimeClock {
                address: Port::new(0x70),
                data: Port::new(0x71),
            }
        }
    }

    /// Read
    unsafe fn read(&mut self, reg: u8) -> u8 {
        self.address.write(reg);
        return self.data.read();
    }

    /// Wait
    unsafe fn wait(&mut self) {
        while self.read(0xA) & 0x80 != 0x80 {}
        while self.read(0xA) & 0x80 == 0x80 {}
    }

    /// Get time
    pub fn time(&mut self) -> Time {
        let mut second;
        let mut minute;
        let mut hour;
        let mut day;
        let mut month;
        let mut year;
        let mut century;
        let register_b;
        unsafe {
            self.wait();
            second = self.read(0) as usize;
            minute = self.read(2) as usize;
            hour = self.read(4) as usize;
            day = self.read(7) as usize;
            month = self.read(8) as usize;
            year = self.read(9) as usize;
            century = self.read(0x32) as usize - 1;
            register_b = self.read(0xB);
        }

        if register_b & 4 != 4 {
            second = cvt_bcd(second);
            minute = cvt_bcd(minute);
            hour = cvt_bcd(hour & 0x7F) | (hour & 0x80);
            day = cvt_bcd(day);
            month = cvt_bcd(month);
            year = cvt_bcd(year);
            century = cvt_bcd(year) - 1;
        }

        if register_b & 2 != 2 || hour & 0x80 == 0x80 {
            hour = ((hour & 0x7F) + 12) % 24;
        }

        year += 1000 + century * 100;

        // Unix time from clock
        let mut secs: u64 = (year as u64 - 1970) * 31536000;

        let mut leap_days = (year as u64 - 1972) / 4 + 1;
        if year % 4 == 0 {
            if month <= 2 {
                leap_days -= 1;
            }
        }
        secs += leap_days * 86400;

        match month {
            2 => secs += 2678400,
            3 => secs += 5097600,
            4 => secs += 7776000,
            5 => secs += 10368000,
            6 => secs += 13046400,
            7 => secs += 15638400,
            8 => secs += 18316800,
            9 => secs += 20995200,
            10 => secs += 23587200,
            11 => secs += 26265600,
            12 => secs += 28857600,
            _ => (),
        }

        secs += (day as u64 - 1) * 86400;
        secs += hour as u64 * 3600;
        secs += minute as u64 * 60;
        secs += second as u64;

        Time::new(secs, 0, 0)
    }
}
