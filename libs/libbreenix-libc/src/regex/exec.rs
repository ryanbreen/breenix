//! Matching a compiled program (XBD 9.1: the leftmost of the longest matches).
//!
//! All capture histories are retained until complete matches can be compared by XBD 9.1.
//! Empty loops are bounded and exhaustion returns REG_ESPACE rather than a weak match.

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
    let prev = if prog.utf8 {
        (pos.saturating_sub(4).max(t.from)..pos).rev().find_map(|i| {
            crate::fnmatch::unit(t.s, i).filter(|&(_, n)| i + n == pos).map(|(c, _)| c)
        })
    } else if pos > t.from { Some(t.s[pos - 1] as u32) } else { None };
    let next = text_unit(prog, t.s, pos).map(|(c, _)| c);
    let word = |c| if prog.utf8 { c == b'_' as u32 || crate::fnmatch::in_class(b"alnum", c).unwrap_or(false) } else { is_word(c as u8) };
    let prev_word = prev.is_some_and(word);
    let next_word = next.is_some_and(word);
    match a {
        Assert::LineStart => (pos == t.from && !t.notbol) || (prog.newline && prev == Some(b'\n' as u32)),
        Assert::LineEnd => (pos == end && !t.noteol) || (prog.newline && next == Some(b'\n' as u32)),
        Assert::WordBoundary => prev_word != next_word,
        Assert::NotWordBoundary => prev_word == next_word,
        Assert::WordStart => !prev_word && next_word,
        Assert::WordEnd => prev_word && !next_word,
        Assert::TextStart => pos == t.from,
        Assert::TextEnd => pos == end,
    }
}

fn text_unit(prog: &Prog, bytes: &[u8], at: usize) -> Option<(u32, usize)> {
    if prog.utf8 { crate::fnmatch::unit(bytes, at) } else { bytes.get(at).map(|&c| (c as u32, 1)) }
}

/// Does this consuming instruction take the decoded character?
fn takes(prog: &Prog, inst: Inst, c: u32) -> bool {
    match inst {
        Inst::Byte(b) => b as u32 == c,
        Inst::Rune(r) => crate::fnmatch::same(r, c, prog.icase),
        Inst::Bracket(start, end, no_newline) => {
            if no_newline && c == b'\n' as u32 { return false; }
            let mut bytes = [0; 4];
            let Some(n) = crate::locale::encode(c, &mut bytes) else { return false; };
            crate::fnmatch::matches(&prog.brackets.as_slice()[start as usize..end as usize], &bytes[..n],
                crate::fnmatch::FNM_NOESCAPE | crate::fnmatch::FNM_REGEX_BRACKET |
                if prog.icase { crate::fnmatch::FNM_CASEFOLD } else { 0 })
        }
        Inst::WideClass(kind, negate) => {
            let hit = if kind == 0 { c == b'_' as u32 || crate::fnmatch::in_class(b"alnum", c).unwrap_or(false) }
                else { crate::fnmatch::in_class(b"space", c).unwrap_or(false) };
            hit != negate && !(negate && prog.newline && c == b'\n' as u32)
        }
        Inst::Set(i) => c <= 255 && set_has(&prog.sets.as_slice()[i as usize], c as u8),
        Inst::Any => true,
        Inst::AnyButNewline => c != b'\n' as u32,
        _ => false,
    }
}

fn match_length(prog: &Prog, t: &Text, caps: &[isize], slot: usize, absent: isize) -> isize {
    if caps[slot] < 0 || caps[slot + 1] < 0 { return absent; }
    if !prog.utf8 { return caps[slot + 1] - caps[slot]; }
    let mut pos = caps[slot] as usize;
    let mut count = 0;
    while pos < caps[slot + 1] as usize {
        let Some((_, width)) = text_unit(prog, t.s, pos) else { return absent; };
        pos += width;
        count += 1;
    }
    count
}

// XBD 9.1: empty participation outranks nonparticipation; earlier subexpressions
// take precedence among equal whole matches. Minimal repetitions select shortest.
fn better(prog: &Prog, t: &Text, found: bool, best: &[isize], candidate: &[isize]) -> bool {
    if !found {
        return true;
    }
    if candidate[0] != best[0] {
        return candidate[0] < best[0];
    }
    for &tag in prog.minimal.as_slice() {
        let slot = 2 * (prog.nsub + 1) + 2 * tag as usize;
        let len = |c: &[isize]| match_length(prog, t, c, slot, isize::MAX);
        if len(candidate) != len(best) {
            return len(candidate) < len(best);
        }
    }
    if candidate[1] != best[1] {
        return candidate[1] > best[1];
    }
    for group in 1..=prog.nsub {
        let k = 2 * group;
        let len = |c: &[isize]| match_length(prog, t, c, k, -1);
        if len(candidate) != len(best) {
            return len(candidate) > len(best);
        }
    }
    false
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
        if start < end && text_unit(prog, t.s, start).is_none() { continue; }
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
                        if better(prog, t, found, caps, work.as_slice()) {
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
                    Inst::Clear(first, end) => {
                        for k in 2 * first..2 * end {
                            if !stack.push(Step::RestoreCap(k, work.as_slice()[k as usize])) {
                                return Err(REG_ESPACE);
                            }
                            work.as_mut_slice()[k as usize] = -1;
                        }
                        pc += 1;
                    }
                    Inst::Mark(tag) => {
                        let k = (2 * (prog.nsub + 1)) as u32 + tag;
                        if !stack.push(Step::RestoreCap(k, work.as_slice()[k as usize])) {
                            return Err(REG_ESPACE);
                        }
                        work.as_mut_slice()[k as usize] = pos as isize;
                        pc += 1;
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
                        let (a, b) = (
                            work.as_slice()[2 * k as usize],
                            work.as_slice()[2 * k as usize + 1],
                        );
                        if a < 0 || b < 0 {
                            break;
                        }
                        if prog.utf8 && prog.icase {
                            let (mut left, mut right) = (a as usize, pos);
                            let mut equal = true;
                            while left < b as usize {
                                let Some((x, nx)) = text_unit(prog, t.s, left) else { equal = false; break; };
                                let Some((y, ny)) = text_unit(prog, t.s, right) else { equal = false; break; };
                                if !crate::fnmatch::same(x, y, true) { equal = false; break; }
                                left += nx; right += ny;
                            }
                            if !equal { break; }
                            pos = right;
                        } else {
                            let len = (b - a) as usize;
                            if pos + len > end { break; }
                            let same = (0..len).all(|i| {
                                let (x, y) = (t.s[a as usize + i], t.s[pos + i]);
                                x == y || (prog.icase && x.eq_ignore_ascii_case(&y))
                            });
                            if !same { break; }
                            pos += len;
                        }
                        pc += 1;
                    }
                    _ => {
                        if let Some((_, width)) = text_unit(prog, t.s, pos).filter(|&(c, _)| takes(prog, inst, c)) {
                            pos += width;
                            pc += 1;
                        } else {
                            break;
                        }
                    }
                }
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
    backtrack(prog, t, caps)
}
