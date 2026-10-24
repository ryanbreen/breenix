//! E18 hit testing: content geometry and scene version under one registry lock.
use super::{Caller, REGISTRY};
use breengel_abi::{
    errno::{EINVAL, EPERM},
    geometry_flags, HitTest,
};
pub fn hit_test(caller: &Caller, query: &mut HitTest) -> Result<u64, i32> {
    if query.flags != 0 || query.reserved != 0 {
        return Err(EINVAL);
    }
    let reg = REGISTRY.lock();
    if !caller.is_root() && !reg.is_compositor(caller) {
        return Err(EPERM);
    }
    query.scene_version = reg.present.changes;
    query.id = reg
        .windows
        .values()
        .filter(|w| {
            w.registered
                && w.geometry & geometry_flags::VISIBLE != 0
                && w.geometry & geometry_flags::MINIMIZED == 0
                && query.x as i64 >= w.x as i64
                && (query.x as i64) < w.x as i64 + w.width as i64
                && query.y as i64 >= w.y as i64
                && (query.y as i64) < w.y as i64 + w.height as i64
        })
        .max_by_key(|w| (w.z, w.id))
        .map_or(0, |w| w.id);
    Ok(0)
}
