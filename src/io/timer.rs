use crate::constants::timer::{PIT_SCALE, PIT_CONTROL, PIT_SET, PIT_A, PIT_MASK, SUBTICKS_PER_TICK};

use x86::io::outb;
use crate::io::Port;
use crate::util::time::Time;

static mut TIMER_START: u64 = 0;
static mut TIMER_TICKS: u64 = 0;
static mut TIMER_SECONDS: u64 = 0;
static mut TIMER_MILLIS: u16 = 0;

pub fn initialize() {
    let divisor: u32 = PIT_SCALE / SUBTICKS_PER_TICK as u32;
    unsafe {
        outb(PIT_CONTROL, PIT_SET);
        outb(PIT_A, (divisor & (PIT_MASK as u32)) as u8);
        outb(PIT_A, ((divisor >> 8) & (PIT_MASK as u32)) as u8);

        TIMER_START = RealTimeClock::new().time().secs;
    }
}

pub fn timer_interrupt() {
    unsafe {
        TIMER_TICKS += 1;
        TIMER_MILLIS += 1;
        if TIMER_MILLIS == SUBTICKS_PER_TICK {
            TIMER_SECONDS += 1;
            TIMER_MILLIS = 0;
        }
    }
}

pub fn time_since_start() -> Time {
    unsafe { Time::new(TIMER_SECONDS, TIMER_MILLIS as u32 * 1000, 0) }
}

#[allow(dead_code)]
pub fn monotonic_clock() -> u64 {
    unsafe { TIMER_TICKS }
}

pub fn real_time() -> u64 {
    unsafe { TIMER_START + TIMER_SECONDS }
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

        unsafe {
            secs += (TIMER_MILLIS / 1000) as u64;
        }

        Time::new(secs, 0, 0)
    }
}
