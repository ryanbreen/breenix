//! Serial marker recognition; called with IRQs enabled by the ribbon consumer.

/// Whether `bytes`, one serial line, is a producer line for `marker`: one of its `|`
/// alternatives appears in it, the line is not one of the loader's announcements, and the
/// alternative does not appear only inside a longer catalogue marker that contains it, so
/// a longer exec-stack marker does not light its shorter argument-check suffix. This is the
/// canonical boot stage scorer's rule (xtask's `marker_alternative_present`).
pub fn matches(bytes: &[u8], marker: &str, all: &[&str]) -> bool {
    if [b"test_exec".as_slice(), b"will emit", b"->", b"marker if"]
        .iter()
        .any(|p| bytes.windows(p.len()).any(|w| w.eq_ignore_ascii_case(p)))
    {
        return false;
    }
    marker
        .split('|')
        .filter(|m| !m.is_empty())
        .any(|m| stands_alone(bytes, m.as_bytes(), all))
}

/// Whether `alt` occurs in `line` at least once outside every occurrence of a longer marker
/// alternative of `all` that contains it.
fn stands_alone(line: &[u8], alt: &[u8], all: &[&str]) -> bool {
    occurrences(line, alt).any(|at| {
        !all.iter()
            .flat_map(|m| m.split('|'))
            .map(str::as_bytes)
            .filter(|longer| longer.len() > alt.len())
            .any(|longer| {
                occurrences(longer, alt).any(|k| {
                    at >= k && line.get(at - k..at - k + longer.len()) == Some(longer)
                })
            })
    })
}

/// The offsets at which `needle` occurs in `hay`.
fn occurrences<'a>(hay: &'a [u8], needle: &'a [u8]) -> impl Iterator<Item = usize> + 'a {
    let last = hay.len().checked_sub(needle.len());
    (0..last.map_or(0, |l| l + 1)).filter(move |&i| &hay[i..i + needle.len()] == needle)
}
