//! Newline-aware console units, bounded in input bytes.
pub fn units(mut bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    core::iter::from_fn(move || {
        if bytes.is_empty() {
            return None;
        }
        let limit = bytes.len().min(4096);
        let end = bytes[..limit]
            .iter()
            .rposition(|&b| b == b'\n')
            .map_or(limit, |i| i + 1);
        let (unit, rest) = bytes.split_at(end);
        bytes = rest;
        Some(unit)
    })
}
