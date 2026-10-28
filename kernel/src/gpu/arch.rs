//! The small OS/architecture seam of the shared GPU core.
use core::sync::atomic::{AtomicBool, Ordering};

#[cfg(target_arch = "aarch64")]
pub(super) const MAX_CPUS: usize = crate::arch_impl::aarch64::constants::MAX_CPUS;
#[cfg(target_arch = "x86_64")]
pub(super) const MAX_CPUS: usize = crate::task::scheduler::MAX_CPUS;

pub(super) fn cpu_id() -> usize {
    #[cfg(target_arch = "aarch64")]
    {
        crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as usize
    }
    #[cfg(target_arch = "x86_64")]
    {
        crate::per_cpu::cpu_id()
    }
}
pub(super) fn counter() -> u64 {
    crate::arch_read_timestamp()
}
pub(super) fn frequency_hz() -> u64 {
    #[cfg(target_arch = "aarch64")]
    {
        crate::arch_impl::aarch64::timer::frequency_hz()
    }
    #[cfg(target_arch = "x86_64")]
    {
        crate::arch_impl::x86_64::timer::frequency_hz()
    }
}
pub(super) fn park() -> ! {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("msr daifset, #3", options(nomem, nostack));
    }
    #[cfg(target_arch = "x86_64")]
    unsafe {
        crate::arch_disable_interrupts();
    }
    loop {
        crate::arch_halt();
    }
}

pub(super) fn is_qemu() -> bool {
    #[cfg(target_arch = "aarch64")]
    {
        crate::platform_config::is_qemu()
    }
    #[cfg(target_arch = "x86_64")]
    {
        false
    }
}
pub(super) fn is_parallels() -> bool {
    #[cfg(target_arch = "aarch64")]
    {
        crate::platform_config::is_parallels()
    }
    #[cfg(target_arch = "x86_64")]
    {
        false
    }
}
pub(super) fn is_vmware() -> bool {
    #[cfg(target_arch = "aarch64")]
    {
        crate::platform_config::is_vmware()
    }
    #[cfg(target_arch = "x86_64")]
    {
        false
    }
}

/// Only direct-map frame addresses, or the ARM direct-mapped heap, enter here.
pub(super) fn device_addr(va: u64) -> u64 {
    #[cfg(target_arch = "aarch64")]
    {
        crate::platform_config::kernel_va_to_ipa(va)
    }
    #[cfg(target_arch = "x86_64")]
    {
        va.checked_sub(crate::memory::physical_memory_offset().as_u64())
            .expect("GPU DMA outside direct map")
    }
}

#[cfg(target_arch = "aarch64")]
pub(super) fn mask_irq(id: u32) {
    #[cfg(target_arch = "aarch64")]
    crate::arch_impl::aarch64::gic::disable_spi(id);
}
#[cfg(target_arch = "aarch64")]
pub(super) fn unmask_irq(id: u32) {
    #[cfg(target_arch = "aarch64")]
    crate::arch_impl::aarch64::gic::unmask_spi(id);
}
#[cfg(target_arch = "aarch64")]
pub(super) fn enable_irq(id: u32) {
    #[cfg(target_arch = "aarch64")]
    crate::arch_impl::aarch64::gic::enable_spi_on_cpu(id, 0);
}

#[cfg(target_arch = "aarch64")]
pub(super) fn configure_irq(dev: &super::virgl::Device) {
    use super::{virgl::Irq, CONFIG_INTID, INTID, ISR};
    use crate::arch_impl::aarch64::gic;
    match dev.irq {
        Irq::Intx(id) => {
            ISR.store(dev.isr, Ordering::Release);
            INTID.store(id, Ordering::Release);
            gic::configure_spi_level_triggered(id);
        }
        Irq::Msix { queue, config } => {
            INTID.store(queue, Ordering::Release);
            CONFIG_INTID.store(config, Ordering::Release);
            for spi in [queue, config] {
                gic::configure_spi_edge_triggered(spi);
                gic::clear_spi_pending(spi);
            }
        }
    }
}

pub(super) fn firmware_writes() -> u64 {
    #[cfg(target_arch = "aarch64")]
    {
        crate::graphics::firmware_writes::count()
    }
    #[cfg(target_arch = "x86_64")]
    {
        crate::firmware_writes::count()
    }
}
pub(super) fn firmware_handoff() {
    #[cfg(target_arch = "aarch64")]
    crate::graphics::firmware_writes::handoff();
    #[cfg(target_arch = "x86_64")]
    crate::firmware_writes::handoff();
}

pub(super) fn firmware_hash() -> u64 {
    #[cfg(target_arch = "aarch64")]
    {
        let Some(fb) = crate::graphics::arm64_fb::SHELL_FRAMEBUFFER.get() else {
            return 0;
        };
        fb.lock().firmware_hash()
    }
    // No shared-core display backend is admitted on x86 in this unit.
    #[cfg(target_arch = "x86_64")]
    {
        0
    }
}
pub(super) fn retire_firmware(retired: &AtomicBool) {
    #[cfg(target_arch = "aarch64")]
    if let Some(fb) = crate::graphics::arm64_fb::SHELL_FRAMEBUFFER.get() {
        let _guard = fb.lock();
        retired.store(true, Ordering::Release);
        return;
    }
    retired.store(true, Ordering::Release);
}

/// The GIC interrupt id of PCI device `slot`'s INTx pin `pin` (1 = INTA) on QEMU's
/// `virt` machine: its device tree's `interrupt-map` routes pin `p` of slot `s` to SPI
/// `3 + (s + p - 1) mod 4`, level-high, and SPI `n` is GIC interrupt `32 + n`.
#[cfg(target_arch = "aarch64")]
fn qemu_virt_intx(slot: u8, pin: u8) -> u32 {
    32 + 3 + ((slot as u32 + pin as u32 - 1) % 4)
}

/// Route the device's interrupts: INTx on QEMU's `virt` machine, MSI-X through the GICv2m
/// doorbell on Parallels. The MSI-X table entries live in the function's BAR and survive a
/// device reset; the virtio vector registers do not, so [`Device::enable_queue`] writes them.
#[cfg(target_arch = "aarch64")]
pub(super) fn route(
    pci_dev: &crate::drivers::pci::Device,
) -> Result<super::virgl::Irq, &'static str> {
    if crate::platform_config::is_qemu() {
        let pin = crate::drivers::pci::pci_read_config_byte(
            pci_dev.bus,
            pci_dev.device,
            pci_dev.function,
            0x3D,
        );
        if pin == 0 || pin > 4 {
            return Err("the virtio-gpu PCI device has no INTx pin");
        }
        // Command register bit 10 masks INTx; the transport needs it clear.
        let command = crate::drivers::pci::pci_read_config_word(
            pci_dev.bus,
            pci_dev.device,
            pci_dev.function,
            0x04,
        );
        crate::drivers::pci::pci_write_config_word(
            pci_dev.bus,
            pci_dev.device,
            pci_dev.function,
            0x04,
            command & !(1 << 10),
        );
        return Ok(super::virgl::Irq::Intx(qemu_virt_intx(pci_dev.device, pin)));
    }
    if !crate::platform_config::is_parallels() {
        return Err("the GPU core drives virtio-gpu on QEMU and Parallels only");
    }
    let cap = pci_dev
        .find_msix_capability()
        .ok_or("the virtio-gpu PCI device has no MSI-X capability")?;
    if pci_dev.msix_table_size(cap) <= 1 {
        return Err("the virtio-gpu MSI-X table has fewer than two vectors");
    }
    let doorbell =
        crate::platform_config::gicv2m_doorbell().ok_or("no GICv2m frame for virtio-gpu MSI-X")?;
    let queue = crate::platform_config::allocate_msi_spi();
    let config = crate::platform_config::allocate_msi_spi();
    if queue == 0 || config == 0 {
        return Err("no GICv2m SPI left for virtio-gpu MSI-X");
    }
    pci_dev.configure_msix_entry(cap, 0, doorbell, queue);
    pci_dev.configure_msix_entry(cap, 1, doorbell, config);
    pci_dev.enable_msix(cap);
    pci_dev.disable_intx();
    Ok(super::virgl::Irq::Msix { queue, config })
}

#[cfg(any(feature = "capture_panic_oracle", feature = "gpu_fault_acceptance"))]
pub(super) fn with_framebuffer_lock(f: impl FnOnce()) {
    #[cfg(target_arch = "aarch64")]
    let Some(fb) = crate::graphics::arm64_fb::SHELL_FRAMEBUFFER.get() else {
        return;
    };
    #[cfg(target_arch = "aarch64")]
    let Some(_guard) = fb.try_lock() else {
        return;
    };
    f();
}

pub(super) fn set_screen(width: u32, height: u32) {
    #[cfg(target_arch = "aarch64")]
    crate::input::pointer::set_screen(width, height);
    // The x86 pointer/window path joins in its own integration unit.
    #[cfg(target_arch = "x86_64")]
    let _ = (width, height);
}
