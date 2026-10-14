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
    let prev = if pos > t.from {
        Some(t.s[pos - 1])
    } else {
        None
    };
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

// XBD 9.1: empty participation outranks nonparticipation; earlier subexpressions
// take precedence among equal whole matches. Minimal repetitions select shortest.
fn better(prog: &Prog, found: bool, best: &[isize], candidate: &[isize]) -> bool {
    if !found {
        return true;
    }
    if candidate[0] != best[0] {
        return candidate[0] < best[0];
    }
    for &tag in prog.minimal.as_slice() {
        let slot = 2 * (prog.nsub + 1) + 2 * tag as usize;
        let len = |c: &[isize]| {
            if c[slot] < 0 {
                isize::MAX
            } else {
                c[slot + 1] - c[slot]
            }
        };
        if len(candidate) != len(best) {
            return len(candidate) < len(best);
        }
    }
    if candidate[1] != best[1] {
        return candidate[1] > best[1];
    }
    for group in 1..=prog.nsub {
        let k = 2 * group;
        let len = |c: &[isize]| {
            if c[k] < 0 || c[k + 1] < 0 {
                -1
            } else {
                c[k + 1] - c[k]
            }
        };
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
                        if better(prog, found, caps, work.as_slice()) {
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
