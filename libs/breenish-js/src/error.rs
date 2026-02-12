//! JavaScript error types for the breenish-js engine.

use alloc::string::String;
use core::fmt;

/// A JavaScript error with a message and optional source location.
#[derive(Debug, Clone)]
pub struct JsError {
    pub kind: ErrorKind,
    pub message: String,
    pub line: u32,
    pub column: u32,
}

/// The kind of error that occurred.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// Syntax error during parsing/compilation.
    SyntaxError,
    /// Type error at runtime.
    TypeError,
    /// Reference error (undefined variable).
    ReferenceError,
    /// Range error (out of bounds, etc).
    RangeError,
    /// Runtime error (uncaught throw).
    RuntimeError,
    /// Internal engine error (should not happen).
    InternalError,
}

impl JsError {
    pub fn syntax(message: impl Into<String>, line: u32, column: u32) -> Self {
        Self {
            kind: ErrorKind::SyntaxError,
            message: message.into(),
            line,
            column,
        }
    }

    pub fn type_error(message: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::TypeError,
            message: message.into(),
            line: 0,
            column: 0,
        }
    }

    pub fn reference_error(message: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::ReferenceError,
            message: message.into(),
            line: 0,
            column: 0,
        }
    }

    pub fn range_error(message: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::RangeError,
            message: message.into(),
            line: 0,
            column: 0,
        }
    }

    pub fn runtime(message: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::RuntimeError,
            message: message.into(),
            line: 0,
            column: 0,
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::InternalError,
            message: message.into(),
            line: 0,
            column: 0,
        }
    }
}

impl fmt::Display for JsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.kind {
            ErrorKind::SyntaxError => "SyntaxError",
            ErrorKind::TypeError => "TypeError",
            ErrorKind::ReferenceError => "ReferenceError",
            ErrorKind::RangeError => "RangeError",
            ErrorKind::RuntimeError => "Error",
            ErrorKind::InternalError => "InternalError",
        };
        if self.line > 0 {
            write!(f, "{}: {} ({}:{})", kind, self.message, self.line, self.column)
        } else {
            write!(f, "{}: {}", kind, self.message)
        }
    }
}

pub type JsResult<T> = Result<T, JsError>;
