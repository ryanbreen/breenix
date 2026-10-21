//! The glyph cache (docs/gui/breengel.md section 8.4): rasterized glyphs keyed by
//! `(glyph, size26)`, least recently used evicted first until both budgets hold.

use alloc::vec::Vec;

use crate::rasterizer::GlyphBitmap;
use crate::work;

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

struct Entry {
    glyph: u16,
    size26: i32,
    bitmap: GlyphBitmap,
    used: u64,
}

pub struct GlyphCache {
    entries: Vec<Entry>,
    max_entries: usize,
    max_bytes: usize,
    clock: u64,
    stats: CacheStats,
}

impl GlyphCache {
    pub fn new(max_entries: usize, max_bytes: usize) -> GlyphCache {
        GlyphCache { entries: Vec::new(), max_entries: max_entries.max(1), max_bytes, clock: 0, stats: CacheStats::default() }
    }

    fn find(&self, glyph: u16, size26: i32) -> Option<usize> {
        work::add(self.entries.len() as u64);
        self.entries.iter().position(|e| e.glyph == glyph && e.size26 == size26)
    }

    /// The cached bitmap, counting a hit or a miss.
    pub fn get(&mut self, glyph: u16, size26: i32) -> Option<&GlyphBitmap> {
        self.clock += 1;
        match self.find(glyph, size26) {
            Some(i) => {
                self.stats.hits += 1;
                self.entries[i].used = self.clock;
                Some(&self.entries[i].bitmap)
            }
            None => {
                self.stats.misses += 1;
                None
            }
        }
    }

    /// The cached bitmap without counting a use.
    pub fn get_quiet(&self, glyph: u16, size26: i32) -> Option<&GlyphBitmap> {
        self.find(glyph, size26).map(|i| &self.entries[i].bitmap)
    }

    /// Store a bitmap, evicting the least recently used entries until both budgets hold.
    pub fn insert(&mut self, glyph: u16, size26: i32, bitmap: GlyphBitmap) -> &GlyphBitmap {
        if let Some(i) = self.find(glyph, size26) {
            self.stats.bytes -= self.entries[i].bitmap.coverage.len();
            self.entries.swap_remove(i);
            self.stats.entries -= 1;
        }
        let bytes = bitmap.coverage.len();
        while !self.entries.is_empty() && (self.entries.len() + 1 > self.max_entries || self.stats.bytes + bytes > self.max_bytes) {
            work::add(self.entries.len() as u64);
            let oldest = (0..self.entries.len()).min_by_key(|&i| self.entries[i].used).unwrap_or(0);
            let e = self.entries.swap_remove(oldest);
            self.stats.bytes -= e.bitmap.coverage.len();
            self.stats.entries -= 1;
        }
        self.clock += 1;
        self.entries.push(Entry { glyph, size26, bitmap, used: self.clock });
        self.stats.bytes += bytes;
        self.stats.entries += 1;
        &self.entries[self.entries.len() - 1].bitmap
    }

    /// Drop every entry; the hit and miss counts are kept.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.stats.entries = 0;
        self.stats.bytes = 0;
    }

    pub fn stats(&self) -> CacheStats {
        self.stats
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
