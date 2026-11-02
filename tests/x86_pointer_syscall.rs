//! Host exercise of the real syscall and pointer state on x86.
#![cfg(target_arch = "x86_64")]
extern crate self as breengel_abi;
pub mod tc_point { pub const MOVE_POINTER: u32 = 17; }
#[path = "../kernel/src/drivers/hid_boot/pointer.rs"]
pub mod hid_pointer;
mod drivers { pub mod hid_boot { pub use crate::hid_pointer as pointer; } }
mod graphics {
    pub mod arm64_fb {
        pub struct Info { pub width: usize, pub height: usize }
        pub static FB_INFO_CACHE: std::sync::OnceLock<Info> = std::sync::OnceLock::new();
    }
}
#[path = "../kernel/src/input/pointer.rs"]
pub mod pointer_state;
mod input {
    pub use crate::pointer_state as pointer;
    pub fn poll_modifier_state() -> u8 { 5 }
    pub fn take_super_tap_count() -> u64 { 2 }
}
mod test_control {
    use std::sync::atomic::{AtomicU64, Ordering};
    static MOVE: AtomicU64 = AtomicU64::new(0);
    pub fn arm(value: u64) { MOVE.store(value, Ordering::SeqCst); }
    pub fn take(point: u32, _: u64) -> Option<u64> {
        assert_eq!(point, 17);
        match MOVE.swap(0, Ordering::SeqCst) { 0 => None, value => Some(value) }
    }
}
mod window { pub mod mem {
    pub fn probe_writable(ptr: u64, _: u64) -> Result<(), ()> { if ptr == 0 { Err(()) } else { Ok(()) } }
} }
#[path = "x86_pointer_syscall_support/mod.rs"]
mod syscall;

#[test]
fn x86_pointer_and_key_state_contract() {
    use syscall::{pointer::sys_get_mouse_pos as get, SyscallResult::*};
    input::pointer::set_screen(100, 80);
    let mut pointer = [0u32; 4];
    let mut keys = [0u32; 2];
    assert_eq!(get(0, 7), Err(22));
    assert_eq!(get(0, 8), Err(14));
    assert_eq!(get(0, 16), Err(14));
    test_control::arm((23u64 << 32) | 42);
    assert_eq!(get(0, 16), Err(14));
    assert_eq!(get(pointer.as_mut_ptr() as u64, 16), Ok(0));
    assert_eq!(pointer, [42, 23, 0, 0]);
    assert_eq!(input::pointer::mouse_state(), (42, 23, 0));
    assert_eq!(get(keys.as_mut_ptr() as u64, 8), Ok(0));
    assert_eq!(keys, [5, 2]);
}
