//! Scrub wire interface shared by the kernel and the breenfs command.
// No allocation, architecture-specific types or kernel dependencies.
pub const SCRUB_START: u64 = 0xb824_1301;
pub const SCRUB_CANCEL: u64 = 0xb824_1302;
pub const SCRUB_STATUS: u64 = 0xb824_1303;

pub const SCRUB_IDLE: u32 = 0;
pub const SCRUB_RUNNING: u32 = 1;
pub const SCRUB_DONE: u32 = 2;
pub const SCRUB_CANCELLED: u32 = 3;
pub const SCRUB_FAILED: u32 = 4;
pub const BAD_NODE: u32 = 1;
pub const BAD_DATA: u32 = 2;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BadBlock {
    pub block: u64,
    pub kind: u32,
    pub reserved: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ScrubStatus {
    pub state: u32,
    pub bad_count: u32,
    pub bytes_scanned: u64,
    pub errors: u64,
    pub bad: [BadBlock; 8],
    pub last_scrub_generation: u64,
    pub last_scrub_errors: u64,
}

const _: () = assert!(core::mem::size_of::<BadBlock>() == 16);
const _: () = assert!(core::mem::size_of::<ScrubStatus>() == 168);
