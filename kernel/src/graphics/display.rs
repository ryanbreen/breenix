//! ARM64 composition and atomic display observation (BreenGL §§18,20) on the three ARM64
//! displays: virtio-gpu 2D (QEMU), VirGL (Parallels) and the SVGA3 screen target (VMware).
//! Composition is in software into `State::pixels`; each display then presents the damaged
//! rectangles its own way (`Device`) and is observed through its §20.2 row.
//! All allocation and pixel work runs in thread context with IRQs enabled. No display
//! lock is taken by an interrupt; the render thread only tries it for the panel cursor.
use crate::drivers::virtio::{gpu_mmio as gpu, gpu_pci};
use crate::drivers::vmware::svga3;
use crate::window::{
    mem,
    present::{self, Backend, Composed, Failure, Frame},
    Caller,
};
use alloc::{vec, vec::Vec};
use breengel_abi::{self as abi, errno::*, tc_point as point, Plain, Rect};
use core::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use spin::Mutex;

const W: usize = 1280;
const H: usize = 800;
const LEN: usize = W * H * 4;
const MAX_OWNERS: usize = 4;
/// The latest completed composite serial of each live compositing thread group. A row
/// leaves only when its group exits, so another group's composites never evict it.
static COMPOSITES: Mutex<Vec<(u64, u64)>> = Mutex::new(Vec::new());

/// Kernel-published completed composite serials, independent of runner timestamps.
pub fn composite_records() -> alloc::string::String {
    use core::fmt::Write;
    let mut out = alloc::string::String::new();
    for (pid, serial) in COMPOSITES.lock().iter() {
        let _ = writeln!(out, "{pid} {serial}");
    }
    out
}
static DISPLAY: Display = Display;
static STATE: Mutex<Option<State>> = Mutex::new(None);
static OWNER: AtomicU64 = AtomicU64::new(0);
struct State {
    pixels: Vec<u8>,
    record: abi::ObservationRecord,
    invalid: i32,
    repair: bool,
    last_fence: u64,
    cursor: abi::Cursor,
    cursor_damage: Vec<Rect>,
    owners: Vec<(u64, Vec<u8>, abi::ObservationRecord)>,
    panel_cursor: Option<(u32, u32, Vec<u8>, bool)>,
}
struct Display;

/// The display a composite is presented on.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Device {
    /// virtio-gpu 2D (ARM64 QEMU): the scanout resource's guest backing, transferred per
    /// damaged rectangle and flushed.
    Virtio2d,
    /// VirGL (Parallels): the compositor texture, drawn onto the scanout resource by
    /// SUBMIT_3D and read back by TRANSFER_FROM_HOST_3D.
    Virgl,
    /// The SVGA3 screen target (VMware): the MOB of the bound surface, updated per damaged
    /// rectangle and fenced; it has no readback of the device's copy.
    Svga3,
}

/// The device whose setup completed: 0 none, else `Device as u8 + 1`.
static READY: AtomicU8 = AtomicU8::new(0);
static SETUP: Mutex<()> = Mutex::new(());

impl Device {
    /// The display this boot has at 1280 x 800, without touching the device.
    fn detect() -> Option<Device> {
        if gpu::dimensions() == Some((W as u32, H as u32)) {
            return Some(Device::Virtio2d);
        }
        if gpu_pci::is_initialized()
            && gpu_pci::dimensions() == Some((W as u32, H as u32))
            && gpu_pci::compositor_ready()
        {
            return Some(Device::Virgl);
        }
        let gop = super::arm64_fb::FB_INFO_CACHE.get().is_some_and(|c| {
            (c.width, c.height, c.stride, c.bytes_per_pixel, c.is_bgr) == (W, H, W, 4, true)
        });
        if svga3::is_initialized() && svga3::has_gbobjects() && super::arm64_fb::is_gop_active() && gop {
            return Some(Device::Svga3);
        }
        None
    }

    /// The device, once its setup has completed.
    fn get() -> Option<Device> {
        match READY.load(Ordering::Acquire) {
            1 => Some(Device::Virtio2d),
            2 => Some(Device::Virgl),
            3 => Some(Device::Svga3),
            _ => None,
        }
    }

    /// The kernel's own screen: what the kernel draws while it owns the display, rows of
    /// `W * 4` bytes.
    fn canvas(self) -> Option<&'static mut [u8]> {
        let c = match self {
            Device::Virtio2d => gpu::framebuffer(),
            Device::Virgl => gpu_pci::framebuffer(),
            Device::Svga3 => super::arm64_fb::gop_framebuffer(),
        }?;
        (c.len() >= LEN).then_some(c)
    }

    /// Copy `src`, part of the canvas, into `dst`. The SVGA3 canvas is the GOP framebuffer
    /// in VRAM, device memory where every access is uncached: a byte-wise copy of the whole
    /// screen takes seconds there, so it is read in aligned eight-byte loads.
    fn copy_canvas(self, dst: &mut [u8], src: &[u8]) {
        if self != Device::Svga3 {
            dst.copy_from_slice(src);
            return;
        }
        let mut i = 0;
        while i < dst.len() {
            let at = src[i..].as_ptr();
            if at as usize % 8 == 0 && dst.len() - i >= 8 {
                let v = unsafe { core::ptr::read_volatile(at.cast::<u64>()) };
                dst[i..i + 8].copy_from_slice(&v.to_ne_bytes());
                i += 8;
            } else {
                // Rows and pixels are four-byte aligned.
                let v = unsafe { core::ptr::read_volatile(at.cast::<u32>()) };
                dst[i..i + 4].copy_from_slice(&v.to_ne_bytes());
                i += 4;
            }
        }
    }

    /// Where `publish` copies composed rows before presenting them.
    fn destination(self) -> Option<&'static mut [u8]> {
        let d = match self {
            Device::Virtio2d => gpu::framebuffer(),
            Device::Virgl => gpu_pci::compositor_texture(),
            Device::Svga3 => svga3::compositor_backing(),
        }?;
        (d.len() >= LEN).then_some(d)
    }

    /// Present `damage` (and, unrecorded, `extra`) with the planted faults; returns the
    /// resource the device is bound to and the answered completions.
    fn present(
        self,
        damage: &[Rect],
        extra: Option<Rect>,
        skip: bool,
        wrong: bool,
        stale: Option<u64>,
    ) -> Result<(u64, Vec<abi::Completion>), &'static str> {
        match self {
            Device::Virtio2d => gpu::present_damage(damage, skip, wrong, stale),
            Device::Virgl => gpu_pci::compositor_present(damage, extra, skip, wrong, stale),
            Device::Svga3 => svga3::compositor_present(damage, extra, skip, wrong, stale),
        }
    }

    /// The completions a composite of `rects` damaged rectangles answers.
    fn expected(self, rects: usize) -> usize {
        match self {
            // A transfer per rectangle, then one flush.
            Device::Virtio2d => rects + usize::from(rects != 0),
            // An upload per rectangle, then the SUBMIT_3D that draws them.
            Device::Virgl => rects + 1,
            // A fence per rectangle.
            Device::Svga3 => rects,
        }
    }

    /// The most damaged rectangles a record has room for: twelve completions, less the
    /// final flush on virtio 2D, and the SUBMIT_3D and the readback transfer on VirGL.
    fn max_rects(self) -> usize {
        match self {
            Device::Virtio2d => 11,
            Device::Virgl => 10,
            Device::Svga3 => 12,
        }
    }

    fn composed_resource(self) -> u64 {
        match self {
            Device::Virtio2d => gpu::composed_resource(),
            Device::Virgl => gpu_pci::compositor_resource(),
            Device::Svga3 => svga3::compositor_resource(),
        }
    }

    fn last_fence(self) -> u64 {
        match self {
            Device::Virtio2d => gpu::last_answer_fence(),
            Device::Virgl => gpu_pci::last_fence(),
            Device::Svga3 => svga3::last_fence(),
        }
    }

    fn code(self) -> u32 {
        match self {
            Device::Virtio2d => abi::backend::VIRTIO_GPU_2D,
            Device::Virgl => abi::backend::VIRGL,
            Device::Svga3 => abi::backend::SVGA3_SCREEN_TARGET,
        }
    }

    /// Section 20.2's observation flags and boundary code for this backend.
    fn boundary(self) -> (u32, u32) {
        match self {
            Device::Virtio2d | Device::Svga3 => (abi::OBSERVATION_DEVICE_COPY_OUTSIDE, 1),
            Device::Virgl => (0, 3),
        }
    }
}

/// Set up this boot's display for composition once; true when it is ready.
fn setup() -> Option<Device> {
    if let Some(d) = Device::get() {
        return Some(d);
    }
    let _once = SETUP.lock();
    if let Some(d) = Device::get() {
        return Some(d);
    }
    let d = Device::detect()?;
    if d == Device::Svga3 {
        if let Err(e) = svga3::init_stdu(W as u32, H as u32) {
            crate::serial_println!("[display] SVGA3 screen target setup failed: {}", e);
            return None;
        }
    }
    READY.store(d as u8 + 1, Ordering::Release);
    if d == Device::Svga3 {
        // The screen target replaces the GOP scanout: show the kernel's screen on it.
        let mut guard = STATE.lock();
        let s = state(&mut guard);
        let _ = publish(s, &[full()], false);
    }
    Some(d)
}

pub fn init() {
    if setup().is_some() {
        present::set_backend(&DISPLAY);
    }
}
fn state(s: &mut Option<State>) -> &mut State {
    s.get_or_insert_with(|| {
        let d = Device::get().or_else(Device::detect);
        let mut pixels = vec![0; LEN];
        if let Some((d, canvas)) = d.and_then(|d| Some((d, d.canvas()?))) {
            d.copy_canvas(&mut pixels, &canvas[..LEN]);
        }
        State {
            pixels,
            record: abi::ObservationRecord::ZERO,
            invalid: 0,
            repair: false,
            last_fence: d.map_or(0, Device::last_fence),
            cursor: abi::Cursor {
                shape: abi::cursor_shape::HIDDEN,
                visible: 0,
                x: 0,
                y: 0,
            },
            cursor_damage: Vec::new(),
            owners: Vec::new(),
            panel_cursor: None,
        }
    })
}
fn full() -> Rect {
    Rect::new(0, 0, W as i32, H as i32)
}
fn cursor_rect(c: &abi::Cursor) -> Rect {
    Rect::new(c.x, c.y, 24, 24)
}
fn ticks() -> u64 {
    let n: u64;
    unsafe {
        core::arch::asm!("mrs {}, cntvct_el0",out(reg)n,options(nomem,nostack));
    }
    n
}
fn micros(n: u64) -> u64 {
    let f: u64;
    unsafe {
        core::arch::asm!("mrs {}, cntfrq_el0",out(reg)f,options(nomem,nostack));
    }
    n.saturating_mul(1_000_000) / f.max(1)
}

/// Submit the exact damaged bands and keep their answered command headers with the backing.
fn publish(s: &mut State, damage: &[Rect], faults: bool) -> Result<(), i32> {
    let start = ticks();
    if faults && crate::test_control::take(point::IRQ_HOLD, 0).is_some() {
        crate::arch_impl::aarch64::cpu::without_interrupts(|| {
            let began = ticks();
            while micros(ticks() - began) < 2000 {
                core::hint::spin_loop();
            }
            crate::tracing::providers::window::record_irq_masked(micros(ticks() - began));
        });
    }
    // The production copy has no IRQ-masked interval. Observe its actual entry state,
    // rather than substituting total wall time for an IRQ-masked measurement.
    let masked = !crate::arch_interrupts_enabled();
    let device = Device::get().ok_or(EIO)?;
    let fb = device.destination().ok_or(EIO)?;
    for r in damage {
        for y in r.y as usize..(r.y + r.h) as usize {
            let a = (y * W + r.x as usize) * 4;
            let b = a + r.w as usize * 4;
            fb[a..b].copy_from_slice(&s.pixels[a..b]);
        }
    }
    if masked {
        crate::tracing::providers::window::record_irq_masked(micros(ticks() - start));
    }
    let skip = faults && crate::test_control::take(point::SKIP_TRANSFER, 0).is_some();
    let wrong = faults && crate::test_control::take(point::WRONG_RESOURCE, 0).is_some();
    let stale = (faults && crate::test_control::take(point::STALE_FENCE, 0).is_some())
        .then_some(s.last_fence);
    let corrupt = faults && crate::test_control::take(point::CORRUPT_OUTSIDE_DAMAGE, 0).is_some();
    // The first pixel of the last row, outside the frame's damage. VirGL and SVGA3 present
    // the corrupted pixel to the device with the frame; virtio 2D writes it into the
    // observed guest backing after the frame's transfers.
    let corner = (H - 1) * W * 4;
    let extra = (corrupt && device != Device::Virtio2d).then(|| {
        fb[corner] ^= 0xFF;
        s.pixels[corner] = fb[corner];
        Rect::new(0, H as i32 - 1, 1, 1)
    });
    let (resource, answers) = device.present(damage, extra, skip, wrong, stale).map_err(|_| EIO)?;
    s.record.resource = resource;
    s.record.completion_count = answers.len() as u32;
    s.record.completions = [abi::Completion {
        fence: 0,
        kind: 0,
        status: 0,
    }; 12];
    if answers.len() > 12 {
        return Err(EIO);
    }
    s.record.completions[..answers.len()].copy_from_slice(&answers);
    let missing = answers.len() != device.expected(damage.len());
    let mut serial_mismatch = false;
    for answer in &answers {
        serial_mismatch |= answer.fence <= s.last_fence;
        s.last_fence = s.last_fence.max(answer.fence);
    }
    s.invalid = if missing {
        EAGAIN
    } else if resource != device.composed_resource() || serial_mismatch {
        EIO
    } else {
        0
    };
    s.repair |= s.invalid != 0 || corrupt;
    if corrupt && device == Device::Virtio2d {
        // Corrupt the actual destination backing outside this frame's damage.
        fb[corner] ^= 0xFF;
        s.pixels[corner] = fb[corner];
    }
    Ok(())
}
fn observation(s: &mut State) {
    let device = Device::get().unwrap_or(Device::Virtio2d);
    let (flags, boundary) = device.boundary();
    s.record.backend = device.code();
    s.record.width = W as u32;
    s.record.height = H as u32;
    s.record.stride = W as u32 * 4;
    // Every backend's destination is B, G, R, X in memory.
    s.record.source_format = 1;
    s.record.flags = flags;
    s.record.boundary = boundary;
}
fn bounded_damage(mut d: Vec<Rect>) -> Vec<Rect> {
    d = d.into_iter().filter_map(present::clip_to_screen).collect();
    // ObservationRecord has room for twelve completions (`Device::max_rects`). When
    // metadata is larger, a covering rectangle updates the device's full backing;
    // composition work still uses exactly the original damage union.
    if d.len() > Device::get().map_or(11, Device::max_rects) {
        let mut r = d[0];
        for b in &d[1..] {
            let x = r.x.min(b.x);
            let y = r.y.min(b.y);
            let endx = (r.x + r.w).max(b.x + b.w);
            let endy = (r.y + r.h).max(b.y + b.h);
            r = Rect::new(x, y, endx - x, endy - y);
        }
        d.clear();
        d.push(r);
    }
    d
}
fn sprite(pixels: &mut [u8], c: &abi::Cursor) {
    if c.visible == 0 || c.shape == abi::cursor_shape::HIDDEN {
        return;
    }
    for y in 0..18i32 {
        for x in 0..18i32 {
            let ink = match c.shape {
                0 => x <= y / 2 && (y < 13 || (x >= 4 && x <= 6)),
                1 => {
                    x == 8
                        || (y < 4 && (x - 8).abs() <= 3 - y)
                        || (y > 13 && (x - 8).abs() <= y - 14)
                }
                2 => {
                    y == 8
                        || (x < 4 && (y - 8).abs() <= 3 - x)
                        || (x > 13 && (y - 8).abs() <= x - 14)
                }
                3 => (x - y).abs() <= 1,
                4 => (x + y - 17).abs() <= 1,
                _ => false,
            };
            let (px, py) = (c.x + x, c.y + y);
            if ink && px >= 0 && py >= 0 && px < W as i32 && py < H as i32 {
                let o = (py as usize * W + px as usize) * 4;
                pixels[o..o + 4].copy_from_slice(&[255; 4]);
            }
        }
    }
}
impl Backend for Display {
    fn code(&self) -> u32 {
        Device::get().map_or(abi::backend::NONE, Device::code)
    }
    fn composite(&self, f: &Frame) -> Result<Composed, Failure> {
        if crate::test_control::take(point::BACKEND_ERROR, 0).is_some() {
            return Err(Failure { reason: 1 });
        }
        let mut guard = STATE.lock();
        let s = state(&mut guard);
        let panel_damage = restore_panel(s);
        let mut damage = if s.repair {
            s.repair = false;
            vec![full()]
        } else {
            f.damage.clone()
        };
        if let Some(r) = panel_damage { damage.push(r); }
        damage.append(&mut s.cursor_damage);
        if s.cursor.visible != 0 {
            damage.push(cursor_rect(&s.cursor));
        }
        for window in &f.windows {
            if crate::test_control::take(point::IGNORE_DAMAGE, window.id).is_some() {
                damage.push(window.rect);
            }
        }
        let mut pixels = 0u64;
        let mut per_window = vec![0u64; f.windows.len()];
        let mut spans: Vec<(usize, usize)> = Vec::new();
        let mut owner = [usize::MAX; W];
        for y in 0..H {
            spans.clear();
            for r in &damage {
                if let Some(r) = present::clip_to_screen(*r) {
                    if y >= r.y as usize && y < (r.y + r.h) as usize {
                        spans.push((r.x as usize, (r.x + r.w) as usize));
                    }
                }
            }
            if spans.is_empty() {
                continue;
            }
            spans.sort_unstable();
            // Merge overlaps on each row: no pixel is processed or counted twice.
            let mut i = 0;
            while i < spans.len() {
                let (a, mut b) = spans[i];
                i += 1;
                while i < spans.len() && spans[i].0 <= b {
                    b = b.max(spans[i].1);
                    i += 1;
                }
                owner[a..b].fill(usize::MAX);
                for (wi, w) in f.windows.iter().enumerate() {
                    if let Some(r) =
                        present::intersect(w.rect, Rect::new(a as i32, y as i32, (b - a) as i32, 1))
                    {
                        owner[r.x as usize..(r.x + r.w) as usize].fill(wi);
                    }
                }
                let mut x = a;
                while x < b {
                    let wi = owner[x];
                    let start = x;
                    x += 1;
                    while x < b && owner[x] == wi {
                        x += 1;
                    }
                    let dst = &mut s.pixels[(y * W + start) * 4..(y * W + x) * 4];
                    if wi != usize::MAX {
                        let w = &f.windows[wi];
                        if let Some(pin) = &w.pixels {
                            pin.mem().read(
                                ((y as i64 - w.rect.y as i64) as u64 * w.stride as u64)
                                    + ((start as i64 - w.rect.x as i64) as u64 * 4),
                                dst,
                            );
                        } else {
                            dst.fill(0);
                        }
                        per_window[wi] += (x - start) as u64;
                    } else {
                        dst.fill(0);
                    }
                    for px in dst.chunks_exact_mut(4) {
                        px[3] = 255;
                    }
                }
                pixels += (b - a) as u64;
            }
        }
        sprite(&mut s.pixels, &s.cursor);
        observation(s);
        s.record.serial = f.serial;
        s.record.window_count = f.windows.len() as u32;
        s.record.windows = [abi::ObservedWindow {
            id: 0,
            commit_id: 0,
        }; 16];
        for (slot, w) in s.record.windows.iter_mut().zip(&f.windows) {
            *slot = abi::ObservedWindow {
                id: w.id,
                commit_id: w.commit_id,
            };
        }
        publish(s, &bounded_damage(damage), true).map_err(|e| Failure { reason: e as u32 })?;
        let caller = Caller::current().map_err(|e| Failure { reason: e as u32 })?;
        let mut records = COMPOSITES.lock();
        match records.iter_mut().find(|(pid, _)| *pid == caller.tgid) {
            Some(record) => record.1 = record.1.max(f.serial),
            None => records.push((caller.tgid, f.serial)),
        }
        Ok(Composed { pixels, per_window })
    }
    fn readback(&self, _caller: &Caller, rb: &mut abi::Readback) -> Result<(), i32> {
        let mut guard = STATE.lock();
        let s = state(&mut guard);
        observation(s);
        // A fixed ABI record cannot describe a larger frame. Composition still succeeds.
        if s.record.window_count as usize > s.record.windows.len() { return Err(EIO); }
        // Read the entire destination through this backend's §20.2 path, under the same
        // lock as composite publication, with X normalized. Never a sampled band.
        let device = Device::get().ok_or(EIO)?;
        let mut record = s.record;
        let mut out = Vec::new();
        out.try_reserve_exact(LEN).map_err(|_| ENOMEM)?;
        match device {
            Device::Virtio2d => out.extend_from_slice(&gpu::framebuffer().ok_or(EIO)?[..LEN]),
            Device::Svga3 => out.extend_from_slice(&svga3::compositor_backing().ok_or(EIO)?[..LEN]),
            Device::Virgl => {
                // The fenced TRANSFER_FROM_HOST_3D is this observation's own completion.
                out.resize(LEN, 0);
                if s.invalid == 0 {
                    let done = gpu_pci::compositor_readback(&mut out).map_err(|_| EIO)?;
                    let n = record.completion_count as usize;
                    if n >= record.completions.len() {
                        return Err(EIO);
                    }
                    record.completions[n] = done;
                    record.completion_count += 1;
                }
            }
        }
        for p in out.chunks_exact_mut(4) {
            p[3] = 255;
        }
        rb.serial = s.record.serial;
        if s.invalid != EAGAIN {
            mem::copy_out(rb.record, unsafe {
                core::slice::from_raw_parts(
                    (&record as *const abi::ObservationRecord).cast::<u8>(),
                    core::mem::size_of::<abi::ObservationRecord>(),
                )
            })?;
        }
        if s.invalid != 0 {
            return Err(s.invalid);
        }
        mem::copy_out(rb.dst, &out)?;
        Ok(())
    }
    fn set_cursor(&self, _caller: &Caller, c: &abi::Cursor) -> Result<(), i32> {
        let mut guard = STATE.lock();
        let s = state(&mut guard);
        if s.cursor.visible != 0 {
            s.cursor_damage.push(cursor_rect(&s.cursor));
        }
        s.cursor = *c;
        if c.visible != 0 {
            s.cursor_damage.push(cursor_rect(c));
        }
        Ok(())
    }
    fn give_display(&self, caller: &Caller, pid: u32) -> Result<(), i32> {
        give(caller.tgid, pid as u64)
    }
}

/// Push caller ownership, saving exactly the display it covered.
pub fn take(pid: u64) -> Result<(), i32> {
    take_from(pid, None)
}
fn take_from(pid: u64, expected: Option<u64>) -> Result<(), i32> {
    setup().ok_or(EIO)?;
    let mut guard = STATE.lock();
    if expected.is_some_and(|owner| OWNER.load(Ordering::Acquire) != owner) { return Err(EPERM); }
    let s = state(&mut guard);
    let previous = OWNER.load(Ordering::Acquire);
    if previous == pid {
        return Ok(());
    }
    if s.owners.len() >= MAX_OWNERS || s.owners.iter().any(|(owner, _, _)| *owner == pid) {
        return Err(EBUSY);
    }
    s.owners.try_reserve(1).map_err(|_| ENOMEM)?;
    let mut snapshot = Vec::new();
    snapshot.try_reserve_exact(LEN).map_err(|_| ENOMEM)?;
    if let Some(r) = restore_panel(s) { publish(s, &[r], false)?; }
    let device = Device::get().ok_or(EIO)?;
    if device == Device::Virtio2d {
        snapshot.extend_from_slice(&gpu::framebuffer().ok_or(EIO)?[..LEN]);
    } else {
        // On VirGL the kernel presents its own screen straight from the canvas, so while the
        // kernel owns the display the screen is the canvas. The SVGA3 screen target shows only
        // what was published from `s.pixels`, which `canvas_presented` keeps current.
        if previous == 0 && device == Device::Virgl {
            let canvas = device.canvas().ok_or(EIO)?;
            s.pixels.copy_from_slice(&canvas[..LEN]);
        }
        snapshot.extend_from_slice(&s.pixels);
    }
    s.owners.push((previous, snapshot, s.record));
    OWNER.store(pid, Ordering::Release);
    crate::syscall::fb::DISPLAY_OWNER_PID.store(pid, Ordering::Release);
    Ok(())
}
pub fn give(from:u64,to:u64)->Result<(),i32> {
    if OWNER.load(Ordering::Acquire)!=from{return Err(EPERM);}
    let live=crate::process::with_process_manager(|m|m.get_process(crate::process::ProcessId::new(to)).is_some_and(|p|!p.is_terminated())).unwrap_or(false);
    if !live{return Err(ESRCH);}
    take_from(to, Some(from))?;
    let live=crate::process::with_process_manager(|m|{
        m.get_process_mut(crate::process::ProcessId::new(to)).is_some_and(|p|{
            if p.is_terminated(){return false;}
            p.has_display_ownership=true;true
        })
    }).unwrap_or(false);
    if !live {gone(to);return Err(ESRCH);}Ok(())
}

pub fn back(pid: u64) -> Result<(), i32> {
    if OWNER.load(Ordering::Acquire) != pid {
        return Err(EPERM);
    }
    setup().ok_or(EIO)?;
    let mut guard = STATE.lock();
    let s = state(&mut guard);
    if OWNER.load(Ordering::Acquire) != pid {
        return Err(EPERM);
    }
    let (previous, pixels, record) = s.owners.pop().ok_or(EPERM)?;
    s.record = record;
    s.pixels = pixels;
    s.panel_cursor = None;
    if previous == 0 {
        super::boot_ribbon::draw(&mut s.pixels);
        // On VirGL the kernel presents from its canvas again: start it from what is shown.
        if let Some(canvas) = Device::get().filter(|d| *d == Device::Virgl).and_then(Device::canvas) {
            canvas[..LEN].copy_from_slice(&s.pixels);
        }
    }
    OWNER.store(previous, Ordering::Release);
    crate::syscall::fb::DISPLAY_OWNER_PID.store(previous, Ordering::Release);
    publish(s, &[full()], false)
}
pub fn gone(pid: u64) {
    if OWNER.load(Ordering::Acquire) == pid {
        let _ = back(pid);
    }
    let mut guard = STATE.lock();
    if let Some(s) = guard.as_mut() {
        s.owners.retain(|(owner, _, _)| *owner != pid);
    }
    drop(guard);
    COMPOSITES.lock().retain(|(owner, _)| *owner != pid);
}

/// Copy a caller's private mmap pane, clipped before touching any pixels.
pub fn flush(pid: u64, pane: crate::process::process::FbMmapInfo, rect: Rect) -> Result<(), i32> {
    setup().ok_or(EIO)?;
    if pane.whole_screen && OWNER.load(Ordering::Acquire) != pid { return Err(EPERM); }
    let Some(r) = present::intersect(rect, Rect::new(0, 0, pane.width as i32, pane.height as i32))
        .and_then(present::clip_to_screen) else { return Ok(()); };
    // Finish every fallible user copy before changing backing, cursor or record.
    let row = r.w as usize * 4;
    let mut incoming = Vec::new();
    incoming.try_reserve_exact(row * r.h as usize).map_err(|_| ENOMEM)?;
    incoming.resize(row * r.h as usize, 0);
    for y in 0..r.h as usize {
        let src = pane.user_addr.checked_add((r.y as usize + y) as u64 * pane.user_stride as u64)
            .and_then(|p| p.checked_add(r.x as u64 * 4)).ok_or(EFAULT)?;
        crate::syscall::userptr::read_user_bytes(incoming[y * row..].as_mut_ptr(), src, row)
            .map_err(|_| EFAULT)?;
    }
    for p in incoming.chunks_exact_mut(4) { p[3] = 255; }
    let mut guard = STATE.lock();
    if pane.whole_screen && OWNER.load(Ordering::Acquire) != pid { return Err(EPERM); }
    let s = state(&mut guard);
    let previous = restore_panel(s);
    for y in 0..r.h as usize {
        let a = ((r.y as usize + y) * W + r.x as usize) * 4;
        s.pixels[a..a + row].copy_from_slice(&incoming[y * row..(y + 1) * row]);
    }
    s.record.serial = 0;
    s.record.window_count = 0;
    observation(s);
    let mut damage = vec![r];
    if let Some(r) = previous { damage.push(r); }
    publish(s, &damage, false)
}

/// Remove an overlay before changing its underlying pixels or saving an owner.
fn restore_panel(s: &mut State) -> Option<Rect> {
    let (px, py, saved, no_restore) = s.panel_cursor.take()?;
    if !no_restore {
        for yy in 0..18usize { for xx in 0..18usize {
            if px as usize + xx < W && py as usize + yy < H {
                let o = ((py as usize + yy) * W + px as usize + xx) * 4;
                let i = (yy * 18 + xx) * 4;
                s.pixels[o..o + 4].copy_from_slice(&saved[i..i + 4]);
            }
        }}
    }
    Some(Rect::new(px as i32, py as i32, 18, 18))
}

/// Panel cursor runs only on the render thread and never blocks on the display lock.
pub fn panel_pointer(x: u32, y: u32) {
    let Some(mut guard) = STATE.try_lock() else {
        return;
    };
    let s = state(&mut guard);
    if s.panel_cursor
        .as_ref()
        .is_some_and(|(px, py, _, _)| (*px, *py) == (x, y))
    {
        return;
    }
    let mut damage = Vec::new();
    if let Some(r) = restore_panel(s) { damage.push(r); }
    let mut saved = vec![0; 18 * 18 * 4];
    for yy in 0..18usize {
        for xx in 0..18usize {
            if x as usize + xx < W && y as usize + yy < H {
                let o = ((y as usize + yy) * W + x as usize + xx) * 4;
                let i = (yy * 18 + xx) * 4;
                saved[i..i + 4].copy_from_slice(&s.pixels[o..o + 4]);
            }
        }
    }
    sprite(
        &mut s.pixels,
        &abi::Cursor {
            x: x as i32,
            y: y as i32,
            visible: 1,
            shape: 0,
        },
    );
    let no_restore = crate::test_control::take(point::PANEL_CURSOR_NO_RESTORE, 0).is_some();
    s.panel_cursor = Some((x, y, saved, no_restore));
    damage.push(Rect::new(x as i32, y as i32, 18, 18));
    let _ = publish(s, &bounded_damage(damage), false);
}

/// Whether this target has a 1280 x 800 display the compositor can present on.
pub fn active() -> bool {
    Device::get().is_some() || Device::detect().is_some()
}

/// The kernel presented rectangle (x, y, w, h) of its canvas. On SVGA3 the screen target,
/// not the GOP framebuffer the kernel draws on, is what the device shows, so while the
/// kernel owns the display the rectangle is copied into the composed screen and presented.
/// Never blocks on the display lock: a composite in progress already owns the screen.
pub fn canvas_presented(x: u32, y: u32, w: u32, h: u32) {
    if Device::get() != Some(Device::Svga3) || OWNER.load(Ordering::Acquire) != 0 {
        return;
    }
    let Some(r) = present::clip_to_screen(Rect::new(x as i32, y as i32, w as i32, h as i32)) else {
        return;
    };
    let Some(mut guard) = STATE.try_lock() else {
        return;
    };
    let s = state(&mut guard);
    let Some(canvas) = Device::Svga3.canvas() else {
        return;
    };
    for y in r.y as usize..(r.y + r.h) as usize {
        let a = (y * W + r.x as usize) * 4;
        let b = a + r.w as usize * 4;
        Device::Svga3.copy_canvas(&mut s.pixels[a..b], &canvas[a..b]);
    }
    let _ = publish(s, &[r], false);
}
