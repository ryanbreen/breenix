//! Judge forced capture overlap using the control witness and u6's row/tree encoding.
use crate::{capture, capture_marker::{self, Generation}};

/// Publish both control deltas only after the forced copy and release completed.
pub fn record_witness(before: u64, fired: u64, release_before: u64, released: u64,
    mut emit: impl FnMut(&'static str, i64)) -> Result<(), String> {
    let delta = |old, new: u64| new.checked_sub(old).filter(|&n| n > 0)
        .and_then(|n| i64::try_from(n).ok()).ok_or_else(|| "capture control did not fire".to_string());
    let fired = delta(before, fired)?;
    let released = delta(release_before, released)?;
    emit("capture-stall-fired", fired);
    emit("capture-release-fired", released);
    Ok(())
}

/// A complete pattern alone cannot establish that the requested race happened.
pub fn judge(before: u64, fired: u64, pixels: &[u8], width: u32, height: u32,
    tree: &[u8], commit: u64, expected: Option<Generation>) -> Result<Generation, String> {
    if fired <= before { return Err("capture-stall did not fire".into()); }
    let marker = capture::root_marker(tree)?;
    if !capture_marker::whole(pixels, width as usize * 4, height as usize, &marker, commit) {
        return Err("capture mixed rows, tree or returned commit".into());
    }
    let generation = capture_marker::decode(&marker).ok_or("invalid capture root marker")?;
    if generation.producer == 0 || expected.is_some_and(|want| want != generation) {
        return Err("capture returned a different producer/ordinal pair".into());
    }
    if pixels != capture_marker::frame(width as usize, height as usize, generation)? {
        return Err("capture differs from the complete committed pattern".into());
    }
    Ok(generation)
}
