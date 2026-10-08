//! Local APIC access on the executing CPU. Only the BSP is started here.
use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use x86_64::registers::model_specific::Msr;

pub const SPURIOUS_VECTOR: u8 = 0xff;
const MASKED: u32 = 1 << 16;
static MODE: AtomicU8 = AtomicU8::new(0);
static MMIO: AtomicUsize = AtomicUsize::new(0);

pub fn active() -> bool {
    MODE.load(Ordering::Acquire) != 0
}

#[inline]
fn read(register: u32) -> u32 {
    unsafe {
        if MODE.load(Ordering::Relaxed) == 2 {
            Msr::new(0x800 + register / 16).read() as u32
        } else {
            core::ptr::read_volatile(
                (MMIO.load(Ordering::Relaxed) + register as usize) as *const u32,
            )
        }
    }
}

#[inline]
fn write(register: u32, value: u32) {
    unsafe {
        if MODE.load(Ordering::Relaxed) == 2 {
            Msr::new(0x800 + register / 16).write(u64::from(value));
        } else {
            core::ptr::write_volatile(
                (MMIO.load(Ordering::Relaxed) + register as usize) as *mut u32,
                value,
            );
        }
    }
}

/// Enable the BSP, masking firmware LVT sources before accepting interrupts.
/// CPUID's x2APIC bit includes the hypervisor's advertised capability.
pub fn init(address: u64) -> &'static str {
    let features = unsafe { core::arch::x86_64::__cpuid(1) };
    assert!(
        features.edx & (1 << 9) != 0,
        "MADT APIC without CPU APIC support"
    );
    unsafe {
        let mut base = Msr::new(0x1b);
        let old = base.read();
        if features.ecx & (1 << 21) != 0 {
            // x2APIC transitions from disabled through xAPIC enabled.
            if old & (1 << 11) == 0 {
                base.write(old | (1 << 11));
            }
            base.write(old | (1 << 11) | (1 << 10));
            MODE.store(2, Ordering::Release);
        } else {
            assert!(old & (1 << 10) == 0, "x2APIC active but not advertised");
            assert!(
                address != 0 && address & 0xfff == 0,
                "Invalid LAPIC address"
            );
            let mapped = crate::memory::map_mmio(address, 4096).expect("Map LAPIC");
            MMIO.store(mapped, Ordering::Relaxed);
            base.write((old & !0x000f_ffff_ffff_f000) | address | (1 << 11));
            MODE.store(1, Ordering::Release);
        }
    }
    let max_lvt = (read(0x30) >> 16) & 0xff;
    for register in [0x320, 0x350, 0x360, 0x370] {
        write(register, MASKED);
    }
    if max_lvt >= 4 {
        write(0x340, MASKED);
    }
    if max_lvt >= 5 {
        write(0x330, MASKED);
    }
    if max_lvt >= 6 {
        write(0x2f0, MASKED);
    }
    write(0x80, 0); // TPR: accept all interrupt priorities.
    write(0xf0, (1 << 8) | u32::from(SPURIOUS_VECTOR));
    if MODE.load(Ordering::Relaxed) == 2 {
        "x2APIC"
    } else {
        "xAPIC"
    }
}

pub fn id() -> u32 {
    let value = read(0x20);
    if MODE.load(Ordering::Relaxed) == 2 {
        value
    } else {
        value >> 24
    }
}

/// Acknowledge on this CPU's LAPIC; no lock or address translation on the path.
#[inline]
pub fn eoi() {
    write(0xb0, 0);
}

/// Read the executing CPU's timer configuration and countdown.
pub fn timer_state() -> (u32, u32, u32) {
    (read(0x320), read(0x380), read(0x390))
}

/// Calibrate the divided LAPIC clock against a PIT channel-2 one-shot.
/// Interrupts remain disabled throughout boot calibration.
pub fn start_timer(hz: u32) -> u32 {
    use x86_64::instructions::port::Port;
    const PIT_TICKS: u16 = 59659; // approximately 50ms at 1,193,182 Hz
    unsafe {
        let mut gate = Port::<u8>::new(0x61);
        let mut command = Port::<u8>::new(0x43);
        let mut counter = Port::<u8>::new(0x42);
        let saved = gate.read();
        gate.write(saved & !3);
        command.write(0xb0);
        counter.write(PIT_TICKS as u8);
        counter.write((PIT_TICKS >> 8) as u8);
        write(
            0x320,
            MASKED | u32::from(crate::interrupts::InterruptIndex::Timer.as_u8()),
        );
        write(0x3e0, 3); // divide by 16
        write(0x380, u32::MAX);
        let start = read(0x390);
        gate.write((saved & !3) | 1);
        let deadline = super::timer::rdtsc() + super::timer::frequency_hz();
        while gate.read() & 0x20 == 0 {
            assert!(
                super::timer::rdtsc() < deadline,
                "PIT LAPIC calibration timed out"
            );
            core::hint::spin_loop();
        }
        let elapsed = start - read(0x390);
        gate.write(saved);
        write(0x380, 0);
        let count = (u64::from(elapsed) * 1_193_182 / u64::from(PIT_TICKS) / u64::from(hz)) as u32;
        assert!(count != 0, "LAPIC timer did not count");
        write(
            0x320,
            (1 << 17) | u32::from(crate::interrupts::InterruptIndex::Timer.as_u8()),
        );
        write(0x380, count);
        count
    }
}

/// Physical-destination IPI kinds for AP startup and subsequent SMP consumers.
#[allow(dead_code)] // Stage 2/3 public startup and reschedule API.
pub enum Ipi {
    Fixed(u8),
    Init,
    Startup(u8),
}

/// Send to an APIC id, with bounded xAPIC delivery waits. No AP is started by init.
#[allow(dead_code)] // Stage 2/3 public startup and reschedule API.
pub fn send_ipi(destination: u32, ipi: Ipi) -> Result<(), &'static str> {
    if !active() {
        return Err("LAPIC is not enabled");
    }
    if destination == u32::MAX {
        return Err("Broadcast is not a physical CPU destination");
    }
    let low = match ipi {
        Ipi::Fixed(vector) if vector >= 32 && vector != SPURIOUS_VECTOR => u32::from(vector),
        Ipi::Fixed(_) => return Err("Invalid fixed IPI vector"),
        // Integrated APIC INIT is edge triggered; no level deassert is needed.
        Ipi::Init => (5 << 8) | (1 << 14),
        Ipi::Startup(page) if page < 0xa0 => (6 << 8) | u32::from(page),
        Ipi::Startup(_) => return Err("Startup page is outside conventional memory"),
    };
    x86_64::instructions::interrupts::without_interrupts(|| {
        // Intel requires preceding stores to be globally visible before x2APIC ICR.
        unsafe {
            core::arch::asm!("mfence", "lfence", options(nostack, preserves_flags));
        }
        if MODE.load(Ordering::Relaxed) == 2 {
            unsafe {
                Msr::new(0x830).write((u64::from(destination) << 32) | u64::from(low));
            }
            Ok(())
        } else {
            if destination >= 255 {
                return Err("Invalid xAPIC physical destination");
            }
            wait_delivery()?;
            write(0x310, destination << 24);
            write(0x300, low);
            wait_delivery()
        }
    })
}

fn wait_delivery() -> Result<(), &'static str> {
    let deadline = super::timer::rdtsc() + super::timer::frequency_hz() / 10;
    while read(0x300) & (1 << 12) != 0 {
        if super::timer::rdtsc() >= deadline {
            return Err("IPI delivery timed out");
        }
        core::hint::spin_loop();
    }
    Ok(())
}

/// Read-only snapshot for a slow device request; never acknowledges an IRQ.
pub fn vector_pending_state(vector: u8) -> (bool, bool) {
    let bank = u32::from(vector / 32) * 16;
    let bit = 1u32 << (vector % 32);
    (read(0x200 + bank) & bit != 0, read(0x100 + bank) & bit != 0)
}
