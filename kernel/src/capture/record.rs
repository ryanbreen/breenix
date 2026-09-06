//! `BXCAP v1` record writer: bounded, lock-free, allocation-free.
//!
//! A capture's bytes go through this writer, which is the one place the
//! byte budget is enforced and the one place the output primitive is
//! chosen.
//!
//! # What this writer may use, and why
//!
//! Only `crate::tracing::output::raw_serial_char`, which is a single port
//! write on x86_64 and a single volatile MMIO store on aarch64. It takes no
//! lock and performs no allocation, and neither does its one callee. The
//! code above it here is integer arithmetic over a stack array. That is the
//! reason the capture path can run from a fault handler or a masked
//! interrupt.
//!
//! # The bound
//!
//! `BXCAP_BUDGET_BYTES` caps the bytes of section content one capture emits.
//! `put()` is the single enforcement point: once `remaining` reaches 0 it
//! latches `truncated` and drops the byte. The `BEGIN` and `END` lines are
//! written with the budget suspended (`budgeted = false`), because they are
//! the bracketing contract a reader uses to tell a complete capture from a
//! cut-off one -- a capture that spent its budget must still be able to say
//! so. So one capture writes at most
//!
//!     len(BEGIN line) + BXCAP_BUDGET_BYTES + 2 + len(END line)
//!
//! bytes, where the `+ 2` is the CRLF the writer inserts before `END` when a
//! record was cut mid-line. Both bracket lines are fixed-shape: their keys
//! are literals and their values are one `u64` each, so each is under 256
//! bytes. The overall bound is therefore under `BXCAP_BUDGET_BYTES + 514`.
//!
//! Sections check `truncated()` before starting a new record, so in practice
//! the budget stops the capture at a record boundary; the mid-record cut is
//! the worst case, not the normal one.

use crate::tracing::output::raw_serial_char;

/// Schema major version. A decoder refuses a record set whose `v=` it does
/// not know rather than mis-decoding it. Bumped on a field removal or a
/// semantic change, not on an addition.
pub const BXCAP_VERSION: u64 = 1;

/// Bytes of section content one capture may emit, enforced in `put()`.
///
/// Sized from the per-section record caps in `sections.rs`
/// (`BXCAP_MAX_EVENTS` + `BXCAP_MAX_COUNTERS` + one row per CPU + three
/// fixed rows) against the widest record shape each of those emits: 4752 to
/// 4799 bytes measured on the 2 committed aarch64 fixtures under
/// docs/planning/green-program/failure-capture/serials/pr3/. 8 KiB leaves
/// headroom for longer counter names without the ordinary case reaching the
/// cap, and still bounds a runaway at a size a serial log absorbs.
#[cfg(not(feature = "capture_selftest_tiny_budget"))]
pub const BXCAP_BUDGET_BYTES: u32 = 8192;

/// Anti-vacuity budget for the truncation leg. Small enough that a normal
/// capture cannot fit, so a boot built with this feature must report
/// `truncated=1` and a non-empty `sections_skipped`. If the enforcement in
/// `put()` is removed, this leg stops reporting truncation and the schema
/// suite goes red on the fixture it produces.
#[cfg(feature = "capture_selftest_tiny_budget")]
pub const BXCAP_BUDGET_BYTES: u32 = 512;

/// Bounded writer over the raw serial primitive.
///
/// Not `Copy` and not `Clone`: one is constructed on the stack per capture
/// and dropped when the capture returns, so there is no writer to outlive
/// it.
pub struct Writer {
    remaining: u32,
    records: u32,
    bytes: u32,
    truncated: bool,
    budgeted: bool,
    /// Bytes of the current line's terminator the budget has not yet let
    /// through: 2 = the whole CRLF is still owed, 1 = the LF alone, 0 = the
    /// line is terminated. `close_dangling_record()` pays exactly this.
    pending_terminator: u8,
}

impl Writer {
    #[inline(always)]
    pub fn new() -> Self {
        Self {
            remaining: BXCAP_BUDGET_BYTES,
            records: 0,
            bytes: 0,
            truncated: false,
            budgeted: true,
            pending_terminator: 0,
        }
    }

    /// The single byte sink, and the single budget-enforcement point.
    ///
    /// Returns whether the byte reached the wire. Most callers have no use
    /// for that; `close()` does, because whether a record's `]` landed is
    /// the difference between a line a reader can parse and a fragment it
    /// cannot.
    #[inline(always)]
    fn put(&mut self, byte: u8) -> bool {
        if self.budgeted {
            if self.remaining == 0 {
                self.truncated = true;
                return false;
            }
            self.remaining -= 1;
        }
        self.bytes = self.bytes.saturating_add(1);
        raw_serial_char(byte);
        true
    }

    #[inline(never)]
    pub fn text(&mut self, s: &str) {
        for byte in s.bytes() {
            self.put(byte);
        }
    }

    /// A `u64` in decimal. Digits are built into a stack array, so no
    /// formatting machinery is reached.
    pub fn dec(&mut self, mut value: u64) {
        if value == 0 {
            self.put(b'0');
            return;
        }
        let mut buf = [0u8; 20];
        let mut i = 20;
        while value > 0 {
            i -= 1;
            buf[i] = b'0' + (value % 10) as u8;
            value /= 10;
        }
        for j in i..20 {
            self.put(buf[j]);
        }
    }

    /// A `u64` in `0x`-prefixed hex, no leading zeros.
    pub fn hex(&mut self, value: u64) {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        self.text("0x");
        if value == 0 {
            self.put(b'0');
            return;
        }
        let mut started = false;
        for shift in (0..16).rev() {
            let nibble = ((value >> (shift * 4)) & 0xF) as usize;
            if nibble != 0 || started {
                self.put(HEX[nibble]);
                started = true;
            }
        }
    }

    /// Open a record. Returns `false` when the budget is already spent, so a
    /// section stops at a record boundary instead of emitting a partial row.
    #[must_use]
    pub fn open(&mut self, token: &str) -> bool {
        if self.budgeted && (self.truncated || self.remaining == 0) {
            self.truncated = true;
            return false;
        }
        self.text("[BXCAP:");
        self.text(token);
        self.pending_terminator = 2;
        true
    }

    /// Close a record: `]` then CRLF, and count it. Returns whether this
    /// record went onto the wire whole.
    ///
    /// A record whose own bytes the budget cut is NOT counted, and the
    /// terminator it still owes is paid by `close_dangling_record()` so the
    /// fragment does not run into the `END` line. That is what makes
    /// `records=` mean "well-formed `[BXCAP:...]` lines before this `END`"
    /// exactly, rather than approximately: a reader can check the count
    /// against what it could actually parse.
    ///
    /// # Where the two questions come apart
    ///
    /// "Did the budget run out during this record?" and "can a reader parse
    /// this record?" are not the same question, and they disagree on exactly
    /// one boundary: the budget ending between the closing `]` and its CRLF.
    /// The `]` is on the wire, `close_dangling_record()` supplies the
    /// terminator with the budget suspended, and the line a reader sees is
    /// complete -- so it is counted. Deciding on `self.truncated` instead
    /// would leave `records=` one short of what decodes, which is the same
    /// field disagreeing with the wire that this writer refuses to allow in
    /// the other direction. The count therefore keys on whether the `]`
    /// itself landed.
    ///
    /// # Why the caller has to look at the answer
    ///
    /// `open()` refuses only when the budget was ALREADY spent when the
    /// record started. A record that starts with a few bytes left runs out
    /// mid-way instead: its `text()`/`kv_*()` calls drop the rest silently,
    /// and only this return value says so. A section that ignored it would
    /// set its own bit in the capture's `completed` word -- and therefore
    /// clear it from `sections_skipped=` -- over a fragment. That is the
    /// one field a reader has for learning which section a truncation ate,
    /// so it is `#[must_use]`: dropping the verdict is a compile-time
    /// warning, and a zero-warning build cannot carry one.
    ///
    /// Both answers come from `put()` rather than from budget arithmetic
    /// here: the terminator is written first and the verdict read off what
    /// those writes returned. `put()` stays the only place the budget is
    /// enforced, which is what makes deleting its guard delete the bound --
    /// a second enforcement point here would keep the emitter bounded and
    /// make that mutation invisible on a real boot.
    #[must_use]
    pub fn close(&mut self) -> bool {
        let cut_before = self.truncated;
        let bracket = self.put(b']');
        if self.put(b'\r') {
            self.pending_terminator = 1;
        }
        if self.put(b'\n') {
            self.pending_terminator = 0;
        }
        if cut_before || !bracket {
            return false;
        }
        self.records = self.records.saturating_add(1);
        true
    }

    pub fn kv_dec(&mut self, key: &str, value: u64) {
        self.put(b' ');
        self.text(key);
        self.put(b'=');
        self.dec(value);
    }

    pub fn kv_hex(&mut self, key: &str, value: u64) {
        self.put(b' ');
        self.text(key);
        self.put(b'=');
        self.hex(value);
    }

    /// A `key=value` whose value is a caller-supplied literal. Values carry
    /// no spaces by the schema, and the callers in this module pass literals
    /// from a fixed set, so there is no escaping to do.
    pub fn kv_text(&mut self, key: &str, value: &str) {
        self.put(b' ');
        self.text(key);
        self.put(b'=');
        self.text(value);
    }

    #[inline(always)]
    pub fn truncated(&self) -> bool {
        self.truncated
    }

    #[inline(always)]
    pub fn records(&self) -> u64 {
        self.records as u64
    }

    #[inline(always)]
    pub fn bytes(&self) -> u64 {
        self.bytes as u64
    }

    /// Suspend the budget for the bracket lines. See the module comment.
    #[inline(always)]
    pub fn set_budgeted(&mut self, budgeted: bool) {
        self.budgeted = budgeted;
    }

    /// If the budget cut a record's line terminator, pay the part of it that
    /// did not land, so the `END` line starts at column 0 and a
    /// line-oriented reader sees it.
    ///
    /// Only the missing bytes: a record that got its `\r` through and lost
    /// only the `\n` is owed one byte, and writing a second `\r` would put a
    /// stray byte inside the line it is repairing. Callers suspend the
    /// budget first -- this repair belongs to the `END` bracket's contract,
    /// not to the section content the budget caps.
    pub fn close_dangling_record(&mut self) {
        if self.pending_terminator == 2 {
            self.put(b'\r');
        }
        if self.pending_terminator > 0 {
            self.put(b'\n');
        }
        self.pending_terminator = 0;
    }
}
