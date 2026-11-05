//! Shared window targeting policy.
use breengel_abi::{errno::EINVAL, geometry_flags};
use core::borrow::Borrow;

#[derive(Clone, Copy)]
pub struct Target {
    pub id: u64,
    pub x: i32,
    pub y: i32,
    pub z: u32,
    pub width: u32,
    pub height: u32,
    pub flags: u32,
    pub registered: bool,
}

pub fn select<T: Borrow<Target>>(windows: impl IntoIterator<Item = T>, x: i32, y: i32, flags: u32) -> Result<u64, i32> {
    if flags != 0 { return Err(EINVAL); }
    Ok(windows.into_iter().filter_map(|item| {
        let w = *item.borrow();
        (w.registered && w.flags & geometry_flags::VISIBLE != 0
            && w.flags & geometry_flags::MINIMIZED == 0
            && i64::from(x) >= i64::from(w.x) && i64::from(x) < i64::from(w.x) + i64::from(w.width)
            && i64::from(y) >= i64::from(w.y) && i64::from(y) < i64::from(w.y) + i64::from(w.height))
            .then_some((w.z, w.id))
    }).max().map_or(0, |(_, id)| id))
}
