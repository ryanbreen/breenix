#![forbid(unsafe_code)]

pub mod ar;
pub mod elf;

#[derive(Debug, PartialEq, Eq)]
pub struct Error {
    pub offset: usize,
    pub message: &'static str,
}

const UNIMPLEMENTED: Error = Error {
    offset: 0,
    message: "reader or writer is not implemented",
};
