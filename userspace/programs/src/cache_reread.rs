//! Counter evidence for the filesystem suite's fixed 1 MiB reread.
pub const REREAD_BYTES: usize = 1024 * 1024;

/// Snapshots are (hits, misses, device reads), in the proc table's order.
pub fn reread_is_cached(before: (u64, u64, u64), after: (u64, u64, u64)) -> bool {
    after.2 == before.2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reread_requires_cache_participation() {
        let before = (10, 20, 20);
        let hits = REREAD_BYTES as u64 / 512;
        assert!(reread_is_cached(before, (10 + hits, 20, 20)));
        assert!(!reread_is_cached(before, before), "bypassing the cache passed");
        assert!(!reread_is_cached(before, (10 + hits - 1, 20, 20)));
        assert!(!reread_is_cached(before, (10 + hits, 21, 20)));
        assert!(!reread_is_cached(before, (10 + hits, 20, 21)));
    }
}
