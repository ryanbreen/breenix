//! Private IPC between the case and its independent client process.
pub const COMMAND_SIZE: usize = 16;
pub const RESPONSE_SIZE: usize = 72;
pub const SNAPSHOT: u32 = 0;
pub const PAINT: u32 = 1;
pub const RESIZE: u32 = 2;
pub const REFUSE: u32 = 3;
pub const CLEAR_EVENTS: u32 = 4;
pub const EXIT: u32 = 6;
pub const EXEC: u32 = 7;
pub const CLIP_SET: u32 = 8;
pub const CLIP_GET: u32 = 9;
pub const ANIMATE: u32 = 10;
pub const STEP: u32 = 11;
pub const REGISTER: u32 = 12;

pub fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

pub const SET_FLAGS: u32 = 13;
pub const SET_SLOW: u32 = 14;
pub const SELECT_WINDOW: u32 = 15;
pub const SET_TITLE: u32 = 16;
pub const CLIP_SUBSCRIBE: u32 = 17;

pub mod flags {
    pub const REFUSE_RESIZE: u32 = 1;
    pub const UNREGISTERED: u32 = 2;
    pub const IGNORE_CLOSE: u32 = 4;
    pub const HANG: u32 = 8;
    pub const NEVER_READ: u32 = 16;
    pub const TWO_WINDOWS: u32 = 32;
    pub const SLOW_COMMIT: u32 = 64;
    pub const ANIMATE: u32 = 128;
    pub const CHROMELESS: u32 = 256;
    pub const CRASH_AFTER: u32 = 512;
    pub const MASK: u32 = 1023;
}
pub const CREATE_WINDOW: u32 = 18;
pub const WAIT_EVENTS: u32 = 19;

pub const FIELD_SET: u32 = 20;
pub const FIELD_GET: u32 = 21;
