//! `BXCAP v1` record writer: bounded, lock-free, allocation-free.
//!
//! Every byte a capture emits goes through this writer. It exists so the
//! emitter has exactly one place where the byte budget is enforced and
//! exactly one place where the output primitive is chosen.
//!
//! # What this writer may use, and why
//!
//! Only `crate::tracing::output::raw_serial_char`, which is a single port
//! write on x86_64 and a single volatile MMIO store on aarch64. It takes no
//! lock, allocates nothing, and calls nothing that does. Everything above it
//! here is integer arithmetic over a stack array. That is the whole reason
//! the capture path can run from a fault handler or a masked interrupt.
//!
//! # The bound
//!
//! `BXCAP_BUDGET_BYTES` caps the bytes of section content one capture emits.
//! `put()` is the single enforcement point: once `remaining` reaches zero it
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
/// semantic change, never on an addition.
pub const BXCAP_VERSION: u64 = 1;

/// Bytes of section content one capture may emit, enforced in `put()`.
///
/// Sized from the per-section record caps in `sections.rs`
/// (`BXCAP_MAX_EVENTS` + `BXCAP_MAX_COUNTERS` + one row per CPU + three
/// fixed rows) against the widest record shape each of those emits, which
/// comes to roughly 5 KiB on a boot with every section populated. 8 KiB
/// leaves headroom for longer counter names without the normal case ever
/// reaching the cap, and still bounds a runaway at a size a serial log
/// absorbs.
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
/// Not `Copy`, not `Clone`, and never stored: one is constructed on the
/// stack per capture and dropped when the capture returns.
pub struct Writer {
    remaining: u32,
    records: u32,
    bytes: u32,
    truncated: bool,
    budgeted: bool,
    in_record: bool,
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
            in_record: false,
        }
    }

    /// The single byte sink, and the single budget-enforcement point.
    #[inline(always)]
    fn put(&mut self, byte: u8) {
        if self.budgeted {
            if self.remaining == 0 {
                self.truncated = true;
                return;
            }
            self.remaining -= 1;
        }
        self.bytes = self.bytes.saturating_add(1);
        raw_serial_char(byte);
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
        self.in_record = true;
        true
    }

    /// Close a record: `]` then CRLF, and count it.
    ///
    /// A record the budget cut is NOT closed and NOT counted. It stays
    /// `in_record`, so `close_dangling_record()` puts the line terminator
    /// back before `END` and the incomplete line does not run into it. That
    /// is what makes `records=` mean "well-formed `[BXCAP:...]` lines before
    /// this `END`" exactly, rather than approximately: a reader can check
    /// the count against what it could actually parse.
    ///
    /// The terminator is written FIRST and the verdict taken afterwards, so
    /// that the case where a record's body fitted but its `]\r\n` did not is
    /// decided by `put()` dropping those bytes rather than by a second piece
    /// of budget arithmetic here. `put()` is the only place the budget is
    /// enforced, which is what makes deleting its guard delete the bound --
    /// a second enforcement point here would keep the emitter bounded and
    /// make that mutation invisible on a real boot.
    pub fn close(&mut self) {
        let cut_before = self.truncated;
        self.put(b']');
        self.put(b'\r');
        self.put(b'\n');
        if cut_before || self.truncated {
            return;
        }
        self.in_record = false;
        self.records = self.records.saturating_add(1);
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
    /// no spaces by the schema, and every caller in this module passes a
    /// literal from a fixed set, so there is nothing to escape.
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

    /// If the budget cut a record mid-line, put the terminator back so the
    /// `END` line starts at column zero and a line-oriented reader sees it.
    pub fn close_dangling_record(&mut self) {
        if self.in_record {
            self.in_record = false;
            self.put(b'\r');
            self.put(b'\n');
        }
    }
}
