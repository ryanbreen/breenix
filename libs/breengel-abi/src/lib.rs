//! The window interface's binary layout, transcribed from the BreenGL contract
//! (`docs/gui/breengel.md`): system call 414 `window` (section 17), its events (section 19),
//! the composite record (section 18), the observation record (section 20.1) and system call
//! 415 `test_control` (section 21).
//!
//! Every structure is `repr(C)` at the contract's literal offsets, which the `layout!` asserts
//! below check at compile time. The kernel, libbreenix, Breengel, the suite's helpers and bwm
//! all take their definitions from here; a change to this crate starts with a change to the
//! contract.
#![no_std]

/// System call 414: `window(op: u32, arg: *mut u8, len: usize) -> isize`.
pub const SYS_WINDOW: u64 = 414;
/// System call 415: `test_control(op: u32, arg: *mut u8, len: usize) -> isize`.
pub const SYS_TEST_CONTROL: u64 = 415;
/// What `window(VERSION)` returns.
pub const INTERFACE_VERSION: u32 = 2;

/// The operations of system call 414 (section 17.1).
pub mod op {
    pub const VERSION: u32 = 0;
    pub const CREATE: u32 = 1;
    pub const DESTROY: u32 = 2;
    pub const REGISTER: u32 = 3;
    pub const SET_TITLE: u32 = 4;
    pub const LIST: u32 = 5;
    pub const ACQUIRE: u32 = 6;
    pub const COMMIT: u32 = 7;
    pub const COMMIT_NO_CHANGE: u32 = 8;
    pub const RESIZE: u32 = 9;
    pub const READ_EVENTS: u32 = 10;
    pub const WRITE_EVENTS: u32 = 11;
    pub const EVENT_FD: u32 = 12;
    pub const SET_GEOMETRY: u32 = 13;
    pub const MAP_COMMITTED: u32 = 14;
    pub const CAPTURE: u32 = 15;
    pub const MAP_TREE: u32 = 16;
    pub const CLAIM_COMPOSITOR: u32 = 17;
    pub const RELEASE_COMPOSITOR: u32 = 18;
    pub const COMPOSITE: u32 = 19;
    pub const COMPOSITE_RECORD: u32 = 20;
    pub const MAP_COMPOSITOR_TEXTURE: u32 = 21;
    pub const SCANOUT_READBACK: u32 = 22;
    pub const SET_CURSOR: u32 = 23;
    pub const HOLD_EVIDENCE: u32 = 24;
    pub const READ_REMNANT: u32 = 25;
    pub const RELEASE_REMNANT: u32 = 26;
    pub const GIVE_DISPLAY: u32 = 27;
    pub const READ_META: u32 = 28;
    pub const COMPOSITE_SNAPSHOT: u32 = 29;
    pub const COMPOSITOR_ACK: u32 = 30;
    pub const COMPOSITOR_STATUS: u32 = 31;
    pub const CLIPBOARD_SET: u32 = 32;
    pub const CLIPBOARD_GET: u32 = 33;
    pub const CLIPBOARD_SUBSCRIBE: u32 = 34;
    pub const HIT_TEST: u32 = 35;
    pub const EVENT_TAG: u32 = 36;
    pub const GESTURE_REPORT: u32 = 37;
    /// Ops above this highest allocated number return EINVAL (section 17.1).
    pub const LAST: u32 = GESTURE_REPORT;
}

/// Linux's errno values, which the interface returns negated (section 25).
pub mod errno {
    pub const EPERM: i32 = 1;
    pub const ENOENT: i32 = 2;
    pub const ESRCH: i32 = 3;
    pub const EINTR: i32 = 4;
    pub const EIO: i32 = 5;
    pub const E2BIG: i32 = 7;
    pub const EAGAIN: i32 = 11;
    pub const ENOMEM: i32 = 12;
    pub const EACCES: i32 = 13;
    pub const EFAULT: i32 = 14;
    pub const EBUSY: i32 = 16;
    pub const EINVAL: i32 = 22;
    pub const ENODEV: i32 = 19;
    pub const EDQUOT: i32 = 122;
    pub const EMFILE: i32 = 24;
    pub const ENOSYS: i32 = 38;
    pub const ETIMEDOUT: i32 = 110;
}

/// The limits of sections 17.2, 17.3 and 19.2.
pub mod limits {
    /// A window is 1 to 4,096 pixels on each side.
    pub const MAX_DIMENSION: u32 = 4096;
    /// A title is at most 64 bytes of UTF-8.
    pub const MAX_TITLE: usize = 64;
    /// A process owns at most 16 windows.
    pub const MAX_WINDOWS_PER_PROCESS: usize = 16;
    /// A commit carries at most 16 damage rectangles, and a replay set at most 16.
    pub const MAX_DAMAGE: usize = 16;
    /// The kernel keeps the damage of each window's last 8 commits.
    pub const DAMAGE_LOG: usize = 8;
    /// Two buffers, or three on request.
    pub const MIN_BUFFERS: u32 = 2;
    pub const MAX_BUFFERS: u32 = 3;
    /// A tree page set of `tree_kib` KiB per buffer: 0 means 64, at most 1,024.
    pub const DEFAULT_TREE_KIB: u32 = 64;
    pub const MAX_TREE_KIB: u32 = 1024;
    /// Each window's event ring holds 64 events.
    pub const EVENT_RING: usize = 64;
    /// An event's text is at most 240 bytes.
    pub const EVENT_TEXT: usize = 240;
    /// A holder keeps at most 16 remnants.
    pub const MAX_REMNANTS: usize = 16;
    /// Entries of a composite record or an observation record's window list.
    pub const RECORD_WINDOWS: usize = 16;
    /// Completions in an observation record.
    pub const RECORD_COMPLETIONS: usize = 12;
    /// The screen every GUI suite boot runs (section 18).
    pub const SCREEN_WIDTH: u32 = 1280;
    pub const SCREEN_HEIGHT: u32 = 800;
}

/// A half-open rectangle (section 1): `x <= px < x + w`, `y <= py < y + h`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    pub const fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }
}

/// Argument of `DESTROY`, `EVENT_FD`, `HOLD_EVIDENCE` and `RELEASE_REMNANT`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WinId {
    pub id: u64,
}

/// Argument of `CREATE`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Create {
    pub width: u32,
    pub height: u32,
    /// 2 or 3.
    pub buffers: u32,
    /// 0 means 64; at most 1,024.
    pub tree_kib: u32,
    /// Out: the new window's id.
    pub id: u64,
    /// Out: bytes per row of every buffer.
    pub stride: u32,
    pub reserved: u32,
    /// Out: the mapped address of each buffer, 0 past `buffers`.
    pub buffer: [u64; 3],
    /// Out: the first tree page; buffer `i`'s page is `tree + i * tree_kib * 1024`.
    pub tree: u64,
}

/// Argument of `REGISTER` and `SET_TITLE`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Title {
    pub id: u64,
    pub ptr: u64,
    pub len: u32,
    pub reserved: u32,
}

/// Argument of `LIST`: an array of `capacity` `WinInfo` at `ptr`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct List {
    pub ptr: u64,
    pub capacity: u32,
    pub reserved: u32,
}

/// `WinInfo.flags` and `Geometry.flags` bits.
pub mod win_flags {
    /// `WinInfo` only.
    pub const REGISTERED: u32 = 1 << 0;
    pub const VISIBLE: u32 = 1 << 1;
    pub const MINIMIZED: u32 = 1 << 2;
    pub const FOCUSED: u32 = 1 << 3;
}

/// `Geometry.flags` bits.
pub mod geometry_flags {
    pub const VISIBLE: u32 = 1 << 0;
    pub const MINIMIZED: u32 = 1 << 1;
    pub const FOCUSED: u32 = 1 << 2;
}

/// The display backends (`WinInfo.backend`, `ObservationRecord.backend`).
pub mod backend {
    pub const NONE: u32 = 0;
    pub const VIRTIO_GPU_2D: u32 = 1;
    pub const GOP_FRAMEBUFFER: u32 = 2;
    pub const SVGA3_SCREEN_TARGET: u32 = 3;
    pub const VIRGL: u32 = 4;

    /// The backend's name as the suite reports it.
    pub const fn name(code: u32) -> &'static str {
        match code {
            VIRTIO_GPU_2D => "virtio-gpu 2D",
            GOP_FRAMEBUFFER => "GOP framebuffer",
            SVGA3_SCREEN_TARGET => "SVGA3 screen target",
            VIRGL => "VirGL",
            _ => "none",
        }
    }
}

/// One entry of `LIST`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WinInfo {
    pub id: u64,
    /// The owner's pid.
    pub owner: u32,
    /// `win_flags` bits.
    pub flags: u32,
    pub width: u32,
    pub height: u32,
    pub x: i32,
    pub y: i32,
    pub z: u32,
    pub title_len: u32,
    pub title: [u8; 64],
    pub commit_id: u64,
    pub pixel_gen: u64,
    /// A `backend` code.
    pub backend: u32,
    pub reserved: u32,
}

/// Argument of `ACQUIRE`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Acquire {
    pub id: u64,
    /// `u64::MAX` waits forever.
    pub timeout_ns: u64,
    /// Out: the acquired buffer's index.
    pub buffer: u32,
    /// Out: 1 when the whole window was replayed.
    pub replay_full: u32,
    /// Out: the commit id the buffer holds, 0 for none.
    pub base: u64,
    /// Out: entries of `replay`.
    pub replay_count: u32,
    pub reserved: u32,
    /// Out: the rectangles replayed into the buffer.
    pub replay: [Rect; 16],
}

/// Argument of `COMMIT`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Commit {
    pub id: u64,
    pub buffer: u32,
    /// At most 16.
    pub damage_count: u32,
    pub damage: [Rect; 16],
    /// A pointer to a `Meta`, or 0.
    pub meta: u64,
    pub meta_len: u32,
    pub reserved: u32,
    /// Out.
    pub commit_id: u64,
    /// Out.
    pub pixel_gen: u64,
}

/// Argument of `COMMIT_NO_CHANGE`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NoChange {
    pub id: u64,
    pub meta: u64,
    pub meta_len: u32,
    pub reserved: u32,
    /// Out.
    pub commit_id: u64,
}

/// The metadata a commit publishes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Meta {
    pub processed_serial: u64,
    pub epoch: u64,
    pub ui_time_ms: u64,
    pub layout_version: u64,
    pub real_clock_reads: u64,
    pub tree_serializations: u64,
    pub rejected_count: u32,
    pub reserved: u32,
    pub rejected: [u64; 8],
    pub flags: u32,
    pub reserved2: u32,
}

/// Argument of `RESIZE`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Resize {
    pub id: u64,
    pub width: u32,
    pub height: u32,
    /// Out.
    pub stride: u32,
    pub reserved: u32,
    /// Out: the new buffers' GPU handles.
    pub buffer: [u64; 3],
}

/// `ReadEvents.flags` bit 0: block until an event arrives.
pub const READ_EVENTS_BLOCK: u32 = 1 << 0;

/// Argument of `READ_EVENTS`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReadEvents {
    pub id: u64,
    /// An array of `capacity` `WindowEvent`.
    pub ptr: u64,
    pub capacity: u32,
    pub flags: u32,
    pub timeout_ns: u64,
    /// Out.
    pub lost: u64,
}

/// Argument of `WRITE_EVENTS`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WriteEvents {
    pub id: u64,
    /// An array of `count` `WindowEvent`.
    pub ptr: u64,
    pub count: u32,
    pub reserved: u32,
}

/// Argument of `SET_GEOMETRY`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Geometry {
    pub id: u64,
    pub x: i32,
    pub y: i32,
    pub z: u32,
    /// `geometry_flags` bits.
    pub flags: u32,
}

/// Argument of `MAP_COMMITTED`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MapBuffers {
    pub id: u64,
    /// Out.
    pub buffer: [u64; 3],
    /// Out.
    pub stride: u32,
    pub reserved: u32,
}

/// Argument of `CAPTURE` and `READ_REMNANT`. Pixels are copied as BGRX rows of `width * 4`
/// bytes, X normalized to 0xFF.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Capture {
    pub id: u64,
    pub dst: u64,
    pub dst_len: u64,
    /// 0 for none.
    pub tree_dst: u64,
    pub tree_len: u64,
    /// Out.
    pub commit_id: u64,
    /// Out.
    pub width: u32,
    /// Out.
    pub height: u32,
}

/// Argument of `MAP_TREE` and `MAP_COMPOSITOR_TEXTURE`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MapTree {
    pub id: u64,
    /// Out.
    pub addr: u64,
    /// Out.
    pub len: u64,
}

/// Argument of `CLAIM_COMPOSITOR`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Claim {
    pub flags: u32,
    pub reserved: u32,
    /// Out: the claim's epoch.
    pub epoch: u64,
}

/// Argument of `COMPOSITE`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Composite {
    pub snapshot: u64,
    pub target: u64,
    pub stream: u64,
    pub length: u64,
    pub flags: u32,
    pub reserved: u32,
    pub serial: u64,
    pub pixels: u64,
    pub render_fence: u64,
    pub present_fence: u64,
    pub estimate: u64,
    pub error_index: u32,
    pub error: u32,
    pub reason: u32,
    pub fence_status: u32,
    pub answer: u32,
    pub phase: u32,
    pub scene_target: u64,
}

/// Metadata-only reservation: no client buffer stays pinned between syscalls.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SceneSnapshot {
    pub context: u64,
    pub windows: u64,
    pub window_capacity: u32,
    pub window_count: u32,
    pub damage: u64,
    pub damage_capacity: u32,
    pub damage_count: u32,
    pub snapshot: u64,
    pub width: u32,
    pub height: u32,
    pub scale: u32,
    pub flags: u32,
    pub cursor: Cursor,
    pub claim_epoch: u64,
    pub owner_sequence: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SceneWindow {
    pub id: u64,
    pub sample: u64,
    pub commit_id: u64,
    pub pixel_gen: u64,
    pub render_fence: u64,
    pub target_generation: u64,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub z: u32,
    pub stride: u32,
    pub format: u32,
    pub reserved: u32,
}

/// Argument of `COMPOSITE_RECORD`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RecordQuery {
    pub serial: u64,
    /// A `CompositeRecord` buffer of `len` bytes.
    pub ptr: u64,
    pub len: u32,
    pub reserved: u32,
}

/// Argument of `SCANOUT_READBACK`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Readback {
    pub dst: u64,
    pub dst_len: u64,
    /// An `ObservationRecord` of `record_len` bytes.
    pub record: u64,
    pub record_len: u32,
    pub reserved: u32,
    /// Out.
    pub serial: u64,
    pub reason: u32,
    pub reserved2: u32,
}

/// Cursor shapes (`Cursor.shape`).
pub mod cursor_shape {
    pub const ARROW: u32 = 0;
    pub const RESIZE_NS: u32 = 1;
    pub const RESIZE_EW: u32 = 2;
    pub const RESIZE_NWSE: u32 = 3;
    pub const RESIZE_NESW: u32 = 4;
    pub const HIDDEN: u32 = 5;
}

/// Argument of `SET_CURSOR`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cursor {
    pub shape: u32,
    pub visible: u32,
    pub x: i32,
    pub y: i32,
}

impl Cursor {
    /// The sprite's offset from the pointer to its box's top-left corner, and the box's side.
    pub const BOX_OFFSET: i32 = 12;
    pub const BOX: i32 = 36;

    /// The screen rectangle the sprite can cover, which moving, reshaping or hiding it
    /// damages (section 18): `BOX` pixels square from `BOX_OFFSET` above and left of the
    /// pointer; while hidden there is no footprint.
    pub fn footprint(&self) -> Option<Rect> {
        (self.visible != 0 && self.shape < cursor_shape::HIDDEN)
            .then(|| Rect::new(self.x - Self::BOX_OFFSET, self.y - Self::BOX_OFFSET, Self::BOX, Self::BOX))
    }
}

/// Argument of `GIVE_DISPLAY`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Give {
    pub pid: u32,
    pub reserved: u32,
}

/// Argument of `READ_META`: the newest commit's metadata, copied to `meta`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MetaQuery {
    pub id: u64,
    /// A `Meta` the kernel fills.
    pub meta: u64,
    /// Out: the commit whose metadata it is, 0 before the first.
    pub commit_id: u64,
}

/// `CompositeRecordHeader.flags` (section 18).
pub mod record_flags {
    /// The composite failed (`COMPOSITE` returned EIO); nothing of it reached the display.
    pub const FAILED: u32 = 1;
    /// A failed composite's reason, the display backend's code, is `(flags >> REASON_SHIFT) & 0xFF`.
    pub const REASON_SHIFT: u32 = 8;
}

/// `COMPOSITE_RECORD`'s header (section 18); `count` `CompositeEntry` follow it.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CompositeRecordHeader {
    pub serial: u64,
    pub pixels: u64,
    pub count: u32,
    pub flags: u32,
}

/// One window of a composite record.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CompositeEntry {
    pub id: u64,
    pub commit_id: u64,
    pub pixel_gen: u64,
    pub pixels: u64,
}

/// E18 compositor acknowledgment (breengel.md 17.1).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CompositorAck {
    pub claim_epoch: u64,
    pub epoch: u64,
    pub handled_serial: u64,
    pub presented: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CompositorStatus {
    pub claim_epoch: u64,
    pub epoch: u64,
    pub handled_serial: u64,
    pub presented: u64,
    pub scene_version: u64,
    pub composites: u64,
    pub next_deadline_ns: u64,
    pub owner: u32,
    pub flags: u32,
    pub reserved: [u64; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClipboardSet {
    pub id: u64,
    pub ptr: u64,
    pub len: u64,
    pub serial: u64,
    pub mime: u32,
    pub reserved: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClipboardGet {
    pub id: u64,
    pub dst: u64,
    pub capacity: u64,
    pub len: u64,
    pub serial: u64,
    pub mime: u32,
    pub reserved: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClipboardSubscribe {
    pub id: u64,
    pub flags: u32,
    pub reserved: u32,
}

pub const CLIPBOARD_TEXT_UTF8: u32 = 1;
pub const CLIPBOARD_MAX_BYTES: u64 = 16 * 1024 * 1024;
pub const COMPOSITOR_TEST_CONFIG: u32 = 1;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HitTest {
    pub x: i32,
    pub y: i32,
    pub id: u64,
    pub scene_version: u64,
    pub flags: u32,
    pub reserved: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EventTag {
    pub id: u64,
    pub serial: u64,
    pub source_serial: u64,
    pub scene_version: u64,
    pub layout_version: u64,
    pub gesture: u64,
    pub derived: u32,
    pub flags: u32,
    pub reserved: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GestureReport {
    pub id: u64,
    pub gesture: u64,
    pub source_serial: u64,
    pub state: u32,
    pub reason: u32,
}

/// A window and the commit an observation shows (`ObservationRecord.windows`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ObservedWindow {
    pub id: u64,
    pub commit_id: u64,
}

/// A device completion an observation waited for (`ObservationRecord.completions`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Completion {
    pub fence: u64,
    pub kind: u32,
    pub status: u32,
}

/// `Completion.kind` and `Completion.status` (section 20.1). On virtio-gpu: the type of the
/// command answered and the type of its response, from the virtio specification's GPU device.
/// On SVGA3: the SVGA3D command id and the command buffer's status.
pub mod completion {
    pub const RESOURCE_FLUSH: u32 = 0x0104;
    pub const TRANSFER_TO_HOST_2D: u32 = 0x0105;
    pub const RESP_OK_NODATA: u32 = 0x1100;
    pub const SVGA3_UPDATE_GB_SCREENTARGET: u32 = 1127;
    pub const SVGA3_READBACK_GB_SURFACE: u32 = 1104;
    pub const SVGA3_CB_COMPLETED: u32 = 1;
}

/// `ObservationRecord.flags` bit 0: the device's copy is outside the guest's view.
pub const OBSERVATION_DEVICE_COPY_OUTSIDE: u32 = 1 << 0;

/// What `SCANOUT_READBACK` writes beside the pixels (section 20.1), 512 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObservationRecord {
    pub serial: u64,
    /// A `backend` code.
    pub backend: u32,
    pub flags: u32,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    /// 0 RGB, 1 BGR.
    pub source_format: u32,
    pub resource: u64,
    pub window_count: u32,
    pub completion_count: u32,
    pub windows: [ObservedWindow; 16],
    pub completions: [Completion; 12],
    /// What stays outside the guest's view (section 20.2).
    pub boundary: u32,
    pub reserved: [u8; 12],
}

/// Event kinds (`WindowEvent.kind`, section 19.1).
pub mod event_kind {
    pub const KEY_PRESS: u16 = 1;
    pub const KEY_RELEASE: u16 = 2;
    pub const TEXT: u16 = 3;
    pub const POINTER_MOVE: u16 = 4;
    pub const POINTER_BUTTON: u16 = 5;
    pub const SCROLL: u16 = 6;
    pub const FOCUS_GAINED: u16 = 7;
    pub const FOCUS_LOST: u16 = 8;
    pub const CLOSE_REQUESTED: u16 = 9;
    pub const RESIZED: u16 = 10;
    pub const RESIZE_FAILED: u16 = 11;
    /// Made by Breengel (section 9.3), never written by a compositor.
    pub const FONT_CHANGED: u16 = 12;
    pub const COMPOSITOR_CHANGED: u16 = 13;
    pub const OVERFLOW: u16 = 14;
    pub const RESIZE_REQUESTED: u16 = 15;
    pub const POINTER_ENTER: u16 = 16;
    pub const POINTER_LEAVE: u16 = 17;
    pub const POINTER_CANCEL: u16 = 18;
    pub const CLIPBOARD_CHANGED: u16 = 19;
    pub const CLIPBOARD_LOST: u16 = 20;
}

/// `WindowEvent.flags` bit 0: a repeat.
pub const EVENT_REPEAT: u16 = 1 << 0;

/// `WindowEvent.modifiers` bits.
pub mod modifier {
    pub const SHIFT: u32 = 1 << 0;
    pub const CTRL: u32 = 1 << 1;
    pub const ALT: u32 = 1 << 2;
    pub const SUPER: u32 = 1 << 3;
}

/// Linux button codes (`WindowEvent.button`).
pub mod button {
    pub const LEFT: u32 = 0x110;
    pub const RIGHT: u32 = 0x111;
    pub const MIDDLE: u32 = 0x112;
}

/// One window event, 320 bytes (section 19.1).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowEvent {
    pub serial: u64,
    pub time_ns: u64,
    /// `CompositorChanged` only.
    pub epoch: u64,
    pub kind: u16,
    pub flags: u16,
    pub modifiers: u32,
    pub keycode: u32,
    pub button: u32,
    pub x: i32,
    pub y: i32,
    pub scroll_x: i32,
    /// Positive up.
    pub scroll_y: i32,
    /// `Resized` and `ResizeFailed`.
    pub width: u32,
    pub height: u32,
    /// `PointerButton` only: 1 press, 0 release.
    pub pressed: u32,
    /// At most 240.
    pub text_len: u32,
    /// `Overflow` only.
    pub lost: u32,
    /// PointerButton click count, 0 unspecified or 1/2.
    pub clicks: u32,
    pub text: [u8; 240],
}

impl WindowEvent {
    /// An event of `kind` with every other field zero.
    pub const fn new(kind: u16) -> WindowEvent {
        let mut e = WindowEvent::ZERO;
        e.kind = kind;
        e
    }

    /// The event's text, `text_len` bytes clamped to the field.
    pub fn text(&self) -> &[u8] {
        let n = if self.text_len as usize > self.text.len() {
            self.text.len()
        } else {
            self.text_len as usize
        };
        &self.text[..n]
    }
}

/// The operations of system call 415 (section 21).
pub mod tc_op {
    pub const STATE: u32 = 0;
    pub const ARM: u32 = 1;
    pub const DISARM_ALL: u32 = 2;
    pub const FIRED: u32 = 3;
    /// Not an operation: the latch has no setter, so this and every op above it is EINVAL.
    pub const SET_LATCH: u32 = 4;
}

/// Test-control fault points (section 21).
pub mod tc_point {
    pub const IGNORE_DAMAGE: u32 = 1;
    pub const IRQ_HOLD: u32 = 2;
    pub const EMIT_DIAGNOSTIC: u32 = 3;
    pub const ACQUIRE_STALL: u32 = 4;
    pub const BACKEND_ERROR: u32 = 5;
    pub const SKIP_TRANSFER: u32 = 6;
    pub const WRONG_RESOURCE: u32 = 7;
    pub const STALE_FENCE: u32 = 8;
    pub const CORRUPT_OUTSIDE_DAMAGE: u32 = 9;
    pub const PANEL_CURSOR_NO_RESTORE: u32 = 10;
    pub const RELEASE_STALL: u32 = 11;
    pub const STAGE_LIE:u32=12;
    pub const STAGE_SUBSTITUTE_FRAME:u32=13;
    pub const STAGE_DRAW_IN_WINDOW:u32=14;
    pub const RIBBON_DROP_STAGE:u32=15;
    pub const COMPOSITE_STALL: u32 = 16;
    pub const MOVE_POINTER: u32 = 17;
    pub const CAPTURE_STALL: u32 = 18;
    pub const INPUT_REPORT_SKIPPED: u32 = 33;
    pub const INPUT_WAKE_SKIPPED: u32 = 34;
    pub const LAUNCH_FORK_FAIL: u32 = 35;
    /// The highest point number section 21 defines.
    pub const LAST: u32 = LAUNCH_FORK_FAIL;

    /// Whether section 21 defines `point`.
    pub const fn defined(point: u32) -> bool {
        matches!(point, IGNORE_DAMAGE..=CAPTURE_STALL | INPUT_REPORT_SKIPPED..=LAUNCH_FORK_FAIL)
    }
}

/// `emit-diagnostic`'s writer paths (section 22), its `param`.
pub mod emit_path {
    pub const SERIAL_PRINTLN: u64 = 1;
    pub const LOG_WARN: u64 = 2;
    pub const WRITE_BYTE: u64 = 3;
    pub const WRITE_BYTES_ATOMIC: u64 = 4;
    pub const TRY_PRINT: u64 = 5;
    pub const WRITE_STR_BOUNDED: u64 = 6;
    pub const LOG_PRINT: u64 = 7;
    pub const NOTICE: u64 = 8;
    pub const EMERGENCY_PRINT: u64 = 9;
    pub const LAST: u64 = EMERGENCY_PRINT;
}

/// The fixed payload `emit-diagnostic` writes (section 22), 32 bytes.
pub const EMIT_DIAGNOSTIC_PAYLOAD: &[u8; 32] = b"BREENGEL-EMIT-DIAGNOSTIC-PAYLOAD";

/// Argument of `test_control(ARM)`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TcArm {
    pub point: u32,
    pub reserved: u32,
    pub target: u64,
    pub param: u64,
}

/// Argument of `test_control(FIRED)`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TcFired {
    pub point: u32,
    pub reserved: u32,
    /// Out.
    pub count: u64,
}

/// A structure every bit pattern of which is valid, exchanged with the kernel by value.
///
/// # Safety
/// Implementors are `repr(C)` with no padding and only integer fields, so any bytes are a
/// valid value and every byte of a value is initialized.
pub unsafe trait Plain: Copy + 'static {
    /// The value with every byte zero.
    const ZERO: Self;
}

macro_rules! layout {
    ($t:ident, $size:expr $(, $field:ident @ $off:expr)*) => {
        const _: () = assert!(core::mem::size_of::<$t>() == $size);
        $(const _: () = assert!(core::mem::offset_of!($t, $field) == $off);)*
        // SAFETY: the asserts above pin every field; each type is integers only, without
        // padding, as the sum of its fields' sizes equal to its size shows.
        unsafe impl Plain for $t {
            // SAFETY: all-zero bytes are a valid value of an integer-only structure.
            const ZERO: Self = unsafe { core::mem::zeroed() };
        }
    };
}

layout!(Rect, 16, x @ 0, y @ 4, w @ 8, h @ 12);
layout!(WinId, 8, id @ 0);
layout!(Create, 64, width @ 0, height @ 4, buffers @ 8, tree_kib @ 12, id @ 16, stride @ 24, reserved @ 28,
    buffer @ 32, tree @ 56);
layout!(Title, 24, id @ 0, ptr @ 8, len @ 16, reserved @ 20);
layout!(List, 16, ptr @ 0, capacity @ 8, reserved @ 12);
layout!(WinInfo, 128, id @ 0, owner @ 8, flags @ 12, width @ 16, height @ 20, x @ 24, y @ 28, z @ 32,
    title_len @ 36, title @ 40, commit_id @ 104, pixel_gen @ 112, backend @ 120, reserved @ 124);
layout!(Acquire, 296, id @ 0, timeout_ns @ 8, buffer @ 16, replay_full @ 20, base @ 24, replay_count @ 32,
    reserved @ 36, replay @ 40);
layout!(Commit, 304, id @ 0, buffer @ 8, damage_count @ 12, damage @ 16, meta @ 272, meta_len @ 280,
    reserved @ 284, commit_id @ 288, pixel_gen @ 296);
layout!(NoChange, 32, id @ 0, meta @ 8, meta_len @ 16, reserved @ 20, commit_id @ 24);
layout!(Meta, 128, processed_serial @ 0, epoch @ 8, ui_time_ms @ 16, layout_version @ 24, real_clock_reads @ 32,
    tree_serializations @ 40, rejected_count @ 48, reserved @ 52, rejected @ 56, flags @ 120, reserved2 @ 124);
layout!(Resize, 48, id @ 0, width @ 8, height @ 12, stride @ 16, reserved @ 20, buffer @ 24);
layout!(ReadEvents, 40, id @ 0, ptr @ 8, capacity @ 16, flags @ 20, timeout_ns @ 24, lost @ 32);
layout!(WriteEvents, 24, id @ 0, ptr @ 8, count @ 16, reserved @ 20);
layout!(Geometry, 24, id @ 0, x @ 8, y @ 12, z @ 16, flags @ 20);
layout!(MapBuffers, 40, id @ 0, buffer @ 8, stride @ 32, reserved @ 36);
layout!(Capture, 56, id @ 0, dst @ 8, dst_len @ 16, tree_dst @ 24, tree_len @ 32, commit_id @ 40, width @ 48,
    height @ 52);
layout!(MapTree, 24, id @ 0, addr @ 8, len @ 16);
layout!(Claim, 16, flags @ 0, reserved @ 4, epoch @ 8);
layout!(SceneSnapshot, 96, context @ 0, windows @ 8, window_capacity @ 16, window_count @ 20, damage @ 24, damage_capacity @ 32, damage_count @ 36, snapshot @ 40, width @ 48, height @ 52, scale @ 56, flags @ 60, cursor @ 64, claim_epoch @ 80, owner_sequence @ 88);
layout!(SceneWindow, 80, id @ 0, sample @ 8, commit_id @ 16, pixel_gen @ 24, render_fence @ 32, target_generation @ 40, x @ 48, y @ 52, width @ 56, height @ 60, z @ 64, stride @ 68, format @ 72, reserved @ 76);
layout!(Composite, 112, snapshot @ 0, target @ 8, stream @ 16, length @ 24, flags @ 32, reserved @ 36, serial @ 40, pixels @ 48, render_fence @ 56, present_fence @ 64, estimate @ 72, error_index @ 80, error @ 84, reason @ 88, fence_status @ 92, answer @ 96, phase @ 100, scene_target @ 104);
layout!(RecordQuery, 24, serial @ 0, ptr @ 8, len @ 16, reserved @ 20);
layout!(Readback, 48, dst @ 0, dst_len @ 8, record @ 16, record_len @ 24, reserved @ 28, serial @ 32, reason @ 40, reserved2 @ 44);
layout!(Cursor, 16, shape @ 0, visible @ 4, x @ 8, y @ 12);
layout!(Give, 8, pid @ 0, reserved @ 4);
layout!(MetaQuery, 24, id @ 0, meta @ 8, commit_id @ 16);
layout!(CompositeRecordHeader, 24, serial @ 0, pixels @ 8, count @ 16, flags @ 20);
layout!(CompositeEntry, 32, id @ 0, commit_id @ 8, pixel_gen @ 16, pixels @ 24);
layout!(ObservedWindow, 16, id @ 0, commit_id @ 8);
layout!(Completion, 16, fence @ 0, kind @ 8, status @ 12);
layout!(ObservationRecord, 512, serial @ 0, backend @ 8, flags @ 12, width @ 16, height @ 20, stride @ 24,
    source_format @ 28, resource @ 32, window_count @ 40, completion_count @ 44, windows @ 48,
    completions @ 304, boundary @ 496, reserved @ 500);
layout!(WindowEvent, 320, serial @ 0, time_ns @ 8, epoch @ 16, kind @ 24, flags @ 26, modifiers @ 28,
    keycode @ 32, button @ 36, x @ 40, y @ 44, scroll_x @ 48, scroll_y @ 52, width @ 56, height @ 60,
    pressed @ 64, text_len @ 68, lost @ 72, clicks @ 76, text @ 80);
layout!(TcArm, 24, point @ 0, reserved @ 4, target @ 8, param @ 16);
layout!(TcFired, 16, point @ 0, reserved @ 4, count @ 8);
layout!(CompositorAck, 32, claim_epoch @ 0, epoch @ 8, handled_serial @ 16, presented @ 24);
layout!(CompositorStatus, 80, claim_epoch @ 0, epoch @ 8, handled_serial @ 16, presented @ 24, scene_version @ 32, composites @ 40, next_deadline_ns @ 48, owner @ 56, flags @ 60, reserved @ 64);
layout!(ClipboardSet, 40, id @ 0, ptr @ 8, len @ 16, serial @ 24, mime @ 32, reserved @ 36);
layout!(ClipboardGet, 48, id @ 0, dst @ 8, capacity @ 16, len @ 24, serial @ 32, mime @ 40, reserved @ 44);
layout!(ClipboardSubscribe, 16, id @ 0, flags @ 8, reserved @ 12);
layout!(HitTest, 32, x @ 0, y @ 4, id @ 8, scene_version @ 16, flags @ 24, reserved @ 28);
layout!(EventTag, 64, id @ 0, serial @ 8, source_serial @ 16, scene_version @ 24, layout_version @ 32, gesture @ 40, derived @ 48, flags @ 52, reserved @ 56);
layout!(GestureReport, 32, id @ 0, gesture @ 8, source_serial @ 16, state @ 24, reason @ 28);


/// The size `len` must equal for `op`'s argument, or `None` for an op with no argument
/// (`arg` null, `len` 0) or an unknown op.
pub const fn arg_size(op: u32) -> Option<usize> {
    use core::mem::size_of;
    Some(match op {
        op::CREATE => size_of::<Create>(),
        op::DESTROY | op::EVENT_FD | op::HOLD_EVIDENCE | op::RELEASE_REMNANT => size_of::<WinId>(),
        op::REGISTER | op::SET_TITLE => size_of::<Title>(),
        op::LIST => size_of::<List>(),
        op::ACQUIRE => size_of::<Acquire>(),
        op::COMMIT => size_of::<Commit>(),
        op::COMMIT_NO_CHANGE => size_of::<NoChange>(),
        op::RESIZE => size_of::<Resize>(),
        op::READ_EVENTS => size_of::<ReadEvents>(),
        op::WRITE_EVENTS => size_of::<WriteEvents>(),
        op::SET_GEOMETRY => size_of::<Geometry>(),
        op::MAP_COMMITTED => size_of::<MapBuffers>(),
        op::CAPTURE | op::READ_REMNANT => size_of::<Capture>(),
        op::MAP_TREE | op::MAP_COMPOSITOR_TEXTURE => size_of::<MapTree>(),
        op::CLAIM_COMPOSITOR => size_of::<Claim>(),
        op::COMPOSITE => size_of::<Composite>(),
        op::COMPOSITE_RECORD => size_of::<RecordQuery>(),
        op::SCANOUT_READBACK => size_of::<Readback>(),
        op::SET_CURSOR => size_of::<Cursor>(),
        op::GIVE_DISPLAY => size_of::<Give>(),
        op::READ_META => size_of::<MetaQuery>(),
        op::COMPOSITE_SNAPSHOT => size_of::<SceneSnapshot>(),
        op::COMPOSITOR_ACK => size_of::<CompositorAck>(),
        op::COMPOSITOR_STATUS => size_of::<CompositorStatus>(),
        op::CLIPBOARD_SET => size_of::<ClipboardSet>(),
        op::CLIPBOARD_GET => size_of::<ClipboardGet>(),
        op::CLIPBOARD_SUBSCRIBE => size_of::<ClipboardSubscribe>(),
        op::HIT_TEST => size_of::<HitTest>(),
        op::EVENT_TAG => size_of::<EventTag>(),
        op::GESTURE_REPORT => size_of::<GestureReport>(),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_op_below_last_has_a_size_or_none() {
        for o in 0..=op::LAST {
            let none = matches!(o, op::VERSION | op::RELEASE_COMPOSITOR);
            assert_eq!(arg_size(o).is_none(), none, "op {o}");
        }
        assert_eq!(arg_size(op::LAST + 1), None);
    }

    #[test]
    fn payload_is_32_bytes() {
        assert_eq!(EMIT_DIAGNOSTIC_PAYLOAD.len(), 32);
    }
}

macro_rules! zero_default {
    ($($t:ident),*) => {$(
        impl Default for $t {
            fn default() -> Self {
                <$t as Plain>::ZERO
            }
        }
    )*};
}

zero_default!(WinInfo, ObservationRecord, WindowEvent);
