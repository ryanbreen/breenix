
use constants::timer::{PIT_SCALE,PIT_CONTROL,PIT_SET,PIT_A,PIT_MASK,SUBTICKS_PER_TICK};

use x86::io::outb;
use io::Port;
use util::time::Duration;

static mut timer_ticks:u64 = 0;
static mut timer_seconds:u64 = 0;
static mut timer_millis:u16 = 0;

pub fn initialize() {
  let divisor:u32 = PIT_SCALE / SUBTICKS_PER_TICK as u32;
  unsafe {
    outb(PIT_CONTROL, PIT_SET);
    outb(PIT_A, (divisor & (PIT_MASK as u32)) as u8);
    outb(PIT_A, ((divisor >> 8) & (PIT_MASK as u32)) as u8);
  }
}

pub fn timer_interrupt() {
  unsafe {
    timer_ticks += 1;
    timer_millis += 1;
    if timer_millis == SUBTICKS_PER_TICK {
      timer_seconds += 1;
      timer_millis = 0;
    }
  }

  //wakeup_sleepers(timer_ticks, timer_subticks);
  //switch_task(1);
}

pub fn time_since_start() -> (u64,u16) {
  unsafe {
    (timer_seconds, timer_millis)
  }
}

pub fn monotonic_clock() -> u64 {
  unsafe {
    timer_ticks
  }
}

pub fn real_time() -> Duration {
  RealTimeClock::new().time()
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
  pub fn time(&mut self) -> Duration {
    let mut second;
    let mut minute;
    let mut hour;
    let mut day;
    let mut month;
    let mut year;
    let register_b;
    unsafe {
      self.wait();
      second = self.read(0) as usize;
      minute = self.read(2) as usize;
      hour = self.read(4) as usize;
      day = self.read(7) as usize;
      month = self.read(8) as usize;
      year = self.read(9) as usize;
      register_b = self.read(0xB);
    }

    if register_b & 4 != 4 {
      second = cvt_bcd(second);
      minute = cvt_bcd(minute);
      hour = cvt_bcd(hour & 0x7F) | (hour & 0x80);
      day = cvt_bcd(day);
      month = cvt_bcd(month);
      year = cvt_bcd(year);
    }

    if register_b & 2 != 2 || hour & 0x80 == 0x80 {
      hour = ((hour & 0x7F) + 12) % 24;
    }

    // TODO: Century Register
    year += 2000;

    // Unix time from clock
    let mut secs: i64 = (year as i64 - 1970) * 31536000;

    let mut leap_days = (year as i64 - 1972) / 4 + 1;
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

    secs += (day as i64 - 1) * 86400;
    secs += hour as i64 * 3600;
    secs += minute as i64 * 60;
    secs += second as i64;

    Duration::new(secs, 0)
  }
}