use core::sync::atomic::{AtomicU64, Ordering};
#[cfg(target_arch = "x86_64")]
use x86_64::instructions::port::Port;

#[cfg(target_arch = "x86_64")]
const RTC_ADDR_PORT: u16 = 0x70;
#[cfg(target_arch = "x86_64")]
const RTC_DATA_PORT: u16 = 0x71;

#[cfg(target_arch = "x86_64")]
const RTC_REG_SECONDS: u8 = 0x00;
#[cfg(target_arch = "x86_64")]
const RTC_REG_MINUTES: u8 = 0x02;
#[cfg(target_arch = "x86_64")]
const RTC_REG_HOURS: u8 = 0x04;
#[cfg(target_arch = "x86_64")]
const RTC_REG_DAY: u8 = 0x07;
#[cfg(target_arch = "x86_64")]
const RTC_REG_MONTH: u8 = 0x08;
#[cfg(target_arch = "x86_64")]
const RTC_REG_YEAR: u8 = 0x09;
#[cfg(target_arch = "x86_64")]
const RTC_REG_STATUS_A: u8 = 0x0A;
#[cfg(target_arch = "x86_64")]
const RTC_REG_STATUS_B: u8 = 0x0B;

#[cfg(target_arch = "x86_64")]
const RTC_UPDATE_IN_PROGRESS: u8 = 0x80;
#[cfg(target_arch = "x86_64")]
const RTC_24HOUR_FORMAT: u8 = 0x02;
#[cfg(target_arch = "x86_64")]
const RTC_BINARY_FORMAT: u8 = 0x04;

/// Unix timestamp at boot time
static BOOT_WALL_TIME: AtomicU64 = AtomicU64::new(0);

/// Human-readable date and time
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

#[derive(Debug, Clone, Copy)]
struct RTCTime {
    second: u8,
    minute: u8,
    hour: u8,
    day: u8,
    month: u8,
    year: u16,
}

#[cfg(target_arch = "x86_64")]
fn read_rtc_register(reg: u8) -> u8 {
    unsafe {
        let mut addr_port = Port::new(RTC_ADDR_PORT);
        let mut data_port = Port::new(RTC_DATA_PORT);

        addr_port.write(reg);
        data_port.read()
    }
}

#[cfg(target_arch = "x86_64")]
fn rtc_update_in_progress() -> bool {
    read_rtc_register(RTC_REG_STATUS_A) & RTC_UPDATE_IN_PROGRESS != 0
}

#[cfg(test)]
pub(super) fn bcd_to_binary(value: u8) -> u8 {
    ((value & 0xF0) >> 4) * 10 + (value & 0x0F)
}

#[cfg(all(not(test), target_arch = "x86_64"))]
fn bcd_to_binary(value: u8) -> u8 {
    ((value & 0xF0) >> 4) * 10 + (value & 0x0F)
}

#[cfg(target_arch = "x86_64")]
fn read_rtc_raw() -> RTCTime {
    while rtc_update_in_progress() {
        core::hint::spin_loop();
    }

    let status_b = read_rtc_register(RTC_REG_STATUS_B);
    let is_binary = status_b & RTC_BINARY_FORMAT != 0;
    let is_24hour = status_b & RTC_24HOUR_FORMAT != 0;

    let mut time = RTCTime {
        second: read_rtc_register(RTC_REG_SECONDS),
        minute: read_rtc_register(RTC_REG_MINUTES),
        hour: read_rtc_register(RTC_REG_HOURS),
        day: read_rtc_register(RTC_REG_DAY),
        month: read_rtc_register(RTC_REG_MONTH),
        year: read_rtc_register(RTC_REG_YEAR) as u16,
    };

    if !is_binary {
        time.second = bcd_to_binary(time.second);
        time.minute = bcd_to_binary(time.minute);
        time.hour = bcd_to_binary(time.hour & 0x7F);
        time.day = bcd_to_binary(time.day);
        time.month = bcd_to_binary(time.month);
        time.year = bcd_to_binary(time.year as u8) as u16;
    }

    if !is_24hour && (time.hour & 0x80) != 0 {
        time.hour = ((time.hour & 0x7F) + 12) % 24;
    }

    time.year += 2000;

    time
}

#[cfg(test)]
pub(super) fn is_leap_year(year: u16) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

#[cfg(not(test))]
fn is_leap_year(year: u16) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

#[cfg(test)]
pub(super) fn days_in_month(month: u8, year: u16) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

#[cfg(not(test))]
fn days_in_month(month: u8, year: u16) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

fn rtc_to_unix_timestamp(rtc: &RTCTime) -> u64 {
    let mut days: u64 = 0;

    for year in 1970..rtc.year {
        days += if is_leap_year(year) { 366 } else { 365 };
    }

    for month in 1..rtc.month {
        days += days_in_month(month, rtc.year) as u64;
    }

    days += (rtc.day - 1) as u64;

    let hours = days * 24 + rtc.hour as u64;
    let minutes = hours * 60 + rtc.minute as u64;
    let seconds = minutes * 60 + rtc.second as u64;

    seconds
}

impl DateTime {
    /// Convert DateTime to Unix timestamp
    #[allow(dead_code)] // Public API used by tests and external code
    pub fn to_unix_timestamp(&self) -> u64 {
        let rtc = RTCTime {
            second: self.second,
            minute: self.minute,
            hour: self.hour,
            day: self.day,
            month: self.month,
            year: self.year,
        };
        rtc_to_unix_timestamp(&rtc)
    }

    /// Create DateTime from Unix timestamp
    pub fn from_unix_timestamp(timestamp: u64) -> Self {
        let seconds = timestamp % 60;
        let total_minutes = timestamp / 60;
        let minutes = total_minutes % 60;
        let total_hours = total_minutes / 60;
        let hours = total_hours % 24;
        let total_days = total_hours / 24;

        // Start from Unix epoch (1970-01-01)
        let mut year = 1970;
        let mut days_remaining = total_days;

        // Calculate year
        loop {
            let days_in_year = if is_leap_year(year) { 366 } else { 365 };
            if days_remaining < days_in_year {
                break;
            }
            days_remaining -= days_in_year;
            year += 1;
        }

        // Calculate month and day
        let mut month = 1;
        let mut day = days_remaining as u8 + 1;

        while month <= 12 {
            let days_in_this_month = days_in_month(month, year);
            if day <= days_in_this_month {
                break;
            }
            day -= days_in_this_month;
            month += 1;
        }

        DateTime {
            year,
            month,
            day,
            hour: hours as u8,
            minute: minutes as u8,
            second: seconds as u8,
        }
    }
}

#[allow(dead_code)]
fn rtc_time_to_datetime(rtc: &RTCTime) -> DateTime {
    DateTime {
        year: rtc.year,
        month: rtc.month,
        day: rtc.day,
        hour: rtc.hour,
        minute: rtc.minute,
        second: rtc.second,
    }
}

#[cfg(target_arch = "x86_64")]
pub fn read_rtc_time() -> Result<u64, &'static str> {
    let time1 = read_rtc_raw();
    let time2 = read_rtc_raw();

    if time1.second != time2.second
        || time1.minute != time2.minute
        || time1.hour != time2.hour
        || time1.day != time2.day
        || time1.month != time2.month
        || time1.year != time2.year
    {
        return Err("RTC time changed during read");
    }

    Ok(rtc_to_unix_timestamp(&time1))
}

/// Read the current date and time from the RTC
#[cfg(target_arch = "x86_64")]
#[allow(dead_code)]
pub fn read_datetime() -> DateTime {
    let time = read_rtc_raw();
    rtc_time_to_datetime(&time)
}

/// Initialize RTC and cache boot time
#[cfg(target_arch = "x86_64")]
pub fn init() {
    match read_rtc_time() {
        Ok(timestamp) => {
            BOOT_WALL_TIME.store(timestamp, Ordering::Relaxed);
            let dt = DateTime::from_unix_timestamp(timestamp);
            log::info!(
                "RTC initialized: {:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
                dt.year,
                dt.month,
                dt.day,
                dt.hour,
                dt.minute,
                dt.second
            );
        }
        Err(e) => {
            log::error!("Failed to initialize RTC: {}", e);
            // Store a default time if RTC read fails
            BOOT_WALL_TIME.store(0, Ordering::Relaxed);
        }
    }
}

// ---------------------------------------------------------------------------
// ARM64: PL031 Real-Time Clock (QEMU virt machine)
// ---------------------------------------------------------------------------
//
// The PL031 is a simple memory-mapped RTC on QEMU's ARM64 virt machine.
// Physical address: 0x0901_0000 (standard QEMU virt layout).
// The Data Register (offset 0x000) returns the current Unix timestamp
// as a 32-bit value, synchronized to the host system clock.
// ---------------------------------------------------------------------------

/// PL031 RTC physical base address on QEMU virt machine.
#[cfg(target_arch = "aarch64")]
const PL031_BASE_PHYS: u64 = 0x0901_0000;

/// PL031 Data Register offset (read-only, returns Unix timestamp).
#[cfg(target_arch = "aarch64")]
const PL031_DR: usize = 0x000;

/// Read the PL031 Data Register via the higher-half direct map.
#[cfg(target_arch = "aarch64")]
fn pl031_read(offset: usize) -> u32 {
    use crate::arch_impl::aarch64::constants::HHDM_BASE;
    let addr = (HHDM_BASE as usize + PL031_BASE_PHYS as usize + offset) as *const u32;
    unsafe { core::ptr::read_volatile(addr) }
}

#[cfg(target_arch = "aarch64")]
pub fn read_rtc_time() -> Result<u64, &'static str> {
    let timestamp = pl031_read(PL031_DR) as u64;
    if timestamp == 0 {
        return Err("PL031 RTC returned 0");
    }
    Ok(timestamp)
}

/// Read the current date and time from the RTC
#[cfg(target_arch = "aarch64")]
#[allow(dead_code)]
pub fn read_datetime() -> DateTime {
    match read_rtc_time() {
        Ok(ts) => DateTime::from_unix_timestamp(ts),
        Err(_) => DateTime::from_unix_timestamp(0),
    }
}

/// Initialize RTC and cache boot time.
/// PL031 is only present on QEMU virt; skip on other platforms.
#[cfg(target_arch = "aarch64")]
pub fn init() {
    if !crate::platform_config::is_qemu() {
        log::info!("PL031 RTC not available on this platform, skipping");
        BOOT_WALL_TIME.store(0, Ordering::Relaxed);
        return;
    }
    match read_rtc_time() {
        Ok(timestamp) => {
            BOOT_WALL_TIME.store(timestamp, Ordering::Relaxed);
            let dt = DateTime::from_unix_timestamp(timestamp);
            log::info!(
                "PL031 RTC initialized: {:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
                dt.year,
                dt.month,
                dt.day,
                dt.hour,
                dt.minute,
                dt.second
            );
        }
        Err(e) => {
            log::error!("Failed to initialize PL031 RTC: {}", e);
            BOOT_WALL_TIME.store(0, Ordering::Relaxed);
        }
    }
}

/// Fallback for other non-x86, non-aarch64 architectures
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
pub fn read_rtc_time() -> Result<u64, &'static str> {
    Err("RTC not supported on this architecture")
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
#[allow(dead_code)]
pub fn read_datetime() -> DateTime {
    DateTime::from_unix_timestamp(0)
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
pub fn init() {
    BOOT_WALL_TIME.store(0, Ordering::Relaxed);
}

/// Get the cached boot wall time
pub fn get_boot_wall_time() -> u64 {
    BOOT_WALL_TIME.load(Ordering::Relaxed)
}
