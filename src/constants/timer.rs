
pub const PIT_A:u16 = 0x40;
pub const PIT_CONTROL:u16 = 0x43;

pub const PIT_MASK:u8 = 0xFF;
pub const PIT_SCALE:u32 = 1193180;
pub const PIT_SET:u8 = 0x36;

pub const TIMER_INTERRUPT:u32 = 0x20;

pub const SUBTICKS_PER_TICK:u16 = 1000;

pub const NANOS_PER_MICRO: i32 = 1000;
pub const NANOS_PER_MILLI: i32 = 1000000;
pub const NANOS_PER_SEC: i32 = 1000000000;