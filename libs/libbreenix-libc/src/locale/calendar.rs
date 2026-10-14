//! Gregorian calendar arithmetic derived from the 4/100/400-year leap rule.
//! Epoch is 1970-01-01. TZ follows POSIX.1-2024 XBD 8.3's second format
//! (std offset [dst [offset] [,rule]]); any other value means UTC.
use super::set_errno;
use core::ptr::{addr_of_mut, null_mut};
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct Tm {
    pub sec: i32,
    pub min: i32,
    pub hour: i32,
    pub mday: i32,
    pub mon: i32,
    pub year: i32,
    pub wday: i32,
    pub yday: i32,
    pub isdst: i32,
    pub gmtoff: i64,
    pub zone: *const u8,
}
#[repr(C)]
#[derive(Default)]
pub struct Timespec {
    pub sec: i64,
    pub nsec: i64,
}
static mut RESULT: Tm = Tm {
    sec: 0,
    min: 0,
    hour: 0,
    mday: 0,
    mon: 0,
    year: 0,
    wday: 0,
    yday: 0,
    isdst: 0,
    gmtoff: 0,
    zone: core::ptr::null(),
};
static mut TEXT: [u8; 26] = [0; 26];
static mut STD_NAME: [u8; 64] = [0; 64];
static mut DST_NAME: [u8; 64] = [0; 64];
static mut ZONE: Zone = Zone::UTC;
static ZONE_LOCK: crate::pthread::RecursiveLock = crate::pthread::RecursiveLock::new();
static mut LAST_TZ: [u8; 128] = [0; 128];
static mut LAST_LEN: usize = usize::MAX;
#[no_mangle]
pub static mut timezone: i64 = 0;
#[no_mangle]
pub static mut daylight: i32 = 0;
#[no_mangle]
pub static mut tzname: [*const u8; 2] = [b"UTC\0".as_ptr(); 2];
pub(super) fn leap(y: i64) -> bool {
    y.rem_euclid(4) == 0 && (y.rem_euclid(100) != 0 || y.rem_euclid(400) == 0)
}
fn year_days(y: i64) -> i64 {
    let p = y - 1;
    365 * p + p.div_euclid(4) - p.div_euclid(100) + p.div_euclid(400)
}
pub(super) fn month_days(y: i64, m: usize) -> i64 {
    [
        31,
        if leap(y) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ][m]
}
pub(super) fn day_number(y: i64, m: usize, d: i64) -> i64 {
    year_days(y) - 719162 + (0..m).map(|i| month_days(y, i)).sum::<i64>() + d - 1
}
pub(super) fn broken(sec: i64, offset: i64, zone: *const u8) -> Option<Tm> {
    let seconds = sec.checked_add(offset)?;
    let days = seconds.div_euclid(86400);
    let within = seconds.rem_euclid(86400);
    let day = days.checked_add(719162)?;
    let era = day.div_euclid(146097);
    let mut y = era * 400 + 1;
    let mut yd = day.rem_euclid(146097);
    while yd >= if leap(y) { 366 } else { 365 } {
        yd -= if leap(y) { 366 } else { 365 };
        y += 1;
    }
    let year = i32::try_from(y - 1900).ok()?;
    let yday = yd as i32;
    let mut mon = 0;
    while yd >= month_days(y, mon) {
        yd -= month_days(y, mon);
        mon += 1;
    }
    Some(Tm {
        sec: (within % 60) as i32,
        min: ((within / 60) % 60) as i32,
        hour: (within / 3600) as i32,
        mday: yd as i32 + 1,
        mon: mon as i32,
        year,
        wday: (days + 4).rem_euclid(7) as i32,
        yday,
        isdst: 0,
        gmtoff: offset,
        zone,
    })
}
#[no_mangle]
pub unsafe extern "C" fn time(out: *mut i64) -> i64 {
    let mut ts = Timespec::default();
    if crate::clock_gettime(0, &mut ts as *mut _ as *mut u8) != 0 {
        return -1;
    }
    if !out.is_null() {
        *out = ts.sec;
    }
    ts.sec
}
#[no_mangle]
pub extern "C" fn difftime(a: i64, b: i64) -> f64 {
    (a as i128 - b as i128) as f64
}
#[no_mangle]
pub unsafe extern "C" fn clock() -> i64 {
    let mut ts = Timespec::default();
    if crate::clock_gettime(2, &mut ts as *mut _ as *mut u8) != 0 {
        return -1;
    }
    ts.sec
        .checked_mul(1_000_000)
        .and_then(|n| n.checked_add(ts.nsec / 1000))
        .unwrap_or(-1)
}
#[no_mangle]
pub unsafe extern "C" fn timespec_get(ts: *mut Timespec, base: i32) -> i32 {
    if base != 1 || crate::clock_gettime(0, ts as *mut u8) != 0 {
        0
    } else {
        base
    }
}
#[no_mangle]
pub unsafe extern "C" fn gmtime_r(t: *const i64, out: *mut Tm) -> *mut Tm {
    match broken(*t, 0, b"UTC\0".as_ptr()) {
        Some(tm) => {
            *out = tm;
            out
        }
        None => {
            set_errno(75);
            null_mut()
        }
    }
}
#[no_mangle]
pub unsafe extern "C" fn gmtime(t: *const i64) -> *mut Tm {
    gmtime_r(t, addr_of_mut!(RESULT))
}
/// One end of a daylight-saving period: a date rule and the local time of day.
#[derive(Clone, Copy)]
struct Change {
    date: Date,
    time: i64,
}
#[derive(Clone, Copy)]
enum Date {
    /// Jn: day 1-365, never counting 29 February.
    Julian(i64),
    /// n: zero-based day 0-365, counting 29 February.
    Day(i64),
    /// Mm.w.d: day d (0 = Sunday) of week w (5 = last) of month m.
    Month(usize, i64, i64),
}
#[derive(Clone, Copy)]
pub(super) struct Zone {
    /// Seconds east of UTC in standard and in daylight-saving time.
    std_east: i64,
    dst_east: i64,
    std_name: *const u8,
    dst_name: *const u8,
    rule: Option<(Change, Change)>,
}
impl Zone {
    const UTC: Zone = Zone {
        std_east: 0,
        dst_east: 0,
        std_name: b"UTC\0".as_ptr(),
        dst_name: b"UTC\0".as_ptr(),
        rule: None,
    };
}
/// Days since the epoch of a rule's date in year y.
fn rule_day(date: Date, y: i64) -> i64 {
    let jan1 = day_number(y, 0, 1);
    match date {
        Date::Julian(n) => jan1 + n - 1 + i64::from(leap(y) && n >= 60),
        Date::Day(n) => jan1 + n,
        Date::Month(m, w, d) => {
            let first = day_number(y, m - 1, 1);
            let mut mday = 1 + (d - (first + 4)).rem_euclid(7) + 7 * (w - 1);
            if mday > month_days(y, m - 1) {
                mday -= 7;
            }
            first + mday - 1
        }
    }
}
fn year_of(seconds: i64) -> i64 {
    let day = seconds.div_euclid(86400) + 719162;
    let mut y = day.div_euclid(146097) * 400 + 1;
    let mut yd = day.rem_euclid(146097);
    while yd >= if leap(y) { 366 } else { 365 } {
        yd -= if leap(y) { 366 } else { 365 };
        y += 1;
    }
    y
}
impl Zone {
    /// Whether daylight-saving time is in effect at UTC instant t. The changes of
    /// the neighbouring years are included so a rule time past midnight, or a
    /// period that spans the new year, is still found.
    fn dst_at(&self, t: i64) -> bool {
        let Some((start, end)) = self.rule else {
            return false;
        };
        let y = year_of(t.saturating_add(self.std_east));
        let mut last: Option<(i64, bool)> = None;
        for year in y - 1..=y + 1 {
            // The start is given in standard time and the end in daylight-saving time.
            for (instant, on) in [
                (rule_day(start.date, year) * 86400 + start.time - self.std_east, true),
                (rule_day(end.date, year) * 86400 + end.time - self.dst_east, false),
            ] {
                // At equal instants the start wins, so a year-round period stays on.
                if instant <= t && last.is_none_or(|l| (instant, on) > l) {
                    last = Some((instant, on));
                }
            }
        }
        last.is_some_and(|(_, on)| on)
    }
    fn east(&self, dst: bool) -> (i64, *const u8) {
        if dst {
            (self.dst_east, self.dst_name)
        } else {
            (self.std_east, self.std_name)
        }
    }
}
pub(super) unsafe fn local_zone() -> Zone {
    ZONE_LOCK.lock();
    update_zone();
    let zone = *addr_of_mut!(ZONE);
    ZONE_LOCK.unlock();
    zone
}
unsafe fn local(t: i64, out: *mut Tm, zone: &Zone) -> *mut Tm {
    let dst = zone.dst_at(t);
    let (east, name) = zone.east(dst);
    match broken(t, east, name) {
        Some(mut tm) => {
            tm.isdst = i32::from(dst);
            *out = tm;
            out
        }
        None => {
            set_errno(75);
            null_mut()
        }
    }
}
/// The local zone's designations with their offsets and tm_isdst values; the
/// daylight-saving entry repeats the standard one when the zone has no DST.
pub(super) unsafe fn local_designations() -> [(i64, *const u8, i32); 2] {
    let zone = local_zone();
    let dst = i32::from(zone.rule.is_some());
    [
        (zone.std_east, zone.std_name, 0),
        (zone.dst_east, zone.dst_name, dst),
    ]
}
pub(super) unsafe fn local_tm(t: i64) -> Option<Tm> {
    let mut tm = Tm::default();
    (!local(t, &mut tm, &local_zone()).is_null()).then_some(tm)
}
#[no_mangle]
pub unsafe extern "C" fn localtime_r(t: *const i64, out: *mut Tm) -> *mut Tm {
    local(*t, out, &local_zone())
}
#[no_mangle]
pub unsafe extern "C" fn localtime(t: *const i64) -> *mut Tm {
    localtime_r(t, addr_of_mut!(RESULT))
}
unsafe fn make(tm: *mut Tm, zone: &Zone) -> i64 {
    let t = &*tm;
    let year = t.year as i64 + 1900 + (t.mon as i64).div_euclid(12);
    let mon = t.mon.rem_euclid(12) as usize;
    let wall = day_number(year, mon, t.mday as i64) * 86400
        + t.hour as i64 * 3600
        + t.min as i64 * 60
        + t.sec as i64;
    let seconds = if zone.rule.is_none() || t.isdst == 0 {
        wall - zone.std_east
    } else if t.isdst > 0 {
        wall - zone.dst_east
    } else {
        // Unknown: take the reading under which the wall time is valid, preferring
        // standard time; a time skipped by the change to DST reads as standard.
        let as_standard = wall - zone.std_east;
        let as_daylight = wall - zone.dst_east;
        if !zone.dst_at(as_standard) || !zone.dst_at(as_daylight) {
            as_standard
        } else {
            as_daylight
        }
    };
    if local(seconds, tm, zone).is_null() {
        -1
    } else {
        seconds
    }
}
#[no_mangle]
pub unsafe extern "C" fn timegm(tm: *mut Tm) -> i64 {
    make(tm, &Zone::UTC)
}
#[no_mangle]
pub unsafe extern "C" fn mktime(tm: *mut Tm) -> i64 {
    make(tm, &local_zone())
}
#[no_mangle]
pub unsafe extern "C" fn tzset() {
    ZONE_LOCK.lock();
    update_zone();
    ZONE_LOCK.unlock();
}
/// A std or dst designation, quoted (<...>: alphanumerics, + and -) or unquoted
/// (alphabetics), 3 to 63 bytes.
fn designation(b: &[u8], i: &mut usize) -> Option<(usize, usize)> {
    let (start, end) = if b.get(*i) == Some(&b'<') {
        let start = *i + 1;
        let mut j = start;
        while b.get(j)? != &b'>' {
            if !(b[j].is_ascii_alphanumeric() || b[j] == b'+' || b[j] == b'-') {
                return None;
            }
            j += 1;
        }
        *i = j + 1;
        (start, j)
    } else {
        let start = *i;
        while b.get(*i).is_some_and(|c| c.is_ascii_alphabetic()) {
            *i += 1;
        }
        (start, *i)
    };
    (3..64).contains(&(end - start)).then_some((start, end))
}
fn number(b: &[u8], i: &mut usize, max: i64) -> Option<i64> {
    let begin = *i;
    let mut n: i64 = 0;
    while let Some(&c) = b.get(*i).filter(|c| c.is_ascii_digit()) {
        n = n * 10 + (c - b'0') as i64;
        *i += 1;
        if n > max {
            return None;
        }
    }
    (*i != begin).then_some(n)
}
/// [+|-]hh[:mm[:ss]] in seconds, positive for + or no sign.
fn hms(b: &[u8], i: &mut usize, hours: i64) -> Option<i64> {
    let sign = match b.get(*i) {
        Some(b'-') => -1,
        Some(b'+') => 1,
        _ => 0,
    };
    if sign != 0 {
        *i += 1;
    }
    let mut total = number(b, i, hours)? * 3600;
    for unit in [60, 1] {
        if b.get(*i) != Some(&b':') {
            break;
        }
        *i += 1;
        total += number(b, i, 59)? * unit;
    }
    Some(if sign < 0 { -total } else { total })
}
fn change(b: &[u8], i: &mut usize) -> Option<Change> {
    let date = match b.get(*i)? {
        b'J' => {
            *i += 1;
            Date::Julian(number(b, i, 365).filter(|&n| n >= 1)?)
        }
        b'M' => {
            *i += 1;
            let m = number(b, i, 12).filter(|&m| m >= 1)?;
            (b.get(*i) == Some(&b'.')).then_some(())?;
            *i += 1;
            let w = number(b, i, 5).filter(|&w| w >= 1)?;
            (b.get(*i) == Some(&b'.')).then_some(())?;
            *i += 1;
            Date::Month(m as usize, w, number(b, i, 6)?)
        }
        _ => Date::Day(number(b, i, 365)?),
    };
    // The default change time is 02:00; an explicit one may be -167 to 167 hours.
    let time = if b.get(*i) == Some(&b'/') {
        *i += 1;
        hms(b, i, 167)?
    } else {
        7200
    };
    Some(Change { date, time })
}
/// Parse POSIX.1-2024 XBD 8.3 TZ "std offset [dst [offset] [,start[/time],end[/time]]]".
fn parse_zone(b: &[u8]) -> Option<(Zone, (usize, usize), Option<(usize, usize)>)> {
    let mut i = 0;
    let std = designation(b, &mut i)?;
    // POSIX offsets count west of UTC; Zone stores seconds east.
    let std_east = -hms(b, &mut i, 24)?;
    let mut zone = Zone {
        std_east,
        dst_east: std_east,
        ..Zone::UTC
    };
    if i == b.len() {
        return Some((zone, std, None));
    }
    let dst = designation(b, &mut i)?;
    zone.dst_east = if matches!(b.get(i), Some(c) if c.is_ascii_digit() || *c == b'+' || *c == b'-')
    {
        -hms(b, &mut i, 24)?
    } else {
        std_east + 3600
    };
    zone.rule = Some(if i == b.len() {
        // POSIX leaves the changes implementation-defined when dst has no rule.
        // Breenix uses the second Sunday of March to the first Sunday of November,
        // at 02:00 local time (TZ rule M3.2.0,M11.1.0).
        (
            Change {
                date: Date::Month(3, 2, 0),
                time: 7200,
            },
            Change {
                date: Date::Month(11, 1, 0),
                time: 7200,
            },
        )
    } else {
        (b[i] == b',').then_some(())?;
        i += 1;
        let start = change(b, &mut i)?;
        (b.get(i) == Some(&b',')).then_some(())?;
        i += 1;
        let end = change(b, &mut i)?;
        (i == b.len()).then_some(())?;
        (start, end)
    });
    Some((zone, std, Some(dst)))
}
fn copy_name(buffer: &mut [u8; 64], name: &[u8]) -> *const u8 {
    buffer[..name.len()].copy_from_slice(name);
    buffer[name.len()] = 0;
    buffer.as_ptr()
}
unsafe fn update_zone() {
    let b = crate::env::getenv_bytes(b"TZ")
        .filter(|s| s.len() <= 128)
        .unwrap_or(b"");
    // Reentrant calendar calls must not rewrite the zone names other threads
    // are reading when the environment has not changed.
    let last = &mut *addr_of_mut!(LAST_TZ);
    if LAST_LEN == b.len() && last[..b.len()] == *b {
        return;
    }
    last[..b.len()].copy_from_slice(b);
    LAST_LEN = b.len();
    let mut zone = Zone::UTC;
    // An empty TZ, the implementation-defined ":..." form and geographical names
    // (Breenix has no zone database) all mean UTC.
    if let Some((parsed, std, dst)) = parse_zone(b) {
        zone = parsed;
        zone.std_name = copy_name(&mut *addr_of_mut!(STD_NAME), &b[std.0..std.1]);
        zone.dst_name = match dst {
            Some((start, end)) => copy_name(&mut *addr_of_mut!(DST_NAME), &b[start..end]),
            None => zone.std_name,
        };
    }
    *addr_of_mut!(ZONE) = zone;
    timezone = -zone.std_east;
    daylight = i32::from(zone.rule.is_some());
    tzname = [zone.std_name, zone.dst_name];
}
const DAYS: [&[u8]; 7] = [
    b"Sunday",
    b"Monday",
    b"Tuesday",
    b"Wednesday",
    b"Thursday",
    b"Friday",
    b"Saturday",
];
const MONTHS: [&[u8]; 12] = [
    b"January",
    b"February",
    b"March",
    b"April",
    b"May",
    b"June",
    b"July",
    b"August",
    b"September",
    b"October",
    b"November",
    b"December",
];
pub(super) fn day_name(i: usize, full: bool) -> &'static [u8] {
    let s = DAYS[i % 7];
    if full {
        s
    } else {
        &s[..3]
    }
}
pub(super) fn month_name(i: usize, full: bool) -> &'static [u8] {
    let s = MONTHS[i % 12];
    if full {
        s
    } else {
        &s[..3]
    }
}
pub(super) fn day_info(i: usize, full: bool) -> *mut u8 {
    const SHORT: [&[u8]; 7] = [
        b"Sun\0", b"Mon\0", b"Tue\0", b"Wed\0", b"Thu\0", b"Fri\0", b"Sat\0",
    ];
    const LONG: [&[u8]; 7] = [
        b"Sunday\0",
        b"Monday\0",
        b"Tuesday\0",
        b"Wednesday\0",
        b"Thursday\0",
        b"Friday\0",
        b"Saturday\0",
    ];
    (if full { LONG[i] } else { SHORT[i] }).as_ptr() as *mut u8
}
pub(super) fn month_info(i: usize, full: bool) -> *mut u8 {
    const SHORT: [&[u8]; 12] = [
        b"Jan\0", b"Feb\0", b"Mar\0", b"Apr\0", b"May\0", b"Jun\0", b"Jul\0", b"Aug\0", b"Sep\0",
        b"Oct\0", b"Nov\0", b"Dec\0",
    ];
    const LONG: [&[u8]; 12] = [
        b"January\0",
        b"February\0",
        b"March\0",
        b"April\0",
        b"May\0",
        b"June\0",
        b"July\0",
        b"August\0",
        b"September\0",
        b"October\0",
        b"November\0",
        b"December\0",
    ];
    (if full { LONG[i] } else { SHORT[i] }).as_ptr() as *mut u8
}
struct Output {
    narrow: *mut u8,
    wide: *mut u32,
    cap: usize,
    n: usize,
}
impl Output {
    unsafe fn put(&mut self, c: u8) -> bool {
        if self.n >= self.cap {
            return false;
        }
        if !self.narrow.is_null() {
            *self.narrow.add(self.n) = c;
        } else {
            *self.wide.add(self.n) = c as u32;
        }
        self.n += 1;
        true
    }
    unsafe fn text(&mut self, s: &[u8]) -> bool {
        for &c in s {
            if !self.put(c) {
                return false;
            }
        }
        true
    }
    unsafe fn number(&mut self, n: i64, width: usize, pad: u8) -> bool {
        let mut buf = [0u8; 24];
        let mut i = buf.len();
        let mut v = n.unsigned_abs();
        loop {
            i -= 1;
            buf[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        let sign = usize::from(n < 0);
        let len = buf.len() - i + sign;
        if n < 0 && pad == b'0' && !self.put(b'-') {
            return false;
        }
        for _ in len..width {
            if !self.put(pad) {
                return false;
            }
        }
        if n < 0 && pad != b'0' && !self.put(b'-') {
            return false;
        }
        self.text(&buf[i..])
    }
}
/// ISO 8601 week-based year and week, from the fields alone (no calendar call
/// that could fail, whatever the year).
fn iso_week(t: &Tm) -> (i64, i64) {
    let y = t.year as i64 + 1900;
    let weekday = match (t.wday as i64).rem_euclid(7) {
        0 => 7,
        d => d,
    };
    let thursday = t.yday as i64 + 4 - weekday;
    if thursday < 0 {
        // This week's Thursday falls in the previous year.
        let previous = if leap(y - 1) { 366 } else { 365 };
        return (y - 1, (thursday + previous) / 7 + 1);
    }
    let length = if leap(y) { 366 } else { 365 };
    if thursday >= length {
        (y + 1, 1)
    } else {
        (y, thursday / 7 + 1)
    }
}
/// A year (or, for %C, a century) under POSIX.1-2024 strftime's field rules: no
/// padding unless a width is given or `digits` (2 for %C) says so; the + flag
/// pads with zeros and adds a + once the field needs more than `limit` bytes.
unsafe fn year(out: &mut Output, v: i64, width: usize, pad: u8, plus: bool, limit: usize) -> bool {
    let mut digits = 0;
    let mut rest = v.unsigned_abs();
    loop {
        digits += 1;
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    let sign = if v < 0 {
        Some(b'-')
    } else if plus && width.max(digits) > limit {
        Some(b'+')
    } else {
        None
    };
    let total = width.max(digits + usize::from(sign.is_some()));
    if let Some(sign) = sign {
        if !out.put(sign) {
            return false;
        }
    }
    let fill = total - digits - usize::from(sign.is_some());
    if pad != 0 {
        for _ in 0..fill {
            if !out.put(pad) {
                return false;
            }
        }
    }
    // Years come from an int tm_year, so the magnitude fits.
    out.number(v.unsigned_abs() as i64, 0, b'0')
}
unsafe fn format(out: &mut Output, fmt: *const u8, t: &Tm) -> bool {
    let mut i = 0;
    while *fmt.add(i) != 0 {
        let c = *fmt.add(i);
        i += 1;
        if c != b'%' {
            if !out.put(c) {
                return false;
            }
            continue;
        }
        let mut c = *fmt.add(i);
        if c == 0 {
            return false;
        }
        i += 1;
        // POSIX flags 0 and +, and the _ (space) and - (no padding) extensions.
        let mut pad = b'0';
        let mut plus = false;
        let mut width: usize = 0;
        if matches!(c, b'_' | b'0' | b'-' | b'+') {
            pad = match c {
                b'_' => b' ',
                b'-' => 0,
                _ => b'0',
            };
            plus = c == b'+';
            c = *fmt.add(i);
            i += 1;
        }
        let explicit = c.is_ascii_digit();
        while c.is_ascii_digit() {
            width = width.saturating_mul(10).saturating_add((c - b'0') as usize);
            if width > 4096 {
                return false;
            }
            c = *fmt.add(i);
            i += 1;
        }
        if c == b'E' || c == b'O' {
            c = *fmt.add(i);
            i += 1;
        }
        let y = t.year as i64 + 1900;
        let okay = match c {
            b'Y' => Some(year(out, y, width, pad, plus, 4)),
            b'G' => Some(year(out, iso_week(t).0, width, pad, plus, 4)),
            // Truncated toward zero; two characters at least without a width.
            b'C' => Some(year(out, y / 100, if explicit { width } else { 2 }, pad, plus, 2)),
            b'F' => {
                // The year as %Y with the same flag and width x - 6 (x at least 6).
                let w = if explicit { width.max(6) - 6 } else { 0 };
                Some(year(out, y, w, pad, plus, 4) && format(out, b"-%m-%d\0".as_ptr(), t))
            }
            _ => None,
        };
        if let Some(okay) = okay {
            if !okay {
                return false;
            }
            continue;
        }
        let numeric = match c {
            b'd' | b'e' => Some((t.mday as i64, 2)),
            b'H' => Some((t.hour as i64, 2)),
            b'I' => Some(((t.hour as i64 + 11).rem_euclid(12) + 1, 2)),
            b'j' => Some((t.yday as i64 + 1, 3)),
            b'm' => Some((t.mon as i64 + 1, 2)),
            b'M' => Some((t.min as i64, 2)),
            b'S' => Some((t.sec as i64, 2)),
            b'y' => Some((y.rem_euclid(100), 2)),
            b'w' => Some((t.wday as i64, 1)),
            b'u' => Some((if t.wday == 0 { 7 } else { t.wday as i64 }, 1)),
            b'U' => Some(((t.yday as i64 + 7 - t.wday as i64) / 7, 2)),
            b'W' => Some(((t.yday as i64 + 7 - (t.wday as i64 + 6) % 7) / 7, 2)),
            b'g' => Some((iso_week(t).0.rem_euclid(100), 2)),
            b'V' => Some((iso_week(t).1, 2)),
            _ => None,
        };
        if let Some((v, w)) = numeric {
            if c == b'e' && pad == b'0' {
                pad = b' ';
            }
            if !out.number(
                v,
                if pad == 0 {
                    0
                } else if width != 0 {
                    width
                } else {
                    w
                },
                pad,
            ) {
                return false;
            }
            continue;
        }
        let composite: &[u8] = match c {
            b'c' => b"%a %b %e %H:%M:%S %Y\0",
            b'D' | b'x' => b"%m/%d/%y\0",
            b'r' => b"%I:%M:%S %p\0",
            b'R' => b"%H:%M\0",
            b'T' | b'X' => b"%H:%M:%S\0",
            _ => b"\0",
        };
        if composite.len() > 1 {
            if !format(out, composite.as_ptr(), t) {
                return false;
            }
            continue;
        }
        let okay = match c {
            b'a' | b'A' => out.text(day_name(t.wday.rem_euclid(7) as usize, c == b'A')),
            b'b' | b'h' | b'B' => out.text(month_name(t.mon.rem_euclid(12) as usize, c == b'B')),
            b'p' => out.text(if t.hour < 12 { b"AM" } else { b"PM" }),
            b'n' => out.put(b'\n'),
            b't' => out.put(b'\t'),
            b'%' => out.put(b'%'),
            b'Z' => {
                let mut n = 0;
                if !t.zone.is_null() {
                    while *t.zone.add(n) != 0 {
                        n += 1;
                    }
                    out.text(core::slice::from_raw_parts(t.zone, n))
                } else {
                    true
                }
            }
            // No offset is determinable when tm_isdst is negative.
            b'z' if t.isdst < 0 => true,
            b'z' => {
                let v = t.gmtoff;
                let a = v.unsigned_abs();
                out.put(if v < 0 { b'-' } else { b'+' })
                    && out.number((a / 3600 % 100) as i64, 2, b'0')
                    && out.number((a / 60 % 60) as i64, 2, b'0')
            }
            // Seconds since the epoch, as mktime computes them.
            b's' => {
                let mut copy = *t;
                out.number(mktime(&mut copy), 0, b'0')
            }
            _ => false,
        };
        if !okay {
            return false;
        }
    }
    true
}
#[no_mangle]
pub unsafe extern "C" fn strftime(s: *mut u8, n: usize, fmt: *const u8, t: *const Tm) -> usize {
    let mut out = Output {
        narrow: s,
        wide: null_mut(),
        cap: n,
        n: 0,
    };
    if !format(&mut out, fmt, &*t) || !out.put(0) {
        return 0;
    }
    out.n - 1
}
#[no_mangle]
pub unsafe extern "C" fn wcsftime(s: *mut u32, n: usize, fmt: *const u32, t: *const Tm) -> usize {
    // Calendar names/formats in our supported locales are ASCII; preserve all
    // non-format wide literals without transcoding or allocating.
    let mut out = Output {
        narrow: null_mut(),
        wide: s,
        cap: n,
        n: 0,
    };
    let mut i = 0;
    while *fmt.add(i) != 0 {
        if *fmt.add(i) != b'%' as u32 {
            if out.n >= n {
                return 0;
            }
            *s.add(out.n) = *fmt.add(i);
            out.n += 1;
            i += 1;
            continue;
        }
        let mut spec = [0u8; 32];
        let mut k = 0;
        loop {
            let c = *fmt.add(i);
            if c > 127 || c == 0 || k >= 31 {
                return 0;
            }
            spec[k] = c as u8;
            k += 1;
            i += 1;
            if k > 1
                && c != b'E' as u32
                && c != b'O' as u32
                && c != b'_' as u32
                && c != b'-' as u32
                && c != b'+' as u32
                && !(c as u8).is_ascii_digit()
            {
                break;
            }
        }
        if !format(&mut out, spec.as_ptr(), &*t) {
            return 0;
        }
    }
    if !out.put(0) {
        0
    } else {
        out.n - 1
    }
}
#[no_mangle]
pub unsafe extern "C" fn asctime_r(t: *const Tm, s: *mut u8) -> *mut u8 {
    // C17 7.27.3.1 defines asctime only for four-digit years.
    if !(-900..=8099).contains(&(*t).year) {
        set_errno(75);
        return null_mut();
    }
    if strftime(s, 26, b"%a %b %e %H:%M:%S %Y\n\0".as_ptr(), t) != 25 {
        set_errno(75);
        return null_mut();
    }
    s
}
#[no_mangle]
pub unsafe extern "C" fn asctime(t: *const Tm) -> *mut u8 {
    asctime_r(t, addr_of_mut!(TEXT) as *mut u8)
}
#[no_mangle]
pub unsafe extern "C" fn ctime_r(t: *const i64, s: *mut u8) -> *mut u8 {
    let mut tm = Tm::default();
    if localtime_r(t, &mut tm).is_null() {
        null_mut()
    } else {
        asctime_r(&tm, s)
    }
}
#[no_mangle]
pub unsafe extern "C" fn ctime(t: *const i64) -> *mut u8 {
    ctime_r(t, addr_of_mut!(TEXT) as *mut u8)
}
