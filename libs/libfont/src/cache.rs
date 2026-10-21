//! GPU glyph cache budgets and accounting values.
/// The contract's default budgets.
pub const DEFAULT_ENTRIES: usize = 256;
pub const DEFAULT_BYTES: usize = 4 << 20;

/// What the cache holds and how it has been used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CacheStats {
    pub entries: usize,
    /// Bytes of bitmap coverage held.
    pub bytes: usize,
    pub hits: u64,
    pub misses: u64,
}

