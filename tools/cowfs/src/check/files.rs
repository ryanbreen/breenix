//! Filesystem-tree invariants F1-F10 (format section 3), per root.
//!
//! Not yet implemented: this unit reports no F findings. The next checker unit fills this in.

use super::walk::Walk;

impl Walk<'_> {
    /// F1-F10 findings as (invariant, detail). Empty until the filesystem-tree checks land.
    pub fn file_findings(&self) -> Vec<(&'static str, String)> {
        Vec::new()
    }
}
