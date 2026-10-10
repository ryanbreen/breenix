//! Physical-mode UEFI GetTime for machines without a memory-mapped RTC.
//! Only firmware runtime regions are mapped, under a private TTBR0. Kernel
//! code and stacks keep their TTBR1 mappings; no firmware page is user accessible.
use alloc::vec::Vec;
use arm64_boot_contract::{RuntimeRegion, MAX_RUNTIME_REGIONS};
use crate::memory::frame_allocator::{allocate_frame_leased, FrameLease};
use super::rtc::DateTime;

struct Config {
    get_time: u64,
    count: usize,
    regions: [RuntimeRegion; MAX_RUNTIME_REGIONS],
}
static CONFIG: spin::Mutex<Config> = spin::Mutex::new(Config {
    get_time: 0, count: 0,
    regions: [RuntimeRegion { base: 0, pages: 0, kind: 0, _pad: 0 }; MAX_RUNTIME_REGIONS],
});
struct Clock {
    get_time: u64,
    // Own every table and the firmware output buffer for the kernel lifetime.
    tables: Vec<FrameLease>,
    scratch: FrameLease,
}
static CLOCK: spin::Mutex<Option<Clock>> = spin::Mutex::new(None);

pub fn configure(get_time: u64, regions: &[RuntimeRegion]) {
    let mut config = CONFIG.lock();
    config.get_time = get_time;
    config.count = regions.len();
    config.regions[..regions.len()].copy_from_slice(regions);
}

fn table(phys: u64) -> *mut u64 {
    (crate::memory::physical_memory_offset().as_u64() + phys) as *mut u64
}
fn new_table(tables: &mut Vec<FrameLease>) -> Result<u64, &'static str> {
    let lease = allocate_frame_leased().ok_or("No memory for firmware RTC map")?;
    let phys = lease.frame().start_address().as_u64();
    unsafe { core::ptr::write_bytes(table(phys), 0, 512); }
    tables.push(lease);
    Ok(phys)
}
fn map(tables: &mut Vec<FrameLease>, phys: u64, flags: u64) -> Result<(), &'static str> {
    let mut root = tables[0].frame().start_address().as_u64();
    for shift in [39, 30, 21] {
        let slot = unsafe { table(root).add(((phys >> shift) & 511) as usize) };
        let mut descriptor = unsafe { slot.read() };
        if descriptor == 0 {
            descriptor = new_table(tables)? | 3;
            unsafe { slot.write(descriptor); }
        }
        root = descriptor & 0x0000_ffff_ffff_f000;
    }
    // nG confines these translations to the temporary ASID-0 context; UXN
    // and EL1-only access hold even for firmware executable pages.
    let descriptor = phys | flags | 3 | (1 << 10) | (1 << 11) | (1 << 54);
    unsafe { table(root).add(((phys >> 12) & 511) as usize).write(descriptor); }
    Ok(())
}
fn build(config: &Config) -> Result<Clock, &'static str> {
    if config.get_time == 0 || !config.regions[..config.count].iter().any(|r| {
        r.kind == 5 && config.get_time >= r.base && config.get_time < r.base.saturating_add(r.pages.saturating_mul(4096))
    }) { return Err("No firmware runtime GetTime"); }
    let mut tables = Vec::new();
    new_table(&mut tables)?;
    for region in &config.regions[..config.count] {
        // UEFI types: runtime code=5, runtime data=6, MMIO=11, MMIO port=12.
        // Preserve code RX, data RW/NX and device RW/NX.
        let flags = match region.kind {
            5 => (1 << 2) | (3 << 8) | (2 << 6),
            6 => (1 << 2) | (3 << 8) | (1 << 53),
            11 | 12 => (2 << 8) | (1 << 53),
            _ => return Err("Unsupported firmware runtime memory type"),
        };
        for page in 0..region.pages {
            map(&mut tables, region.base + page * 4096, flags)?;
        }
    }
    let scratch = allocate_frame_leased().ok_or("No firmware RTC buffer")?;
    map(&mut tables, scratch.frame().start_address().as_u64(), (1 << 2) | (3 << 8) | (1 << 53))?;
    Ok(Clock { get_time: config.get_time, tables, scratch })
}
pub fn init() {
    match build(&CONFIG.lock()) {
        Ok(clock) => *CLOCK.lock() = Some(clock),
        Err(why) => log::warn!("Firmware RTC unavailable: {}", why),
    }
}
pub fn available() -> bool { CLOCK.lock().is_some() }

#[repr(C)]
struct EfiTime {
    year: u16, month: u8, day: u8, hour: u8, minute: u8, second: u8,
    pad1: u8, nanosecond: u32, timezone: i16, daylight: u8, pad2: u8,
}
pub fn read() -> Result<DateTime, &'static str> {
    // Firmware is non-reentrant. Mask before taking its lock so a context
    // switch cannot strand the holder, and restore exactly the caller's root.
    crate::arch_without_interrupts(|| {
        let guard = CLOCK.lock();
        let clock = guard.as_ref().ok_or("No firmware RTC")?;
        let root = clock.tables[0].frame().start_address().as_u64();
        let buffer = clock.scratch.frame().start_address().as_u64();
        use crate::arch_impl::aarch64::{fpsimd, percpu::Aarch64PerCpu};
        let saved: u64;
        unsafe { core::arch::asm!("mrs {saved}, ttbr0_el1", saved = out(reg) saved, options(nostack)); }
        let saved_shadow = Aarch64PerCpu::saved_process_cr3();
        let next_shadow = Aarch64PerCpu::next_cr3();
        let mut fp = crate::signal::types::FpsimdContext {
            magic: crate::signal::types::FpsimdContext::MAGIC,
            size: crate::signal::types::FpsimdContext::SIZE,
            fpsr: 0, fpcr: 0, vregs: [0; 32],
        };
        fpsimd::save(&mut fp);
        install_firmware_root(root);
        let get_time: unsafe extern "efiapi" fn(*mut EfiTime, *mut u8) -> usize =
            unsafe { core::mem::transmute(clock.get_time as usize) };
        let status = unsafe { get_time(buffer as *mut EfiTime, core::ptr::null_mut()) };
        install_firmware_root(saved);
        unsafe {
            Aarch64PerCpu::set_saved_process_cr3(saved_shadow);
            Aarch64PerCpu::set_next_cr3(next_shadow);
        }
        fpsimd::restore(&fp);
        if status != 0 { return Err("Firmware GetTime failed"); }
        let time = unsafe { &*(table(buffer) as *const EfiTime) };
        if time.year < 1970 || time.year > 9999 || !(1..=12).contains(&time.month)
            || time.day == 0 || time.day > super::rtc::days_in_month(time.month, time.year)
            || time.hour > 23 || time.minute > 59 || time.second > 59 {
            return Err("Firmware GetTime returned an invalid date");
        }
        Ok(DateTime { year: time.year, month: time.month, day: time.day,
            hour: time.hour, minute: time.minute, second: time.second })
    })
}

/// Install a temporary firmware context with the same barriers and shadow
/// reconciliation as process-root installation. Preserve the supplied ASID:
/// this also restores the exact kernel or user context after GetTime.
fn install_firmware_root(root: u64) {
    unsafe {
        core::arch::asm!(
            "dsb ishst", "msr ttbr0_el1, {root}", "isb",
            "tlbi vmalle1is", "dsb ish", "isb",
            root = in(reg) root, options(nostack),
        );
        crate::arch_impl::aarch64::percpu::Aarch64PerCpu::set_saved_process_cr3(root);
        crate::arch_impl::aarch64::percpu::Aarch64PerCpu::set_next_cr3(0);
    }
}
