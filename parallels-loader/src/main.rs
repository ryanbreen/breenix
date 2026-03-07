#![no_main]
#![no_std]
#![allow(dead_code)]

mod acpi_discovery;
mod gop_discovery;
pub mod hw_config;
mod kernel_entry;
mod kernel_load;
mod page_tables;

use uefi::prelude::*;
use uefi::mem::memory_map::{MemoryMap, MemoryType};
use uefi::table::cfg::ACPI2_GUID;

use hw_config::HardwareConfig;
use page_tables::PageTableStorage;

/// Page table storage - static so it survives ExitBootServices.
static mut PAGE_TABLES: PageTableStorage = PageTableStorage::new();

/// HardwareConfig - static so it survives ExitBootServices.
static mut HW_CONFIG: HardwareConfig = unsafe { core::mem::zeroed() };

#[entry]
fn main() -> Status {
    uefi::helpers::init().unwrap();

    log::info!("===========================================");
    log::info!("  Breenix Parallels Loader v0.1.0");
    log::info!("  UEFI ARM64 Boot Application");
    log::info!("===========================================");

    // Initialize HW_CONFIG with proper magic/version
    let config = unsafe {
        let ptr = &raw mut HW_CONFIG;
        *ptr = HardwareConfig::new();
        &mut *ptr
    };

    // Find RSDP from UEFI configuration tables
    let rsdp_addr = find_rsdp();
    let rsdp_addr = match rsdp_addr {
        Some(addr) => {
            log::info!("RSDP found at: 0x{:016x}", addr);
            config.rsdp_addr = addr as u64;
            addr
        }
        None => {
            log::error!("RSDP not found in UEFI configuration tables!");
            halt();
        }
    };

    // Discover hardware via ACPI
    log::info!("--- ACPI Discovery ---");
    match acpi_discovery::discover_hardware(rsdp_addr, config) {
        Ok(()) => {
            log::info!("--- Discovery Complete ---");
            log::info!("  UART:    0x{:08x} (IRQ {})", config.uart_base_phys, config.uart_irq);
            log::info!(
                "  GICv{}:   GICD=0x{:08x}",
                config.gic_version,
                config.gicd_base
            );
            if config.gicr_range_count > 0 {
                log::info!(
                    "           GICR=0x{:08x} ({}x ranges)",
                    config.gicr_ranges[0].base,
                    config.gicr_range_count
                );
            }
            if config.pci_ecam_base != 0 {
                log::info!(
                    "  PCI:     ECAM=0x{:08x}, MMIO=0x{:08x}",
                    config.pci_ecam_base,
                    config.pci_mmio_base
                );
            }
            log::info!("  Timer:   {} MHz", config.timer_freq_hz / 1_000_000);
        }
        Err(e) => {
            log::error!("ACPI discovery failed: {}", e);
            halt();
        }
    }

    // Populate RAM regions from UEFI memory map FIRST — we need the actual RAM
    // base address to know where to load the kernel on platforms like VMware
    // where guest RAM starts at 0x80000000 instead of 0x40000000.
    populate_ram_regions(config);

    log::info!("RAM regions: {} regions found", config.ram_region_count);
    for i in 0..config.ram_region_count as usize {
        if i >= config.ram_regions.len() {
            break;
        }
        let r = &config.ram_regions[i];
        log::info!("  RAM: {:#x} - {:#x} ({} MB)",
            r.base, r.base + r.size, r.size / (1024 * 1024));
    }

    // Compute RAM base offset for kernel relocation.
    // The kernel linker script assumes physical RAM starts at 0x40000000.
    // On VMware Fusion, RAM starts at 0x80000000, so offset = 0x40000000.
    // On QEMU/Parallels, RAM starts at 0x40000000, so offset = 0.
    let expected_ram_base: u64 = 0x4000_0000;
    let actual_ram_base = if config.ram_region_count > 0 {
        config.ram_regions[0].base & !0x3FFF_FFFF // Round down to 1GB boundary
    } else {
        expected_ram_base
    };
    let ram_base_offset = if actual_ram_base > expected_ram_base {
        actual_ram_base - expected_ram_base
    } else {
        0
    };
    if ram_base_offset != 0 {
        log::info!("RAM relocation: offset={:#x} (actual={:#x}, expected={:#x})",
            ram_base_offset, actual_ram_base, expected_ram_base);
    }

    // Load kernel ELF from ESP (with relocation offset applied to load addresses)
    log::info!("--- Loading Kernel ---");
    let loaded_kernel = match kernel_load::load_kernel(ram_base_offset) {
        Ok(k) => {
            log::info!("Kernel loaded: entry_phys={:#x}, range={:#x}-{:#x}",
                k.entry_phys, k.load_base, k.load_end);
            k
        }
        Err(e) => {
            log::error!("Failed to load kernel: {}", e);
            halt();
        }
    };

    // Log RAM regions and entry address to serial for VMware debugging
    {
        use core::fmt::Write;
        struct Su(u64, u8);
        impl core::fmt::Write for Su {
            fn write_str(&mut self, s: &str) -> core::fmt::Result {
                for c in s.bytes() {
                    unsafe {
                        if c == b'\n' {
                            if self.1 == 1 {
                                let lsr = (self.0 + 5) as *const u8;
                                while core::ptr::read_volatile(lsr) & 0x20 == 0 { core::hint::spin_loop(); }
                                core::ptr::write_volatile(self.0 as *mut u8, b'\r');
                            } else { core::ptr::write_volatile(self.0 as *mut u32, b'\r' as u32); }
                        }
                        if self.1 == 1 {
                            let lsr = (self.0 + 5) as *const u8;
                            while core::ptr::read_volatile(lsr) & 0x20 == 0 { core::hint::spin_loop(); }
                            core::ptr::write_volatile(self.0 as *mut u8, c);
                        } else { core::ptr::write_volatile(self.0 as *mut u32, c as u32); }
                    }
                }
                Ok(())
            }
        }
        let mut u = Su(config.uart_base_phys, config.uart_type);
        let _ = writeln!(u, "[ram] {} regions:", config.ram_region_count);
        for i in 0..config.ram_region_count as usize {
            if i >= config.ram_regions.len() { break; }
            let r = &config.ram_regions[i];
            let _ = writeln!(u, "[ram]   {:#x} - {:#x} ({} MB)",
                r.base, r.base + r.size, r.size / (1024 * 1024));
        }
        let _ = writeln!(u, "[kern] entry_phys={:#x} load={:#x}-{:#x}",
            loaded_kernel.entry_phys, loaded_kernel.load_base, loaded_kernel.load_end);
    }

    // Discover UEFI GOP framebuffer (optional — kernel works without display)
    log::info!("--- GOP Framebuffer Discovery ---");
    match gop_discovery::discover_gop(config) {
        Ok(()) => {
            log::info!("GOP framebuffer: {}x{} stride={} fmt={} base={:#x} size={:#x}",
                config.framebuffer.width,
                config.framebuffer.height,
                config.framebuffer.stride,
                if config.framebuffer.pixel_format == 1 { "BGR" } else { "RGB" },
                config.framebuffer.base,
                config.framebuffer.size);
        }
        Err(e) => {
            log::warn!("GOP not available: {} (continuing without display)", e);
        }
    }

    // --- xHCI: DisconnectController DISABLED ---
    //
    // EXPERIMENT: DisconnectController was destroying UEFI's endpoint state
    // (sending ConfigureEndpoint-Deconfigure commands) before ExitBootServices.
    // This caused the Parallels hypervisor to show NO "DisableEndpoint while
    // io_cnt is not zero!" during HCRST, and NO ep creates after HCRST → CC=12.
    //
    // On linux-probe (where CC=12 does NOT occur), the Linux EFI stub does NOT
    // call DisconnectController. UEFI's endpoints survive through ExitBootServices
    // to the linux module's HCRST, where "DisableEndpoint while io_cnt is not zero!"
    // appears, and subsequent ConfigureEndpoint commands DO produce ep creates.
    //
    // By SKIPPING DisconnectController, we let UEFI's in-flight USB I/O persist
    // through ExitBootServices, matching linux-probe's behavior.
    config.xhci_hcrst_done = 0;
    log::info!("xHCI DisconnectController SKIPPED (matching linux-probe behavior)");

    log::info!("--- Exiting Boot Services ---");

    // Pre-EBS breadcrumb: 'Z' to serial (proves serial write works before EBS)
    unsafe {
        if config.uart_type == 1 {
            let lsr = (config.uart_base_phys + 5) as *const u8;
            while core::ptr::read_volatile(lsr) & 0x20 == 0 { core::hint::spin_loop(); }
            core::ptr::write_volatile(config.uart_base_phys as *mut u8, b'Z');
        } else {
            core::ptr::write_volatile(config.uart_base_phys as *mut u32, b'Z' as u32);
        }
    }

    // Exit UEFI boot services. After this, NO UEFI calls are possible.
    unsafe {
        let _ = uefi::boot::exit_boot_services(MemoryType::LOADER_DATA);
    }

    // Post-EBS BAR re-enable DISABLED — let the kernel find the device in
    // whatever state EBS leaves it (matches linux-probe where Command=0x0010).

    // Jump to kernel with our page tables and HardwareConfig
    let page_tables = unsafe { &mut *(&raw mut PAGE_TABLES) };
    let hw_config = unsafe { &*(&raw const HW_CONFIG) };

    // Post-EBS serial diagnostic: single-char breadcrumb 'Z' proves we survived EBS
    unsafe {
        if hw_config.uart_type == 1 {
            let lsr = (hw_config.uart_base_phys + 5) as *const u8;
            while core::ptr::read_volatile(lsr) & 0x20 == 0 {
                core::hint::spin_loop();
            }
            core::ptr::write_volatile(hw_config.uart_base_phys as *mut u8, b'Z');
        } else {
            core::ptr::write_volatile(hw_config.uart_base_phys as *mut u32, b'Z' as u32);
        }
    }

    // Log entry address as hex nibbles for diagnostics
    unsafe {
        let hex = b"0123456789abcdef";
        let addr = loaded_kernel.entry_phys;
        for shift in (0..16).rev() {
            let nibble = ((addr >> (shift * 4)) & 0xF) as usize;
            if hw_config.uart_type == 1 {
                let lsr = (hw_config.uart_base_phys + 5) as *const u8;
                while core::ptr::read_volatile(lsr) & 0x20 == 0 {
                    core::hint::spin_loop();
                }
                core::ptr::write_volatile(hw_config.uart_base_phys as *mut u8, hex[nibble]);
            } else {
                core::ptr::write_volatile(hw_config.uart_base_phys as *mut u32, hex[nibble] as u32);
            }
        }
        // Newline
        if hw_config.uart_type == 1 {
            let lsr = (hw_config.uart_base_phys + 5) as *const u8;
            while core::ptr::read_volatile(lsr) & 0x20 == 0 { core::hint::spin_loop(); }
            core::ptr::write_volatile(hw_config.uart_base_phys as *mut u8, b'\n');
        }
    }

    kernel_entry::jump_to_kernel(loaded_kernel.entry_phys, hw_config, page_tables, ram_base_offset);
}

/// Populate HardwareConfig RAM regions from the UEFI memory map.
///
/// Scans the UEFI memory map for conventional memory (usable RAM) regions
/// and adds them to the HardwareConfig. The kernel uses these to configure
/// its frame allocator.
fn populate_ram_regions(config: &mut HardwareConfig) {
    // Get the UEFI memory map
    let buf = [0u8; 8192];
    let memory_map = match uefi::boot::memory_map(MemoryType::LOADER_DATA) {
        Ok(map) => map,
        Err(_) => {
            log::warn!("Failed to get UEFI memory map for RAM regions");
            return;
        }
    };

    let mut count = 0usize;

    for desc in memory_map.entries() {
        // Only count conventional memory (usable RAM)
        let mem_type = desc.ty;
        let is_usable = matches!(
            mem_type,
            MemoryType::CONVENTIONAL
                | MemoryType::BOOT_SERVICES_CODE
                | MemoryType::BOOT_SERVICES_DATA
        );

        if !is_usable {
            continue;
        }

        let base = desc.phys_start;
        let size = desc.page_count * 4096;

        // Merge with previous region if contiguous
        if count > 0 {
            let prev = &mut config.ram_regions[count - 1];
            if prev.base + prev.size == base {
                prev.size += size;
                continue;
            }
        }

        if count < config.ram_regions.len() {
            config.ram_regions[count] = hw_config::RamRegion { base, size };
            count += 1;
        }
    }

    config.ram_region_count = count as u32;

    let _ = buf; // Suppress unused warning
}

/// Find the ACPI RSDP address from UEFI configuration tables.
fn find_rsdp() -> Option<usize> {
    let st = uefi::table::system_table_raw().expect("no system table");

    // Safety: we're in boot services, system table is valid
    let st_ref = unsafe { st.as_ref() };

    // Iterate configuration tables looking for ACPI 2.0 RSDP
    let cfg_entries = st_ref.number_of_configuration_table_entries;
    let cfg_table = st_ref.configuration_table;

    if cfg_table.is_null() || cfg_entries == 0 {
        return None;
    }

    let entries = unsafe { core::slice::from_raw_parts(cfg_table, cfg_entries) };

    for entry in entries {
        if entry.vendor_guid == ACPI2_GUID {
            return Some(entry.vendor_table as usize);
        }
    }

    // Fall back to ACPI 1.0 RSDP
    let acpi1_guid = uefi::table::cfg::ACPI_GUID;
    for entry in entries {
        if entry.vendor_guid == acpi1_guid {
            return Some(entry.vendor_table as usize);
        }
    }

    None
}

/// Halt the CPU in an infinite loop (unrecoverable error).
fn halt() -> ! {
    loop {
        unsafe { core::arch::asm!("wfi") };
    }
}

// ---------------------------------------------------------------------------
// Minimal EFI_PCI_IO_PROTOCOL binding for disconnect_xhci_driver()
// ---------------------------------------------------------------------------

/// GetLocation function pointer type.
type PciIoGetLocationFn = unsafe extern "efiapi" fn(
    this: *const PciIoProtocol,
    segment: *mut usize,
    bus: *mut usize,
    device: *mut usize,
    function: *mut usize,
) -> Status;

/// Minimal EFI_PCI_IO_PROTOCOL — only need GetLocation for identifying the device.
/// Layout must match UEFI Spec 2.10 Section 14.4 up through GetLocation.
#[repr(C)]
#[uefi::proto::unsafe_protocol("4cf5b200-68b8-4ca5-9eec-b23e3f50029a")]
struct PciIoProtocol {
    poll_mem: usize,
    poll_io: usize,
    mem_read: usize,
    mem_write: usize,
    io_read: usize,
    io_write: usize,
    pci_read: usize,
    pci_write: usize,
    copy_mem: usize,
    map: usize,
    unmap: usize,
    allocate_buffer: usize,
    free_buffer: usize,
    flush: usize,
    get_location: PciIoGetLocationFn,
}

/// Disconnect UEFI's xHCI driver from the PCI device at 00:03.0.
///
/// Returns a sentinel value for diagnostics:
///   0x01 = success (driver disconnected)
///   0xA1 = no PCI handles found
///   0xA2 = xHCI device not found among handles
///   0xA3 = disconnect_controller failed
fn disconnect_xhci_driver() -> u32 {
    use uefi::boot;

    // Find all handles with EFI_PCI_IO_PROTOCOL
    let handles = match boot::locate_handle_buffer(
        boot::SearchType::ByProtocol(&<PciIoProtocol as uefi::Identify>::GUID),
    ) {
        Ok(h) => h,
        Err(_) => {
            log::warn!("xHCI disconnect: no PCI handles found");
            return 0xA1;
        }
    };

    log::info!("xHCI disconnect: found {} PCI device handles", handles.len());

    let image = boot::image_handle();

    for &handle in handles.iter() {
        // Open protocol just to peek at GetLocation
        let pci_io = match unsafe {
            boot::open_protocol::<PciIoProtocol>(
                boot::OpenProtocolParams {
                    handle,
                    agent: image,
                    controller: None,
                },
                boot::OpenProtocolAttributes::GetProtocol,
            )
        } {
            Ok(p) => p,
            Err(_) => continue,
        };

        // Check if this is 00:03.0 (the xHCI device)
        let (mut seg, mut bus, mut dev, mut fun) = (0usize, 0, 0, 0);
        let status = unsafe {
            (pci_io.get_location)(
                &*pci_io as *const PciIoProtocol,
                &mut seg,
                &mut bus,
                &mut dev,
                &mut fun,
            )
        };

        if status != Status::SUCCESS {
            continue;
        }

        if bus != 0 || dev != 3 || fun != 0 {
            continue;
        }

        log::info!("xHCI disconnect: found device at {:04x}:{:02x}:{:02x}.{:x}", seg, bus, dev, fun);

        // Drop the protocol reference before disconnecting
        drop(pci_io);

        // Disconnect ALL drivers from this controller
        match boot::disconnect_controller(handle, None, None) {
            Ok(_) => {
                log::info!("xHCI disconnect: SUCCESS — UEFI driver detached");
                return 0x01;
            }
            Err(e) => {
                log::warn!("xHCI disconnect: disconnect_controller failed: {:?}", e);
                return 0xA3;
            }
        }
    }

    log::warn!("xHCI disconnect: device 00:03.0 not found");
    0xA2
}

/// Halt and reset the xHCI controller BEFORE ExitBootServices.
///
/// The Parallels hypervisor tracks internal USB endpoint state. If we reset
/// the xHCI after EBS, the endpoint teardown fails ("DisableEndpoint while
/// io_cnt is not zero") and subsequent ConfigureEndpoint commands don't
/// create new internal endpoints. Doing halt+HCRST before EBS, while UEFI
/// services are alive, lets the hypervisor cleanly shut down endpoints.
fn pre_ebs_xhci_halt_reset(ecam_base: u64) {
    if ecam_base == 0 {
        log::warn!("xHCI pre-EBS reset: no ECAM base, skipping");
        return;
    }

    // Read BAR0 from PCI config space (ECAM offset for 00:03.0 = 0x18000)
    let ecam_xhci = ecam_base + 0x18000;
    let bar0 = unsafe { core::ptr::read_volatile((ecam_xhci + 0x10) as *const u32) };
    let bar0_phys = (bar0 & 0xFFFFF000) as u64;

    if bar0_phys == 0 {
        log::warn!("xHCI pre-EBS reset: BAR0 is zero, skipping");
        return;
    }

    log::info!("xHCI pre-EBS reset: BAR0=0x{:08x}", bar0_phys);

    // Read cap_length to find operational registers
    let cap_word = unsafe { core::ptr::read_volatile(bar0_phys as *const u32) };
    let cap_length = (cap_word & 0xFF) as u64;
    let op_base = bar0_phys + cap_length;

    log::info!("xHCI pre-EBS reset: cap_length={} op_base=0x{:x}", cap_length, op_base);

    // Read current USBCMD and USBSTS
    let usbcmd = unsafe { core::ptr::read_volatile(op_base as *const u32) };
    let usbsts = unsafe { core::ptr::read_volatile((op_base + 4) as *const u32) };
    log::info!("xHCI pre-EBS: USBCMD=0x{:08x} USBSTS=0x{:08x}", usbcmd, usbsts);

    // Step 1: Halt the controller (clear RS bit 0)
    if usbcmd & 1 != 0 {
        unsafe {
            core::ptr::write_volatile(op_base as *mut u32, usbcmd & !1);
        }
        // Wait for HCH (USBSTS bit 0) — up to 16ms per xHCI spec
        let mut halted = false;
        for _ in 0..1_000_000 {
            let sts = unsafe { core::ptr::read_volatile((op_base + 4) as *const u32) };
            if sts & 1 != 0 {
                halted = true;
                break;
            }
        }
        if halted {
            log::info!("xHCI pre-EBS: controller halted (HCH=1)");
        } else {
            log::warn!("xHCI pre-EBS: halt timeout, proceeding with HCRST anyway");
        }
    } else {
        log::info!("xHCI pre-EBS: controller already halted (RS=0)");
    }

    // Step 2: HCRST (set bit 1 of USBCMD)
    let usbcmd_now = unsafe { core::ptr::read_volatile(op_base as *const u32) };
    unsafe {
        core::ptr::write_volatile(op_base as *mut u32, usbcmd_now | (1 << 1));
    }

    // Wait for HCRST bit to self-clear (up to 16ms per spec)
    let mut reset_done = false;
    for _ in 0..1_000_000 {
        let cmd = unsafe { core::ptr::read_volatile(op_base as *const u32) };
        if cmd & (1 << 1) == 0 {
            reset_done = true;
            break;
        }
    }

    if reset_done {
        // Wait for CNR (Controller Not Ready, USBSTS bit 11) to clear
        let mut ready = false;
        for _ in 0..1_000_000 {
            let sts = unsafe { core::ptr::read_volatile((op_base + 4) as *const u32) };
            if sts & (1 << 11) == 0 {
                ready = true;
                break;
            }
        }
        if ready {
            log::info!("xHCI pre-EBS: HCRST complete, controller ready");
        } else {
            log::warn!("xHCI pre-EBS: CNR still set after HCRST");
        }
    } else {
        log::warn!("xHCI pre-EBS: HCRST timeout");
    }

    // Read final state
    let usbcmd_final = unsafe { core::ptr::read_volatile(op_base as *const u32) };
    let usbsts_final = unsafe { core::ptr::read_volatile((op_base + 4) as *const u32) };
    log::info!("xHCI pre-EBS: final USBCMD=0x{:08x} USBSTS=0x{:08x}", usbcmd_final, usbsts_final);
}
