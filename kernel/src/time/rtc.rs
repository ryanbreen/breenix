use core::sync::atomic::{AtomicU64, Ordering};
#[cfg(target_arch = "x86_64")]
use x86_64::instructions::port::Port;

#[cfg(target_arch = "x86_64")]
const RTC_ADDR_PORT: u16 = 0x70;
#[cfg(target_arch = "x86_64")]
const RTC_DATA_PORT: u16 = 0x71;

#[cfg(target_arch = "x86_64")]
#[path = "cmos.rs"]
mod cmos;

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
struct CachedRtc {
    raw: [u8; 7],
    sampled_tsc: u64,
}

#[cfg(target_arch = "x86_64")]
static CMOS_LOCK: spin::Mutex<Option<CachedRtc>> = spin::Mutex::new(None);

#[cfg(target_arch = "x86_64")]
// Called only with interrupts masked and CMOS_LOCK held for the snapshot.
fn read_rtc_register(reg: u8) -> u8 {
    unsafe {
        let mut addr_port = Port::new(RTC_ADDR_PORT);
        let mut data_port = Port::new(RTC_DATA_PORT);
        addr_port.write(reg);
        data_port.read()
    }
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
fn read_rtc_raw() -> Result<RTCTime, &'static str> {
    read_rtc_raw_with(read_rtc_register)
}

#[cfg(target_arch = "x86_64")]
fn read_rtc_raw_with(mut read: impl FnMut(u8) -> u8) -> Result<RTCTime, &'static str> {
    // UIP can cover a millisecond-scale update. A count bound can expire too
    // soon on physical CMOS, whose port accesses are much faster than KVM's.
    let frequency = super::tsc::frequency_hz();
    let started = super::tsc::read_tsc_serialized();
    let timeout = frequency / 100; // 10 ms stabilization retry window.
    let mut attempts = 0;
    loop {
        attempts += 1;
        let raw = crate::arch_without_interrupts(|| {
            let mut cache = CMOS_LOCK.lock();
            let before = super::tsc::read_tsc_serialized();
            let budget = frequency / 2;
            let raw = match cache.as_ref() {
                Some(cached) if before.wrapping_sub(cached.sampled_tsc) < budget => {
                    cmos::live_time(&mut read).and_then(|(second, minute)| {
                        let age = super::tsc::read_tsc_serialized().wrapping_sub(cached.sampled_tsc);
                        cmos::advance_cached(cached.raw, second, minute, age, budget)
                            .or_else(|| cmos::snapshot(&mut read))
                    })
                }
                _ => cmos::snapshot(&mut read),
            };
            let elapsed = super::tsc::read_tsc_serialized().wrapping_sub(before);
            // A VM pause or NMI lasting a full minute can hide a seconds wrap.
            // A conservative half-second budget also keeps multiple updates
            // out of any accepted snapshot; time is still read from the RTC.
            if budget != 0 && elapsed < budget {
                if let Some(raw) = raw {
                    *cache = Some(CachedRtc { raw, sampled_tsc: before });
                }
                raw
            } else {
                *cache = None;
                None
            }
        });
        if let Some(raw) = raw {
            return Ok(decode_rtc(raw));
        }
        if cmos::retry_exhausted(
            super::tsc::read_tsc_serialized().wrapping_sub(started), timeout, attempts,
        ) {
            return Err("RTC did not stabilize");
        }
        core::hint::spin_loop();
    }
}

#[cfg(target_arch = "x86_64")]
fn decode_rtc(raw: [u8; 7]) -> RTCTime {
    let is_binary = raw[6] & RTC_BINARY_FORMAT != 0;
    let mut time = RTCTime {
        second: raw[0], minute: raw[1], hour: cmos::decode_hour(raw[2], raw[6]),
        day: raw[3], month: raw[4], year: u16::from(raw[5]),
    };

    if !is_binary {
        time.second = bcd_to_binary(time.second);
        time.minute = bcd_to_binary(time.minute);
        time.day = bcd_to_binary(time.day);
        time.month = bcd_to_binary(time.month);
        time.year = bcd_to_binary(time.year as u8) as u16;
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
pub(crate) fn days_in_month(month: u8, year: u16) -> u8 {
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
pub(crate) fn days_in_month(month: u8, year: u16) -> u8 {
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
        let mut day = days_remaining + 1;

        while month <= 12 {
            let days_in_this_month = u64::from(days_in_month(month, year));
            if day <= days_in_this_month {
                break;
            }
            day -= days_in_this_month;
            month += 1;
        }

        DateTime {
            year,
            month,
            day: day as u8,
            hour: hours as u8,
            minute: minutes as u8,
            second: seconds as u8,
        }
    }
}

#[cfg(target_arch = "x86_64")]
pub fn read_rtc_time() -> Result<u64, &'static str> {
    read_rtc_raw().map(|time| rtc_to_unix_timestamp(&time))
}

/// Emit the UTC boot sample on the console, including production ARM64 boots
/// which do not install a logger. Called only during RTC initialization.
fn report_boot_time(timestamp: u64, source: &str) {
    let dt = DateTime::from_unix_timestamp(timestamp);
    crate::serial_println!(
        "[clock] Boot time: {:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
        dt.year, dt.month, dt.day, dt.hour, dt.minute, dt.second,
    );
    crate::serial_println!("[clock] UTC epoch={} source={}", timestamp, source);
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
            report_boot_time(timestamp, "CMOS");
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
    if !crate::platform_config::is_qemu() {
        let dt = super::firmware_rtc::read()?;
        return Ok(dt.to_unix_timestamp());
    }
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
/// On QEMU: reads PL031 RTC directly.
/// On other platforms (Parallels, VMware): uses UEFI GetTime() from the loader.
#[cfg(target_arch = "aarch64")]
pub fn init() {
    if !crate::platform_config::is_qemu() {
        super::firmware_rtc::init();
        // Use boot wall time from UEFI GetTime() (provided by the loader)
        let loader_time = crate::platform_config::boot_wall_time_utc();
        if loader_time != 0 {
            BOOT_WALL_TIME.store(loader_time, Ordering::Relaxed);
            let dt = DateTime::from_unix_timestamp(loader_time);
            log::info!(
                "UEFI RTC initialized: {:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
                dt.year,
                dt.month,
                dt.day,
                dt.hour,
                dt.minute,
                dt.second
            );
            report_boot_time(loader_time, "UEFI");
        } else {
            log::warn!("No RTC available (PL031 absent, UEFI GetTime not provided)");
            BOOT_WALL_TIME.store(0, Ordering::Relaxed);
        }
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
            report_boot_time(timestamp, "PL031");
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

/// MC146818-compatible CMOS: SET inhibits calendar updates, then releasing SET
/// resumes calendar updates. Preserve data format, 12/24-hour mode and NMI masking.
#[cfg(target_arch = "x86_64")]
pub fn write_rtc_time(dt: DateTime) -> Result<(), &'static str> {
    crate::arch_without_interrupts(|| {
        let mut cache = CMOS_LOCK.lock();
        unsafe {
            let mut index: Port<u8> = Port::new(RTC_ADDR_PORT);
            let mut data: Port<u8> = Port::new(RTC_DATA_PORT);
            let saved = index.read();
            index.write((saved & 0x80) | 0x0b);
            let status = data.read();
            data.write(status | 0x80);
            let encode = |n: u8| if status & 4 != 0 { n } else { (n / 10) << 4 | (n % 10) };
            let hour = if status & 2 != 0 { encode(dt.hour) } else {
                encode(if dt.hour % 12 == 0 { 12 } else { dt.hour % 12 })
                    | if dt.hour >= 12 { 0x80 } else { 0 }
            };
            for (reg, value) in [(0, encode(dt.second)), (2, encode(dt.minute)),
                (4, hour), (7, encode(dt.day)), (8, encode(dt.month)), (9, encode((dt.year % 100) as u8))] {
                index.write((saved & 0x80) | reg);
                data.write(value);
            }
            index.write((saved & 0x80) | 0x0b);
            data.write(status);
            index.write(saved);
        }
        *cache = None;
    });
    Ok(())
}

#[cfg(target_arch = "aarch64")]
pub fn write_rtc_time(dt: DateTime) -> Result<(), &'static str> {
    if !crate::platform_config::is_qemu() { return Err("Firmware RTC writing awaits persistent VM catch-up"); }
    use crate::arch_impl::aarch64::constants::HHDM_BASE;
    let addr = (HHDM_BASE + PL031_BASE_PHYS + 0x008) as *mut u32;
    unsafe { core::ptr::write_volatile(addr, dt.to_unix_timestamp() as u32); }
    Ok(())
}
