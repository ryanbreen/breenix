//! The one stderr line a fatal unwind leaves behind. It names the phase that
//! failed, the last PC the engine looked up and that lookup's outcome, so a
//! process that aborts after `_Unwind_RaiseException` returns still shows
//! which frame stopped the unwind. Formatting uses a fixed buffer.

use core::cell::Cell;
use core::fmt::{self, Write};

use crate::level1::{
    Actions, Context, Exception, Frame, FrameSource, LookupError, ReasonCode,
};

/// What the last frame lookup found.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    /// Metadata was found, so the CFA, a caller register or the personality failed.
    Found,
    /// No loaded object's index covers the PC.
    NoFrame,
    Failed(LookupError),
}

/// A frame source that remembers its most recent lookup.
pub struct Recorded<'s, S> {
    inner: &'s S,
    last: Cell<Option<(u64, Outcome)>>,
}

impl<'s, S> Recorded<'s, S> {
    pub fn new(inner: &'s S) -> Self {
        Self {
            inner,
            last: Cell::new(None),
        }
    }

    pub fn last(&self) -> Option<(u64, Outcome)> {
        self.last.get()
    }
}

impl<const N: usize, S: FrameSource<N, Error = LookupError>> FrameSource<N> for Recorded<'_, S> {
    type Error = LookupError;

    fn find_frame(&self, pc: u64) -> Result<Option<Frame<'_, N>>, LookupError> {
        let found = self.inner.find_frame(pc);
        let outcome = match &found {
            Ok(Some(_)) => Outcome::Found,
            Ok(None) => Outcome::NoFrame,
            Err(error) => Outcome::Failed(*error),
        };
        self.last.set(Some((pc, outcome)));
        found
    }

    fn stack_pointer_register(&self) -> u64 {
        self.inner.stack_pointer_register()
    }

    fn read_saved_register(&self, register: u64, address: u64) -> Option<u64> {
        self.inner.read_saved_register(register, address)
    }

    fn evaluate_expression(
        &self,
        expression: &[u8],
        context: &Context<N>,
        cfa: Option<u64>,
    ) -> Option<u64> {
        self.inner.evaluate_expression(expression, context, cfa)
    }

    fn personality(
        &self,
        address: u64,
        version: u32,
        actions: Actions,
        exception: &mut Exception<N>,
        context: &mut Context<N>,
    ) -> ReasonCode {
        self.inner
            .personality(address, version, actions, exception, context)
    }
}

/// A formatted line, truncated to its buffer.
pub struct Line {
    bytes: [u8; 256],
    len: usize,
}

impl Line {
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

impl Write for Line {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let room = self.bytes.len() - self.len;
        let count = text.len().min(room);
        self.bytes[self.len..self.len + count].copy_from_slice(&text.as_bytes()[..count]);
        self.len += count;
        Ok(())
    }
}

/// Describe a fatal `reason`. `object` is the name and load bias of the
/// object containing the last PC, when one does.
pub fn failure_line(
    reason: ReasonCode,
    last: Option<(u64, Outcome)>,
    object: Option<(&[u8], u64)>,
) -> Line {
    let mut line = Line {
        bytes: [0; 256],
        len: 0,
    };
    // Writing into Line cannot fail: it truncates instead.
    let _ = describe(&mut line, reason, last, object);
    line
}

fn describe(
    line: &mut Line,
    reason: ReasonCode,
    last: Option<(u64, Outcome)>,
    object: Option<(&[u8], u64)>,
) -> fmt::Result {
    let phase = if reason == ReasonCode::FatalPhase1Error { 1 } else { 2 };
    write!(line, "bunwind: fatal phase {phase} unwind error")?;
    let Some((pc, outcome)) = last else {
        return line.write_str(" before any frame lookup\n");
    };
    write!(line, " after pc {pc:#x}: ")?;
    match outcome {
        Outcome::Found => line.write_str("frame found; its CFA, a caller register or the personality failed")?,
        Outcome::NoFrame => line.write_str("no FDE covers this pc")?,
        Outcome::Failed(LookupError::Fde(error)) => {
            write!(line, "FDE {:?}+{}: {:?}", error.section, error.offset, error.kind)?
        }
        Outcome::Failed(LookupError::Cfi(error)) => {
            write!(line, "CFI {:?}+{}: {:?}", error.stream, error.offset, error.kind)?
        }
        Outcome::Failed(LookupError::MissingRow) => line.write_str("no CFI row covers this pc")?,
    }
    if let Some((name, bias)) = object {
        line.write_str(" in ")?;
        for chunk in name.utf8_chunks() {
            line.write_str(chunk.valid())?;
        }
        write!(line, " at offset {:#x}", pc.wrapping_sub(bias))?;
    }
    line.write_str("\n")
}
