//! Host fault injection for the actual bwm compositor. Run with rustc --test.
extern crate self as bgl;
extern crate self as libbreenix;
use std::cell::RefCell;

#[path = "../userspace/programs/src/bwm/compositor.rs"]
pub mod compositor;

#[derive(Default)]
struct Device {
    listed: (u32, u32),
    frame: (u32, u32),
    pending: bool,
    second_pending: bool,
    fail_upload: bool,
    fail_texture: bool,
    uploads: Vec<Vec<u8>>,
}
thread_local! { static DEVICE: RefCell<Device> = RefCell::new(Device::default()); }
pub struct Context;
impl Context {
    pub fn new() -> Result<Self, i64> { Ok(Self) }
    pub fn handle(&self) -> u64 { 1 }
    pub fn begin(&mut self, _: u64, _: u32, _: u32) {}
    pub fn copy(&mut self, _: u64, _: &[[i32; 6]]) {}
    pub fn solid(&mut self, _: u32, _: &[[i32; 4]]) {}
    pub fn submit(&mut self) -> Result<(), i64> { Ok(()) }
    pub fn present(&self, _: u64) -> Result<(), i64> { Ok(()) }
}
pub mod time { pub fn sleep_ms(_: u32) -> Result<(), i64> { Ok(()) } }
pub mod graphics {
    #[derive(Clone, Copy, Default)]
    pub struct WindowInfo { pub buffer_id: u32, pub width: u32, pub height: u32 }
    pub mod cursor_shape { pub const RESIZE_NS: u32 = 1; pub const RESIZE_EW: u32 = 2; pub const RESIZE_NWSE: u32 = 3; pub const RESIZE_NESW: u32 = 4; }
    pub fn list_windows(out: &mut [WindowInfo]) -> Result<usize, &'static str> {
        super::DEVICE.with(|d| { let d = d.borrow(); out[0] = WindowInfo { buffer_id: 1, width: d.listed.0, height: d.listed.1 }; });
        super::DEVICE.with(|d| {
            if d.borrow().second_pending {
                out[1] = WindowInfo { buffer_id: 2, ..out[0] };
                Ok(2)
            } else { Ok(1) }
        })
    }
    pub fn read_window_buffer(id: u32, out: &mut [u8]) -> Result<(u32, u32), &'static str> {
        super::DEVICE.with(|d| {
            let mut d = d.borrow_mut();
            if !(if id == 2 { d.second_pending } else { d.pending }) { return Ok((0, 0)); }
            let dims = d.frame;
            let size = dims.0 as usize * dims.1 as usize * 4;
            if out.len() >= size { out[..size].fill(0x5a); if id == 2 { d.second_pending = false; } else { d.pending = false; } }
            Ok(dims)
        })
    }
}
pub mod gpu {
    pub mod abi {
        pub mod format { pub const B8G8R8A8: u32 = 1; }
        pub mod upload_source { pub const B8G8R8A8: u32 = 1; }
        pub mod purpose { pub const IMAGE: u32 = 1; }
        pub mod status { pub const ADMITTED: u32 = 1; }
        pub mod record { pub const SIZE: usize = 64; pub const FLAGS: usize = 24; pub const TARGET_WIDTH: usize = 48; pub const TARGET_HEIGHT: usize = 52; pub const RESOURCE: usize = 80; }
        #[derive(Clone, Copy)]
        pub struct Mode { pub width: u32, pub height: u32, pub scale: u32 }
        pub struct Modes { pub count: u32, pub current: u32, pub binding_known: u32, pub modes: [Mode; 2], pub scanout_resource: u64 }
    }
    pub struct Caps { pub status: u32, pub reason: [u8; 8] }
    pub struct Answer { pub reason: u32 }
    pub fn caps() -> (i64, Caps) { (0, Caps { status: 1, reason: [0; 8] }) }
    pub fn modes() -> (i64, abi::Modes) {
        let mode = abi::Mode { width: 8, height: 8, scale: 1 };
        (0, abi::Modes { count: 2, current: 0, binding_known: 1, modes: [mode; 2], scanout_resource: 0 })
    }
    pub fn scanout_target_create(_: u32, _: u32, _: u32) -> Result<u64, i64> { Ok(2) }
    pub fn texture_create(_: u32, _: u32, _: u32) -> Result<u64, i64> {
        super::DEVICE.with(|d| if d.borrow().fail_texture { Err(-5) } else { Ok(3) })
    }
    pub fn resource_destroy(_: u64) -> Result<(), i64> { Ok(()) }
    pub fn upload(_: u64, _: u64, _: [u32; 4], bytes: &[u8], _: u32, _: u32, _: u32) -> (i64, Answer) {
        let r = super::DEVICE.with(|d| { let mut d = d.borrow_mut(); if d.fail_upload { -5 } else { d.uploads.push(bytes.to_vec()); 0 } });
        (r, Answer { reason: 0 })
    }
    pub fn readback_why(_: u64, _: [u32; 4], _: &mut [u8], _: u32, _: &mut [u8; 64]) -> (i64, Answer) { (-5, Answer { reason: 0 }) }
}
fn open(listed: (u32, u32), frame: (u32, u32)) -> compositor::Compositor {
    DEVICE.with(|d| *d.borrow_mut() = Device { listed, frame, pending: true, ..Device::default() });
    compositor::Compositor::open(8, 8, 1, 1).unwrap()
}
fn layers() -> [compositor::Layer; 1] {
    [compositor::Layer { window_id: 1, frame: [[0; 4]; 3], frame_len: 0, content: [0, 0, 8, 8] }]
}
#[test]
fn resize_between_list_and_read_keeps_the_static_frame() {
    for (listed, actual) in [((2, 2), (4, 4)), ((4, 4), (2, 2)), ((2, 4), (4, 2))] {
        let mut c = open(listed, actual);
        assert!(c.refresh_windows().unwrap());
        assert_eq!(c.shown_clients(&layers(), None).first().map(|(id, _)| *id), Some(1));
        assert!(!c.refresh_windows().unwrap());
        DEVICE.with(|d| { let d = d.borrow(); assert!(!d.pending); assert_eq!(d.uploads, vec![vec![0x5a; (actual.0 * actual.1 * 4) as usize]]); });
    }
}
#[test]
fn upload_failure_retains_consumed_snapshot_and_reopen_reports_failure() {
    let mut c = open((2, 2), (2, 2));
    DEVICE.with(|d| d.borrow_mut().fail_upload = true);
    assert!(c.refresh_windows().is_err());
    DEVICE.with(|d| assert!(!d.borrow().pending));
    assert!(c.reopen().is_err());
    DEVICE.with(|d| d.borrow_mut().fail_upload = false);
    c.reopen().unwrap();
    assert_eq!(c.shown_clients(&layers(), None).first().map(|(id, _)| *id), Some(1));
    assert!(!c.refresh_windows().unwrap());
    DEVICE.with(|d| assert_eq!(d.borrow().uploads, vec![vec![0x5a; 16]]));
}
#[test]
fn texture_creation_failure_retains_snapshot_for_retry() {
    let mut c = open((2, 2), (2, 2));
    DEVICE.with(|d| d.borrow_mut().fail_texture = true);
    assert!(c.refresh_windows().is_err());
    assert!(c.reopen().is_err());
    DEVICE.with(|d| d.borrow_mut().fail_texture = false);
    c.reopen().unwrap();
    assert_eq!(c.shown_clients(&layers(), None).first().map(|(id, _)| *id), Some(1));
}

#[test]
fn covered_clients_wait_until_their_pixels_are_exposed() {
    let mut c = open((4, 4), (4, 4));
    DEVICE.with(|d| d.borrow_mut().second_pending = true);
    c.refresh_windows().unwrap();
    let lower = compositor::Layer { window_id: 1, frame: [[0; 4]; 3], frame_len: 0, content: [0, 0, 4, 4] };
    let mut upper = compositor::Layer { window_id: 2, frame: [[0; 4]; 3], frame_len: 0, content: [0, 0, 4, 4] };
    let ids = |layers: &[compositor::Layer]| c.shown_clients(layers, None).into_iter().map(|(id, _)| id).collect::<Vec<_>>();
    assert_eq!(ids(&[lower, upper]), vec![2]);
    // Moving the covering client uncovers the lower client without a new frame.
    let lower = compositor::Layer { window_id: 1, frame: [[0; 4]; 3], frame_len: 0, content: [0, 0, 4, 4] };
    upper = compositor::Layer { window_id: 2, frame: [[0; 4]; 3], frame_len: 0, content: [4, 0, 4, 4] };
    assert_eq!(ids(&[lower, upper]), vec![2, 1]);
    // Decorations alone also occlude client content.
    let lower = compositor::Layer { window_id: 1, frame: [[0; 4]; 3], frame_len: 0, content: [0, 0, 4, 4] };
    let upper = compositor::Layer { window_id: 2, frame: [[0, 0, 2, 4], [2, 0, 4, 4], [0; 4]], frame_len: 2, content: [4, 0, 4, 4] };
    assert_eq!(ids(&[lower, upper]), vec![2]);
    let hidden = compositor::Layer { window_id: 1, frame: [[0; 4]; 3], frame_len: 0, content: [0, 7, 4, 1] };
    assert!(ids(&[hidden]).is_empty());
}
