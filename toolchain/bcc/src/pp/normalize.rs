//! N1570 5.1.1.2 phases 1 and 2, and the trigraph table in 5.2.1.1.

use super::{Error, Location};

pub(super) struct Normalized {
    pub text: String,
    // Each spelling byte maps to the start of its physical character or replacement.
    pub origins: Vec<usize>,
}

pub(super) fn normalize(input: &str) -> (Normalized, Option<Error>) {
    let mut text = String::with_capacity(input.len());
    let mut origins = Vec::with_capacity(input.len());
    // UTF-8 is the input encoding. A leading signature is not part of the source.
    let mut offset = if input.starts_with('\u{feff}') { 3 } else { 0 };
    while let Some((ch, width)) = phase_one(&input[offset..]) {
        let next = offset + width;
        if ch == '\0' {
            let error = Error {
                location: PhysicalCursor::new(input.as_bytes()).locate(offset),
                message: "null byte in source",
            };
            return (Normalized { text, origins }, Some(error));
        }
        if ch == '\\' {
            // Neither a trigraph nor any other phase-1 replacement can create
            // a new-line except normalization of the physical line ending.
            if let Some(width) = newline_width(&input.as_bytes()[next..]) {
                offset = next + width;
                continue;
            }
        }
        text.push(ch);
        origins.extend(std::iter::repeat_n(offset, ch.len_utf8()));
        offset = next;
    }
    (Normalized { text, origins }, None)
}

pub(super) fn phase_one(input: &str) -> Option<(char, usize)> {
    if let Some(width) = newline_width(input.as_bytes()) {
        return Some(('\n', width));
    }
    let replacement = match input.as_bytes() {
        [b'?', b'?', third, ..] => match third {
            b'=' => Some('#'),
            b'(' => Some('['),
            b'/' => Some('\\'),
            b')' => Some(']'),
            b'\'' => Some('^'),
            b'<' => Some('{'),
            b'!' => Some('|'),
            b'>' => Some('}'),
            b'-' => Some('~'),
            _ => None,
        },
        _ => None,
    };
    replacement
        .map(|ch| (ch, 3))
        .or_else(|| input.chars().next().map(|ch| (ch, ch.len_utf8())))
}

fn newline_width(bytes: &[u8]) -> Option<usize> {
    match bytes {
        [b'\r', b'\n', ..] => Some(2),
        [b'\r' | b'\n', ..] => Some(1),
        _ => None,
    }
}

/// Maps monotonically increasing physical offsets without rescanning the file.
pub(super) struct PhysicalCursor<'a> {
    bytes: &'a [u8],
    offset: usize,
    location: Location,
}

impl<'a> PhysicalCursor<'a> {
    pub(super) fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            offset: 0,
            location: Location { line: 1, column: 1 },
        }
    }

    pub(super) fn locate(&mut self, target: usize) -> Location {
        while self.offset < target {
            if let Some(width) = newline_width(&self.bytes[self.offset..target]) {
                self.location.line += 1;
                self.location.column = 1;
                self.offset += width;
            } else {
                self.location.column += 1;
                self.offset += 1;
            }
        }
        self.location
    }
}
