//! Choose an advertised display entry from monitor dimensions and scale.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    Unsupported,
}

/// An absent selection preserves the kernel default without a mode change.
pub fn select(
    modes: &[(u32, u32, u32)],
    mode: Option<(u32, u32)>,
    scale: Option<u32>,
) -> Option<Result<usize, Refusal>> {
    if mode.is_none() && scale.is_none() {
        return None;
    }
    let Some(&(width, height, default_scale)) = modes.get(1) else {
        return Some(Err(Refusal::Unsupported));
    };
    let Some(default_mode) = monitor_mode((width, height, default_scale)) else {
        return Some(Err(Refusal::Unsupported));
    };
    let wanted = mode.unwrap_or(default_mode);
    let find = |scale| modes.iter().position(|&entry| {
        entry.2 == scale && monitor_mode(entry) == Some(wanted)
    });
    let index = match scale {
        Some(scale) => find(scale),
        None => find(default_scale).or_else(|| find(1)),
    };
    Some(index.ok_or(Refusal::Unsupported))
}

fn monitor_mode((width, height, scale): (u32, u32, u32)) -> Option<(u32, u32)> {
    Some((width.checked_div(scale)?, height.checked_div(scale)?))
}
