//! Regular expression compiler: XBD 9 (Regular Expressions) basic and extended syntax,
//! parsed by recursive descent straight into a program for the matchers in exec.rs.
//!
//! The program is a Thompson-style instruction list. Each atom's code is one contiguous
//! range, so a quantifier wraps the range by inserting a split in front of it (with jump
//! targets relocated) and an interval {m,n} appends copies of it. Accepted beyond POSIX,
//! as locally defined extensions: `\|`, `\+` and `\?` in basic syntax; back-references in extended
//! syntax; `\w \W \s \S` classes; `\b \B \< \> \` \'` assertions.

use super::buf::Buf;
use super::*;

/// What an assertion requires of the position it is tested at.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Assert {
    LineStart,
    LineEnd,
    WordBoundary,
    NotWordBoundary,
    WordStart,
    WordEnd,
    TextStart,
    TextEnd,
}

#[derive(Clone, Copy)]
pub(crate) enum Inst {
    Byte(u8),
    Set(u32),
    Any,
    AnyButNewline,
    /// Try the first target, then the second.
    Split(u32, u32),
    Jmp(u32),
    Save(u32),
    Clear(u32, u32),
    Mark(u32),
    Assert(Assert),
    Backref(u32),
    Match,
}

pub(crate) struct Prog {
    pub(crate) insts: Buf<Inst>,
    pub(crate) sets: Buf<[u32; 8]>,
    pub(crate) nsub: usize,
    pub(crate) icase: bool,
    pub(crate) newline: bool,
    pub(crate) repeats: u32,
    pub(crate) minimal: Buf<u32>,
    pub(crate) default_minimal: bool,
}

/// A jump target not yet known.
const PENDING: u32 = u32::MAX;
/// POSIX.1-2024 <limits.h> minimum _POSIX_RE_DUP_MAX is 255; our parser uses that limit.
pub(crate) const DUP_MAX: u32 = 255;
/// Instructions a program may hold.
const MAX_INSTS: usize = 1 << 18;

type Result<T> = core::result::Result<T, i32>;

pub(crate) fn set_has(set: &[u32; 8], c: u8) -> bool {
    set[(c >> 5) as usize] & (1 << (c & 31)) != 0
}

fn set_add(set: &mut [u32; 8], c: u8) {
    set[(c >> 5) as usize] |= 1 << (c & 31);
}

pub(crate) fn is_word(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// The POSIX-locale classes of XBD 7.3.1.
fn class(name: &[u8]) -> Option<fn(u8) -> bool> {
    Some(match name {
        b"alnum" => |c: u8| c.is_ascii_alphanumeric(),
        b"alpha" => |c: u8| c.is_ascii_alphabetic(),
        b"blank" => |c: u8| c == b' ' || c == b'\t',
        b"cntrl" => |c: u8| c.is_ascii_control(),
        b"digit" => |c: u8| c.is_ascii_digit(),
        b"graph" => |c: u8| c.is_ascii_graphic(),
        b"lower" => |c: u8| c.is_ascii_lowercase(),
        b"print" => |c: u8| c.is_ascii_graphic() || c == b' ',
        b"punct" => |c: u8| c.is_ascii_punctuation(),
        b"space" => |c: u8| matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r'),
        b"upper" => |c: u8| c.is_ascii_uppercase(),
        b"xdigit" => |c: u8| c.is_ascii_hexdigit(),
        _ => return None,
    })
}

fn targets(inst: &mut Inst, f: impl Fn(u32) -> u32) {
    match inst {
        Inst::Split(a, b) => {
            *a = f(*a);
            *b = f(*b);
        }
        Inst::Jmp(a) => *a = f(*a),
        _ => {}
    }
}

struct Parser<'a> {
    p: &'a [u8],
    i: usize,
    ere: bool,
    prog: Prog,
    /// Groups opened so far, and which of them are closed (for back-references).
    groups: u32,
    closed: Buf<bool>,
    depth: u32,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<u8> {
        self.p.get(self.i).copied()
    }

    fn peek2(&self) -> Option<u8> {
        self.p.get(self.i + 1).copied()
    }

    fn len(&self) -> u32 {
        self.prog.insts.len() as u32
    }

    fn emit(&mut self, inst: Inst) -> Result<()> {
        if self.prog.insts.len() >= MAX_INSTS || !self.prog.insts.push(inst) {
            return Err(REG_ESPACE);
        }
        Ok(())
    }

    /// Insert `inst` at `at`. Targets past `at` move up by one; a target equal to `at` moves
    /// only when its instruction moved too, so code before `at` that jumps to `at` now
    /// reaches the inserted instruction.
    fn insert(&mut self, at: u32, inst: Inst) -> Result<()> {
        if self.prog.insts.len() >= MAX_INSTS {
            return Err(REG_ESPACE);
        }
        for (k, ins) in self.prog.insts.as_mut_slice().iter_mut().enumerate() {
            let moved = k as u32 >= at;
            targets(ins, |t| {
                if t != PENDING && (t > at || (t == at && moved)) {
                    t + 1
                } else {
                    t
                }
            });
        }
        if !self.prog.insts.insert(at as usize, inst) {
            return Err(REG_ESPACE);
        }
        Ok(())
    }

    fn set_inst(&mut self, at: u32, inst: Inst) {
        self.prog.insts.as_mut_slice()[at as usize] = inst;
    }

    fn add_set(&mut self, set: [u32; 8]) -> Result<()> {
        let idx = self.prog.sets.len() as u32;
        if !self.prog.sets.push(set) {
            return Err(REG_ESPACE);
        }
        self.emit(Inst::Set(idx))
    }

    fn literal(&mut self, c: u8) -> Result<()> {
        if self.prog.icase && c.is_ascii_alphabetic() {
            let mut set = [0u32; 8];
            set_add(&mut set, c.to_ascii_lowercase());
            set_add(&mut set, c.to_ascii_uppercase());
            self.add_set(set)
        } else {
            self.emit(Inst::Byte(c))
        }
    }

    fn class_set(&mut self, f: fn(u8) -> bool, negate: bool) -> Result<()> {
        let mut set = [0u32; 8];
        for c in 0..=255u8 {
            if f(c) != negate && !(negate && self.prog.newline && c == b'\n') {
                set_add(&mut set, c);
            }
        }
        self.add_set(set)
    }

    /// Is the parser at an alternation operator?
    fn at_alt(&self) -> bool {
        if self.ere {
            self.peek() == Some(b'|')
        } else {
            self.peek() == Some(b'\\') && self.peek2() == Some(b'|')
        }
    }

    /// Is the parser at the end of a group?
    fn at_close(&self) -> bool {
        if self.ere {
            self.depth > 0 && self.peek() == Some(b')')
        } else {
            self.peek() == Some(b'\\') && self.peek2() == Some(b')')
        }
    }

    fn alternation(&mut self) -> Result<()> {
        let mut start = self.len();
        let mut jumps: Buf<u32> = Buf::new();
        self.branch()?;
        while self.at_alt() {
            self.i += if self.ere { 1 } else { 2 };
            self.insert(start, Inst::Split(start + 1, PENDING))?;
            self.emit(Inst::Jmp(PENDING))?;
            if !jumps.push(self.len() - 1) {
                return Err(REG_ESPACE);
            }
            let next = self.len();
            self.set_inst(start, Inst::Split(start + 1, next));
            start = next;
            self.branch()?;
        }
        let end = self.len();
        for &j in jumps.as_slice() {
            self.set_inst(j, Inst::Jmp(end));
        }
        Ok(())
    }

    fn branch(&mut self) -> Result<()> {
        let mut first = true;
        while self.peek().is_some() && !self.at_alt() && !self.at_close() {
            self.piece(first)?;
            first = false;
        }
        Ok(())
    }

    fn piece(&mut self, first: bool) -> Result<()> {
        let start = self.len();
        let quantifiable = self.atom(first)?;
        loop {
            let c = self.peek();
            let (min, max) = if c == Some(b'*') {
                self.i += 1;
                (0, None)
            } else if self.ere && c == Some(b'+') {
                self.i += 1;
                (1, None)
            } else if self.ere && c == Some(b'?') {
                self.i += 1;
                (0, Some(1))
            } else if self.ere
                && c == Some(b'{')
                && self
                    .peek2()
                    .is_some_and(|d| d.is_ascii_digit() || d == b',')
            {
                self.i += 1;
                self.interval()?
            } else if !self.ere && c == Some(b'\\') && self.peek2() == Some(b'{') {
                self.i += 2;
                self.interval()?
            } else if !self.ere && c == Some(b'\\') && self.peek2() == Some(b'+') {
                self.i += 2;
                (1, None)
            } else if !self.ere && c == Some(b'\\') && self.peek2() == Some(b'?') {
                self.i += 2;
                (0, Some(1))
            } else {
                return Ok(());
            };
            if !quantifiable {
                return Err(REG_BADRPT);
            }
            let mut minimal = self.ere && self.prog.default_minimal;
            if self.ere && self.peek() == Some(b'?') {
                self.i += 1;
                minimal = !minimal;
            }
            // Private repetition tags use consecutive pairs, separate from public captures.
            let tag = self.prog.repeats;
            self.prog.repeats += 1;
            self.repeat(start, min, max)?;
            self.insert(start, Inst::Mark(2 * tag))?;
            self.emit(Inst::Mark(2 * tag + 1))?;
            if minimal && !self.prog.minimal.push(tag) {
                return Err(REG_ESPACE);
            }
        }
    }

    /// `{m}`, `{m,}`, `{m,n}` (or `\{...\}`), after the opening brace.
    fn interval(&mut self) -> Result<(u32, Option<u32>)> {
        let number = |s: &mut Self| -> Result<Option<u32>> {
            let mut v: Option<u32> = None;
            while let Some(d) = s.peek().filter(|d| d.is_ascii_digit()) {
                let n = v.unwrap_or(0) * 10 + (d - b'0') as u32;
                if n > DUP_MAX {
                    return Err(REG_BADBR);
                }
                v = Some(n);
                s.i += 1;
            }
            Ok(v)
        };
        let lo = number(self)?;
        let hi = if self.peek() == Some(b',') {
            self.i += 1;
            number(self)?
        } else {
            Some(lo.ok_or(REG_BADBR)?)
        };
        let closed = if self.ere {
            self.peek() == Some(b'}')
        } else {
            self.peek() == Some(b'\\') && self.peek2() == Some(b'}')
        };
        if !closed {
            return Err(if self.peek().is_none() {
                REG_EBRACE
            } else {
                REG_BADBR
            });
        }
        self.i += if self.ere { 1 } else { 2 };
        let lo = lo.unwrap_or(0);
        if hi.is_some_and(|h| h < lo) {
            return Err(REG_BADBR);
        }
        Ok((lo, hi))
    }

    /// Copy the code in [s, e) to the end of the program.
    fn copy(&mut self, s: u32, e: u32) -> Result<()> {
        let base = self.len();
        for k in s..e {
            let mut inst = self.prog.insts.as_slice()[k as usize];
            targets(&mut inst, |t| {
                if t != PENDING && t >= s && t <= e {
                    t - s + base
                } else {
                    t
                }
            });
            self.emit(inst)?;
        }
        Ok(())
    }

    fn star(&mut self, s: u32) -> Result<()> {
        self.insert(s, Inst::Split(s + 1, PENDING))?;
        self.emit(Inst::Jmp(s))?;
        let end = self.len();
        self.set_inst(s, Inst::Split(s + 1, end));
        Ok(())
    }

    fn optional(&mut self, s: u32) -> Result<()> {
        self.insert(s, Inst::Split(s + 1, PENDING))?;
        let end = self.len();
        self.set_inst(s, Inst::Split(s + 1, end));
        Ok(())
    }

    /// Apply {min,max} to the code in [s, end).
    fn repeat(&mut self, s: u32, min: u32, max: Option<u32>) -> Result<()> {
        let e = self.len();
        if max == Some(0) {
            self.prog.insts.truncate(s as usize);
            return Ok(());
        }
        if min == 0 {
            match max {
                None => return self.star(s),
                Some(n) => {
                    // The first copy, then n-1 more, each optional.
                    let len = e - s;
                    for _ in 1..n {
                        let at = self.len();
                        self.copy(s, s + len)?;
                        self.optional(at)?;
                    }
                    return self.optional(s);
                }
            }
        }
        let len = e - s;
        for _ in 1..min {
            self.copy(s, s + len)?;
        }
        match max {
            None => {
                let at = self.len();
                self.copy(s, s + len)?;
                self.star(at)
            }
            Some(n) => {
                for _ in min..n {
                    let at = self.len();
                    self.copy(s, s + len)?;
                    self.optional(at)?;
                }
                Ok(())
            }
        }
    }

    /// Emit one atom; false when it is an assertion, which takes no quantifier.
    fn atom(&mut self, first: bool) -> Result<bool> {
        let c = self.peek().ok_or(REG_BADPAT)?;
        self.i += 1;
        match c {
            b'(' if self.ere => self.group(),
            b'.' => {
                let inst = if self.prog.newline {
                    Inst::AnyButNewline
                } else {
                    Inst::Any
                };
                self.emit(inst)?;
                Ok(true)
            }
            b'[' => {
                self.bracket()?;
                Ok(true)
            }
            b'^' if self.ere || first => {
                self.emit(Inst::Assert(Assert::LineStart))?;
                if !self.ere && self.peek() == Some(b'*') {
                    // `^*` in basic syntax: the star is an ordinary character.
                    self.i += 1;
                    self.literal(b'*')?;
                }
                Ok(false)
            }
            b'$' if self.ere || self.peek().is_none() || self.at_alt() || self.at_close() => {
                self.emit(Inst::Assert(Assert::LineEnd))?;
                Ok(false)
            }
            b'*' if first => {
                self.literal(b'*')?;
                Ok(true)
            }
            b'+' | b'?' | b'{' if self.ere && first => {
                self.literal(c)?;
                Ok(true)
            }
            b'*' | b'+' | b'?' if self.ere => Err(REG_BADRPT),
            b'\\' => self.escape(first),
            _ => {
                self.literal(c)?;
                Ok(true)
            }
        }
    }

    fn group(&mut self) -> Result<bool> {
        self.groups += 1;
        let n = self.groups;
        if !self.closed.push(false) {
            return Err(REG_ESPACE);
        }
        let clear = self.len();
        self.emit(Inst::Clear(n, n + 1))?;
        self.emit(Inst::Save(2 * n))?;
        self.depth += 1;
        self.alternation()?;
        self.depth -= 1;
        let closed = if self.ere {
            self.peek() == Some(b')')
        } else {
            self.peek() == Some(b'\\') && self.peek2() == Some(b')')
        };
        if !closed {
            return Err(REG_EPAREN);
        }
        self.i += if self.ere { 1 } else { 2 };
        self.set_inst(clear, Inst::Clear(n, self.groups + 1));
        self.emit(Inst::Save(2 * n + 1))?;
        self.closed.as_mut_slice()[(n - 1) as usize] = true;
        Ok(true)
    }

    fn escape(&mut self, first: bool) -> Result<bool> {
        let c = self.peek().ok_or(REG_EESCAPE)?;
        self.i += 1;
        let assert = |s: &mut Self, a: Assert| -> Result<bool> {
            s.emit(Inst::Assert(a))?;
            Ok(false)
        };
        match c {
            b'(' if !self.ere => self.group(),
            b')' if !self.ere => Err(REG_EPAREN),
            b'{' if !self.ere => {
                if first {
                    self.literal(b'{')?;
                    Ok(true)
                } else {
                    Err(REG_BADRPT)
                }
            }
            b'1'..=b'9' => {
                let n = (c - b'0') as u32;
                if n > self.groups || !self.closed.as_slice()[(n - 1) as usize] {
                    return Err(REG_ESUBREG);
                }

                self.emit(Inst::Backref(n))?;
                Ok(true)
            }
            b'w' | b'W' => {
                self.class_set(is_word, c == b'W')?;
                Ok(true)
            }
            b's' | b'S' => {
                self.class_set(class(b"space").ok_or(REG_ECTYPE)?, c == b'S')?;
                Ok(true)
            }
            b'b' => assert(self, Assert::WordBoundary),
            b'B' => assert(self, Assert::NotWordBoundary),
            b'<' => assert(self, Assert::WordStart),
            b'>' => assert(self, Assert::WordEnd),
            b'`' => assert(self, Assert::TextStart),
            b'\'' => assert(self, Assert::TextEnd),
            _ => {
                self.literal(c)?;
                Ok(true)
            }
        }
    }

    /// One endpoint of a bracket range: a byte or a single-character `[.c.]` or `[=c=]`.
    fn bracket_char(&mut self) -> Result<u8> {
        let c = self.peek().ok_or(REG_EBRACK)?;
        if c == b'[' && matches!(self.peek2(), Some(b'.') | Some(b'=')) {
            let kind = self.p[self.i + 1];
            let body = self.i + 2;
            let close = (body..self.p.len().saturating_sub(1))
                .find(|&k| self.p[k] == kind && self.p[k + 1] == b']')
                .ok_or(REG_EBRACK)?;
            if close != body + 1 {
                return Err(REG_ECOLLATE);
            }
            self.i = close + 2;
            return Ok(self.p[body]);
        }
        self.i += 1;
        Ok(c)
    }

    fn bracket(&mut self) -> Result<()> {
        let mut set = [0u32; 8];
        let negate = self.peek() == Some(b'^');
        if negate {
            self.i += 1;
        }
        let mut first = true;
        loop {
            let c = self.peek().ok_or(REG_EBRACK)?;
            if c == b']' && !first {
                self.i += 1;
                break;
            }
            first = false;
            if c == b'[' && self.peek2() == Some(b':') {
                let body = self.i + 2;
                let close = (body..self.p.len().saturating_sub(1))
                    .find(|&k| self.p[k] == b':' && self.p[k + 1] == b']')
                    .ok_or(REG_EBRACK)?;
                let f = class(&self.p[body..close]).ok_or(REG_ECTYPE)?;
                for b in 0..=255u8 {
                    if f(b) {
                        set_add(&mut set, b);
                    }
                }
                self.i = close + 2;
                if self.peek() == Some(b'-') && self.peek2().is_some_and(|d| d != b']') {
                    return Err(REG_ERANGE);
                }
                continue;
            }
            let lo = self.bracket_char()?;
            if self.peek() == Some(b'-') && self.peek2().is_some_and(|d| d != b']') {
                self.i += 1;
                let hi = self.bracket_char()?;
                if hi < lo {
                    return Err(REG_ERANGE);
                }
                for b in lo..=hi {
                    set_add(&mut set, b);
                }
            } else {
                set_add(&mut set, lo);
            }
        }
        if self.prog.icase {
            for b in 0..=255u8 {
                if b.is_ascii_alphabetic() && set_has(&set, b) {
                    set_add(&mut set, b.to_ascii_lowercase());
                    set_add(&mut set, b.to_ascii_uppercase());
                }
            }
        }
        if negate {
            for w in set.iter_mut() {
                *w = !*w;
            }
            if self.prog.newline {
                set[(b'\n' >> 5) as usize] &= !(1 << (b'\n' & 31));
            }
        }
        self.add_set(set)
    }
}

/// Compile `p` with regcomp's flags.
pub(crate) fn compile(p: &[u8], cflags: i32) -> Result<Prog> {
    let mut parser = Parser {
        p,
        i: 0,
        ere: cflags & REG_EXTENDED != 0,
        prog: Prog {
            insts: Buf::new(),
            sets: Buf::new(),
            nsub: 0,
            icase: cflags & REG_ICASE != 0,
            newline: cflags & REG_NEWLINE != 0,
            repeats: 0,
            minimal: Buf::new(),
            default_minimal: cflags & REG_MINIMAL != 0,
        },
        groups: 0,
        closed: Buf::new(),
        depth: 0,
    };
    parser.emit(Inst::Save(0))?;
    parser.alternation()?;
    if parser.peek().is_some() {
        // Only an unmatched close of a group stops the top level.
        return Err(REG_EPAREN);
    }
    parser.emit(Inst::Save(1))?;
    parser.emit(Inst::Match)?;
    parser.prog.nsub = parser.groups as usize;
    Ok(parser.prog)
}
