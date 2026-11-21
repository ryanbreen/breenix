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

pub const SNAPSHOT_CREATE: u64 = 0xb824_1401;
pub const SNAPSHOT_DELETE: u64 = 0xb824_1402;
pub const SNAPSHOT_STATUS: u64 = 0xb824_1403;
pub const SNAPSHOT_RENAME: u64 = 0xb824_1404;
pub const SNAPSHOT_NEXT: u64 = 0xb824_1405;
pub const SNAPSHOT_READ_ONLY: u32 = 1;
pub const SNAPSHOT_EMPTY: u32 = 2;

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SnapshotRequest {
    pub flags: u32,
    pub name_len: u32,
    pub name: [u8; 256],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SnapshotStatus {
    pub root_id: u64,
    pub flags: u32,
    pub roots: u32,
    pub dead_remaining: u64,
}

const _: () = assert!(core::mem::size_of::<SnapshotRequest>() == 264);
const _: () = assert!(core::mem::size_of::<SnapshotStatus>() == 24);

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SnapshotRename {
    pub from: SnapshotRequest,
    pub to: SnapshotRequest,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SnapshotNext {
    /// On input this is the exclusive cursor. On output this is the next ID or the sentinel 0.
    pub id: u64,
    pub root: SnapshotRequest,
}

const _: () = assert!(core::mem::size_of::<SnapshotRename>() == 528);
const _: () = assert!(core::mem::size_of::<SnapshotNext>() == 272);
