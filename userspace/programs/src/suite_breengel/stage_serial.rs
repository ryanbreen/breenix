//! Independent interpretation of captured serial producer lines for the ribbon oracle.
pub fn reached(serial: &str, markers: &[&str]) -> Vec<bool> {
    markers
        .iter()
        .map(|marker| {
            serial.lines().any(|line| {
                // Loader announcements describe a future producer, never a completed stage.
                let lower = line.to_ascii_lowercase();
                if lower.contains("test_exec")
                    || lower.contains("will emit")
                    || line.contains("->")
                    || lower.contains("marker if")
                {
                    return false;
                }
                marker
                    .split('|')
                    .filter(|m| !m.is_empty())
                    .any(|m| alone(line, m, markers))
            })
        })
        .collect()
}

/// Whether `alt` occurs in `line` outside every occurrence of a longer marker that contains
/// it: `EXEC_STACK_ARGV_TEST_PASSED` is not `ARGV_TEST_PASSED`'s producer (section 23.7).
fn alone(line: &str, alt: &str, markers: &[&str]) -> bool {
    line.match_indices(alt).any(|(at, _)| {
        !markers
            .iter()
            .flat_map(|m| m.split('|'))
            .filter(|longer| longer.len() > alt.len())
            .any(|longer| {
                longer
                    .match_indices(alt)
                    .any(|(k, _)| at >= k && line.get(at - k..at - k + longer.len()) == Some(longer))
            })
    })
}
