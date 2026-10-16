//! ARM64 virtio 2D composition and atomic display observation (BreenGL §§18,20).
//! All allocation and pixel work runs in thread context with IRQs enabled. No display
//! lock is taken by an interrupt; the render thread only tries it for the panel cursor.
use crate::drivers::virtio::gpu_mmio as gpu;
use crate::window::{
    mem,
    present::{self, Backend, Composed, Failure, Frame},
    Caller,
};
use alloc::{vec, vec::Vec};
use breengel_abi::{self as abi, errno::*, tc_point as point, Plain, Rect};
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

const W: usize = 1280;
const H: usize = 800;
const LEN: usize = W * H * 4;
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

pub fn init() {
    if gpu::dimensions() == Some((W as u32, H as u32)) {
        present::set_backend(&DISPLAY);
    }
}
fn state(s: &mut Option<State>) -> &mut State {
    s.get_or_insert_with(|| {
        let pixels = gpu::framebuffer().map_or_else(|| vec![0; LEN], |p| p[..LEN].to_vec());
        State {
            pixels,
            record: abi::ObservationRecord::ZERO,
            invalid: 0,
            repair: false,
            last_fence: gpu::last_answer_fence(),
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
    let fb = gpu::framebuffer().ok_or(EIO)?;
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
    let (resource, answers) = gpu::present_damage(damage, skip, wrong, stale).map_err(|_| EIO)?;
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
    let missing = answers.len() != damage.len() + usize::from(!damage.is_empty());
    let mut serial_mismatch = false;
    for answer in &answers {
        serial_mismatch |= answer.fence <= s.last_fence;
        s.last_fence = s.last_fence.max(answer.fence);
    }
    s.invalid = if missing {
        EAGAIN
    } else if resource != gpu::composed_resource() || serial_mismatch {
        EIO
    } else {
        0
    };
    let corrupt = faults && crate::test_control::take(point::CORRUPT_OUTSIDE_DAMAGE, 0).is_some();
    s.repair |= s.invalid != 0 || corrupt;
    if corrupt {
        // Corrupt the actual destination backing outside this frame's damage.
        fb[(H - 1) * W * 4] ^= 0xFF;
        s.pixels[(H - 1) * W * 4] = fb[(H - 1) * W * 4];
    }
    Ok(())
}
fn observation(s: &mut State) {
    s.record.backend = abi::backend::VIRTIO_GPU_2D;
    s.record.width = W as u32;
    s.record.height = H as u32;
    s.record.stride = W as u32 * 4;
    s.record.source_format = 1;
    s.record.flags = abi::OBSERVATION_DEVICE_COPY_OUTSIDE;
    s.record.boundary = 1;
}
fn bounded_damage(mut d: Vec<Rect>) -> Vec<Rect> {
    d = d.into_iter().filter_map(present::clip_to_screen).collect();
    // ObservationRecord has room for eleven transfers and their final flush. When
    // metadata is larger, a covering transfer updates the device's full backing;
    // composition work still uses exactly the original damage union.
    if d.len() > 11 {
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
        abi::backend::VIRTIO_GPU_2D
    }
    fn composite(&self, f: &Frame) -> Result<Composed, Failure> {
        if crate::test_control::take(point::BACKEND_ERROR, 0).is_some() {
            return Err(Failure { reason: 1 });
        }
        let mut guard = STATE.lock();
        let s = state(&mut guard);
        s.panel_cursor = None;
        let mut damage = if s.repair {
            s.repair = false;
            vec![full()]
        } else {
            f.damage.clone()
        };
        damage.append(&mut s.cursor_damage);
        if s.cursor.visible != 0 {
            damage.push(cursor_rect(&s.cursor));
        }
        if crate::test_control::take(point::IGNORE_DAMAGE, 0).is_some() {
            damage = vec![full()];
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
        for (i, w) in f.windows.iter().enumerate() {
            s.record.windows[i] = abi::ObservedWindow {
                id: w.id,
                commit_id: w.commit_id,
            };
        }
        publish(s, &bounded_damage(damage), true).map_err(|e| Failure { reason: e as u32 })?;
        Ok(Composed { pixels, per_window })
    }
    fn readback(&self, _caller: &Caller, rb: &mut abi::Readback) -> Result<(), i32> {
        let mut guard = STATE.lock();
        let s = state(&mut guard);
        observation(s);
        // Read the entire actual bound resource's backing, under the same lock as
        // composite publication, with X normalized. Never a sampled band.
        let source = gpu::framebuffer().ok_or(EIO)?;
        let mut out = source[..LEN].to_vec();
        for p in out.chunks_exact_mut(4) {
            p[3] = 255;
        }
        rb.serial = s.record.serial;
        if s.invalid != EAGAIN {
            mem::copy_out(rb.record, unsafe {
                core::slice::from_raw_parts(
                    (&s.record as *const abi::ObservationRecord).cast::<u8>(),
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
    let mut guard = STATE.lock();
    let s = state(&mut guard);
    let previous = OWNER.load(Ordering::Acquire);
    if previous == pid {
        return Ok(());
    }
    let snapshot = gpu::framebuffer().ok_or(EIO)?[..LEN].to_vec();
    s.owners.push((previous, snapshot, s.record));
    OWNER.store(pid, Ordering::Release);
    crate::syscall::fb::DISPLAY_OWNER_PID.store(pid, Ordering::Release);
    Ok(())
}
pub fn give(from:u64,to:u64)->Result<(),i32> {
    if OWNER.load(Ordering::Acquire)!=from{return Err(EPERM);}
    let live=crate::process::with_process_manager(|m|m.get_process(crate::process::ProcessId::new(to)).is_some_and(|p|!p.is_terminated())).unwrap_or(false);
    if !live{return Err(ESRCH);}
    take(to)?;
    let live=crate::process::with_process_manager(|m|{
        m.get_process_mut(crate::process::ProcessId::new(to)).is_some_and(|p|{
            if p.is_terminated(){return false;}
            p.has_display_ownership=true;true
        })
    }).unwrap_or(false);
    if !live {gone(to);return Err(ESRCH);}Ok(())
}

pub fn back(pid: u64) -> Result<(), i32> {
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
}

/// Copy a caller's private mmap pane, clipped before touching any pixels.
pub fn flush(pid: u64, ptr: u64, stride: usize, rect: Rect) -> Result<(), i32> {
    if OWNER.load(Ordering::Acquire) != pid {
        return Err(EPERM);
    }
    let Some(r) = present::clip_to_screen(rect) else {
        return Ok(());
    };
    let mut guard = STATE.lock();
    let s = state(&mut guard);
    s.panel_cursor = None;
    s.record.serial = 0;
    s.record.window_count = 0;
    for y in r.y as usize..(r.y + r.h) as usize {
        let a = (y * W + r.x as usize) * 4;
        let b = a + r.w as usize * 4;
        let src = ptr + y as u64 * stride as u64 + r.x as u64 * 4;
        crate::syscall::userptr::read_user_bytes(s.pixels[a..b].as_mut_ptr(), src, b - a)
            .map_err(|_| EFAULT)?;
        for p in s.pixels[a..b].chunks_exact_mut(4) {
            p[3] = 255;
        }
    }
    observation(s);
    publish(s, &[r], false)
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
    if let Some((px, py, saved, no_restore)) = s.panel_cursor.take() {
        if !no_restore {
            for yy in 0..18usize {
                for xx in 0..18usize {
                    if px as usize + xx < W && py as usize + yy < H {
                        let o = ((py as usize + yy) * W + px as usize + xx) * 4;
                        let i = (yy * 18 + xx) * 4;
                        s.pixels[o..o + 4].copy_from_slice(&saved[i..i + 4]);
                    }
                }
            }
        }
        damage.push(Rect::new(px as i32, py as i32, 18, 18));
    }
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

/// Whether this target registered the software display backend.
pub fn active() -> bool {
    gpu::dimensions() == Some((W as u32, H as u32))
}
