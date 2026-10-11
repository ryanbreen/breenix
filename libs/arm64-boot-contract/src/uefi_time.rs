//! UEFI calendar timestamps normalized to the Unix UTC epoch.
//!
//! UEFI 2.10, section 8.3.1: Localtime = UTC - TimeZone. Firmware must
//! decrease TimeZone when entering daylight time, so Daylight is metadata,
//! not another hour to subtract. An unspecified zone carries no conversion;
//! Breenix launchers must supply UTC for such RTCs (also for CMOS and PL031).

pub fn utc_epoch(local_epoch: u64, timezone: i16, daylight: u8) -> Result<u64, &'static str> {
    if daylight & !3 != 0 {
        return Err("Invalid UEFI daylight flags");
    }
    let offset_minutes = match timezone {
        2047 => 0,
        -1440..=1440 => i64::from(timezone),
        _ => return Err("Invalid UEFI time zone"),
    };
    let local = i64::try_from(local_epoch).map_err(|_| "UEFI time overflow")?;
    let utc = local
        .checked_add(offset_minutes * 60)
        .ok_or("UEFI time overflow")?;
    u64::try_from(utc).map_err(|_| "UEFI time precedes Unix epoch")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zones_cross_calendar_boundaries_in_both_directions() {
        // 2000-01-01 00:00:00; include fractional-hour and extreme zones.
        let local = 946684800;
        for zone in [-1440, -345, 0, 330, 480, 1440] {
            assert_eq!(
                utc_epoch(local, zone, 0),
                Ok((local as i64 + i64::from(zone) * 60) as u64)
            );
        }
    }

    #[test]
    fn daylight_is_already_in_the_zone() {
        let local = 946684800;
        assert_eq!(utc_epoch(local, 480, 1), Ok(local + 480 * 60));
        assert_eq!(utc_epoch(local, 420, 3), Ok(local + 420 * 60));
        assert_eq!(utc_epoch(local, 420, 2), Ok(local + 420 * 60));
    }

    #[test]
    fn unspecified_zone_uses_the_launchers_utc_contract() {
        assert_eq!(utc_epoch(946684800, 2047, 0), Ok(946684800));
    }

    #[test]
    fn reject_invalid_metadata_and_unrepresentable_epochs() {
        for zone in [-1441, 1441, 2046, 2048] {
            assert!(utc_epoch(946684800, zone, 0).is_err());
        }
        assert!(utc_epoch(946684800, 0, 4).is_err());
        assert!(utc_epoch(0, -1, 0).is_err());
        assert!(utc_epoch(u64::MAX, 0, 0).is_err());
        assert!(utc_epoch(i64::MAX as u64, 1, 0).is_err());
    }
}
