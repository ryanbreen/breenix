// claim-lint:ok: tests/breengel_gpu_only.rs parses this negative-control fixture.
// Parsed by the structural check, never installed: a renamed raw-pixel renderer must be refused.
pub fn disguise(pixels: &mut [u8]) {
    for pixel in pixels.chunks_exact_mut(4) { pixel.copy_from_slice(&[17, 34, 51, 255]); }
    bgl::upload(pixels);
}
