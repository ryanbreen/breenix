//! Current per-boot target metadata, consumed before RAM block-device registration.

static BOOT_INTENT: spin::Once<([u8; 256], usize)> = spin::Once::new();

/// Retain the current-format payload before root discovery. Parsing is deferred
/// until PID 1 selection, after allocator initialization.
pub fn capture(bytes: &[u8]) -> bool {
    let Some(line) = bytes.strip_prefix(b"BREENIX-BOOT 1\n") else { return false; };
    if line.is_empty() || line.len() > 256 { return false; }
    let mut saved = [0; 256];
    saved[..line.len()].copy_from_slice(line);
    BOOT_INTENT.call_once(|| (saved, line.len()));
    true
}

/// The selected target line, without the format header.
pub fn get() -> Option<&'static [u8]> {
    BOOT_INTENT.get().map(|(bytes, len)| &bytes[..*len])
}
