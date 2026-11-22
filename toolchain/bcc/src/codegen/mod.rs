//! Assembly output for the typed C translation unit.

use std::path::Path;

use crate::pp::{Diagnostic, Location, Position, Target};
use crate::sema::TranslationUnit;

mod x86_64;

pub fn emit_assembly(
    unit: &TranslationUnit<'_>,
    source: &Path,
    target: Target,
) -> Result<String, Diagnostic> {
    match target {
        Target::X86_64 => x86_64::emit(unit),
        Target::Aarch64 => Err(Diagnostic::new(
            &Position::new(source, Location { line: 1, column: 1 }),
            "assembly generation for aarch64 is not implemented",
        )),
    }
}
