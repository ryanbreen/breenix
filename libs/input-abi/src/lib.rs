//! Pure wire definitions for `docs/gui/input.md` (E18 W1 and W2).
#![no_std]

pub const VERSION: u32 = 1;
pub const EVENT_SIZE: usize = 32;
pub const TEXT_LIMIT: usize = 256;
pub const READER_EVENTS: usize = 1024;
pub const MAX_CAPABILITIES: usize = 512;
pub const CREATE_MAGIC: [u8; 8] = *b"BINPUT01";
pub const TIME_INJECTOR: u32 = 1;

pub mod kind {
    pub const SYN: u16 = 0;
    pub const KEY: u16 = 1;
    pub const REL: u16 = 2;
    pub const ABS: u16 = 3;
    pub const TEXT: u16 = 6;
}
pub mod syn {
    pub const REPORT: u16 = 0;
    pub const DROPPED: u16 = 3;
    pub const SCENE: u16 = 4;
    pub const DEVICE_REMOVED: u16 = 5;
}
pub mod axis {
    pub const X: u16 = 0;
    pub const Y: u16 = 1;
    pub const HWHEEL: u16 = 6;
    pub const WHEEL: u16 = 8;
}
pub mod key {
    pub const ESC: u16 = 1;
    pub const TAB: u16 = 15;
    pub const Q: u16 = 16;
    pub const ENTER: u16 = 28;
    pub const LEFT_CTRL: u16 = 29;
    pub const A: u16 = 30;
    pub const C: u16 = 46;
    pub const B: u16 = 48;
    pub const V: u16 = 47;
    pub const LEFT_SHIFT: u16 = 42;
    pub const RIGHT_SHIFT: u16 = 54;
    pub const LEFT_ALT: u16 = 56;
    pub const F1: u16 = 59;
    pub const F10: u16 = 68;
    pub const F11: u16 = 87;
    pub const F12: u16 = 88;
    pub const RIGHT_CTRL: u16 = 97;
    pub const RIGHT_ALT: u16 = 100;
    pub const HOME: u16 = 102;
    pub const UP: u16 = 103;
    pub const PAGE_UP: u16 = 104;
    pub const LEFT: u16 = 105;
    pub const RIGHT: u16 = 106;
    pub const END: u16 = 107;
    pub const DOWN: u16 = 108;
    pub const PAGE_DOWN: u16 = 109;
    pub const DELETE: u16 = 111;
    pub const LEFT_SUPER: u16 = 125;
    pub const RIGHT_SUPER: u16 = 126;
    pub const BUTTON_LEFT: u16 = 0x110;
    pub const BUTTON_RIGHT: u16 = 0x111;
    pub const BUTTON_MIDDLE: u16 = 0x112;
}
pub mod modifier {
    pub const SHIFT: u32 = 1;
    pub const CTRL: u32 = 2;
    pub const ALT: u32 = 4;
    pub const SUPER: u32 = 8;
    pub const MASK: u32 = 15;
}
pub mod ioctl {
    pub const GRAB: u64 = 0x424901;
    pub const UNGRAB: u64 = 0x424902;
    pub const DISPOSITION: u64 = 0x424903;
    pub const INPUT_DEVICE_INFO: u64 = 0x424904;
}
pub mod disposition {
    pub const WM_HANDLED: u32 = 1;
    pub const DELIVERED: u32 = 2;
    pub const REJECTED: u32 = 3;
    pub const CLIENT_REJECTED: u32 = 4;
    pub const LOST: u32 = 5;
}
pub mod reason {
    pub const NONE: u32 = 0;
    pub const NO_FOCUS: u32 = 1;
    pub const MINIMIZED: u32 = 2;
    pub const NOT_SELECTED: u32 = 3;
    pub const STALE_SCENE: u32 = 4;
    pub const TARGET_GONE: u32 = 5;
    pub const GRAB: u32 = 6;
    pub const STALE_LAYOUT: u32 = 7;
    pub const GESTURE_CANCELLED: u32 = 8;
    pub const OVERFLOW: u32 = 9;
    pub const RING_FULL: u32 = 10;
    pub const COMPOSITOR_DIED: u32 = 11;
}

/// Device origin returned by INPUT_DEVICE_INFO (W29).
pub mod device_kind {
    pub const PHYSICAL: u32 = 1;
    pub const VIRTUAL: u32 = 2;
}

/// Read-only reader query. Set device, capabilities and capacity on input;
/// kind, generation and count are output. Reserved must be zero.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DeviceInfo {
    pub device: u32,
    pub kind: u32,
    pub generation: u64,
    pub capabilities: u64,
    pub capacity: u32,
    pub count: u32,
    pub reserved: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Event {
    pub serial: u64,
    pub time_ns: u64,
    pub device: u32,
    pub kind: u16,
    pub code: u16,
    pub value: i32,
    pub flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CreateHeader {
    pub magic: [u8; 8],
    pub flags: u32,
    pub capabilities: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Capability {
    pub kind: u16,
    pub code: u16,
    pub minimum: i32,
    pub maximum: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub devices: u64,
    pub count: u32,
    pub reserved: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SceneTag {
    pub scene_version: u64,
    pub window: u64,
    pub layout_version: u64,
    pub gesture: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Disposition {
    pub serial: u64,
    pub window: u64,
    pub disposition: u32,
    pub reason: u32,
    pub derived: u32,
    pub reserved: u32,
}

macro_rules! layout {
    ($t:ty, $size:expr, $($field:ident @ $offset:expr),+ $(,)?) => {
        const _: () = {
            assert!(core::mem::size_of::<$t>() == $size);
            $(assert!(core::mem::offset_of!($t, $field) == $offset);)+
        };
    };
}
layout!(Event, 32, serial @ 0, time_ns @ 8, device @ 16, kind @ 20, code @ 22, value @ 24, flags @ 28);
layout!(CreateHeader, 16, magic @ 0, flags @ 8, capabilities @ 12);
layout!(Capability, 12, kind @ 0, code @ 2, minimum @ 4, maximum @ 8);
layout!(Selection, 16, devices @ 0, count @ 8, reserved @ 12);
layout!(SceneTag, 32, scene_version @ 0, window @ 8, layout_version @ 16, gesture @ 24);
layout!(Disposition, 32, serial @ 0, window @ 8, disposition @ 16, reason @ 20, derived @ 24, reserved @ 28);

layout!(DeviceInfo, 40, device @ 0, kind @ 4, generation @ 8, capabilities @ 16, capacity @ 24, count @ 28, reserved @ 32);
