//! Fallible copies for namespace mutation plans, before disk writes.
use alloc::vec::Vec;
pub(super) fn copy_image(data: &[u8]) -> Result<Vec<u8>, &'static str> {
    let mut image = Vec::new();
    image
        .try_reserve_exact(data.len())
        .map_err(|_| "No memory for namespace image")?;
    image.extend_from_slice(data);
    Ok(image)
}
