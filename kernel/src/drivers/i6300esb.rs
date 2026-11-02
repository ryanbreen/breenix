//! Intel 6300ESB watchdog transport for ARM64 QEMU.
//!
//! Register contract: Intel 300641-004US, sections 16.4.15--16.5.3:
//! https://www.intel.cn/content/dam/doc/datasheet/6300esb-io-controller-hub-datasheet.pdf
//! Emulation contract (including access widths and the 30 ns clock):
//! https://github.com/qemu/qemu/blob/v11.0.3/hw/watchdog/wdt_i6300esb.c
//!
//! QEMU's i6300esb_timer_expired sets previous_reboot_flag and its reset
//! callback deliberately preserves it. A watchdog reset within the same QEMU
//! process therefore supports previous-expired=1; a new process does not.
//! QEMU returns zero for preload reads, so readback verifies config/lock and
//! tracks whether this transport programmed the preloads, not their contents.
//! claim-lint:ok: 2/2 preload offsets (0x00, 0x04) return 0 in QEMU v11.0.3's
//! i6300esb_mem_readl, at the QEMU source URL above.

use super::watchdog_round::Transport;

const CONFIG: u16 = 0x60;
const LOCK: u16 = 0x68;
const PRELOAD1: u16 = 0;
const PRELOAD2: u16 = 4;
const RELOAD: u16 = 0x0c;
const TIMEOUT: u16 = 1 << 9;
const KICK: u16 = 1 << 8;
const ENABLE: u8 = 2;
const LOCKED: u8 = 1;
// Interrupt disabled; output/reset enabled (bit 5 is active low); prescale 2^15.
const CONFIGURED: u16 = 3;
const CONFIG_MASK: u16 = 0x27;
// ceil(120 seconds / (2^15 * 30 ns)) = 122071 ticks, split over both stages.
// QEMU uses preload directly, whereas the physical counter counts zero too.
// claim-lint:ok: 2/2 preload register descriptions specify zero-based counting
// in Intel 300641-004US sections 16.4.18 and 16.4.19, at the datasheet URL above.
const PRELOADS: [u32; 2] = [61_035, 61_036];

/// Exact-width PCI config and BAR0 accesses, also implemented by the host model.
pub trait Bus {
    fn config_read16(&mut self, offset: u16) -> u16;
    fn config_write16(&mut self, offset: u16, value: u16);
    fn config_read8(&mut self, offset: u16) -> u8;
    fn config_write8(&mut self, offset: u16, value: u8);
    fn mmio_read16(&mut self, offset: u16) -> u16;
    fn mmio_write8(&mut self, offset: u16, value: u8);
    fn mmio_write16(&mut self, offset: u16, value: u16);
    fn mmio_write32(&mut self, offset: u16, value: u32);
}

pub struct State {
    config: u16,
    lock: u8,
    preloads_programmed: bool,
}

/// A single feeding thread owns this transport; no interrupt handler feeds it.
pub struct Watchdog<B> {
    bus: B,
    previous_expired: bool,
    preloads_programmed: bool,
}

impl<B: Bus> Watchdog<B> {
    /// Capture and clear the sticky timeout status without enabling or feeding.
    pub fn new(bus: B) -> Self {
        let mut watchdog = Self {
            bus,
            previous_expired: false,
            preloads_programmed: false,
        };
        watchdog.previous_expired = watchdog.bus.mmio_read16(RELOAD) & TIMEOUT != 0;
        if watchdog.previous_expired {
            watchdog.unlock();
            watchdog.bus.mmio_write16(RELOAD, TIMEOUT);
        }
        watchdog
    }

    pub fn previous_expired(&self) -> bool {
        self.previous_expired
    }

    fn unlock(&mut self) {
        self.bus.mmio_write8(RELOAD, 0x80);
        self.bus.mmio_write8(RELOAD, 0x86);
    }
}

impl<B: Bus> Transport for Watchdog<B> {
    type State = State;

    fn configured(state: &State) -> bool {
        state.preloads_programmed
            && state.config & CONFIG_MASK == CONFIGURED
            && state.lock & 7 == ENABLE | LOCKED
    }

    fn get(&mut self) -> Result<State, &'static str> {
        let raw = self.bus.config_read16(CONFIG);
        // QEMU's ESB_WDT_INTTYPE mask is 0x11, so writing the disabled
        // encoding 3 reads back as 1. Decode that emulator encoding here;
        // reset, prescaler and enable/lock readback stay independently checked.
        let config = if raw & 3 == 1 { raw | 2 } else { raw };
        Ok(State {
            config,
            lock: self.bus.config_read8(LOCK),
            preloads_programmed: self.preloads_programmed,
        })
    }

    fn set(&mut self) -> Result<(), &'static str> {
        let lock = self.bus.config_read8(LOCK);
        if lock & LOCKED != 0 && lock & 6 != ENABLE {
            return Err("i6300esb locked in unsafe mode");
        }
        self.bus.config_write16(CONFIG, CONFIGURED);
        self.unlock();
        self.bus.mmio_write32(PRELOAD1, PRELOADS[0]);
        self.unlock();
        self.bus.mmio_write32(PRELOAD2, PRELOADS[1]);
        self.preloads_programmed = true;
        self.bus.config_write8(LOCK, ENABLE);
        self.bus.config_write8(LOCK, ENABLE | LOCKED);
        if !Self::configured(&self.get()?) {
            return Err("i6300esb configuration readback");
        }
        Ok(())
    }

    fn reset(&mut self) -> Result<(), &'static str> {
        self.unlock();
        self.bus.mmio_write16(RELOAD, KICK);
        Ok(())
    }
}

#[cfg(all(target_arch = "aarch64", not(test)))]
mod hardware {
    use super::{Bus, Watchdog};
    use crate::drivers::pci;

    pub struct MmioBus {
        config: usize,
        bar0: usize,
    }

    /// Call after PCI enumeration and assign_bars, once for the feeding thread.
    /// Discovery leaves the timer disabled until the policy's first healthy round.
    pub fn discover() -> Result<Watchdog<MmioBus>, &'static str> {
        let ecam = crate::platform_config::pci_ecam_base();
        if !crate::platform_config::is_qemu() || ecam == 0 {
            return Err("i6300esb requires QEMU PCI");
        }
        let device = pci::find_device(0x8086, 0x25ab).ok_or("i6300esb absent")?;
        let bar = &device.bars[0];
        if bar.is_io || bar.address == 0 || bar.size < 0x10 {
            return Err("i6300esb BAR0 not assigned");
        }
        device.enable_memory_space();
        // ARM's PCI windows are Device memory in the HHDM, as for AHCI.
        const HHDM: u64 = 0xffff_0000_0000_0000;
        let config = HHDM
            + ecam
            + ((u64::from(device.bus) << 20)
                | (u64::from(device.device) << 15)
                | (u64::from(device.function) << 12));
        Ok(Watchdog::new(MmioBus {
            config: config as usize,
            bar0: (HHDM + bar.address) as usize,
        }))
    }

    // These registers require 16-bit CONFIG and 8-bit LOCK transactions.
    // pci's byte/word helpers use dword RMW, which QEMU does not dispatch to
    // the watchdog config callbacks. Use native ECAM widths instead.
    // DSB orders each Device access, including between ECAM and BAR0.
    fn ordered_read8(address: usize) -> u8 {
        let value: u32;
        unsafe {
            core::arch::asm!("ldrb {value:w}, [{address}]", "dsb osh",
                address = in(reg) address, value = out(reg) value,
                options(nostack, preserves_flags));
        }
        value as u8
    }

    fn ordered_read16(address: usize) -> u16 {
        let value: u32;
        unsafe {
            core::arch::asm!("ldrh {value:w}, [{address}]", "dsb osh",
                address = in(reg) address, value = out(reg) value,
                options(nostack, preserves_flags));
        }
        value as u16
    }

    fn ordered_write<T: Into<u32>>(address: usize, value: T) {
        let value = value.into();
        // HVF cannot decode a stage-2 MMIO abort from pre/post-indexed
        // stores (ISV=0). A volatile Rust store can be folded into that
        // form, so fix the addressing mode as well as the access width.
        macro_rules! store {
            ($instruction:literal) => {
                core::arch::asm!(
                    concat!($instruction, " {value:w}, [{address}]"),
                    "dsb osh",
                    address = in(reg) address,
                    value = in(reg) value,
                    options(nostack, preserves_flags),
                )
            };
        }
        unsafe {
            match core::mem::size_of::<T>() {
                1 => store!("strb"),
                2 => store!("strh"),
                4 => store!("str"),
                _ => unreachable!(),
            }
        }
    }

    impl Bus for MmioBus {
        fn config_read16(&mut self, offset: u16) -> u16 {
            u16::from_le(ordered_read16(self.config + usize::from(offset)))
        }
        fn config_write16(&mut self, offset: u16, value: u16) {
            ordered_write(self.config + usize::from(offset), value.to_le());
        }
        fn config_read8(&mut self, offset: u16) -> u8 {
            ordered_read8(self.config + usize::from(offset))
        }
        fn config_write8(&mut self, offset: u16, value: u8) {
            ordered_write(self.config + usize::from(offset), value);
        }
        fn mmio_read16(&mut self, offset: u16) -> u16 {
            u16::from_le(ordered_read16(self.bar0 + usize::from(offset)))
        }
        fn mmio_write8(&mut self, offset: u16, value: u8) {
            ordered_write(self.bar0 + usize::from(offset), value);
        }
        fn mmio_write16(&mut self, offset: u16, value: u16) {
            ordered_write(self.bar0 + usize::from(offset), value.to_le());
        }
        fn mmio_write32(&mut self, offset: u16, value: u32) {
            ordered_write(self.bar0 + usize::from(offset), value.to_le());
        }
    }
}

#[cfg(all(target_arch = "aarch64", not(test)))]
pub use hardware::{discover, MmioBus};

#[cfg(all(target_arch = "aarch64", not(any(test, feature = "testing", feature = "boot_tests"))))]
mod feeder {
    use super::{discover, Bus, Watchdog};
    use crate::drivers::watchdog_round::{feed, health, wait};

    fn run<B: Bus>(mut device: Watchdog<B>) -> Result<(), &'static str> {
        crate::serial_println!("\n[watchdog] previous-expired={}", u8::from(device.previous_expired()));
        let mut announced = false;
        let mut rounds = 0u64;
        loop {
            match feed(&mut device, health) {
                Ok(rearmed) => {
                    if announced {
                        rounds += 1;
                        if rounds % 3 == 0 { crate::serial_println!("[watchdog] healthy feeds={}", rounds); }
                    }
                    if rearmed || !announced {
                        crate::serial_println!("\n[watchdog] armed seconds=120 action=hard-reset health=root-io,root-inode");
                        announced = true;
                    }
                }
                Err(reason) => crate::serial_println!("[watchdog] round failed; retrying: {}", reason),
            }
            wait(20_000)?;
        }
    }

    pub fn start() {
        let device = discover();
        if device.is_err() { crate::serial_println!("[watchdog] disabled: no device"); }
        // The boot CPU can remain in its boot/idle context; secondary CPUs
        // dispatch ordinary pinned kthreads after SMP bring-up.
        let Some(cpu) = (1..crate::arch_impl::aarch64::smp::MAX_CPUS)
            .find(|&cpu| crate::arch_impl::aarch64::smp::is_cpu_online(cpu)) else {
                crate::serial_println!("[watchdog] feeder could not start: no scheduling CPU");
                return;
            };
        #[cfg(feature = "watchdog_fault_inject")]
        if crate::task::kthread::kthread_run_on_cpu(|| {
            if let Err(reason) = fault() { crate::serial_println!("[watchdog-fault] request failed: {}", reason); }
        }, "watchdog-fault", cpu).is_err() {
            crate::serial_println!("[watchdog-fault] thread could not start");
        }
        let Ok(device) = device else { return; };
        if crate::task::kthread::kthread_run_on_cpu(move || {
            if let Err(reason) = run(device) { crate::serial_println!("[watchdog] stopped: {}", reason); }
        }, "watchdog", cpu).is_err() {
            crate::serial_println!("[watchdog] feeder could not start");
        }
    }

    #[cfg(feature = "watchdog_fault_inject")]
    fn fault() -> Result<(), &'static str> {
        use crate::drivers::watchdog_fault_intent::{parse, Mode};
        let mut guard = crate::fs::ext2::root_fs_write()?;
        let fs = guard.as_mut().ok_or("fault root absent")?;
        let number = fs.resolve_path("/etc/breenix/watchdog-fault")?;
        let inode = fs.read_inode(number)?;
        if inode.size() > 32 { return Err("fault intent too long"); }
        let bytes = fs.read_file_content_coherent(number, &inode)?;
        let Some(mode) = parse(&bytes)? else { return Ok(()); };
        let ticket = fs.begin_resize_file_as(number, 0, false)?;
        if let Some(ticket) = ticket {
            guard.finish_shrink(ticket)?;
        } else {
            drop(guard);
        }
        let guard = crate::fs::ext2::root_fs_write()?;
        guard.as_ref().ok_or("fault root absent")?.sync()?;
        drop(guard);
        wait(180_000)?;
        if mode == Mode::HealthHang {
            let _guard = crate::fs::ext2::root_fs_write()?;
            crate::serial_println!("\n[watchdog-fault] trigger health-hang");
            loop { crate::arch_halt_with_interrupts(); }
        }
        crate::serial_println!("\n[watchdog-fault] trigger feeder-hang");
        unsafe { core::arch::asm!("msr daifset, #0xf", options(nomem, nostack)); }
        loop { unsafe { core::arch::asm!("wfi", options(nomem, nostack)); } }
    }
}

#[cfg(all(target_arch = "aarch64", not(any(test, feature = "testing", feature = "boot_tests"))))]
pub use feeder::start;
