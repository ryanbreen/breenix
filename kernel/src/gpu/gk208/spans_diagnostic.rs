//! Compare the packed crossing image with two completed CE readbacks.
/// Refuse incomplete transfers or poison-dependent contents before judging the
/// upload; a consistent mismatch reports its first byte in packed-table order.
pub fn compare(expected: &[u8], first: &[u8], second: &[u8])
    -> Result<Option<(usize, u8, u8)>, &'static str> {
    if expected.is_empty() || first.len() != expected.len() || second.len() != expected.len() {
        return Err("incomplete native spans readback");
    }
    if first != second { return Err("native spans P/~P readbacks differ"); }
    Ok(expected.iter().zip(first).enumerate()
        .find_map(|(at, (&expected, &actual))| (expected != actual).then_some((at, expected, actual))))
}
