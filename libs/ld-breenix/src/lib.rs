#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]

//! Slice-based ELF64 dynamic linking for the Breenix interpreter.

mod elf;
mod object;
#[cfg(feature = "interpreter")]
pub mod runtime;
mod scope;
#[path = "../../libbreenix-libc/src/tls_module.rs"]
pub mod tls_module;
#[path = "../../libbreenix-libc/src/tls_variant_i.rs"]
mod tls_variant_i;

pub use object::{Object, Patch};
pub use scope::{Dependency, InitArray, InitKind, InitState, Scope};

#[path = "../../libbreenix-libc/src/dl_abi.rs"]
pub mod dl_abi;
pub use dl_abi::LoadedObject;

use core::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Bounds,
    UnsupportedElf,
    InvalidProgramHeader,
    MissingDynamic,
    InvalidDynamic,
    UnsupportedDynamic(u64),
    InvalidString,
    InvalidHash,
    InvalidSymbol,
    UndefinedSymbol,
    UnsupportedRelocation(u32),
    InvalidRelocation,
    UnsupportedSubstitution,
    MissingDependency,
    InvalidDependency,
    Capacity,
    SystemCall(&'static str, i32),
}

/// A diagnostic at a file offset in the named object. Detail, when present,
/// is the unresolved symbol, dependency, or substitution as raw bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Error<'a> {
    pub object: &'a [u8],
    pub offset: u64,
    pub kind: ErrorKind,
    pub detail: &'a [u8],
}

impl fmt::Display for Error<'_> {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_bytes(out, self.object)?;
        write!(out, "+{:#x}: {:?}", self.offset, self.kind)?;
        if !self.detail.is_empty() {
            out.write_str(": ")?;
            write_bytes(out, self.detail)?;
        }
        Ok(())
    }
}

fn write_bytes(out: &mut fmt::Formatter<'_>, bytes: &[u8]) -> fmt::Result {
    match core::str::from_utf8(bytes) {
        Ok(text) => out.write_str(text),
        Err(_) => write!(out, "{bytes:?}"),
    }
}
