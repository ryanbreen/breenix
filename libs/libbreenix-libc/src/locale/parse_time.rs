//! POSIX strptime, with the ASCII date names of our C and C.UTF-8 locales.
//! Behavior references: POSIX XSH strptime and Linux man-pages strptime(3).
use super::calendar::{self, Tm};
use core::ptr::null_mut;

fn space(c: u8) -> bool {
    c == b' ' || (b'\t'..=b'\r').contains(&c)
}

#[derive(Default)]
struct Fields {
    century: Option<i32>,
    short_year: Option<i32>,
    full_year: Option<i32>,
    hour12: Option<i32>,
    pm: Option<bool>,
    month: bool,
    day: bool,
    weekday: bool,
    yearday: bool,
    year: bool,
    week: Option<(i32, bool)>,
    iso_week: Option<i32>,
    iso_year: Option<i32>,
}
unsafe fn number(p: &mut *const u8, width: usize, lo: i32, hi: i32) -> Option<i32> {
    while space(**p) {
        *p = p.add(1);
    }
    let mut value = 0i32;
    let mut n = 0;
    while n < width && (**p).is_ascii_digit() {
        value = value.checked_mul(10)?.checked_add((**p - b'0') as i32)?;
        *p = p.add(1);
        n += 1;
    }
    if n != 0 && (lo..=hi).contains(&value) {
        Some(value)
    } else {
        None
    }
}
unsafe fn matching(p: *const u8, word: &[u8]) -> bool {
    for (i, &c) in word.iter().enumerate() {
        if (*p.add(i)).to_ascii_lowercase() != c.to_ascii_lowercase() {
            return false;
        }
    }
    true
}
unsafe fn named(p: &mut *const u8, month: bool) -> Option<i32> {
    for i in 0..if month { 12 } else { 7 } {
        for full in [true, false] {
            let name = if month {
                calendar::month_name(i, full)
            } else {
                calendar::day_name(i, full)
            };
            if matching(*p, name) {
                *p = p.add(name.len());
                return Some(i as i32);
            }
        }
    }
    None
}
unsafe fn parse(p: &mut *const u8, fmt: *const u8, t: &mut Tm, f: &mut Fields) -> Option<()> {
    let mut i = 0;
    while *fmt.add(i) != 0 {
        let c = *fmt.add(i);
        i += 1;
        if space(c) {
            while space(**p) {
                *p = p.add(1);
            }
            continue;
        }
        if c != b'%' {
            if **p != c {
                return None;
            }
            *p = p.add(1);
            continue;
        }
        let mut c = *fmt.add(i);
        if c == 0 {
            return None;
        }
        i += 1;
        let mut width = 0usize;
        while c.is_ascii_digit() {
            width = width.checked_mul(10)?.checked_add((c - b'0') as usize)?;
            if width > 19 {
                return None;
            }
            c = *fmt.add(i);
            i += 1;
        }
        if c == b'E' || c == b'O' {
            c = *fmt.add(i);
            if c == 0 {
                return None;
            }
            i += 1;
        }
        let digits = |default| if width == 0 { default } else { width };
        let composite: &[u8] = match c {
            b'c' => b"%a %b %e %H:%M:%S %Y\0",
            b'D' | b'x' => b"%m/%d/%y\0",
            b'F' => b"%Y-%m-%d\0",
            b'r' => b"%I:%M:%S %p\0",
            b'R' => b"%H:%M\0",
            b'T' | b'X' => b"%H:%M:%S\0",
            _ => b"\0",
        };
        if composite.len() > 1 {
            parse(p, composite.as_ptr(), t, f)?;
            continue;
        }
        match c {
            b'%' => {
                if **p != b'%' {
                    return None;
                }
                *p = p.add(1);
            }
            b'n' | b't' => {
                while space(**p) {
                    *p = p.add(1);
                }
            }
            b'a' | b'A' => {
                t.wday = named(p, false)?;
                f.weekday = true;
            }
            b'b' | b'B' | b'h' => {
                t.mon = named(p, true)?;
                f.month = true;
            }
            b'd' | b'e' => {
                t.mday = number(p, digits(2), 1, 31)?;
                f.day = true;
            }
            b'm' => {
                t.mon = number(p, digits(2), 1, 12)? - 1;
                f.month = true;
            }
            b'H' | b'k' => {
                t.hour = number(p, digits(2), 0, 23)?;
                f.hour12 = None;
            }
            b'I' | b'l' => {
                let hour = number(p, digits(2), 1, 12)?;
                f.hour12 = Some(hour % 12);
                t.hour = hour % 12;
            }
            b'M' => t.min = number(p, digits(2), 0, 59)?,
            b'S' => t.sec = number(p, digits(2), 0, 60)?,
            b'j' => {
                t.yday = number(p, digits(3), 1, 366)? - 1;
                f.yearday = true;
            }
            b'w' => {
                t.wday = number(p, digits(1), 0, 6)?;
                f.weekday = true;
            }
            b'u' => {
                t.wday = number(p, digits(1), 1, 7)? % 7;
                f.weekday = true;
            }
            b'y' => {
                f.short_year = Some(number(p, digits(2), 0, 99)?);
                f.full_year = None;
                f.year = true;
            }
            b'C' => {
                f.century = Some(number(p, digits(2), 0, 99)?);
                f.full_year = None;
                f.year = true;
            }
            b'Y' => {
                f.full_year = Some(number(p, digits(4), 0, i32::MAX)?);
                f.year = true;
            }
            b'G' => f.iso_year = Some(number(p, digits(4), 0, i32::MAX)?),
            b'g' => {
                let y = number(p, digits(2), 0, 99)?;
                f.iso_year = Some(if y < 69 { 2000 + y } else { 1900 + y });
            }
            b'V' => f.iso_week = Some(number(p, digits(2), 1, 53)?),
            b'U' | b'W' => f.week = Some((number(p, digits(2), 0, 53)?, c == b'W')),
            b'p' | b'P' => {
                if matching(*p, b"AM") {
                    f.pm = Some(false);
                } else if matching(*p, b"PM") {
                    f.pm = Some(true);
                } else {
                    return None;
                }
                *p = p.add(2);
            }
            b'z' => {
                if **p == b'Z' {
                    *p = p.add(1);
                    t.gmtoff = 0;
                } else {
                    let sign = match **p {
                        b'+' => 1,
                        b'-' => -1,
                        _ => return None,
                    };
                    *p = p.add(1);
                    let hour = number(p, 2, 0, 23)?;
                    let colon = **p == b':';
                    if colon {
                        *p = p.add(1);
                    }
                    let minute = if colon || (**p).is_ascii_digit() {
                        number(p, 2, 0, 59)?
                    } else {
                        0
                    };
                    t.gmtoff = sign * (hour as i64 * 3600 + minute as i64 * 60);
                }
                t.isdst = 0;
                t.zone = b"\0".as_ptr();
            }
            b'Z' => {
                if matching(*p, b"UTC") || matching(*p, b"GMT") {
                    *p = p.add(3);
                    t.gmtoff = 0;
                    t.zone = b"UTC\0".as_ptr();
                    t.isdst = 0;
                } else {
                    // The local zone's standard or daylight-saving designation.
                    let mut found = false;
                    for (east, zone, dst) in calendar::local_designations() {
                        let mut n = 0;
                        while *zone.add(n) != 0 {
                            n += 1;
                        }
                        if n != 0 && matching(*p, core::slice::from_raw_parts(zone, n)) {
                            *p = p.add(n);
                            t.gmtoff = east;
                            t.zone = zone;
                            t.isdst = dst;
                            found = true;
                            break;
                        }
                    }
                    if !found {
                        return None;
                    }
                }
            }
            b's' => {
                let negative = **p == b'-';
                if negative || **p == b'+' {
                    *p = p.add(1);
                }
                let mut value = 0u64;
                let mut n = 0;
                while (**p).is_ascii_digit() {
                    value = value.checked_mul(10)?.checked_add((**p - b'0') as u64)?;
                    *p = p.add(1);
                    n += 1;
                }
                let limit = if negative {
                    1u64 << 63
                } else {
                    i64::MAX as u64
                };
                if n == 0 || value > limit {
                    return None;
                }
                let seconds = if negative {
                    0u64.wrapping_sub(value) as i64
                } else {
                    value as i64
                };
                *t = calendar::local_tm(seconds)?;
                f.year = true;
                f.month = true;
                f.day = true;
                f.weekday = true;
                f.yearday = true;
            }
            _ => return None,
        }
    }
    Some(())
}
fn finish(t: &mut Tm, f: Fields) -> Option<()> {
    if let Some(hour) = f.hour12 {
        t.hour = hour + if f.pm.unwrap_or(false) { 12 } else { 0 };
    }
    let y = if let Some(y) = f.full_year {
        y as i64
    } else if let Some(c) = f.century {
        (c as i64) * 100 + f.short_year.unwrap_or(0) as i64
    } else if let Some(y) = f.short_year {
        if y < 69 {
            2000 + y as i64
        } else {
            1900 + y as i64
        }
    } else {
        t.year as i64 + 1900
    };
    if f.year {
        t.year = i32::try_from(y - 1900).ok()?;
    }
    let year = y as i64;
    let mut absolute = None;
    if let (Some(iso_year), Some(week)) = (f.iso_year, f.iso_week) {
        if f.weekday {
            let jan4 = calendar::day_number(iso_year as i64, 0, 4);
            let monday = jan4 - (jan4 + 3).rem_euclid(7);
            absolute = Some(monday + (week as i64 - 1) * 7 + (t.wday + 6).rem_euclid(7) as i64);
        }
    } else if let Some((week, monday)) = f.week {
        if f.weekday && f.year {
            let jan1 = calendar::day_number(year, 0, 1);
            let wd = (jan1 + 4).rem_euclid(7);
            let first = if monday {
                (7 - (wd + 6) % 7) % 7
            } else {
                (7 - wd) % 7
            };
            let weekday = if monday { (t.wday + 6) % 7 } else { t.wday } as i64;
            absolute = Some(jan1 + first + (week as i64 - 1) * 7 + weekday);
        }
    } else if f.yearday && !(f.month && f.day) {
        if t.yday as i64 >= if calendar::leap(year) { 366 } else { 365 } {
            return None;
        }
        absolute = Some(calendar::day_number(year, 0, 1) + t.yday as i64);
    }
    if let Some(day) = absolute {
        let date = calendar::broken(day.checked_mul(86400)?, 0, b"UTC\0".as_ptr())?;
        t.year = date.year;
        t.mon = date.mon;
        t.mday = date.mday;
        t.yday = date.yday;
        if !f.weekday {
            t.wday = date.wday;
        }
    } else if (f.year || f.month || f.day)
        && (0..12).contains(&t.mon)
        && t.mday >= 1
        && t.mday as i64 <= calendar::month_days(year, t.mon as usize)
    {
        let day = calendar::day_number(year, t.mon as usize, t.mday as i64);
        if !f.weekday {
            t.wday = (day + 4).rem_euclid(7) as i32;
        }
        if !f.yearday {
            t.yday = (day - calendar::day_number(year, 0, 1)) as i32;
        }
    }
    Some(())
}
#[no_mangle]
pub unsafe extern "C" fn strptime(s: *const u8, fmt: *const u8, t: *mut Tm) -> *mut u8 {
    let mut p = s;
    let mut fields = Fields::default();
    if parse(&mut p, fmt, &mut *t, &mut fields).is_none() || finish(&mut *t, fields).is_none() {
        null_mut()
    } else {
        p as *mut u8
    }
}
