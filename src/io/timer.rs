
use constants::timer::{PIT_SCALE,PIT_CONTROL,PIT_SET,PIT_A,PIT_MASK,SUBTICKS_PER_TICK};

use x86::io::outb;

pub fn initialize() {
  let divisor:u32 = PIT_SCALE / SUBTICKS_PER_TICK as u32;
  unsafe {
    outb(PIT_CONTROL, PIT_SET);
    outb(PIT_A, (divisor & PIT_MASK as u32) as u8);
    outb(PIT_A, ((divisor >> 8) & (PIT_MASK as u32)) as u8);
  }
}
