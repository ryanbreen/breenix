//! fnmatch (POSIX.1-2024 XSH fnmatch; the pattern language is XCU 2.14, Pattern Matching
//! Notation). Flags are independent bits assigned by this implementation.
//!
//! Matching consumes bytes in C/POSIX and decoded characters in C.UTF-8; bracket classes are the
//! POSIX-locale classes of XBD 7.3.1. A `*` is matched by remembering the most recent star
//! and, on a mismatch, letting it take one more character. Every other pattern element matches
//! exactly one character, so retrying only the latest star finds a match whenever one exists;
//! with FNM_PATHNAME a star cannot take a `/`, so each `/` in the pattern pairs with a
//! fixed `/` in the string and the same argument holds per component.

pub(crate) const FNM_NOMATCH: i32 = 1;
pub(crate) const FNM_PATHNAME: i32 = 1 << 0;
pub(crate) const FNM_NOESCAPE: i32 = 1 << 1;
pub(crate) const FNM_PERIOD: i32 = 1 << 2;
const FNM_LEADING_DIR: i32 = 1 << 3;
// Private grammar switch: regex brackets use ^ only and treat backslash literally.
pub(crate) const FNM_REGEX_BRACKET: i32 = 1 << 5;
pub(crate) const FNM_CASEFOLD: i32 = 1 << 4;

pub(crate) fn unit(bytes: &[u8], at: usize) -> Option<(u32, usize)> {
    if at >= bytes.len() { return None; }
    if crate::locale::charset() == crate::locale::Charset::Byte {
        return Some((bytes[at] as u32, 1));
    }
    match crate::locale::decode(&bytes[at..], &mut crate::locale::MbState::default()) {
        crate::locale::Decoded::Char(c, n) => Some((c, n)),
        _ => None,
    }
}
fn lower(c: u32) -> u32 {
    if crate::locale::charset() == crate::locale::Charset::Utf8 {
        crate::locale::classify::towlower(c)
    } else { (c as u8).to_ascii_lowercase() as u32 }
}
fn upper(c: u32) -> u32 {
    if crate::locale::charset() == crate::locale::Charset::Utf8 {
        crate::locale::classify::towupper(c)
    } else { (c as u8).to_ascii_uppercase() as u32 }
}
pub(crate) fn same(a: u32, b: u32, fold: bool) -> bool {
    a == b || (fold && lower(a) == lower(b))
}

/// Character classes use the LOCALE owner's thread-specific LC_CTYPE service.
pub(crate) fn in_class(name: &[u8], c: u32) -> Option<bool> {
    if crate::locale::charset() == crate::locale::Charset::Utf8 {
        use crate::locale::classify::*;
        return Some(match name {
            b"alnum" => iswalnum(c), b"alpha" => iswalpha(c), b"blank" => iswblank(c),
            b"cntrl" => iswcntrl(c), b"digit" => iswdigit(c), b"graph" => iswgraph(c),
            b"lower" => iswlower(c), b"print" => iswprint(c), b"punct" => iswpunct(c),
            b"space" => iswspace(c), b"upper" => iswupper(c), b"xdigit" => iswxdigit(c),
            _ => return None,
        } != 0);
    }
    let c = c as u8;
    Some(match name {
        b"alnum" => c.is_ascii_alphanumeric(), b"alpha" => c.is_ascii_alphabetic(),
        b"blank" => c == b' ' || c == b'\t', b"cntrl" => c.is_ascii_control(),
        b"digit" => c.is_ascii_digit(), b"graph" => c.is_ascii_graphic(),
        b"lower" => c.is_ascii_lowercase(), b"print" => c.is_ascii_graphic() || c == b' ',
        b"punct" => c.is_ascii_punctuation(), b"space" => matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r'),
        b"upper" => c.is_ascii_uppercase(), b"xdigit" => c.is_ascii_hexdigit(),
        _ => return None,
    })
}

enum Bracket {
    /// The bracket expression ends just before this pattern index; did it match?
    Done(usize, bool),
    /// No closing `]`, or an unknown class: the `[` is an ordinary character.
    Literal,
}

/// One endpoint of a bracket item: a byte, an escaped byte or a `[.c.]`/`[=c=]` element.
fn endpoint(p: &[u8], i: usize, noescape: bool) -> Option<(u32, usize)> {
    let c = *p.get(i)?;
    if c == b'[' && matches!(p.get(i + 1), Some(b'.') | Some(b'=')) {
        let kind = p[i + 1];
        let body = i + 2;
        let close =
            (body..p.len().saturating_sub(1)).find(|&k| p[k] == kind && p[k + 1] == b']')?;
        let (scalar, width) = unit(p, body)?;
        if close != body + width {
            return None;
        }
        return Some((scalar, close + 2));
    }
    if c == b'\\' && !noescape && i + 1 < p.len() {
        let (scalar, width) = unit(p, i + 1)?;
        return Some((scalar, i + 1 + width));
    }
    let (scalar, width) = unit(p, i)?;
    Some((scalar, i + width))
}

/// Match `c` against the bracket expression starting at p[start] == '['.
fn bracket(p: &[u8], start: usize, c: u32, flags: i32) -> Bracket {
    let noescape = flags & FNM_NOESCAPE != 0;
    let fold = flags & FNM_CASEFOLD != 0;
    let mut i = start + 1;
    let negate = if flags & FNM_REGEX_BRACKET != 0 { p.get(i) == Some(&b'^') } else { matches!(p.get(i), Some(b'!') | Some(b'^')) };
    if negate {
        i += 1;
    }
    let mut matched = false;
    let mut first = true;
    loop {
        let Some(&ch) = p.get(i) else {
            return Bracket::Literal;
        };
        if ch == b']' && !first {
            i += 1;
            break;
        }
        first = false;
        if ch == b'[' && p.get(i + 1) == Some(&b':') {
            let body = i + 2;
            let Some(close) =
                (body..p.len().saturating_sub(1)).find(|&k| p[k] == b':' && p[k + 1] == b']')
            else {
                return Bracket::Literal;
            };
            let name = &p[body..close];
            let Some(hit) = in_class(name, c) else {
                return Bracket::Literal;
            };
            let folded = fold
                && ((name == b"upper" && in_class(b"lower", c).unwrap_or(false))
                    || (name == b"lower" && in_class(b"upper", c).unwrap_or(false)));
            matched |= hit || folded;
            i = close + 2;
            continue;
        }
        let Some((lo, after)) = endpoint(p, i, noescape) else {
            return Bracket::Literal;
        };
        i = after;
        if p.get(i) == Some(&b'-') && p.get(i + 1).is_some_and(|&b| b != b']') {
            let Some((hi, after)) = endpoint(p, i + 1, noescape) else {
                return Bracket::Literal;
            };
            i = after;
            let within = |x: u32| lo <= x && x <= hi;
            if within(c) || (fold && (within(lower(c)) || within(upper(c)))) {
                matched = true;
            }
        } else if same(c, lo, fold) {
            matched = true;
        }
    }
    Bracket::Done(i, matched != negate)
}

/// Is string index `i` a leading position for FNM_PERIOD?
fn leading(s: &[u8], i: usize, flags: i32) -> bool {
    flags & FNM_PERIOD != 0
        && s.get(i) == Some(&b'.')
        && (i == 0 || (flags & FNM_PATHNAME != 0 && s[i - 1] == b'/'))
}

/// Can a wildcard (`?`, `*` or a bracket) take the byte at string index `i`?
fn wild_ok(s: &[u8], i: usize, flags: i32) -> bool {
    !(flags & FNM_PATHNAME != 0 && s[i] == b'/') && !leading(s, i, flags)
}

pub(crate) fn matches(p: &[u8], s: &[u8], flags: i32) -> bool {
    let noescape = flags & FNM_NOESCAPE != 0;
    let fold = flags & FNM_CASEFOLD != 0;
    let (mut pi, mut si) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    loop {
        let current = if si < s.len() {
            let Some(value) = unit(s, si) else { return false; };
            Some(value)
        } else { None };
        let step = if pi < p.len() {
            match p[pi] {
                b'*' => {
                    if si < s.len() && leading(s, si, flags) {
                        None
                    } else {
                        while p.get(pi) == Some(&b'*') {
                            pi += 1;
                        }
                        star = Some((pi, si));
                        continue;
                    }
                }
                b'?' => (si < s.len() && wild_ok(s, si, flags)).then_some(pi + 1),
                b'[' if si < s.len() => match bracket(p, pi, current.unwrap().0, flags) {
                    Bracket::Done(next, hit) => (hit && wild_ok(s, si, flags)).then_some(next),
                    Bracket::Literal => (s[si] == b'[').then_some(pi + 1),
                },
                b'\\' if !noescape && pi + 1 < p.len() => {
                    let Some((c, width)) = unit(p, pi + 1) else { return false; };
                    current.filter(|&(value, _)| same(value, c, fold)).map(|_| pi + 1 + width)
                }
                _ => {
                    let Some((c, width)) = unit(p, pi) else { return false; };
                    current.filter(|&(value, _)| same(value, c, fold)).map(|_| pi + width)
                }
            }
        } else if si == s.len() || (flags & FNM_LEADING_DIR != 0 && s[si] == b'/') {
            return true;
        } else {
            None
        };
        match step {
            Some(next) => {
                pi = next;
                si += current.unwrap().1;
            }
            None => {
                let Some((sp, ss)) = star else {
                    return false;
                };
                if ss >= s.len() || (flags & FNM_PATHNAME != 0 && s[ss] == b'/') {
                    return false;
                }
                let Some((_, width)) = unit(s, ss) else { return false; };
                star = Some((sp, ss + width));
                pi = sp;
                si = ss + width;
            }
        }
    }
}

/// fnmatch - 0 when `string` matches `pattern`, else FNM_NOMATCH.
#[no_mangle]
pub unsafe extern "C" fn fnmatch(pattern: *const u8, string: *const u8, flags: i32) -> i32 {
    let p = core::ffi::CStr::from_ptr(pattern as *const core::ffi::c_char).to_bytes();
    let s = core::ffi::CStr::from_ptr(string as *const core::ffi::c_char).to_bytes();
    if matches(p, s, flags) {
        0
    } else {
        FNM_NOMATCH
    }
}
