// Host GPU boundary: deterministic eight-window snapshots, no device execution.
extern crate self as libbreenix;
extern crate self as libgfx;

pub mod input { pub use input_abi::{Event, kind, syn}; }
pub mod errno { #[derive(Debug)] pub enum Errno { EINVAL } }
pub mod error {
    #[derive(Debug)] pub enum Error { Os(super::errno::Errno) }
    impl std::fmt::Display for Error {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{self:?}") }
    }
}
pub mod time {
    pub struct Time;
    impl Time { pub fn as_nanos(&self) -> u128 { 0 } }
    pub fn now_monotonic() -> Result<Time, i64> { Ok(Time) }
    pub fn sleep_ms(_: u64) -> Result<(), i64> { Ok(()) }
}
pub mod gpu {
    pub use gpu_abi as abi;
    use abi::Plain;
    pub fn caps() -> (i64, abi::Caps) { let mut c = abi::Caps::ZERO; c.status = abi::status::ADMITTED; (0, c) }
    pub fn context_create() -> Result<u64, i64> { Ok(1) }
    pub fn context_destroy(_: u64) -> i64 { 0 }
    pub fn resource_destroy(_: u64) -> i64 { 0 }
    pub fn target_create(_: u32, _: u32, _: u32) -> Result<u64, i64> { Ok(2) }
    pub fn scanout_target_create(_: u32, _: u32, _: u32) -> Result<u64, i64> { Ok(3) }
    pub fn submit(_: u64, _: &[u32]) -> (i64, abi::Submit) { (0, abi::Submit::ZERO) }
    pub fn wait(_: u64, _: u64, _: u64) -> (i64, abi::Wait) { let mut w = abi::Wait::ZERO; w.status = abi::fence_status::OK; (0, w) }
    pub fn present(_: u64, _: u64) -> (i64, abi::Present) { (0, abi::Present::ZERO) }
    pub fn modes() -> (i64, abi::Modes) { (0, abi::Modes::ZERO) }
    pub fn readback_why(_: u64, _: [u32; 4], _: &mut [u8], _: u32, _: &mut [u8; abi::record::SIZE]) -> (i64, abi::Readback) { panic!("no readback in the allocation test") }
}
pub mod window {
    pub use breengel_abi as abi;
    use abi::Plain;
    pub fn capture(_: u64, _: &mut [u8], _: Option<&mut [u8]>) -> Result<abi::Capture, super::error::Error> { panic!("no capture") }
    pub fn list(_: &mut [abi::WinInfo]) -> Result<usize, super::error::Error> { panic!("no list") }
    pub unsafe fn raw_call(op: u32, p: *mut u8, _: usize) -> i64 {
        match op {
            abi::op::COMPOSITE_SNAPSHOT => {
                let s = unsafe { &mut *p.cast::<abi::SceneSnapshot>() };
                if s.window_capacity != 0 {
                    assert!(s.window_capacity >= 8);
                    for i in 0..8 {
                        let mut w = abi::SceneWindow::ZERO;
                        w.id = i + 1; w.sample = i + 10; w.commit_id = super::EPOCH.load(std::sync::atomic::Ordering::Relaxed) as u64;
                        w.width = 32; w.height = 32;
                        unsafe { (s.windows as *mut abi::SceneWindow).add(i as usize).write(w); }
                    }
                    s.snapshot = 1;
                }
                s.width = 800; s.height = 600; s.window_count = 8; s.damage_count = 0;
                0
            }
            abi::op::COMPOSITE => {
                let c = unsafe { &mut *p.cast::<abi::Composite>() };
                c.serial = super::EPOCH.load(std::sync::atomic::Ordering::Relaxed) as u64;
                0
            }
            _ => panic!("unexpected window operation {op}"),
        }
    }
}
pub mod surface {
    pub enum Format { BGRX }
    pub struct Surface;
    impl Surface {
        pub fn new(_: usize, _: usize, _: Format) -> Result<Self, i64> { Ok(Self) }
        pub fn wait(&self) -> Result<(), i64> { Ok(()) }
        pub fn handle(&self) -> u64 { 4 }
        pub fn read_rect(&self, _: [usize; 4], _: &mut [u8], _: usize) -> Result<(), i64> { panic!("no readback") }
    }
}
