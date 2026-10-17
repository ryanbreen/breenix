//! Local APIC access on the executing CPU.
//!
//! `init` chooses the mode (x2APIC or xAPIC) and maps the xAPIC window once
//! for the machine; `init_local` and `start_local_timer` are the per-CPU half
//! every CPU runs from its per-CPU init. Only the BSP is started here.
use core::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use x86_64::registers::model_specific::Msr;

pub const SPURIOUS_VECTOR: u8 = 0xff;
const MASKED: u32 = 1 << 16;
static MODE: AtomicU8 = AtomicU8::new(0);
static MMIO: AtomicUsize = AtomicUsize::new(0);
/// xAPIC physical base every CPU's APIC base MSR is pointed at.
static PHYS: AtomicU64 = AtomicU64::new(0);
/// Initial count for the periodic scheduler tick, calibrated once on the BSP.
static TIMER_COUNT: AtomicU32 = AtomicU32::new(0);

/// The NMI wiring of each processor's local interrupt pins, from the MADT's
/// Local APIC NMI structures (`set_local_nmis`): processor APIC ids and, for
/// each, `LOCAL_NMI_VALID | lint << 16 | MPS INTI flags`, or 0 where the MADT
/// names no pin.
const MAX_NMI_ROUTES: usize = super::acpi::MAX_ENUMERATED_CPUS;
const LOCAL_NMI_VALID: u32 = 1 << 31;
static NMI_ROUTE_IDS: [AtomicU32; MAX_NMI_ROUTES] = [const { AtomicU32::new(0) }; MAX_NMI_ROUTES];
static NMI_ROUTES: [AtomicU32; MAX_NMI_ROUTES] = [const { AtomicU32::new(0) }; MAX_NMI_ROUTES];
static NMI_ROUTE_COUNT: AtomicUsize = AtomicUsize::new(0);
/// The route for a processor the census did not record: an every-processor
/// structure's, or 0.
static NMI_ROUTE_OTHERS: AtomicU32 = AtomicU32::new(0);

fn encode_local_nmi(entry: Option<super::acpi::LocalNmi>) -> u32 {
    entry.map_or(0, |entry| {
        LOCAL_NMI_VALID | (u32::from(entry.lint) << 16) | u32::from(entry.flags)
    })
}

/// Record which local interrupt pin the platform wires to NMI on each
/// processor the MADT lists, so each CPU's `init_local` programs it. Called
/// once on the BSP before `init`.
pub fn set_local_nmis(madt: &super::acpi::MadtCensus) {
    let count = madt.recorded.min(MAX_NMI_ROUTES);
    for index in 0..count {
        let apic_id = madt.apic_ids[index];
        NMI_ROUTE_IDS[index].store(apic_id, Ordering::Relaxed);
        NMI_ROUTES[index].store(encode_local_nmi(madt.local_nmi_for(apic_id)), Ordering::Relaxed);
    }
    let others = madt.local_nmis[..madt.local_nmi_count]
        .iter()
        .copied()
        .find(|entry| entry.uid == super::acpi::ALL_PROCESSORS);
    NMI_ROUTE_OTHERS.store(encode_local_nmi(others), Ordering::Relaxed);
    NMI_ROUTE_COUNT.store(count, Ordering::Release);
}

/// The local interrupt pin wired to NMI on the processor with `apic_id`, as
/// (LVT register, LVT value): NMI delivery, edge triggered (the SDM requires
/// edge for NMI), active low only when the flags say so.
fn local_nmi_lvt(apic_id: u32) -> Option<(u32, u32)> {
    let count = NMI_ROUTE_COUNT.load(Ordering::Acquire);
    let route = (0..count)
        .find(|&index| NMI_ROUTE_IDS[index].load(Ordering::Relaxed) == apic_id)
        .map(|index| NMI_ROUTES[index].load(Ordering::Relaxed))
        .unwrap_or_else(|| NMI_ROUTE_OTHERS.load(Ordering::Relaxed));
    if route & LOCAL_NMI_VALID == 0 {
        return None;
    }
    let register = if (route >> 16) & 1 == 0 { 0x350 } else { 0x360 };
    let active_low = route & 3 == 3;
    Some((register, (4 << 8) | if active_low { 1 << 13 } else { 0 }))
}

pub fn active() -> bool {
    MODE.load(Ordering::Acquire) != 0
}

/// The local APIC as firmware handed it over: IA32_APIC_BASE, read before
/// this kernel writes any APIC register. Its base field and its mode: the
/// global enable bit (11) clear is `disabled`; set, with the x2APIC enable
/// bit (10) `x2apic`, else `xapic`.
pub struct Handoff {
    pub msr: u64,
}

impl Handoff {
    pub fn read() -> Self {
        // SAFETY: IA32_APIC_BASE is an architectural MSR on x86-64 (Intel SDM
        // vol. 3, "Local APIC"), and a read of it has no side effect.
        Self { msr: unsafe { Msr::new(0x1b).read() } }
    }

    pub fn base(&self) -> u64 {
        self.msr & 0x000f_ffff_ffff_f000
    }

    pub fn mode(&self) -> &'static str {
        match (self.msr & (1 << 11) != 0, self.msr & (1 << 10) != 0) {
            (false, _) => "disabled",
            (true, true) => "x2apic",
            (true, false) => "xapic",
        }
    }
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

/// Choose the APIC mode for the machine, map the xAPIC window if that is the
/// mode, and enable the BSP's local APIC.
/// CPUID's x2APIC bit includes the hypervisor's advertised capability.
pub fn init(address: u64) -> &'static str {
    let features = unsafe { core::arch::x86_64::__cpuid(1) };
    assert!(
        features.edx & (1 << 9) != 0,
        "MADT APIC without CPU APIC support"
    );
    if features.ecx & (1 << 21) != 0 {
        MODE.store(2, Ordering::Release);
    } else {
        assert!(
            address != 0 && address & 0xfff == 0,
            "Invalid LAPIC address"
        );
        let mapped = crate::memory::map_mmio(address, 4096).expect("Map LAPIC");
        MMIO.store(mapped, Ordering::Relaxed);
        PHYS.store(address, Ordering::Relaxed);
        MODE.store(1, Ordering::Release);
    }
    init_local();
    if MODE.load(Ordering::Relaxed) == 2 {
        "x2APIC"
    } else {
        "xAPIC"
    }
}

/// Enable the executing CPU's local APIC in the machine's mode, masking
/// firmware LVT sources before it accepts interrupts. Every CPU runs this from
/// its per-CPU init; `init` runs it once for the BSP before routing devices.
/// Repeating it on a CPU rewrites the same enable bits and masks.
pub fn init_local() {
    unsafe {
        let mut base = Msr::new(0x1b);
        let old = base.read();
        if MODE.load(Ordering::Relaxed) == 2 {
            // x2APIC transitions from disabled through xAPIC enabled.
            if old & (1 << 11) == 0 {
                base.write(old | (1 << 11));
            }
            base.write(old | (1 << 11) | (1 << 10));
        } else {
            assert!(old & (1 << 10) == 0, "x2APIC active but not advertised");
            base.write((old & !0x000f_ffff_ffff_f000) | PHYS.load(Ordering::Relaxed) | (1 << 11));
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
    // The pin the platform wires to NMI (the BMC's NMI button and its
    // Redfish/IPMI NMI on servers) stays live; the other LINT stays masked.
    if let Some((register, value)) = local_nmi_lvt(id()) {
        write(register, value);
    }
    write(0x80, 0); // TPR: accept all interrupt priorities.
    write(0xf0, (1 << 8) | u32::from(SPURIOUS_VECTOR));
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

/// Calibrate the divided LAPIC clock against a PIT channel-2 one-shot, on the
/// BSP, and record the initial count a `hz` periodic tick needs. The timer is
/// left masked; each CPU's per-CPU init starts its own with
/// `start_local_timer`. Interrupts remain disabled throughout calibration.
pub fn calibrate_timer(hz: u32) -> u32 {
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
        TIMER_COUNT.store(count, Ordering::Release);
        count
    }
}

/// Start the executing CPU's periodic scheduler tick with the count the BSP
/// calibrated. Every CPU's LAPIC timer runs from the same bus clock, so one
/// calibration serves them all.
pub fn start_local_timer() {
    let count = TIMER_COUNT.load(Ordering::Acquire);
    assert!(count != 0, "LAPIC timer started before calibration");
    write(0x3e0, 3); // divide by 16, as calibrated
    write(
        0x320,
        (1 << 17) | u32::from(crate::interrupts::InterruptIndex::Timer.as_u8()),
    );
    write(0x380, count);
}

/// The executing CPU's running scheduler tick, measured against the TSC: how
/// fast its LAPIC timer counts down over about 2 ms, divided by the initial
/// count it reloads. In millihertz; no value before the timer runs, or when four
/// tries were each stretched past a tick period (a stalled virtual CPU).
/// Each count read is bracketed by TSC reads and placed at their midpoint.
pub fn measured_tick_millihz() -> Option<u64> {
    let count = u64::from(TIMER_COUNT.load(Ordering::Acquire));
    let tsc_hz = super::timer::frequency_hz();
    if count == 0 || tsc_hz == 0 || read(0x380) == 0 {
        return None;
    }
    let sample = || {
        let before = super::timer::rdtsc_serialized();
        let value = u64::from(read(0x390));
        let after = super::timer::rdtsc_serialized();
        (value, before + (after - before) / 2)
    };
    for _ in 0..4 {
        let (start, start_at) = sample();
        let deadline = start_at + tsc_hz / 500;
        while super::timer::rdtsc() < deadline {
            core::hint::spin_loop();
        }
        let (end, end_at) = sample();
        let elapsed = end_at - start_at;
        // Under a tick period (5 ms at 200 Hz) the count reloads once at most.
        if elapsed >= tsc_hz * 4 / 1000 {
            continue;
        }
        let counted = if end <= start { start - end } else { start + count - end };
        return Some(
            (u128::from(tsc_hz) * u128::from(counted) * 1000 / (u128::from(elapsed) * u128::from(count)))
                as u64,
        );
    }
    None
}

/// Physical-destination IPI kinds for AP startup and subsequent SMP consumers.
#[allow(dead_code)] // Stage 2/3 public startup and reschedule API.
pub enum Ipi {
    Fixed(u8),
    /// Non-maskable: delivered whatever the target's RFLAGS.IF, so a CPU
    /// spinning with interrupts masked still answers a TLB shootdown.
    Nmi,
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
        Ipi::Nmi => (4 << 8) | (1 << 14),
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
