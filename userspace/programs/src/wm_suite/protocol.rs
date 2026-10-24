//! Private IPC between the case and its independent client process.
pub const COMMAND_SIZE: usize = 16;
pub const RESPONSE_SIZE: usize = 40;
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

pub fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}
