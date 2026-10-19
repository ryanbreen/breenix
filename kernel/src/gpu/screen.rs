//! The kernel's screen on the core's display (`docs/console/contract.md` section 8, ruling F25):
//! one display-sized scanout target the core shows whenever no process owns the display.
//!
//! The screen console draws on the CPU into its shadow and copies the rows that changed into
//! this target's guest backing (`write`); `present` uploads those rows (TRANSFER_TO_HOST_3D),
//! binds the target to scanout 0 when it is not the binding, and flushes them. A process that
//! owns the display through the framebuffer interface (`fb_mmap` and its flushes) has its pane
//! copied into a private target and presented as its own frame (`present_pane`), so the
//! kernel does not write the firmware framebuffer after the core has shown a frame.
//!
//! A read-back (`settle`) presents the whole screen, requires the target to be the completed
//! binding of scanout 0, then poisons the backing and transfers the target back from the
//! device, so what the console then reads (`read`) is what the device scans out.
//!
//! The entries run inside a display transition, which serializes users of the target; the
//! spin lock only moves it in and out, and device waits run without it.

use super::*;
use alloc::vec;
use core::ops::Range;
use display::Binding;

/// The kernel screen's size: the display mode it returns to with no owner.
pub const WIDTH: u32 = display::MODES[display::KERNEL_MODE].width;
pub const HEIGHT: u32 = display::MODES[display::KERNEL_MODE].height;
const PITCH: usize = WIDTH as usize * 4;

static BACKING: conquer_once::spin::OnceCell<Arc<Pages>> = conquer_once::spin::OnceCell::uninit();
static TARGET: crate::irq_safe_mutex::IrqSafeMutex<Option<Target>> = crate::irq_safe_mutex::IrqSafeMutex::new(None);
/// The device resource of the kernel's screen, for MODES' `console_resource`.
static RESOURCE: AtomicU64 = AtomicU64::new(0);
/// Completed presents of the kernel's screen, for MODES' `console_draws`.
static PRESENTS: AtomicU64 = AtomicU64::new(0);

/// The target in the device: the private context it was created in, in one reset epoch.
struct Target {
    epoch: u64,
    context: u32,
    handle: u64,
    id: u32,
    pages: Arc<Pages>,
}

impl Target {
    fn new() -> Result<Target, i64> {
        open().ok_or(ENOMEM)?;
        let pages = BACKING.get().ok_or(ENODEV)?.clone();
        let (handle, _) = create_context(0)?;
        let (context, epoch) = match with_core(|c| Some((c.ctx_of(0, handle)?, c.epoch))).flatten() {
            Some(found) => found,
            None => {
                destroy_context(0, handle);
                return Err(EIO);
            }
        };
        let id = match console::resource_with(context, kind::TARGET, format::B8G8R8A8, WIDTH, HEIGHT, true, pages) {
            Ok(id) => id,
            Err(e) => {
                destroy_context(0, handle);
                return Err(e);
            }
        };
        RESOURCE.store(id as u64, Ordering::Release);
        Ok(Target { epoch, context, handle, id, pages: BACKING.get().ok_or(ENODEV)?.clone() })
    }

    fn live(&self) -> bool {
        with_core(|c| {
            c.epoch == self.epoch
                && c.contexts.get(&self.context).is_some_and(|x| x.state == CtxState::Live && x.device_alive)
                && c.resources.get(&self.id).is_some_and(|r| !r.lost)
        })
        .unwrap_or(false)
    }

    /// Whether the display shows this target: the completed, known binding of scanout 0.
    fn bound(&self) -> bool {
        with_core(|c| c.display.known && c.display.binding.is_some_and(|b| b.id == self.id)).unwrap_or(false)
    }

    /// Run fenced request `build` in the target's context and require its answer.
    fn fenced(&self, build: impl FnOnce(Tag, u64) -> Option<Request>) -> Result<Done, i64> {
        let fence = with_core(|c| c.device_fence()).ok_or(ENODEV)?;
        let d = exec_ok(|t| build(t, fence), self.context, &[self.id])?;
        if d.echoed != fence || !d.fenced {
            return Err(EIO);
        }
        Ok(d)
    }

    /// Upload pixel rows `rows` of the backing.
    fn upload(&self, rows: Range<u32>) -> Result<(), i64> {
        if rows.is_empty() {
            return Ok(());
        }
        let pages = &self.pages;
        let offset = rows.start as usize * PITCH;
        pages.maintain(offset, rows.len() * PITCH, false);
        let fence = with_core(|c| c.device_fence()).ok_or(ENODEV)?;
        let serial = with_core(|c| c.serial()).ok_or(ENODEV)?;
        let req = dev::transfer_to_host(
            Tag::Sync(serial),
            self.context,
            fence,
            self.id,
            pages.clone(),
            0,
            rows.start,
            WIDTH,
            rows.len() as u32,
            offset as u64,
            PITCH as u32,
        ).ok_or(EIO)?;
        let d = exec_req(serial, req, self.context, &[self.id], &[])?;
        if !dev::ok(d.answer) || d.echoed != fence || !d.fenced {
            return Err(EIO);
        }
        with_core(|c| {
            if let Some(r) = c.resources.get_mut(&self.id) {
                r.last = Some(LastWrite {
                    ctx: self.context,
                    device_ctx: dev::context_id(self.context),
                    fence,
                    serial,
                    answer: d.answer,
                    kind: dev::upload_kind(),
                });
                r.generation += 1;
                r.invalid = false;
            }
        });
        Ok(())
    }

    /// Show rows `rows` of the bound target.
    fn flush(&self, rows: Range<u32>) -> Result<(), i64> {
        if rows.is_empty() {
            return Ok(());
        }
        let (id, context, len) = (self.id, self.context, rows.len() as u32);
        self.fenced(|t, fence| dev::flush_rect(t, context, fence, id, 0, rows.start, WIDTH, len)).map(drop)
    }

    /// Bind `b.id`, a target of this size, to scanout 0 and flush it whole. The kernel's
    /// binding is complete once the flush answered; no `PRESENT` is outstanding meanwhile.
    fn bind(&self, mut b: Binding) -> Result<(), i64> {
        let (w, h) = with_core(|c| c.resources.get(&b.id).filter(|r| !r.lost).map(|r| (r.width, r.height)))
            .flatten()
            .ok_or(EIO)?;
        with_core(|c| {
            c.display.known = false;
            if c.display.holds.insert(b.id) {
                if let Some(r) = c.resources.get_mut(&b.id) {
                    r.holds += 1;
                }
            }
        });
        exec_ok(|t| dev::set_scanout(t, self.context, b.id, w, h), self.context, &[b.id])?;
        let fence = with_core(|c| c.device_fence()).ok_or(ENODEV)?;
        let d = exec_ok(|t| dev::flush_resource(t, self.context, fence, b.id, w, h), self.context, &[b.id])?;
        if d.echoed != fence || !d.fenced {
            return Err(EIO);
        }
        b.answer = d.answer;
        display::restored(b);
        Ok(())
    }

    /// Show the kernel's screen: bind it, the first time ending the firmware framebuffer's
    /// part (`console::retire_firmware`, then `console::handed_off` once a frame is shown).
    fn show(&self) -> Result<(), i64> {
        console::retire_firmware();
        self.bind(Binding { id: self.id, fence: 0, answer: 0 })?;
        console::handed_off();
        Ok(())
    }
}

impl Drop for Target {
    fn drop(&mut self) {
        with_core(|c| {
            if let Some(r) = c.resources.get_mut(&self.id) {
                r.dropped = true;
            }
            c.unhold(self.id);
        });
        if self.handle != 0 {
            destroy_context(0, self.handle);
        }
        let _ = RESOURCE.compare_exchange(self.id as u64, 0, Ordering::AcqRel, Ordering::Acquire);
    }
}

/// The target, taken out of its slot for the length of one transition.
struct Lease(Option<Target>);

impl Lease {
    fn take() -> Lease {
        Lease(super::panic::lock_irq(&TARGET).take())
    }

    /// The live target, created (and its whole backing uploaded) when the slot is empty or
    /// a reset lost it.
    fn ensure(&mut self) -> Result<&Target, i64> {
        if self.0.as_ref().is_none_or(|t| !t.live()) {
            self.0 = None;
            let target = Target::new()?;
            target.upload(0..HEIGHT)?;
            self.0 = Some(target);
        }
        self.0.as_ref().ok_or(EIO)
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        // Releasing a displaced target may wait for the device: drop it after the lock.
        let displaced = {
            let mut slot = super::panic::lock_irq(&TARGET);
            core::mem::replace(&mut *slot, self.0.take())
        };
        drop(displaced);
    }
}

/// Reserve the backing of the kernel's screen, once, on a boot whose display the core owns,
/// and give its size when the core owns the display and the frames were reserved.
pub fn open() -> Option<(usize, usize)> {
    if !display::active() {
        return None;
    }
    let pages = BACKING.try_get_or_init(|| Arc::new(Pages::new((PITCH * HEIGHT as usize).div_ceil(PAGE)).unwrap_or_else(Pages::none)));
    pages.ok().filter(|p| p.bytes() >= PITCH * HEIGHT as usize)?;
    Some((WIDTH as usize, HEIGHT as usize))
}

/// Copy `bytes` into the backing at byte `offset` (rows laid out `WIDTH * 4` bytes apart).
pub fn write(offset: usize, bytes: &[u8]) {
    if let Some(pages) = BACKING.get() {
        if offset + bytes.len() <= PITCH * HEIGHT as usize {
            pages.write(offset, bytes);
        }
    }
}

/// Copy the completed scanout's backing (the console's after `settle`, or an
/// owner's pane for framebuffer ReadPixel) at byte `offset` into `out`.
pub fn read(offset: usize, out: &mut [u8]) {
    let pages = with_core(|c| {
        let b = c.display.binding.filter(|_| c.display.known)?;
        Some(c.resources.get(&b.id)?.backing.clone())
    }).flatten();
    if let Some(pages) = pages {
        if offset + out.len() <= pages.bytes() {
            pages.read(offset, out);
        }
    }
}

fn rows_of(lines: Range<usize>) -> Range<u32> {
    (lines.start.min(HEIGHT as usize) as u32)..(lines.end.min(HEIGHT as usize) as u32)
}

/// The console's present of pixel rows `lines`, while no process owns the display. Returns
/// the number of the kernel screen's completed presents.
pub fn present(lines: Range<usize>) -> Result<u64, &'static str> {
    let _path = GpuPath::enter();
    let Some(_transition) = display::Transition::for_console() else {
        return Err("the display is owned or changing hands");
    };
    let mut lease = Lease::take();
    let target = lease.ensure().map_err(|_| "the GPU core could not create the screen's target")?;
    let rows = rows_of(lines);
    target.upload(rows.clone()).map_err(|_| "the upload of the screen's rows failed")?;
    let shown = if target.bound() { target.flush(rows) } else { target.show() };
    shown.map_err(|_| "the screen's target was not shown")?;
    Ok(PRESENTS.fetch_add(1, Ordering::AcqRel) + 1)
}

/// Present the whole screen, then read the bound target back from the device into the
/// backing, through both poisons' first: the resource and the present number read.
pub fn settle() -> Result<(u32, u64), &'static str> {
    let _path = GpuPath::enter();
    let Some(_transition) = display::Transition::for_console() else {
        return Err("the display is owned or changing hands");
    };
    let mut lease = Lease::take();
    let target = lease.ensure().map_err(|_| "the GPU core could not create the screen's target")?;
    target.upload(0..HEIGHT).map_err(|_| "the upload of the screen failed")?;
    let shown = if target.bound() { target.flush(0..HEIGHT) } else { target.show() };
    shown.map_err(|_| "the screen's target was not shown")?;
    let flush = PRESENTS.fetch_add(1, Ordering::AcqRel) + 1;
    if !target.bound() {
        return Err("the screen's target is not the completed binding of scanout 0");
    }
    let pages = BACKING.get().ok_or("the screen has no backing")?;
    let band = (READBACK_BAND / PITCH).max(1) as u32;
    let mut y = 0;
    while y < HEIGHT {
        let rows = band.min(HEIGHT - y);
        let (offset, len) = (y as usize * PITCH, rows as usize * PITCH);
        let saved = Pages::new(len.div_ceil(PAGE)).ok_or("no frames to preserve the screen's read-back band")?;
        let mut row = vec![0; PITCH];
        for n in 0..rows as usize {
            pages.read(offset + n * PITCH, &mut row);
            saved.write(n * PITCH, &row);
        }
        pages.fill(offset, len, POISON);
        pages.maintain(offset, len, false);
        let (id, context) = (target.id, target.context);
        let backing = pages.clone();
        let transferred = target
            .fenced(|t, fence| dev::transfer_from_host(t, context, fence, id, backing, 0, y, WIDTH, rows, offset as u64, PITCH as u32));
        pages.maintain(offset, len, true);
        if transferred.is_err() {
            for n in 0..rows as usize {
                saved.read(n * PITCH, &mut row);
                pages.write(offset + n * PITCH, &row);
            }
            return Err("the device did not transfer the screen back");
        }
        y += rows;
    }
    Ok((target.id, flush))
}

/// A framebuffer owner has its own resource and backing: nested owners must not
/// overwrite the pixels a previous binding will restore. The owner's hold survives
/// this temporary view; the ordinary resource retirement path releases it on pop.
fn pane_target(console: &Target, group: u64) -> Result<Target, i64> {
    let existing = with_core(|c| {
        let id = c.display.stack.last()?.pane?;
        let r = c.resources.get_mut(&id)?;
        if r.lost { return None; }
        r.holds += 1;
        Some((id, r.backing.clone()))
    }).flatten();
    let (id, pages) = if let Some(found) = existing {
        found
    } else {
        let pages = Arc::new(Pages::new((PITCH * HEIGHT as usize).div_ceil(PAGE)).ok_or(ENOMEM)?);
        // A first partial flush retains the screen as the owner first saw it.
        let mut row = vec![0; PITCH];
        for y in 0..HEIGHT as usize {
            console.pages.read(y * PITCH, &mut row);
            pages.write(y * PITCH, &row);
        }
        let id = console::resource_with(console.context, kind::TARGET, format::B8G8R8A8, WIDTH, HEIGHT, true, pages.clone())?;
        with_core(|c| {
            let o = c.display.stack.last_mut().filter(|o| o.group == group).expect("display transition retains owner");
            let old = o.pane.replace(id);
            c.resources.get_mut(&id).expect("new pane resource").holds += 1;
            if let Some(old) = old { c.unhold(old); }
        });
        (id, pages)
    };
    Ok(Target { epoch: console.epoch, context: console.context, handle: 0, id, pages })
}

/// Draw the calling process's framebuffer pane, at `pane` with `stride` bytes per row, onto
/// its retained target: rows `rows`, bytes `columns` of each, then present it as the
/// caller's frame. EPERM unless the caller owns the display.
///
/// # Safety
/// `pane` must map `rows.end` rows of `stride` bytes the caller may read.
pub unsafe fn present_pane(pane: *const u8, stride: usize, rows: Range<usize>, columns: Range<usize>) -> Result<(), i64> {
    let _path = GpuPath::enter();
    let tgid = caller()?;
    let _transition = display::Transition::for_observation(tgid)?;
    let mut lease = Lease::take();
    let target = pane_target(lease.ensure()?, tgid)?;
    let rows = rows_of(rows);
    let columns = columns.start.min(PITCH)..columns.end.min(PITCH);
    if rows.is_empty() || columns.is_empty() {
        return Ok(());
    }
    for y in rows.clone() {
        // SAFETY: the caller's contract; row `y` is inside the pane.
        let row = unsafe { core::slice::from_raw_parts(pane.add(y as usize * stride + columns.start), columns.len()) };
        target.pages.write(y as usize * PITCH + columns.start, row);
    }
    let uploaded = with_core(|c| c.resources.get(&target.id).is_some_and(|r| r.last.is_some())).unwrap_or(false);
    target.upload(if uploaded { rows.clone() } else { 0..HEIGHT })?;
    let current = with_core(|c| {
        c.display.known
            && c.display.binding.is_some_and(|b| b.id == target.id)
            && c.display.stack.last().is_some_and(|o| o.group == tgid && o.last.is_some_and(|b| b.id == target.id))
    })
    .unwrap_or(false);
    if current {
        return target.flush(rows);
    }
    // The owner's first frame on this target: a present of its own, so the ownership stack
    // keeps it as the owner's last frame and restores it when a later owner gives back.
    let seq = with_core(|c| {
        let d = &mut c.display;
        d.sent += 1;
        d.outstanding += 1;
        d.known = false;
        if d.holds.insert(target.id) {
            if let Some(r) = c.resources.get_mut(&target.id) {
                r.holds += 1;
            }
        }
        c.display.sent
    })
    .ok_or(ENODEV)?;
    let presenting = display::Presenting { id: target.id, seq, owner: tgid };
    let (id, context) = (target.id, target.context);
    let sent = exec_ok(|t| dev::set_scanout(t, context, id, WIDTH, HEIGHT), context, &[id])
        .and_then(|_| target.fenced(|t, fence| dev::flush_resource(t, context, fence, id, WIDTH, HEIGHT)));
    let answer = sent.as_ref().map_or(0, |d| d.answer);
    with_core(|c| c.presented(presenting, sent.is_ok(), 0, answer));
    notify();
    sent.map(drop)
}

/// Show the kernel's screen as it was left, inside a transition.
pub(super) fn show() -> Result<(), i64> {
    let mut lease = Lease::take();
    lease.ensure()?.show()?;
    PRESENTS.fetch_add(1, Ordering::AcqRel);
    Ok(())
}

/// The display came back to the kernel: show its screen again and let the console redraw it.
pub(super) fn restore() -> Result<(), i64> {
    show()?;
    crate::console::give_back_display();
    Ok(())
}

/// Bind an owner's saved frame again, in the kernel screen's context.
pub(super) fn bind_saved(b: Binding) -> Result<(), i64> {
    let mut lease = Lease::take();
    lease.ensure()?.bind(b)
}

/// MODES' kernel screen fields: its resource and its completed presents.
pub(super) fn describe() -> (u64, u64) {
    (dev::resource_id(RESOURCE.load(Ordering::Acquire) as u32).map_or(0, u64::from), PRESENTS.load(Ordering::Acquire))
}

/// Hold the target's lock for `f`: the panic path's acceptance runs it while holding the
/// locks a display user takes.
#[cfg(any(feature = "capture_panic_oracle", feature = "gpu_fault_acceptance"))]
pub(super) fn with_lock(f: impl FnOnce()) {
    let Some(_target) = TARGET.try_lock() else {
        return;
    };
    f();
}
