//! Matching a compiled program (XBD 9.1: the leftmost of the longest matches).
//!
//! Programs without back-references run on a Pike VM: every thread advances one byte at a
//! time, threads are kept in priority order, and two threads at the same instruction
//! merge into the higher-priority one. Threads seeded at later start positions are lower
//! priority, so a merge never loses an earlier start. Instead of stopping at the first
//! thread to reach Match, the VM runs on and keeps the match that starts leftmost and,
//! among those, ends last; at equal ends the higher-priority thread's submatches win, which
//! prefers earlier alternatives and longer repetitions.
//!
//! Back-references make the language non-regular, so those programs are searched by
//! backtracking from each start position, exploring every path for the longest end. An
//! iteration of a loop that consumes nothing is abandoned, and a step budget bounds the
//! search (REG_ESPACE when it runs out).

use super::buf::Buf;
use super::compile::{is_word, set_has, Assert, Inst, Prog};
use super::REG_ESPACE;

pub(crate) struct Text<'a> {
    /// The string up to the end of the searched region.
    pub(crate) s: &'a [u8],
    /// Where the region starts.
    pub(crate) from: usize,
    pub(crate) notbol: bool,
    pub(crate) noteol: bool,
}

const STEP_BUDGET: u64 = 50_000_000;

fn holds(prog: &Prog, t: &Text, a: Assert, pos: usize) -> bool {
    let end = t.s.len();
    let prev = if pos > t.from { Some(t.s[pos - 1]) } else { None };
    let next = t.s.get(pos).copied();
    let prev_word = prev.is_some_and(is_word);
    let next_word = next.is_some_and(is_word);
    match a {
        Assert::LineStart => (pos == t.from && !t.notbol) || (prog.newline && prev == Some(b'\n')),
        Assert::LineEnd => (pos == end && !t.noteol) || (prog.newline && next == Some(b'\n')),
        Assert::WordBoundary => prev_word != next_word,
        Assert::NotWordBoundary => prev_word == next_word,
        Assert::WordStart => !prev_word && next_word,
        Assert::WordEnd => prev_word && !next_word,
        Assert::TextStart => pos == t.from,
        Assert::TextEnd => pos == end,
    }
}

/// Does the one-byte instruction `inst` take byte `c`?
fn takes(prog: &Prog, inst: Inst, c: u8) -> bool {
    match inst {
        Inst::Byte(b) => b == c,
        Inst::Set(i) => set_has(&prog.sets.as_slice()[i as usize], c),
        Inst::Any => true,
        Inst::AnyButNewline => c != b'\n',
        _ => false,
    }
}

/// Is `best` beaten by a match from `start` to `end`?
fn better(found: bool, best: &[isize], start: isize, end: isize) -> bool {
    !found || start < best[0] || (start == best[0] && end > best[1])
}

/// Threads at one position: a sparse set of instructions with each thread's submatches.
struct Threads {
    dense: Buf<u32>,
    sparse: Buf<u32>,
    caps: Buf<isize>,
}

impl Threads {
    fn new(n: usize, slots: usize) -> Option<Self> {
        let mut t = Threads { dense: Buf::new(), sparse: Buf::new(), caps: Buf::new() };
        if !t.sparse.resize(n, 0) || !t.dense.reserve(n) || !t.caps.resize(n.checked_mul(slots)?, -1) {
            return None;
        }
        Some(t)
    }
    fn contains(&self, pc: u32) -> bool {
        let i = self.sparse.as_slice()[pc as usize] as usize;
        i < self.dense.len() && self.dense.as_slice()[i] == pc
    }
    fn insert(&mut self, pc: u32) -> usize {
        let i = self.dense.len();
        self.dense.push(pc);
        self.sparse.as_mut_slice()[pc as usize] = i as u32;
        i
    }
}

#[derive(Clone, Copy)]
enum Frame {
    Explore(u32),
    Restore(u32, isize),
}

/// Add the thread at `pc` and everything it reaches without consuming a byte.
fn add(prog: &Prog, t: &Text, list: &mut Threads, stack: &mut Buf<Frame>, work: &mut [isize], pc: u32, pos: usize) {
    let slots = work.len();
    stack.truncate(0);
    stack.push(Frame::Explore(pc));
    while stack.len() > 0 {
        let top = stack.as_slice()[stack.len() - 1];
        stack.truncate(stack.len() - 1);
        let pc = match top {
            Frame::Restore(slot, old) => {
                work[slot as usize] = old;
                continue;
            }
            Frame::Explore(pc) => pc,
        };
        if list.contains(pc) {
            continue;
        }
        let i = list.insert(pc);
        match prog.insts.as_slice()[pc as usize] {
            Inst::Jmp(x) => {
                stack.push(Frame::Explore(x));
            }
            Inst::Split(x, y) => {
                stack.push(Frame::Explore(y));
                stack.push(Frame::Explore(x));
            }
            Inst::Save(n) => {
                if (n as usize) < slots {
                    stack.push(Frame::Restore(n, work[n as usize]));
                    work[n as usize] = pos as isize;
                }
                stack.push(Frame::Explore(pc + 1));
            }
            Inst::Assert(a) => {
                if holds(prog, t, a, pos) {
                    stack.push(Frame::Explore(pc + 1));
                }
            }
            _ => {
                list.caps.as_mut_slice()[i * slots..(i + 1) * slots].copy_from_slice(work);
            }
        }
    }
}

/// The Pike VM. On a match, fills `caps` and returns true; None when memory runs out.
fn pike(prog: &Prog, t: &Text, caps: &mut [isize]) -> Option<bool> {
    let n = prog.insts.len();
    let slots = caps.len();
    let mut clist = Threads::new(n, slots)?;
    let mut nlist = Threads::new(n, slots)?;
    let mut stack: Buf<Frame> = Buf::new();
    if !stack.reserve(2 * n + 2) {
        return None;
    }
    let mut work: Buf<isize> = Buf::new();
    if !work.resize(slots, -1) {
        return None;
    }
    let mut found = false;
    let end = t.s.len();
    let mut pos = t.from;
    loop {
        if !found {
            work.as_mut_slice().fill(-1);
            add(prog, t, &mut clist, &mut stack, work.as_mut_slice(), 0, pos);
        }
        if clist.dense.len() == 0 && found {
            break;
        }
        nlist.dense.truncate(0);
        for k in 0..clist.dense.len() {
            let pc = clist.dense.as_slice()[k];
            let tc = &clist.caps.as_slice()[k * slots..(k + 1) * slots];
            if found && tc[0] > caps[0] {
                continue;
            }
            let inst = prog.insts.as_slice()[pc as usize];
            if let Inst::Match = inst {
                if better(found, caps, tc[0], tc[1]) {
                    caps.copy_from_slice(tc);
                    found = true;
                }
                continue;
            }
            if pos < end && takes(prog, inst, t.s[pos]) {
                work.as_mut_slice().copy_from_slice(tc);
                add(prog, t, &mut nlist, &mut stack, work.as_mut_slice(), pc + 1, pos + 1);
            }
        }
        core::mem::swap(&mut clist, &mut nlist);
        if pos >= end {
            break;
        }
        pos += 1;
    }
    Some(found)
}

#[derive(Clone, Copy)]
enum Step {
    Try(u32, usize),
    RestoreCap(u32, isize),
    RestoreLoop(u32, usize),
}

/// Backtracking search for programs with back-references.
fn backtrack(prog: &Prog, t: &Text, caps: &mut [isize]) -> Result<bool, i32> {
    let n = prog.insts.len();
    let slots = caps.len();
    let mut work: Buf<isize> = Buf::new();
    let mut last: Buf<usize> = Buf::new();
    let mut stack: Buf<Step> = Buf::new();
    if !work.resize(slots, -1) || !last.resize(n, usize::MAX) {
        return Err(REG_ESPACE);
    }
    let end = t.s.len();
    let mut steps: u64 = 0;
    for start in t.from..=end {
        let mut found = false;
        work.as_mut_slice().fill(-1);
        stack.truncate(0);
        if !stack.push(Step::Try(0, start)) {
            return Err(REG_ESPACE);
        }
        while stack.len() > 0 {
            let top = stack.as_slice()[stack.len() - 1];
            stack.truncate(stack.len() - 1);
            let (mut pc, mut pos) = match top {
                Step::RestoreCap(slot, old) => {
                    work.as_mut_slice()[slot as usize] = old;
                    continue;
                }
                Step::RestoreLoop(k, old) => {
                    last.as_mut_slice()[k as usize] = old;
                    continue;
                }
                Step::Try(pc, pos) => (pc, pos),
            };
            loop {
                steps += 1;
                if steps > STEP_BUDGET {
                    return Err(REG_ESPACE);
                }
                let inst = prog.insts.as_slice()[pc as usize];
                // A jump back to an earlier instruction closes a loop iteration; one that
                // consumed nothing since the last time is abandoned.
                let back = match inst {
                    Inst::Jmp(x) => x <= pc,
                    Inst::Split(x, _) => x <= pc,
                    _ => false,
                };
                if back {
                    let prev = last.as_slice()[pc as usize];
                    if prev == pos {
                        if let Inst::Split(_, y) = inst {
                            pc = y;
                            continue;
                        }
                        break;
                    }
                    if !stack.push(Step::RestoreLoop(pc, prev)) {
                        return Err(REG_ESPACE);
                    }
                    last.as_mut_slice()[pc as usize] = pos;
                }
                match inst {
                    Inst::Match => {
                        if better(found, caps, start as isize, pos as isize) {
                            caps.copy_from_slice(work.as_slice());
                            found = true;
                        }
                        break;
                    }
                    Inst::Jmp(x) => pc = x,
                    Inst::Split(x, y) => {
                        if !stack.push(Step::Try(y, pos)) {
                            return Err(REG_ESPACE);
                        }
                        pc = x;
                    }
                    Inst::Save(k) => {
                        if (k as usize) < slots {
                            if !stack.push(Step::RestoreCap(k, work.as_slice()[k as usize])) {
                                return Err(REG_ESPACE);
                            }
                            work.as_mut_slice()[k as usize] = pos as isize;
                        }
                        pc += 1;
                    }
                    Inst::Assert(a) => {
                        if !holds(prog, t, a, pos) {
                            break;
                        }
                        pc += 1;
                    }
                    Inst::Backref(k) => {
                        let (a, b) = (work.as_slice()[2 * k as usize], work.as_slice()[2 * k as usize + 1]);
                        if a < 0 || b < 0 {
                            break;
                        }
                        let len = (b - a) as usize;
                        if pos + len > end {
                            break;
                        }
                        let same = (0..len).all(|i| {
                            let (x, y) = (t.s[a as usize + i], t.s[pos + i]);
                            x == y || (prog.icase && x.eq_ignore_ascii_case(&y))
                        });
                        if !same {
                            break;
                        }
                        pos += len;
                        pc += 1;
                    }
                    _ => {
                        if pos < end && takes(prog, inst, t.s[pos]) {
                            pos += 1;
                            pc += 1;
                        } else {
                            break;
                        }
                    }
                }
            }
            if found && caps[1] as usize == end {
                break;
            }
        }
        if found {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Search `t` for `prog`, leaving the submatch offsets in `caps` (two per group, -1 for a
/// group that did not take part).
pub(crate) fn search(prog: &Prog, t: &Text, caps: &mut [isize]) -> Result<bool, i32> {
    caps.fill(-1);
    if prog.backrefs {
        backtrack(prog, t, caps)
    } else {
        pike(prog, t, caps).ok_or(REG_ESPACE)
    }
}
