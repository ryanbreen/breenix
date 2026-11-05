#[path = "../kernel/src/window/policy.rs"]
mod policy;

use breengel_abi::{errno::EINVAL, geometry_flags as g, HitTest};
use policy::{select, Target};

fn window(id: u64, z: u32, flags: u32) -> Target {
    Target { id, z, x: 10, y: 20, width: 30, height: 40, flags, registered: true }
}

#[test]
fn selects_topmost_visible_registered_window() {
    let mut windows = [window(1, 1, g::VISIBLE), window(2, 2, g::VISIBLE), window(3, 9, g::VISIBLE | g::MINIMIZED), window(4, 10, 0)];
    assert_eq!(select(&windows, 10, 20, 0), Ok(2));
    windows[1].registered = false;
    assert_eq!(select(&windows, 10, 20, 0), Ok(1));
    windows[0].z = 9;
    windows[1] = window(5, 9, g::VISIBLE);
    assert_eq!(select(&windows, 10, 20, 0), Ok(5));
}

#[test]
fn edges_are_exclusive_and_flags_refused() {
    let windows = [window(1, 0, g::VISIBLE)];
    assert_eq!(select(&windows, 39, 59, 0), Ok(1));
    for (x, y) in [(9, 20), (10, 19), (40, 20), (10, 60)] {
        assert_eq!(select(&windows, x, y, 0), Ok(0));
    }
    assert_eq!(select(&windows, 10, 20, 1), Err(EINVAL));
}

#[test]
fn hit_test_abi_layout() {
    use core::mem::{offset_of, size_of};
    assert_eq!(breengel_abi::op::HIT_TEST, 32);
    assert_eq!(breengel_abi::op::LAST, 32);
    assert_eq!(breengel_abi::arg_size(32), Some(32));
    assert_eq!(size_of::<HitTest>(), 32);
    assert_eq!([offset_of!(HitTest, x), offset_of!(HitTest, y), offset_of!(HitTest, id), offset_of!(HitTest, scene_version), offset_of!(HitTest, flags), offset_of!(HitTest, reserved)], [0, 4, 8, 16, 24, 28]);
}
