//! String interning for the breenish-js engine.
//!
//! All strings are stored in a central pool and referenced by index.
//! This enables O(1) equality comparison and deduplication.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

/// An interned string identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StringId(pub u32);

/// A string interning pool that stores unique strings.
pub struct StringPool {
    strings: Vec<String>,
}

impl StringPool {
    pub fn new() -> Self {
        Self {
            strings: Vec::new(),
        }
    }

    /// Intern a string, returning its ID.
    /// If the string already exists, returns the existing ID.
    pub fn intern(&mut self, s: &str) -> StringId {
        // Linear search for deduplication (acceptable for Phase 1; hash map in Phase 2+)
        for (i, existing) in self.strings.iter().enumerate() {
            if existing == s {
                return StringId(i as u32);
            }
        }
        let id = StringId(self.strings.len() as u32);
        self.strings.push(String::from(s));
        id
    }

    /// Look up a string by its ID.
    pub fn get(&self, id: StringId) -> &str {
        &self.strings[id.0 as usize]
    }

    /// Get the number of interned strings.
    pub fn len(&self) -> usize {
        self.strings.len()
    }

    /// Check if the pool is empty.
    pub fn is_empty(&self) -> bool {
        self.strings.is_empty()
    }
}

impl fmt::Debug for StringPool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StringPool")
            .field("count", &self.strings.len())
            .finish()
    }
}
