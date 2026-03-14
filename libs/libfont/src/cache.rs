//! LRU glyph bitmap cache keyed on (glyph_index, size).
//!
//! Uses a simple Vec with linear scan — avoids BTreeMap which has
//! issues on the Breenix aarch64 target.

use alloc::vec::Vec;
use crate::rasterizer::GlyphBitmap;

struct CacheEntry {
    glyph_index: u16,
    size_x100: u32,
    bitmap: GlyphBitmap,
    access_order: u64,
}

pub struct GlyphCache {
    entries: Vec<CacheEntry>,
    max_entries: usize,
    access_counter: u64,
}

impl GlyphCache {
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: Vec::with_capacity(max_entries.min(256)),
            max_entries,
            access_counter: 0,
        }
    }

    pub fn get(&mut self, glyph_index: u16, pixel_size: f32) -> Option<&GlyphBitmap> {
        let size_x100 = (pixel_size * 100.0) as u32;
        self.access_counter += 1;
        for entry in self.entries.iter_mut() {
            if entry.glyph_index == glyph_index && entry.size_x100 == size_x100 {
                entry.access_order = self.access_counter;
                return Some(&entry.bitmap);
            }
        }
        None
    }

    pub fn insert(&mut self, glyph_index: u16, pixel_size: f32, bitmap: GlyphBitmap) {
        let size_x100 = (pixel_size * 100.0) as u32;
        // Check if already present (update in place)
        for entry in self.entries.iter_mut() {
            if entry.glyph_index == glyph_index && entry.size_x100 == size_x100 {
                self.access_counter += 1;
                entry.bitmap = bitmap;
                entry.access_order = self.access_counter;
                return;
            }
        }
        // Evict LRU if at capacity
        if self.entries.len() >= self.max_entries {
            self.evict_lru();
        }
        self.access_counter += 1;
        self.entries.push(CacheEntry {
            glyph_index,
            size_x100,
            bitmap,
            access_order: self.access_counter,
        });
    }

    fn evict_lru(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        let mut oldest_idx = 0;
        let mut oldest_order = u64::MAX;
        for (i, entry) in self.entries.iter().enumerate() {
            if entry.access_order < oldest_order {
                oldest_order = entry.access_order;
                oldest_idx = i;
            }
        }
        self.entries.swap_remove(oldest_idx);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.access_counter = 0;
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}
