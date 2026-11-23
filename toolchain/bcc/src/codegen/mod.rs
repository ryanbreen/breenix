//! Assembly output for the typed C translation unit.

use std::path::Path;

use crate::pp::{Diagnostic, Target};
use crate::sema::TranslationUnit;

mod aarch64;
mod x86_64;

pub fn emit_assembly(
    unit: &TranslationUnit<'_>,
    _source: &Path,
    target: Target,
) -> Result<String, Diagnostic> {
    match target {
        Target::X86_64 => x86_64::emit(unit),
        Target::Aarch64 => {
            let assembly = aarch64::emit(unit)?;
            unit.check_target_types()?;
            Ok(assembly)
        }
    }
}
