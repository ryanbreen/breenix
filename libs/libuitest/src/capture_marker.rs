//! Generation witness in the first eight BGRX pixels of each row.
//! RGB carries a version tag, LE ordinal, LE producer tid and FNV-1a checksum;
//! X is opaque, so CAPTURE's X normalization preserves the witness.
pub const MARKER_BYTES: usize = 32;
pub const BACKGROUND: [u8; 4] = [0x30, 0x20, 0x18, 255];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Generation { pub commit: u64, pub producer: u32 }
fn checksum(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0x811c9dc5u32, |hash, &b| (hash ^ u32::from(b)).wrapping_mul(0x01000193))
}
pub fn encode(generation: Generation) -> [u8; MARKER_BYTES] {
    let mut payload = [0u8; 24];
    payload[..8].copy_from_slice(b"UTGEN001");
    payload[8..16].copy_from_slice(&generation.commit.to_le_bytes());
    payload[16..20].copy_from_slice(&generation.producer.to_le_bytes());
    let sum = checksum(&payload[..20]);
    payload[20..24].copy_from_slice(&sum.to_le_bytes());
    let mut pixels = [255; MARKER_BYTES];
    for (pixel, rgb) in pixels.chunks_exact_mut(4).zip(payload.chunks_exact(3)) {
        pixel[..3].copy_from_slice(rgb);
    }
    pixels
}
pub fn decode(marker: &[u8]) -> Option<Generation> {
    if marker.len() != MARKER_BYTES { return None; }
    let mut payload = [0; 24];
    for (pixel, rgb) in marker.chunks_exact(4).zip(payload.chunks_exact_mut(3)) {
        if pixel[3] != 255 { return None; }
        rgb.copy_from_slice(&pixel[..3]);
    }
    if &payload[..8] != b"UTGEN001" || checksum(&payload[..20]) != u32::from_le_bytes(payload[20..24].try_into().ok()?) { return None; }
    Some(Generation { commit: u64::from_le_bytes(payload[8..16].try_into().ok()?), producer: u32::from_le_bytes(payload[16..20].try_into().ok()?) })
}
/// Both witnesses are independently decoded; the rows must carry the same pair.
pub fn whole(pixels: &[u8], stride: usize, height: usize, tree: &[u8], commit: u64) -> bool {
    let Some(generation) = decode(tree) else { return false };
    generation.commit == commit && height != 0 && stride >= MARKER_BYTES && stride % 4 == 0
        && stride.checked_mul(height) == Some(pixels.len())
        && pixels.chunks_exact(stride).all(|row| decode(&row[..MARKER_BYTES]) == Some(generation))
}
/// Independently reconstruct the app's complete committed fill, including markers.
pub fn frame(width: usize, height: usize, generation: Generation) -> Result<Vec<u8>, String> {
    if width < MARKER_BYTES/4 || height == 0 { return Err("marker frame too small".into()); }
    let len = width.checked_mul(height).and_then(|n| n.checked_mul(4)).filter(|&n| n <= 2560*1600*4).ok_or("marker frame too large")?;
    let mut pixels = Vec::with_capacity(len);
    for _ in 0..len/4 { pixels.extend_from_slice(&BACKGROUND); }
    let marker = encode(generation);
    for row in pixels.chunks_exact_mut(width*4) { row[..MARKER_BYTES].copy_from_slice(&marker); }
    Ok(pixels)
}
