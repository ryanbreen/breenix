use std::path::Path;

/// Read a complete process census; a disappearing or malformed status invalidates it.
pub(super) fn read(proc: &Path) -> Option<(usize, usize, String)> {
    let mut processes = Vec::new();
    for entry in std::fs::read_dir(proc).ok()? {
        let entry = entry.ok()?;
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
        let status = std::fs::read_to_string(entry.path().join("status")).ok()?;
        let name = status.lines().find_map(|line| line.strip_prefix("Name:"))?.trim();
        if name.is_empty() || name.chars().any(|c| c.is_whitespace() || c == ':' || c == ',') { return None; }
        let kb = status.lines().find_map(|line| {
            let mut fields = line.strip_prefix("VmPTE:")?.split_whitespace();
            let value = fields.next()?.parse::<usize>().ok()?;
            (fields.next() == Some("kB") && fields.next().is_none()).then_some(value)
        })?;
        processes.push((pid, name.to_string(), kb));
    }
    if processes.is_empty() { return None; }
    let total = processes.iter().try_fold(0usize, |sum, entry| sum.checked_add(entry.2))?;
    processes.sort_unstable_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
    let top = processes.iter().take(3).map(|(pid, name, kb)| format!("{pid}:{name}:{kb}"))
        .collect::<Vec<_>>().join(",");
    Some((processes.len(), total, top))
}
