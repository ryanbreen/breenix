
use constants::timer::{PIT_SCALE,PIT_CONTROL,PIT_SET,PIT_A,PIT_MASK,SUBTICKS_PER_TICK};

use x86::io::outb;

static mut timer_ticks:u64 = 0;
static mut timer_subticks:u8 = 0;

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
    timer_subticks += 1;
    if timer_subticks == SUBTICKS_PER_TICK {
      timer_ticks += 1;
      timer_subticks = 0;

      println!("{}", timer_ticks);
    }
  }

  //wakeup_sleepers(timer_ticks, timer_subticks);
  //switch_task(1);
}