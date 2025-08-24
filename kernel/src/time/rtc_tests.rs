//! Unit tests for RTC functionality

use super::rtc::*;

#[test]
fn test_bcd_to_binary() {
        assert_eq!(bcd_to_binary(0x00), 0);
        assert_eq!(bcd_to_binary(0x59), 59);
        assert_eq!(bcd_to_binary(0x12), 12);
        assert_eq!(bcd_to_binary(0x99), 99);
        assert_eq!(bcd_to_binary(0x47), 47);
    }

    #[test]
    fn test_leap_year() {
        assert!(is_leap_year(2020));  // Divisible by 4
        assert!(is_leap_year(2000));  // Divisible by 400
        assert!(!is_leap_year(1900)); // Divisible by 100 but not 400
        assert!(!is_leap_year(2021)); // Not divisible by 4
        assert!(!is_leap_year(2022)); // Not divisible by 4
        assert!(!is_leap_year(2023)); // Not divisible by 4
        assert!(is_leap_year(2024));  // Divisible by 4
    }

    #[test]
    fn test_days_in_month() {
        // Regular year
        assert_eq!(days_in_month(1, 2021), 31); // January
        assert_eq!(days_in_month(2, 2021), 28); // February
        assert_eq!(days_in_month(3, 2021), 31); // March
        assert_eq!(days_in_month(4, 2021), 30); // April
        assert_eq!(days_in_month(5, 2021), 31); // May
        assert_eq!(days_in_month(6, 2021), 30); // June
        assert_eq!(days_in_month(7, 2021), 31); // July
        assert_eq!(days_in_month(8, 2021), 31); // August
        assert_eq!(days_in_month(9, 2021), 30); // September
        assert_eq!(days_in_month(10, 2021), 31); // October
        assert_eq!(days_in_month(11, 2021), 30); // November
        assert_eq!(days_in_month(12, 2021), 31); // December
        
        // Leap year February
        assert_eq!(days_in_month(2, 2020), 29);
    }

    #[test]
    fn test_datetime_unix_conversion() {
        // Test Unix epoch
        let epoch = DateTime {
            year: 1970,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
        };
        assert_eq!(epoch.to_unix_timestamp(), 0);
        
        // Test a known timestamp
        let dt = DateTime {
            year: 2025,
            month: 1,
            day: 21,
            hour: 12,
            minute: 0,
            second: 0,
        };
        let timestamp = dt.to_unix_timestamp();
        let converted_back = DateTime::from_unix_timestamp(timestamp);
        assert_eq!(dt, converted_back);
        
        // Test Y2K
        let y2k = DateTime {
            year: 2000,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
        };
        let y2k_timestamp = y2k.to_unix_timestamp();
        assert_eq!(y2k_timestamp, 946684800); // Known Y2K timestamp
}