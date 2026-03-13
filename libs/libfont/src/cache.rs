//! LRU glyph bitmap cache keyed on (glyph_index, size).
//!
//! Uses BTreeMap because no_std+alloc doesn't provide HashMap.

use alloc::collections::BTreeMap;
use crate::rasterizer::GlyphBitmap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct CacheKey {
    glyph_index: u16,
    size_x100: u32,
}

struct CacheEntry {
    bitmap: GlyphBitmap,
    access_order: u64,
}

pub struct GlyphCache {
    entries: BTreeMap<CacheKey, CacheEntry>,
    max_entries: usize,
    access_counter: u64,
}

impl GlyphCache {
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: BTreeMap::new(),
            max_entries,
            access_counter: 0,
        }
    }

    pub fn get(&mut self, glyph_index: u16, pixel_size: f32) -> Option<&GlyphBitmap> {
        let key = make_key(glyph_index, pixel_size);
        if let Some(entry) = self.entries.get_mut(&key) {
            self.access_counter += 1;
            entry.access_order = self.access_counter;
            Some(&entry.bitmap)
        } else {
            None
        }
    }

    pub fn insert(&mut self, glyph_index: u16, pixel_size: f32, bitmap: GlyphBitmap) {
        if self.entries.len() >= self.max_entries {
            self.evict_lru();
        }
        self.access_counter += 1;
        let key = make_key(glyph_index, pixel_size);
        self.entries.insert(key, CacheEntry {
            bitmap,
            access_order: self.access_counter,
        });
    }

    fn evict_lru(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        // Find the entry with lowest access_order
        let mut oldest_key = None;
        let mut oldest_order = u64::MAX;
        for (key, entry) in &self.entries {
            if entry.access_order < oldest_order {
                oldest_order = entry.access_order;
                oldest_key = Some(*key);
            }
        }
        if let Some(key) = oldest_key {
            self.entries.remove(&key);
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.access_counter = 0;
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

fn make_key(glyph_index: u16, pixel_size: f32) -> CacheKey {
    CacheKey {
        glyph_index,
        size_x100: (pixel_size * 100.0) as u32,
    }
}
