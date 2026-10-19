//! Architecture boundaries for xHCI DMA and interrupt delivery.
use super::pci;

#[cfg(target_arch = "x86_64")]
pub(super) fn skip_controller() -> bool {
    crate::drivers::fw_cfg::present()
}

#[cfg(target_arch = "aarch64")]
pub(super) fn skip_controller() -> bool {
    crate::platform_config::is_qemu()
}

#[cfg(target_arch = "x86_64")]
pub(super) fn bar(device: &pci::Device) -> Result<(u64, usize), &'static str> {
    super::size_bar(device)
}

#[cfg(target_arch = "aarch64")]
pub(super) fn bar(device: &pci::Device) -> Result<(u64, usize), &'static str> {
    let bar = &device.bars[0];
    if bar.is_io || bar.address == 0 {
        return Err("xHCI has no memory BAR");
    }
    // Firmware already assigned this aperture. Do not toggle its decoding:
    // the ARM PCI enumerator deliberately leaves USB BARs unsized. Bound our
    // register accesses to the first 64 KiB, including extended capabilities.
    Ok((bar.address, 65536))
}

#[cfg(target_arch = "x86_64")]
pub(super) fn map_registers(phys: u64, size: usize) -> Result<usize, &'static str> {
    crate::memory::map_mmio(phys, size)
}

#[cfg(target_arch = "aarch64")]
pub(super) fn map_registers(phys: u64, _size: usize) -> Result<usize, &'static str> {
    Ok((crate::arch_impl::aarch64::constants::HHDM_BASE + phys) as usize)
}

#[cfg(target_arch = "x86_64")]
pub(super) fn enable_interrupt(device: &pci::Device, handler: fn()) -> Result<u32, &'static str> {
    crate::arch_impl::x86_64::msi::enable(device, handler).map(u32::from)
}

#[cfg(target_arch = "aarch64")]
static IRQ: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

#[cfg(target_arch = "aarch64")]
pub fn get_irq() -> Option<u32> {
    let irq = IRQ.load(core::sync::atomic::Ordering::Acquire);
    (irq != 0).then_some(irq)
}

#[cfg(target_arch = "aarch64")]
pub(super) fn enable_interrupt(device: &pci::Device, _handler: fn()) -> Result<u32, &'static str> {
    use crate::{arch_impl::aarch64::gic, platform_config as config};
    use core::sync::atomic::Ordering;
    let msi = device.find_msi_capability();
    let msix = device.find_msix_capability();
    if config::gicv2m_base_phys() == 0 {
        if let Some(frame) = super::firmware::msi_frame() {
            if let Some((base, count)) = frame.range {
                config::install_msi_frame(frame.base, base, count);
            } else {
                config::probe_gicv2m(frame.base);
            }
        }
    }
    if config::gicv2m_base_phys() == 0 && config::is_parallels() {
        config::probe_gicv2m(0x0225_0000);
    }
    let frame = config::gicv2m_base_phys();
    if frame == 0 {
        let dist =
            (crate::arch_impl::aarch64::constants::HHDM_BASE + config::gicd_base_phys()) as usize;
        let typer = super::rd(dist, 4);
        log::error!("[xhci] interrupt routing unavailable: MSI={msi:?} MSI-X={msix:?} INTx-pin={} line={} GICD_TYPER={typer:#x}", device.interrupt_pin, device.interrupt_line);
        return Err("xHCI MSI has no platform interrupt translator");
    }
    // Validate routing before consuming an SPI. configure_msi writes a
    // 32-bit destination; use MSI-X for frames outside that address space.
    let destination = frame.checked_add(0x40).ok_or("GICv2m address overflow")?;
    let msi = msi.filter(|_| u32::try_from(destination).is_ok());
    if msi.is_none() && msix.is_none() {
        return Err("xHCI has no usable MSI destination");
    }
    let irq = config::allocate_msi_spi();
    if irq == 0 {
        return Err("no GICv2m SPI available for xHCI");
    }
    IRQ.store(irq, Ordering::Release);
    gic::configure_spi_edge_triggered(irq);
    gic::clear_spi_pending(irq);
    gic::enable_spi_on_cpu(irq, 0);
    if let Some(cap) = msi {
        if let Some(msix) = msix {
            device.disable_msix(msix);
        }
        device.configure_msi(cap, destination as u32, irq as u16);
    } else if let Some(cap) = msix {
        device.disable_msi();
        device.configure_msix_entry(cap, 0, destination, irq);
        device.enable_msix(cap);
        device.disable_intx();
    } else {
        gic::disable_spi(irq);
        IRQ.store(0, Ordering::Release);
        return Err("xHCI has neither MSI nor MSI-X");
    }
    Ok(irq)
}

// Dma::new allocates page-aligned frames and rounds lengths to pages. Its
// separate command/input and event/output/report allocations isolate their cache lines.
// ARM ARM: DC CVAC/IVAC to PoC and DSB SY order DMA against device MMIO.
#[cfg(target_arch = "aarch64")]
pub(super) fn clean(address: usize, bytes: usize) {
    cache(address, bytes, false);
}

#[cfg(target_arch = "aarch64")]
pub(super) fn invalidate(address: usize, bytes: usize) {
    cache(address, bytes, true);
}

#[cfg(target_arch = "aarch64")]
fn cache(address: usize, bytes: usize, invalidate: bool) {
    unsafe {
        let ctr: usize;
        core::arch::asm!("mrs {}, ctr_el0", out(reg) ctr, options(nomem, nostack));
        let line = 4usize << ((ctr >> 16) & 15);
        for at in ((address & !(line - 1))..address + bytes).step_by(line) {
            if invalidate {
                core::arch::asm!("dc ivac, {}", in(reg) at, options(nostack, preserves_flags));
            } else {
                core::arch::asm!("dc cvac, {}", in(reg) at, options(nostack, preserves_flags));
            }
        }
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }
}

#[cfg(target_arch = "x86_64")]
pub(super) fn clean(_address: usize, _bytes: usize) {}
#[cfg(target_arch = "x86_64")]
pub(super) fn invalidate(_address: usize, _bytes: usize) {}
