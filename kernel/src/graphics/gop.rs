//! x86 GUI mode and the actual virtio-vga DISPI scanout (BreenGL §20.2).
//! The bootloader's mapping is retained, but its mode is never used after a mode set.
use alloc::{vec, vec::Vec};
use bootloader_api::info::{FrameBufferInfo, PixelFormat};
use breengel_abi::{Completion, Rect};
use conquer_once::spin::OnceCell;
use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::instructions::port::Port;

const WIDTH: usize = 1280;
const HEIGHT: usize = 800;
const LEN: usize = WIDTH * HEIGHT * 4;
struct Mapping {
    address: usize,
    len: usize,
}
static MAPPING: OnceCell<Mapping> = OnceCell::uninit();
static MODE: OnceCell<FrameBufferInfo> = OnceCell::uninit();
static FENCE: AtomicU64 = AtomicU64::new(0);
static RESOURCE: AtomicU64 = AtomicU64::new(0);

/// Preserve the firmware's mapping before its mutable slice is handed to the logger.
pub fn record_mapping(address: usize, len: usize) {
    let _ = MAPPING.get_or_init(|| Mapping { address, len });
}
fn read(index: u16) -> u16 {
    unsafe {
        Port::<u16>::new(0x1ce).write(index);
        Port::<u16>::new(0x1cf).read()
    }
}
fn write(index: u16, value: u16) {
    unsafe {
        Port::<u16>::new(0x1ce).write(index);
        Port::<u16>::new(0x1cf).write(value);
    }
}
/// Called once, after selecting a valid GUI suite and before starting PID 1.
pub fn set_suite_mode() -> Result<(), &'static str> {
    let mapping = MAPPING.get().ok_or("no GOP mapping")?;
    if mapping.len < 2 * LEN || !(0xb0c0..=0xb0c5).contains(&read(0)) {
        return Err("virtio-vga DISPI or scanout memory unavailable");
    }
    let device = crate::drivers::pci::get_devices()
        .ok_or("PCI unavailable")?
        .into_iter()
        .find(|d| d.vendor_id == 0x1af4 && d.device_id == 0x1050)
        .ok_or("virtio-vga unavailable")?;
    let vram = device.bars[0];
    if vram.is_io || vram.address == 0 || vram.size < (2 * LEN) as u64 {
        return Err("virtio-vga VRAM unavailable");
    }
    crate::logger::stop_display_logging();
    write(4, 0);
    write(1, WIDTH as u16);
    write(2, HEIGHT as u16);
    write(3, 32);
    write(4, 0x41); // enabled, linear framebuffer
    write(6, WIDTH as u16);
    write(8, 0);
    write(9, 0);
    if read(1) != WIDTH as u16
        || read(2) != HEIGHT as u16
        || read(3) != 32
        || read(6) != WIDTH as u16
        || read(7) < 2 * HEIGHT as u16
        || read(4) & 0x41 != 0x41
    {
        return Err("virtio-vga refused 1280x800x32");
    }
    RESOURCE.store(vram.address, Ordering::Release);
    let _ = MODE.get_or_init(|| FrameBufferInfo {
        byte_len: LEN,
        width: WIDTH,
        height: HEIGHT,
        stride: WIDTH,
        bytes_per_pixel: 4,
        pixel_format: PixelFormat::Bgr,
    });
    Ok(())
}
pub fn mode() -> Option<FrameBufferInfo> {
    MODE.get().copied()
}
pub fn dimensions() -> Option<(u32, u32)> {
    mode().map(|m| (m.width as u32, m.height as u32))
}
pub fn resource() -> u64 {
    RESOURCE.load(Ordering::Acquire)
}

/// Snapshot the bound destination, not the compositor's staging pixels.
/// All callers hold the shared display state lock; the logger has relinquished VRAM.
pub fn snapshot() -> Result<Vec<u8>, i32> {
    if mode().is_none() {
        return Err(breengel_abi::errno::EIO);
    }
    let mapping = MAPPING.get().ok_or(breengel_abi::errno::EIO)?;
    let offset = read(9) as usize * WIDTH * 4;
    if offset + LEN > mapping.len {
        return Err(breengel_abi::errno::EIO);
    }
    let mut out = vec![0; LEN];
    for (i, pixel) in out.chunks_exact_mut(4).enumerate() {
        let word =
            unsafe { core::ptr::read_volatile((mapping.address + offset + i * 4) as *const u32) };
        pixel.copy_from_slice(&(word | 0xff00_0000).to_le_bytes());
    }
    Ok(out)
}

/// Copy each damaged band with IRQs enabled, then complete it with a store barrier.
/// The fault points act on the real copy, binding and completion paths.
pub fn present(
    pixels: &[u8],
    damage: &[Rect],
    skip: bool,
    wrong: bool,
    stale: Option<u64>,
) -> Result<(u64, Vec<Completion>), i32> {
    if mode().is_none() {
        return Err(breengel_abi::errno::EIO);
    }
    let address = MAPPING.get().ok_or(breengel_abi::errno::EIO)?.address;
    write(9, if wrong { HEIGHT as u16 } else { 0 });
    let mut done = Vec::new();
    for (i, r) in damage.iter().enumerate() {
        if skip && i == 0 {
            continue;
        }
        for y in r.y as usize..(r.y + r.h) as usize {
            let a = (y * WIDTH + r.x as usize) * 4;
            let b = a + r.w as usize * 4;
            unsafe {
                core::ptr::copy_nonoverlapping(
                    pixels[a..b].as_ptr(),
                    (address + a) as *mut u8,
                    b - a,
                );
            }
        }
        unsafe {
            core::arch::asm!("sfence", options(nostack, preserves_flags));
        }
        let fence = if i == 0 {
            stale.unwrap_or_else(|| FENCE.fetch_add(1, Ordering::AcqRel) + 1)
        } else {
            FENCE.fetch_add(1, Ordering::AcqRel) + 1
        };
        // GOP has no device command: kind 1 is a returned band copy, status 1 completed.
        done.push(Completion {
            fence,
            kind: 1,
            status: 1,
        });
    }
    Ok((resource() + read(9) as u64 * WIDTH as u64 * 4, done))
}
pub fn corrupt_last_row() {
    let Some(mapping) = MAPPING.get() else {
        return;
    };
    let p = (mapping.address + (HEIGHT - 1) * WIDTH * 4) as *mut u8;
    unsafe {
        core::ptr::write_volatile(p, core::ptr::read_volatile(p) ^ 255);
    }
}
