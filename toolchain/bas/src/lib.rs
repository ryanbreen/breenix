#![forbid(unsafe_code)]

//! Breenix's assembler for x86-64 and AArch64 ELF64 relocatable objects.

mod aarch64;
mod nasm;
mod object;
mod parser;
mod x86;

use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Location {
    line: usize,
    column: usize,
}

impl Location {
    fn error(self, message: &'static str) -> Diagnostic {
        Diagnostic { at: self, message }
    }
}

/// An assembly error with a one-based line and byte column.
#[derive(Debug, Eq, PartialEq)]
pub struct Diagnostic {
    at: Location,
    message: &'static str,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(out, "{}:{}: {}", self.at.line, self.at.column, self.message)
    }
}

impl std::error::Error for Diagnostic {}

type Result<T> = std::result::Result<T, Diagnostic>;

/// Architecture of the output object, independent of the host architecture.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Target {
    X86_64,
    Aarch64,
}

/// Assemble x86-64 source or return its first diagnostic.
pub fn assemble(source: &[u8]) -> Result<Vec<u8>> {
    assemble_for(Target::X86_64, source)
}

/// Assemble for the selected target or return the first diagnostic.
pub fn assemble_for(target: Target, source: &[u8]) -> Result<Vec<u8>> {
    let mut object = object::Object::new(target);
    let lines = || source.split(|&byte| byte == b'\n');
    let mut nasm = (target == Target::X86_64 && lines().any(nasm::is_marker))
        .then(nasm::Preprocessor::new);
    for (index, line) in lines().enumerate() {
        let line_number = index + 1;
        if let Some(index) = line.iter().position(|&byte| byte == 0) {
            return Err(Location {
                line: line_number,
                column: index + 1,
            }
            .error("NUL in source"));
        }
        if let Some(preprocessor) = &mut nasm {
            preprocessor.line(line, line_number, &mut object)?;
        } else {
            parser::Parser::new(line, line_number, &mut object).assemble_line()?;
        }
    }
    if let Some(preprocessor) = nasm {
        preprocessor.finish()?;
    }
    object.finish()
}
