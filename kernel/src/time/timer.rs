use core::sync::atomic::{AtomicU64, Ordering};
use spin::Once;
use x86_64::instructions::port::Port;
use super::Time;

const PIT_FREQUENCY: u32 = 1193182;
pub const TIMER_INTERRUPT_HZ: u32 = 1000;
const TIMER_DIVIDER: u16 = (PIT_FREQUENCY / TIMER_INTERRUPT_HZ) as u16;
pub const SUBTICKS_PER_TICK: u64 = 1000;

const PIT_CHANNEL0_DATA: u16 = 0x40;
const PIT_COMMAND: u16 = 0x43;

const PIT_CMD_CHANNEL0: u8 = 0x00;
const PIT_CMD_ACCESS_LOHI: u8 = 0x30;
const PIT_CMD_MODE2: u8 = 0x04;

pub struct Timer {
    start: AtomicU64,
    ticks: AtomicU64,
    seconds: AtomicU64,
    millis: AtomicU64,
}

impl Timer {
    const fn new() -> Self {
        Self {
            start: AtomicU64::new(0),
            ticks: AtomicU64::new(0),
            seconds: AtomicU64::new(0),
            millis: AtomicU64::new(0),
        }
    }

    pub fn time_since_start(&self) -> Time {
        let seconds = self.seconds.load(Ordering::Relaxed);
        let millis = self.millis.load(Ordering::Relaxed);
        Time::new(seconds, millis, 0)
    }

    pub fn monotonic_clock(&self) -> u64 {
        self.ticks.load(Ordering::Relaxed)
    }

    pub fn real_time(&self) -> u64 {
        let start = self.start.load(Ordering::Relaxed);
        let elapsed = self.time_since_start();
        start + elapsed.seconds
    }

    pub fn tick(&self) {
        self.ticks.fetch_add(1, Ordering::Relaxed);
        
        let new_millis = self.millis.fetch_add(1, Ordering::Relaxed) + 1;
        if new_millis >= 1000 {
            self.millis.store(0, Ordering::Relaxed);
            self.seconds.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn set_start_time(&self, unix_timestamp: u64) {
        self.start.store(unix_timestamp, Ordering::Relaxed);
    }
}

static TIMER: Once<Timer> = Once::new();

pub fn init() {
    TIMER.call_once(|| {
        let timer = Timer::new();
        
        unsafe {
            let mut cmd_port = Port::new(PIT_COMMAND);
            let mut data_port = Port::new(PIT_CHANNEL0_DATA);
            
            let command = PIT_CMD_CHANNEL0 | PIT_CMD_ACCESS_LOHI | PIT_CMD_MODE2;
            cmd_port.write(command);
            
            data_port.write((TIMER_DIVIDER & 0xFF) as u8);
            data_port.write((TIMER_DIVIDER >> 8) as u8);
        }
        
        if let Ok(rtc_time) = super::rtc::read_rtc_time() {
            timer.set_start_time(rtc_time);
            log::info!("Timer initialized with RTC time: {}", rtc_time);
        } else {
            log::warn!("Failed to read RTC time, timer starting from epoch");
        }
        
        timer
    });
}

pub fn timer_interrupt() {
    if let Some(timer) = TIMER.get() {
        timer.tick();
        super::increment_ticks();
        
        // Log every 1000 ticks (1 second)
        let ticks = timer.ticks.load(Ordering::Relaxed);
        if ticks % 1000 == 0 {
            log::info!("Timer interrupt: {} ticks ({}s)", ticks, ticks / 1000);
        }
    }
}

pub fn time_since_start() -> Time {
    TIMER.get()
        .map(|t| t.time_since_start())
        .unwrap_or_else(|| Time::new(0, 0, 0))
}

pub fn monotonic_clock() -> u64 {
    TIMER.get()
        .map(|t| t.monotonic_clock())
        .unwrap_or(0)
}

pub fn real_time() -> u64 {
    TIMER.get()
        .map(|t| t.real_time())
        .unwrap_or(0)
}