//! Where the console's pixels go.
//!
//! The render thread draws into a RAM shadow and hands pixel rows of it to the
//! display, which puts them on the screen:
//!
//! - `linear`: memory the display controller scans out, written directly. The
//!   x86 firmware framebuffer (in the display function's BAR, mapped
//!   write-combining) and the ARM64 GOP framebuffer on Parallels and VMware.
//! - `virtio_2d` (ARM64 QEMU): the guest backing of the virtio-gpu 2D resource,
//!   then TRANSFER_TO_HOST_2D and RESOURCE_FLUSH for the rows written.
//! - `virgl_backing` (ARM64 Parallels with VirGL): the guest backing of the
//!   resource the kernel's screens present through the virtio-gpu PCI driver,
//!   until userspace takes the display.
//!
//! Rows are written one text row's band at a time, with interrupts masked and
//! the screen held (`holding_screen`), so a display owner's flush, a read and
//! `wait_until_idle` do not see a band half-copied. A pass then flushes the rows
//! it wrote once (`flush`), outside that hold: on virtio-gpu a flush waits for
//! the device.
//!
//! A read-back reads what the display shows: the scanout memory itself, or on
//! virtio-gpu the backing of the resource the device last accepted for scanout
//! 0, after a transfer and flush of the whole screen completed (`settle`).

pub mod linear;
#[cfg(target_arch = "aarch64")]
pub mod virgl_backing;
#[cfg(target_arch = "aarch64")]
pub mod virtio_2d;

use super::raster::Layout;
use core::fmt;
use core::ops::Range;
use core::sync::atomic::{AtomicBool, Ordering};

/// Held, with interrupts masked, by whoever is writing a row of the screen.
static SCREEN_BUSY: AtomicBool = AtomicBool::new(false);

/// Drain this CPU's writes to the screen before it is released or read.
#[inline(always)]
pub(super) fn drain_writes() {
    // SAFETY: barriers only.
    unsafe {
        #[cfg(target_arch = "x86_64")]
        core::arch::asm!("mfence", options(nostack, preserves_flags));
        #[cfg(target_arch = "aarch64")]
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }
}

/// Run `f` with interrupts masked and the screen held; false, without running
/// it, when someone else holds it.
fn holding_screen(f: impl FnOnce()) -> bool {
    crate::arch_without_interrupts(|| {
        if SCREEN_BUSY.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
            return false;
        }
        f();
        drain_writes();
        SCREEN_BUSY.store(false, Ordering::Release);
        true
    })
}

/// Wait until no row of the screen is being written. A writer holds it for one
/// band with interrupts masked, so this is microseconds.
pub fn wait_until_idle() {
    while SCREEN_BUSY.load(Ordering::Acquire) {
        core::hint::spin_loop();
    }
}

/// Copy the bands of pixel rows `lines` from `shadow` into `target`, a mapping
/// laid out as `layout`, each under the screen hold and only while `allowed`.
/// False when the console may no longer draw or the screen was busy.
///
/// # Safety
/// `target` must map `layout.bytes()` writable bytes.
unsafe fn write_bands(target: *mut u8, layout: &Layout, shadow: &[u8], lines: Range<usize>, allowed: fn() -> bool) -> bool {
    let line_height = layout.font.line_height();
    let mut y = lines.start;
    while y < lines.end {
        let end = (y + line_height).min(lines.end);
        let bytes = layout.line(y).start..layout.line(end - 1).end;
        let mut drawn = false;
        let held = holding_screen(|| {
            if !allowed() {
                return;
            }
            // SAFETY: inside the mapping, which holds `layout.bytes()` bytes.
            unsafe {
                core::ptr::copy_nonoverlapping(shadow[bytes.clone()].as_ptr(), target.add(bytes.start), bytes.len());
                #[cfg(target_arch = "aarch64")]
                crate::graphics::firmware_writes::note_range(target.add(bytes.start), bytes.len());
            }
            drawn = true;
        });
        if !held || !drawn {
            return false;
        }
        y = end;
    }
    true
}

pub enum Display {
    Linear(linear::Linear),
    #[cfg(target_arch = "aarch64")]
    Virtio2d(virtio_2d::Virtio2d),
    #[cfg(target_arch = "aarch64")]
    VirglBacking(virgl_backing::VirglBacking),
}

/// What a read-back read.
pub enum Source {
    /// The memory the display controller scans out.
    Scanout { phys: u64, place: linear::Place },
    /// The guest backing of the resource bound to scanout 0, read after a
    /// transfer and flush of the whole screen completed.
    #[cfg(target_arch = "aarch64")]
    Backing { resource: u32, phys: u64, flush: u64 },
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::Scanout { phys, place } => write!(f, "scanout memory at {:#x}, {}", phys, place),
            #[cfg(target_arch = "aarch64")]
            Source::Backing { resource, phys, flush } => write!(
                f,
                "backing after flush {} of resource {} bound to scanout 0, at {:#x}",
                flush, resource, phys
            ),
        }
    }
}

impl Display {
    pub fn layout(&self) -> Layout {
        match self {
            Display::Linear(display) => display.layout(),
            #[cfg(target_arch = "aarch64")]
            Display::Virtio2d(display) => display.layout(),
            #[cfg(target_arch = "aarch64")]
            Display::VirglBacking(display) => display.layout(),
        }
    }

    /// What drives the screen, for the log.
    pub fn kind(&self) -> &'static str {
        match self {
            Display::Linear(_) if cfg!(target_arch = "x86_64") => "firmware framebuffer",
            Display::Linear(_) => "GOP framebuffer",
            #[cfg(target_arch = "aarch64")]
            Display::Virtio2d(_) => "virtio-gpu 2D resource",
            #[cfg(target_arch = "aarch64")]
            Display::VirglBacking(_) => "virtio-gpu PCI scanout backing",
        }
    }

    /// Write pixel rows `lines` of `shadow` to the screen, band by band. Each
    /// band is written only if `allowed`, asked with interrupts masked and the
    /// screen held. False when the console may no longer draw or the screen
    /// was busy.
    pub fn write(&self, shadow: &[u8], lines: Range<usize>, allowed: fn() -> bool) -> bool {
        match self {
            Display::Linear(display) => display.write(shadow, lines, allowed),
            #[cfg(target_arch = "aarch64")]
            Display::Virtio2d(display) => display.write(shadow, lines, allowed),
            #[cfg(target_arch = "aarch64")]
            Display::VirglBacking(display) => display.write(shadow, lines, allowed),
        }
    }

    /// Show pixel rows `lines` written since the last flush.
    pub fn flush(&self, lines: Range<usize>) -> Result<(), &'static str> {
        if lines.is_empty() {
            return Ok(());
        }
        match self {
            Display::Linear(_) => Ok(()),
            #[cfg(target_arch = "aarch64")]
            Display::Virtio2d(display) => display.flush(lines),
            #[cfg(target_arch = "aarch64")]
            Display::VirglBacking(display) => display.flush(lines),
        }
    }

    /// Make what was written complete and visible, and say what a read-back
    /// will read; an error when it cannot be tied to the scanout.
    pub fn settle(&self) -> Result<Source, &'static str> {
        match self {
            Display::Linear(display) => display.settle(),
            #[cfg(target_arch = "aarch64")]
            Display::Virtio2d(display) => display.settle(),
            #[cfg(target_arch = "aarch64")]
            Display::VirglBacking(display) => display.settle(),
        }
    }

    /// Pixel row `y` as the screen shows it, `width * bytes_per_pixel` bytes
    /// into `out`. Call after `settle`.
    pub fn read_line(&self, y: usize, out: &mut [u8]) {
        match self {
            Display::Linear(display) => display.read_line(y, out),
            #[cfg(target_arch = "aarch64")]
            Display::Virtio2d(display) => display.read_line(y, out),
            #[cfg(target_arch = "aarch64")]
            Display::VirglBacking(display) => display.read_line(y, out),
        }
    }
}

/// The physical address of kernel memory at `virt`, through the direct map.
#[cfg(target_arch = "aarch64")]
fn phys_of(virt: *const u8) -> u64 {
    virt as u64 - crate::memory::physical_memory_offset().as_u64()
}
