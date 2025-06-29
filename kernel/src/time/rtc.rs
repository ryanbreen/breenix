use x86_64::instructions::port::Port;

const RTC_ADDR_PORT: u16 = 0x70;
const RTC_DATA_PORT: u16 = 0x71;

const RTC_REG_SECONDS: u8 = 0x00;
const RTC_REG_MINUTES: u8 = 0x02;
const RTC_REG_HOURS: u8 = 0x04;
const RTC_REG_DAY: u8 = 0x07;
const RTC_REG_MONTH: u8 = 0x08;
const RTC_REG_YEAR: u8 = 0x09;
const RTC_REG_STATUS_A: u8 = 0x0A;
const RTC_REG_STATUS_B: u8 = 0x0B;

const RTC_UPDATE_IN_PROGRESS: u8 = 0x80;
const RTC_24HOUR_FORMAT: u8 = 0x02;
const RTC_BINARY_FORMAT: u8 = 0x04;

#[derive(Debug, Clone, Copy)]
struct RTCTime {
    second: u8,
    minute: u8,
    hour: u8,
    day: u8,
    month: u8,
    year: u16,
}

fn read_rtc_register(reg: u8) -> u8 {
    unsafe {
        let mut addr_port = Port::new(RTC_ADDR_PORT);
        let mut data_port = Port::new(RTC_DATA_PORT);
        
        addr_port.write(reg);
        data_port.read()
    }
}

fn rtc_update_in_progress() -> bool {
    read_rtc_register(RTC_REG_STATUS_A) & RTC_UPDATE_IN_PROGRESS != 0
}

fn bcd_to_binary(value: u8) -> u8 {
    ((value & 0xF0) >> 4) * 10 + (value & 0x0F)
}

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

fn is_leap_year(year: u16) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

fn days_in_month(month: u8, year: u16) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => if is_leap_year(year) { 29 } else { 28 },
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

pub fn read_rtc_time() -> Result<u64, &'static str> {
    let time1 = read_rtc_raw();
    let time2 = read_rtc_raw();
    
    if time1.second != time2.second || 
       time1.minute != time2.minute ||
       time1.hour != time2.hour ||
       time1.day != time2.day ||
       time1.month != time2.month ||
       time1.year != time2.year {
        return Err("RTC time changed during read");
    }
    
    Ok(rtc_to_unix_timestamp(&time1))
}