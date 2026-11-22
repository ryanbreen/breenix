/// A physical source position with a 1-based line and byte column.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Location {
    pub line: usize,
    pub column: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Header,
    Identifier,
    Number,
    Character,
    String,
    Punctuator,
    Other,
    Newline,
}

/// A preprocessing token with a half-open physical range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Token<'a> {
    pub spelling: &'a str,
    pub kind: Kind,
    pub space_before: bool,
    pub begin: Location,
    pub end: Location,
}

#[derive(Debug, Eq, PartialEq)]
pub struct Error {
    pub location: Location,
    pub message: &'static str,
}

#[derive(Debug)]
pub struct Source;

impl Source {
    pub fn tokens(&self) -> impl ExactSizeIterator<Item = Token<'_>> {
        std::iter::empty()
    }
}

pub fn scan(_bytes: &[u8]) -> Result<Source, Error> {
    Ok(Source)
}
