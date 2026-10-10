//! A CMOS calendar snapshot guarded against the RTC's asynchronous update.

const SECONDS: u8 = 0x00;
const STATUS_A: u8 = 0x0a;
const STATUS_B: u8 = 0x0b;
const UIP: u8 = 0x80;

/// Read seconds before UIP so an update completed during a pause is visible.
/// Check UIP and seconds again after the calendar: UIP catches an ongoing
/// update, and a changed second catches an update that has already finished.
/// The caller serializes the CMOS index/data pair and rejects long pauses.
/// Returns [seconds, minutes, hours, day, month, year, status B].
pub(crate) fn snapshot(mut read: impl FnMut(u8) -> u8) -> Option<[u8; 7]> {
    let second = read(SECONDS);
    if read(STATUS_A) & UIP != 0 {
        return None;
    }
    let time = [
        second,
        read(0x02),
        read(0x04),
        read(0x07),
        read(0x08),
        read(0x09),
        read(STATUS_B),
    ];
    if read(STATUS_A) & UIP != 0 || read(SECONDS) != second {
        return None;
    }
    Some(time)
}

/// A live seconds sample. Reading seconds on both sides of UIP rejects an
/// ongoing update as well as one completed while the caller was paused.
pub(crate) fn seconds(mut read: impl FnMut(u8) -> u8) -> Option<u8> {
    let second = read(SECONDS);
    if read(STATUS_A) & UIP != 0 || read(SECONDS) != second {
        return None;
    }
    Some(second)
}

/// Reuse calendar fields only within the same RTC minute and for a short
/// observation gap. Seconds are always read from CMOS, never extrapolated
/// from the processor counter. A minute/date rollover or a long pause needs
/// a full guarded snapshot. RTC writes must invalidate the driver's cache.
pub(crate) fn advance_cached(
    mut raw: [u8; 7],
    second: u8,
    age: u64,
    budget: u64,
) -> Option<[u8; 7]> {
    if budget == 0 || age >= budget {
        return None;
    }
    let decode = |value: u8| {
        if raw[6] & 0x04 != 0 {
            value
        } else {
            (value >> 4) * 10 + (value & 0x0f)
        }
    };
    let previous = decode(raw[0]);
    let current = decode(second);
    if current >= 60 || current < previous || current - previous > 1 {
        return None;
    }
    raw[0] = second;
    Some(raw)
}

/// Preserve the PM bit before BCD conversion; 12 AM is zero, 12 PM is twelve.
pub(crate) fn decode_hour(raw: u8, status_b: u8) -> u8 {
    let pm = raw & 0x80 != 0;
    let hour = raw & 0x7f;
    let hour = if status_b & 0x04 != 0 {
        hour
    } else {
        (hour >> 4) * 10 + (hour & 0x0f)
    };
    if status_b & 0x02 != 0 {
        hour
    } else {
        hour % 12 + if pm { 12 } else { 0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OLD: [u8; 7] = [0x59, 0x59, 0x23, 0x31, 0x12, 0x26, 0x02];
    const NEW: [u8; 7] = [0x00, 0x00, 0x00, 0x01, 0x01, 0x27, 0x02];

    fn register(time: [u8; 7], reg: u8) -> u8 {
        match reg {
            0x00 => time[0],
            0x02 => time[1],
            0x04 => time[2],
            0x07 => time[3],
            0x08 => time[4],
            0x09 => time[5],
            STATUS_B => time[6],
            STATUS_A => 0,
            _ => panic!("unexpected CMOS register"),
        }
    }

    #[test]
    fn cached_calendar_tracks_live_seconds_but_never_crosses_a_minute() {
        let mut raw = OLD;
        raw[0] = 0x09;
        let next = advance_cached(raw, 0x10, 1, 10).unwrap();
        assert_eq!(next[0], 0x10);
        assert_eq!(next[1..], raw[1..]);
        assert_eq!(advance_cached(OLD, NEW[0], 1, 10), None);
        assert_eq!(advance_cached(raw, 0x08, 1, 10), None);
        assert_eq!(advance_cached(raw, 0x11, 1, 10), None);
        assert_eq!(advance_cached(raw, raw[0], 10, 10), None);
        assert_eq!(advance_cached(raw, raw[0], 1, 0), None);
        raw[6] |= 0x04;
        raw[0] = 9;
        assert_eq!(advance_cached(raw, 10, 1, 10).unwrap()[0], 10);
        raw[0] = 59;
        assert_eq!(advance_cached(raw, 0, 1, 10), None);
    }

    #[test]
    fn live_seconds_reject_uip_and_completed_updates() {
        assert_eq!(seconds(|reg| register(OLD, reg)), Some(OLD[0]));
        assert_eq!(
            seconds(|reg| if reg == STATUS_A { UIP } else { OLD[0] }),
            None
        );
        let mut accesses = 0;
        assert_eq!(
            seconds(|reg| {
                accesses += 1;
                register(if accesses < 3 { OLD } else { NEW }, reg)
            }),
            None
        );
    }

    #[test]
    fn decodes_midnight_noon_and_pm_in_bcd_and_binary_modes() {
        for binary in [false, true] {
            let status = if binary { 0x04 } else { 0 };
            let twelve = if binary { 12 } else { 0x12 };
            assert_eq!(decode_hour(twelve, status), 0);
            assert_eq!(decode_hour(twelve | 0x80, status), 12);
            assert_eq!(decode_hour(0x81, status), 13);
            assert_eq!(
                decode_hour(if binary { 23 } else { 0x23 }, status | 0x02),
                23
            );
        }
    }

    #[test]
    fn reads_one_stable_calendar() {
        let mut accesses = 0;
        assert_eq!(
            snapshot(|reg| {
                accesses += 1;
                register(OLD, reg)
            }),
            Some(OLD)
        );
        assert_eq!(accesses, 10);
    }

    #[test]
    fn rejects_an_update_already_in_progress() {
        assert_eq!(
            snapshot(|reg| if reg == STATUS_A {
                UIP
            } else {
                register(OLD, reg)
            }),
            None
        );
    }

    #[test]
    fn rejects_an_update_still_in_progress_after_calendar_reads() {
        let mut statuses = 0;
        assert_eq!(
            snapshot(|reg| {
                if reg == STATUS_A {
                    statuses += 1;
                    if statuses == 2 {
                        UIP
                    } else {
                        0
                    }
                } else {
                    register(OLD, reg)
                }
            }),
            None
        );
    }

    #[test]
    fn completed_rollover_at_every_read_boundary_never_returns_a_torn_date() {
        // Even if UIP was entirely missed, a completed update at any port
        // boundary can return only the old date, the new date, or a retry.
        for boundary in 0..=10 {
            let mut accesses = 0;
            let result = snapshot(|reg| {
                let time = if accesses < boundary { OLD } else { NEW };
                accesses += 1;
                register(time, reg)
            });
            assert!(
                result.is_none() || result == Some(OLD) || result == Some(NEW),
                "boundary {boundary}: {result:?}"
            );
        }
    }
}
