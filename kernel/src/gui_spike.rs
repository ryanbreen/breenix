//! Scratch display entry spikes for the GUI plan (branch scratch/gui-spikes, never merged).
//!
//! Each target runs one spike once at boot and prints `GUI_SPIKE ...` lines on serial.

/// FNV-1a 64-bit offset basis.
pub const FNV_INIT: u64 = 0xcbf2_9ce4_8422_2325;

/// Fold one byte into an FNV-1a 64-bit hash.
#[inline]
pub fn fnv(h: u64, b: u8) -> u64 {
    (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3)
}

/// The known test pattern: canonical BGRX bytes of pixel (x, y) in scene `s`, X byte zero.
#[inline]
pub fn pattern_bgrx(s: u32, x: u32, y: u32) -> [u8; 4] {
    let b = (x.wrapping_mul(3) ^ s.wrapping_mul(0x35)) as u8;
    let g = (y.wrapping_mul(5) ^ s.wrapping_mul(0x6b)) as u8;
    let r = ((x >> 3) ^ (y >> 2) ^ s.wrapping_mul(0x9d)) as u8;
    [b, g, r, 0]
}

/// Run the ARM64 spike for whichever display backend this boot found.
#[cfg(target_arch = "aarch64")]
pub fn run_arm64() {
    use crate::drivers::{virtio, vmware};
    if virtio::gpu_pci::is_initialized() && virtio::gpu_pci::is_virgl_enabled() {
        virtio::gpu_pci::gui_spike_virgl_readback();
    } else if vmware::svga3::is_initialized() {
        vmware::svga3::gui_spike_stdu();
    } else if virtio::gpu_mmio::dimensions().is_some() {
        virtio::gpu_mmio::gui_spike_fenced_2d();
    } else {
        crate::serial_println!("GUI_SPIKE none: no display backend initialized");
    }
}

/// Run the x86-64 spike: the GOP mode and full-memory readback, then the tablet.
#[cfg(target_arch = "x86_64")]
pub fn run_x86() {
    crate::logger::gui_spike_gop_readback();
    dispi_mode_set();
    tablet_probe();
}

/// Set 1280x800x32 through virtio-vga's Bochs VBE DISPI registers, read the mode back,
/// check the linear framebuffer at that stride, then restore the boot mode.
#[cfg(target_arch = "x86_64")]
fn dispi_mode_set() {
    use crate::serial_println;
    use x86_64::instructions::port::Port;

    let rd = |i: u16| -> u16 {
        let mut idx = Port::<u16>::new(0x1ce);
        let mut dat = Port::<u16>::new(0x1cf);
        unsafe {
            idx.write(i);
            dat.read()
        }
    };
    let wr = |i: u16, v: u16| {
        let mut idx = Port::<u16>::new(0x1ce);
        let mut dat = Port::<u16>::new(0x1cf);
        unsafe {
            idx.write(i);
            dat.write(v);
        }
    };
    let id = rd(0);
    let before = (rd(1), rd(2), rd(3), rd(4), rd(6), rd(7));
    serial_println!(
        "GUI_SPIKE x86 dispi id={:#06x} before xres={} yres={} bpp={} enable={:#x} virt={}x{} vram_64k={}",
        id,
        before.0,
        before.1,
        before.2,
        before.3,
        before.4,
        before.5,
        rd(0xa)
    );
    if !(0xb0c0..=0xb0cf).contains(&id) {
        serial_println!("GUI_SPIKE x86 dispi absent");
        return;
    }
    let old_enable = before.3;
    wr(4, 0x02); // GETCAPS
    let caps = (rd(1), rd(2), rd(3));
    wr(4, 0);
    wr(1, 1280);
    wr(2, 800);
    wr(3, 32);
    wr(4, 0x01 | 0x40 | 0x80); // ENABLED | LFB_ENABLED | NOCLEARMEM
    let after = (rd(1), rd(2), rd(3), rd(4), rd(6), rd(7));
    let (fnv_got, fnv_want, bad) = crate::logger::gui_spike_lfb_check(1280, 800, 1280);
    serial_println!(
        "GUI_SPIKE x86 dispi set 1280x800x32 caps_max={}x{}x{} after xres={} yres={} bpp={} enable={:#x} virt={}x{} lfb fnv={:#018x} expected_fnv={:#018x} mismatched_pixels={}",
        caps.0,
        caps.1,
        caps.2,
        after.0,
        after.1,
        after.2,
        after.3,
        after.4,
        after.5,
        fnv_got,
        fnv_want,
        bad
    );
    wr(4, 0);
    wr(1, before.0);
    wr(2, before.1);
    wr(3, before.2);
    wr(4, old_enable | 0x80);
    serial_println!(
        "GUI_SPIKE x86 dispi restored xres={} yres={} bpp={} enable={:#x}",
        rd(1),
        rd(2),
        rd(3),
        rd(4)
    );
}

/// Read the virtio-tablet's virtio-input config through the PCI_CFG capability window.
#[cfg(target_arch = "x86_64")]
fn tablet_probe() {
    use crate::drivers::pci::{
        find_device, pci_read_config_byte, pci_read_config_dword, pci_write_config_byte,
        pci_write_config_dword,
    };
    use crate::serial_println;

    let Some(dev) = find_device(0x1af4, 0x1052) else {
        serial_println!("GUI_SPIKE x86 tablet absent: no PCI function 1af4:1052");
        return;
    };
    let (bus, d, f) = (dev.bus, dev.device, dev.function);
    serial_println!(
        "GUI_SPIKE x86 tablet present at {:02x}:{:02x}.{} 1af4:1052",
        bus,
        d,
        f
    );

    // Walk the vendor capabilities for DEVICE_CFG (type 4) and PCI_CFG (type 5).
    let mut dev_cfg: Option<(u8, u32)> = None;
    let mut pci_cfg: Option<u8> = None;
    let mut ptr = pci_read_config_byte(bus, d, f, 0x34) & 0xfc;
    let mut guard = 0;
    while ptr != 0 && guard < 48 {
        let id = pci_read_config_byte(bus, d, f, ptr);
        if id == 0x09 {
            let cfg_type = pci_read_config_byte(bus, d, f, ptr + 3);
            let bar = pci_read_config_byte(bus, d, f, ptr + 4);
            let offset = pci_read_config_dword(bus, d, f, ptr + 8);
            if cfg_type == 4 && dev_cfg.is_none() {
                dev_cfg = Some((bar, offset));
            } else if cfg_type == 5 && pci_cfg.is_none() {
                pci_cfg = Some(ptr);
            }
        }
        ptr = pci_read_config_byte(bus, d, f, ptr + 1) & 0xfc;
        guard += 1;
    }
    let (Some((bar, base)), Some(cap)) = (dev_cfg, pci_cfg) else {
        serial_println!(
            "GUI_SPIKE x86 tablet caps missing: device_cfg={} pci_cfg={}",
            dev_cfg.is_some(),
            pci_cfg.is_some()
        );
        return;
    };

    let window = |off: u32| {
        pci_write_config_byte(bus, d, f, cap + 4, bar);
        pci_write_config_dword(bus, d, f, cap + 12, 1);
        pci_write_config_dword(bus, d, f, cap + 8, base + off);
    };
    let read8 = |off: u32| -> u8 {
        window(off);
        pci_read_config_dword(bus, d, f, cap + 16) as u8
    };
    let write8 = |off: u32, v: u8| {
        window(off);
        pci_write_config_dword(bus, d, f, cap + 16, v as u32);
    };
    let read32 = |off: u32| -> u32 {
        (read8(off) as u32)
            | (read8(off + 1) as u32) << 8
            | (read8(off + 2) as u32) << 16
            | (read8(off + 3) as u32) << 24
    };
    // virtio_input_config: select @0, subsel @1, size @2, payload @8.
    let select = |sel: u8, sub: u8| -> u8 {
        write8(0, sel);
        write8(1, sub);
        read8(2)
    };

    let n = select(0x01, 0) as u32; // VIRTIO_INPUT_CFG_ID_NAME
    let mut name = [0u8; 64];
    let mut take = 0usize;
    for i in 0..n.min(64) {
        let b = read8(8 + i);
        if b == 0 {
            break;
        }
        name[take] = if b.is_ascii_graphic() || b == b' ' { b } else { b'?' };
        take += 1;
    }
    let name = core::str::from_utf8(&name[..take]).unwrap_or("<non-utf8>");
    serial_println!("GUI_SPIKE x86 tablet name_len={} name=\"{}\"", n, name);

    for (label, ev) in [("EV_KEY", 0x01u8), ("EV_REL", 0x02u8), ("EV_ABS", 0x03u8)] {
        let size = select(0x11, ev); // VIRTIO_INPUT_CFG_EV_BITS
        let b0 = if size > 0 { read8(8) } else { 0 };
        serial_println!(
            "GUI_SPIKE x86 tablet ev_bits {} size={} byte0={:#04x}",
            label,
            size,
            b0
        );
    }
    for (label, axis) in [("ABS_X", 0u8), ("ABS_Y", 1u8)] {
        let size = select(0x12, axis); // VIRTIO_INPUT_CFG_ABS_INFO
        let min = read32(8);
        let max = read32(12);
        serial_println!(
            "GUI_SPIKE x86 tablet abs_info {} size={} min={} max={}",
            label,
            size,
            min,
            max
        );
    }
}
