//! Independent interpretation of captured serial producer lines for the stage-record oracle.
pub fn reached(serial: &str, markers: &[&str]) -> Vec<bool> {
    markers
        .iter()
        .map(|marker| {
            serial.lines().any(|line| {
                // Loader announcements describe a future producer rather than a completed stage.
                let lower = line.to_ascii_lowercase();
                if lower.contains("test_exec")
                    || lower.contains("will emit")
                    || lower.contains("marker if")
                {
                    return false;
                }
                marker
                    .split('|')
                    .filter(|m| !m.is_empty())
                    .any(|m| {
                        line.match_indices("->").all(|(arrow, _)| {
                            line.match_indices(m).any(|(at, _)| arrow >= at && arrow + 2 <= at + m.len())
                        }) && alone(line, m, markers)
                    })
            })
        })
        .collect()
}

/// Whether `alt` occurs in `line` outside occurrences of longer markers that contain
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
