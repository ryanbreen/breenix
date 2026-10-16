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
                    .any(|m| line.contains(m))
            })
        })
        .collect()
}
