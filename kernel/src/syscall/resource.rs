//! Resource enforcement helpers outside syscall entry and filesystem locks.
pub fn write_length(offset: u64, length: usize, limit: u64) -> Result<usize, u64> {
    if length != 0 && offset >= limit {
        return Err(super::errno::EFBIG as u64);
    }
    Ok((length as u64).min(limit.saturating_sub(offset)) as usize)
}

pub fn signal_fsize() {
    let Some(thread) = super::memory_common::get_current_thread_id() else {
        return;
    };
    let mut guard = crate::process::manager();
    if let Some((_, process)) = guard
        .as_mut()
        .and_then(|m| m.find_process_by_thread_mut(thread))
    {
        process
            .signals
            .set_pending(crate::signal::constants::SIGXFSZ);
    }
}

/// Snapshot before taking a filesystem guard (PM must never nest inside it).
pub fn current_fsize() -> u64 {
    let Some(thread) = super::memory_common::get_current_thread_id() else {
        return u64::MAX;
    };
    let guard = crate::process::manager();
    guard
        .as_ref()
        .and_then(|m| m.find_process_by_thread(thread))
        .map_or(u64::MAX, |(_, p)| {
            p.limits.get(crate::process::limits::FSIZE).soft
        })
}
