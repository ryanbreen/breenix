//! Judge forced capture overlap using the control witness and u6's row/tree encoding.
use crate::{capture, capture_marker::{self, Generation}};

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
