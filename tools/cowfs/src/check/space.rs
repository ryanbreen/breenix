//! Space accounting, invariants A1-A6 (format section 3), over the set L the walk built.
//!
//! Not yet implemented: this unit reports no A findings. The next checker unit fills this in.

use super::walk::Walk;

impl Walk<'_> {
    /// A1-A6 findings as (invariant, detail). Empty until the space checks land.
    pub fn space_findings(&self) -> Vec<(&'static str, String)> {
        Vec::new()
    }
}
