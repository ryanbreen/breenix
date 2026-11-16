//! Flush-boundary crash replay (breenfs design section 5).
use crate::recording::SECTOR_SIZE;
use crate::{Error, Result};
use alloc::{collections::BTreeMap, vec::Vec};

/// Sparse bytes of a plain in-memory sector device; absent sectors are zero.
#[derive(Clone, Debug)]
pub struct Image {
    pub sectors: BTreeMap<u64, [u8; SECTOR_SIZE]>,
    pub sector_count: u64,
}
#[derive(Debug)]
pub struct Boundary {
    pub writes: usize,
    pub images: usize,
    pub acknowledged: u64,
    pub issued: u64,
}
#[derive(Debug)]
pub struct Report {
    pub commits: usize,
    pub boundaries: Vec<Boundary>,
}

/// Replay the bounded seeded stream and validate every generated crash image.
pub fn subset_images(
    _initial: Vec<u8>,
    _seed: u64,
    _operations: usize,
    _checker: impl FnMut(&Image) -> Result<()>,
) -> Result<Report> {
    Err(Error::Unimplemented)
}

/// Ordered write selections, including the empty and complete image.
pub fn subsets(_writes: usize, _seed: u64) -> Result<Vec<Vec<bool>>> {
    Err(Error::Unimplemented)
}
