//! Serial marker recognition; called with IRQs enabled by the ribbon consumer.
pub fn matches(bytes: &[u8], marker: &str) -> bool {
    if [b"test_exec".as_slice(), b"will emit", b"->", b"marker if"]
        .iter()
        .any(|p| bytes.windows(p.len()).any(|w| w.eq_ignore_ascii_case(p)))
    {
        return false;
    }
    marker
        .split('|')
        .filter(|m| !m.is_empty())
        .any(|m| bytes.windows(m.len()).any(|w| w == m.as_bytes()))
}
