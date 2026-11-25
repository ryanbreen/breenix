//! Snapshot a user vector and describe the buffers in a transfer's prefix.

use super::errno::{EFAULT, EINVAL, ENOMEM};
use alloc::vec::Vec;
use core::ops::Range;

pub(super) const UIO_MAXIOV: u64 = 1024;

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct IoVec {
    pub(super) iov_base: u64,
    pub(super) iov_len: u64,
}

pub(super) fn positioned_offset(offset: i64, count: i32) -> Result<u64, u64> {
    if offset < 0 || !(1..=UIO_MAXIOV as i32).contains(&count) {
        return Err(EINVAL as u64);
    }
    Ok(offset as u64)
}

pub(super) fn snapshot_vectors(
    ptr: u64,
    count: u64,
    mut read: impl FnMut(u64) -> Result<IoVec, u64>,
) -> Result<(Vec<IoVec>, usize), u64> {
    if count > UIO_MAXIOV {
        return Err(EINVAL as u64);
    }
    if ptr == 0 {
        return Err(EFAULT as u64);
    }
    let mut vectors = Vec::new();
    vectors
        .try_reserve_exact(count as usize)
        .map_err(|_| ENOMEM as u64)?;
    let mut length = 0;
    for index in 0..count {
        let address = ptr
            .checked_add(index * core::mem::size_of::<IoVec>() as u64)
            .ok_or(EFAULT as u64)?;
        let vector = read(address)?;
        let vector_length = isize::try_from(vector.iov_len).map_err(|_| EINVAL as u64)?;
        length = isize::checked_add(length, vector_length).ok_or(EINVAL as u64)?;
        vectors.push(vector);
    }
    Ok((vectors, length as usize))
}

/// Visit nonempty buffer spans until the requested prefix is exhausted.
pub(super) fn vector_prefix(
    vectors: &[IoVec],
    length: usize,
) -> impl Iterator<Item = (u64, Range<usize>)> + '_ {
    vectors
        .iter()
        .scan(0usize, move |cursor, vector| {
            if *cursor == length {
                return None;
            }
            let end = *cursor + (vector.iov_len as usize).min(length - *cursor);
            let span = *cursor..end;
            *cursor = end;
            Some((vector.iov_base, span))
        })
        .filter(|(_, span)| !span.is_empty())
}
