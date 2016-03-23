
use constants::timer::{PIT_SCALE,PIT_CONTROL,PIT_SET,PIT_A,PIT_MASK,SUBTICKS_PER_TICK};

use x86::io::outb;

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

pub fn monotonic_clock() -> (u64) {
  unsafe {
    timer_ticks
  }
}