//! Assembly output for the typed C translation unit.

use std::collections::BTreeSet;
use std::fmt::Write;
use std::path::Path;

use crate::pp::{Diagnostic, Target};
use crate::sema::{Linkage, Object, TranslationUnit};

mod aarch64;
mod x86_64;

pub fn emit_assembly(
    unit: &TranslationUnit<'_>,
    _source: &Path,
    target: Target,
) -> Result<String, Diagnostic> {
    emit_with_options(unit, target, false)
}

pub(crate) fn emit_with_options(
    unit: &TranslationUnit<'_>,
    target: Target,
    optimize: bool,
) -> Result<String, Diagnostic> {
    match target {
        Target::X86_64 => x86_64::emit(unit, optimize),
        Target::Aarch64 => aarch64::emit(unit),
    }
}

// bas/README.md documents this section and ELF symbol directive dialect.
fn select_section(out: &mut String, name: &str, flags: &str, kind: &str) {
    out.push_str("\n.section \"");
    for byte in name.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'$') {
            out.push(char::from(byte));
        } else {
            write!(out, "\\x{byte:02x}").unwrap();
        }
    }
    writeln!(out, "\", {flags}, @{kind}").unwrap();
}

fn object_section(out: &mut String, section: &str, object: &Object<'_>, tls: bool) {
    let flags = if tls {
        "awT"
    } else if object.ty.qualifiers.constant {
        "a"
    } else {
        "aw"
    };
    let kind = if section == ".bss"
        || section.starts_with(".bss.")
        || section == ".tbss"
        || section.starts_with(".tbss.")
    {
        "nobits"
    } else {
        "progbits"
    };
    select_section(out, section, flags, kind);
}

fn emit_symbol_attributes(
    out: &mut String,
    unit: &TranslationUnit<'_>,
    externs: &BTreeSet<&str>,
    thread_externs: &BTreeSet<&str>,
) {
    if unit.symbol_attributes.is_empty() {
        return;
    }
    let definitions: BTreeSet<_> = unit
        .functions
        .iter()
        .filter(|function| function.body.is_some())
        .map(|function| function.name)
        .chain(unit.static_objects.iter().map(|object| object.object.name))
        .chain(unit.thread_objects.iter().map(|object| object.object.name))
        .collect();
    for (&name, attrs) in &unit.symbol_attributes {
        if let Some(target) = &attrs.alias {
            if unit.aliases[name] == Linkage::External {
                writeln!(out, ".global {name}").unwrap();
            }
            writeln!(out, ".alias {name}, {target}").unwrap();
        } else if !definitions.contains(name)
            && !externs.contains(name)
            && !thread_externs.contains(name)
        {
            continue;
        }
        if attrs.weak {
            writeln!(out, ".weak {name}").unwrap();
        }
        if let Some(visibility) = attrs.visibility.filter(|&value| value != "default") {
            writeln!(out, ".{visibility} {name}").unwrap();
        }
    }
}
