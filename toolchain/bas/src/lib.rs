#![forbid(unsafe_code)]

//! Breenix's Intel-style assembler. The current backend emits AMD64 ELF64
//! relocatable objects on either host architecture.

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

/// Assemble the source or return its first diagnostic.
pub fn assemble(source: &[u8]) -> Result<Vec<u8>> {
    let mut object = object::Object::new();
    for (index, line) in source.split(|&byte| byte == b'\n').enumerate() {
        let line_number = index + 1;
        if let Some(index) = line.iter().position(|&byte| byte == 0) {
            return Err(Location {
                line: line_number,
                column: index + 1,
            }
            .error("NUL in source"));
        }
        parser::Parser::new(line, line_number, &mut object).assemble_line()?;
    }
    object.finish()
}
