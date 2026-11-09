use breenix_suite::suite::{fail, CaseResult};

pub fn not_built() -> CaseResult {
    fail("not built (E23 23.1)")
}
