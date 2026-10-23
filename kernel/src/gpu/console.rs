//! The core's own console: a bitmap_text pipeline and a fixture glyph atlas, prepared once
//! the core shows the kernel's screen and drawn only by the terminal panic screen (`panic`,
//! gpu.md section 1.7.1). The kernel's screen itself is the screen console's (`screen`,
//! ruling F25). This module also keeps the firmware framebuffer's retirement and the handoff
//! that MODES reports, and the render thread's pump: panic preparation and the restores an
//! ended owner asks for.
use super::*;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use noto_sans_mono_bitmap::{get_raster, get_raster_width, FontWeight, RasterHeight};

pub(super) const WIDTH: u32 = 2560;
pub(super) const HEIGHT: u32 = 1600;
const BG: u32 = 0xff181512;
const FG: u32 = 0xffe8edf2;
const VB_BYTES: u32 = 128 * 1024;
static RESTORE: AtomicBool = AtomicBool::new(false);
/// The owner sequence a pending restore was requested for.
static RESTORE_SEQ: AtomicU64 = AtomicU64::new(0);
static RETIRED: AtomicBool = AtomicBool::new(false);
static HANDOFF: AtomicBool = AtomicBool::new(false);
static HASH: AtomicU64 = AtomicU64::new(0);
static FIXTURE: AtomicU64 = AtomicU64::new(0);
static PANIC_EPOCH: AtomicU64 = AtomicU64::new(u64::MAX);
static PANIC_ATTEMPT_EPOCH: AtomicU64 = AtomicU64::new(u64::MAX);
static PANIC_ATTEMPTS: AtomicU64 = AtomicU64::new(0);
const PANIC_MAX_ATTEMPTS: u64 = 3;
static NEXT_DRAW: AtomicU64 = AtomicU64::new(0);
/// When a failed restore is tried again.
static NEXT_RESTORE: AtomicU64 = AtomicU64::new(0);
const DRAW_INTERVAL_NS: u64 = 250_000_000;

pub(super) struct Console {
    pub(super) epoch: u64,
    pub(super) context: u32,
    handle: u64,
    pub(super) target: u32,
    pub(super) atlas: u32,
    pub(super) scratch: u32,
    pub(super) vertices: u32,
    pub(super) glyph_width: u32,
}

pub fn active() -> bool {
    RETIRED.load(Ordering::Acquire)
}
/// Ask the render thread to restore the frame of the owner stack as it is at owner sequence
/// `seq`.
pub(super) fn request_restore(seq: u64) {
    RESTORE_SEQ.store(seq, Ordering::Release);
    RESTORE.store(true, Ordering::Release);
}
/// Whether a restore is pending for owner sequence `seq`. A request made for an earlier
/// sequence is obsolete: a later change of owner (a take, a give-back, another ended owner)
/// decides what the display shows itself.
pub(super) fn restoring(seq: u64) -> bool {
    RESTORE.load(Ordering::Acquire) && RESTORE_SEQ.load(Ordering::Acquire) == seq
}
pub(super) fn restored_pending() {
    RESTORE.store(false, Ordering::Release);
}

pub(super) fn describe(a: &mut abi::Modes) {
    (a.console_resource, a.console_draws) = super::screen::describe();
    a.console_fixture_bytes = FIXTURE.load(Ordering::Acquire);
    a.handoff = HANDOFF.load(Ordering::Acquire) as u64;
    a.firmware_hash_at_handoff = HASH.load(Ordering::Acquire);
    a.firmware_hash_now = if active() && caller().is_ok_and(display::owner) {
        firmware_hash()
    } else {
        0
    };
    a.firmware_writes_after_handoff = crate::graphics::firmware_writes::count();
}

/// Inspect the actual framebuffer memory while its writer lock is held.
fn firmware_hash() -> u64 {
    let Some(fb) = crate::graphics::arm64_fb::SHELL_FRAMEBUFFER.get() else {
        return 0;
    };
    let guard = fb.lock();
    guard.firmware_hash()
}

/// The core is about to show the kernel's screen: the firmware framebuffer is no longer
/// shown, so no kernel writer may write it from here on. The screen console draws through
/// the core (`screen`) and keeps drawing.
pub(super) fn retire_firmware() {
    if RETIRED.load(Ordering::Acquire) {
        return;
    }
    // Quiesce direct canvas writers under the framebuffer lock before retiring it.
    if let Some(fb) = crate::graphics::arm64_fb::SHELL_FRAMEBUFFER.get() {
        let _guard = fb.lock();
        RETIRED.store(true, Ordering::Release);
    } else {
        RETIRED.store(true, Ordering::Release);
    }
}

/// The first frame of the kernel's screen completed: count firmware writes from here on.
pub(super) fn handed_off() {
    if !HANDOFF.load(Ordering::Acquire) {
        HASH.store(firmware_hash(), Ordering::Release);
        crate::graphics::firmware_writes::handoff();
        HANDOFF.store(true, Ordering::Release);
    }
}

fn pitch_of(kind_: u32, fmt: u32, w: u32) -> u32 {
    if kind_ == kind::BUFFER {
        w
    } else if dev::is_svga() {
        w * format::bytes(fmt)
    } else {
        (w * format::bytes(fmt)).next_multiple_of(256)
    }
}

/// A kernel resource with fresh backing of its size when `backed`, else without backing.
fn resource(
    ctx: u32,
    kind_: u32,
    fmt: u32,
    w: u32,
    h: u32,
    scanout: bool,
    backed: bool,
) -> Result<u32, i64> {
    let bytes = if backed {
        pitch_of(kind_, fmt, w) as usize * h as usize
    } else {
        0
    };
    let pages = Arc::new(Pages::new(bytes.div_ceil(PAGE)).ok_or(ENOMEM)?);
    resource_with(ctx, kind_, fmt, w, h, scanout, pages)
}

/// A kernel resource has no process handle; its backing (`pages`, attached when it has any
/// frames) and its device holds use the same resource table as clients, so reset and DMA
/// lifetime use the core's rules.
pub(super) fn resource_with(
    ctx: u32,
    kind_: u32,
    fmt: u32,
    w: u32,
    h: u32,
    scanout: bool,
    pages: Arc<Pages>,
) -> Result<u32, i64> {
    let pitch = pitch_of(kind_, fmt, w);
    let backed = pages.count() > 0;
    let id = with_core(|c| {
        let id = c.next_res;
        c.next_res += 1;
        let mut r = new_res(0, kind_, fmt, w, h, 0, pitch, pages.clone(), c.epoch);
        r.scanout = scanout;
        r.holds = 1;
        c.insert_res(id, r);
        id
    })
    .ok_or(ENODEV)?;
    let result = (|| {
        exec_ok(|t| dev::resource_create(t, id, kind_, fmt, w, h, scanout), 0, &[id])?;
        if backed {
            exec_ok(|t| dev::attach_backing(t, id, pages.clone()), 0, &[id])?;
        }
        exec_ok(|t| Some(dev::ctx_attach(t, ctx, id)), ctx, &[id])?;
        Ok(id)
    })();
    if result.is_err() {
        with_core(|c| {
            if let Some(r) = c.resources.get_mut(&id) {
                r.dropped = true;
            }
            c.unhold(id);
        });
    }
    result
}

struct Building {
    ids: Vec<u32>,
    armed: bool,
    handle: u64,
}
impl Drop for Building {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        with_core(|c| {
            for &id in &self.ids {
                if let Some(r) = c.resources.get_mut(&id) {
                    r.dropped = true;
                }
                c.unhold(id);
            }
        });
        destroy_context(0, self.handle);
    }
}

impl Drop for Console {
    fn drop(&mut self) {
        with_core(|c| {
            for id in [self.target, self.atlas, self.scratch, self.vertices] {
                if let Some(r) = c.resources.get_mut(&id) {
                    r.dropped = true;
                }
                c.unhold(id);
            }
        });
        destroy_context(0, self.handle);
    }
}

impl Console {
    pub(super) fn new() -> Result<Self, i64> {
        let (handle, _) = create_context(0)?;
        let mut build = Building {
            ids: Vec::new(),
            armed: true,
            handle,
        };
        let (ctx, epoch) = with_core(|c| {
            let id = c.ctx_of(0, handle).ok_or(EIO)?;
            // The private console context compiles its programs on demand; kgpu warms
            // client contexts, whose catalogue contract is unchanged.
            Ok::<_, i64>((id, c.epoch))
        })
        .ok_or(ENODEV)??;
        let gw = get_raster_width(FontWeight::Regular, RasterHeight::Size16) as u32;
        let target = resource(
            ctx,
            kind::TARGET,
            format::B8G8R8A8,
            WIDTH,
            HEIGHT,
            true,
            cfg!(any(feature = "capture_panic_oracle", feature = "gpu_fault_acceptance")),
        )?;
        build.ids.push(target);
        let atlas = resource(ctx, kind::TEXTURE, format::R8, gw * 95, 16, false, true)?;
        build.ids.push(atlas);
        let scratch = resource(
            ctx,
            kind::TEXTURE,
            format::B8G8R8A8,
            WIDTH,
            HEIGHT,
            false,
            false,
        )?;
        build.ids.push(scratch);
        let vertices = resource(ctx, kind::BUFFER, format::R8, VB_BYTES, 1, false, true)?;
        build.ids.push(vertices);
        let (pages, pitch) = with_core(|c| {
            c.resources
                .get(&atlas)
                .map(|r| (r.backing.clone(), r.pitch))
        })
        .flatten()
        .ok_or(EIO)?;
        for ch in 32u8..=126 {
            if let Some(g) = get_raster(ch as char, FontWeight::Regular, RasterHeight::Size16) {
                for (y, row) in g.raster().iter().enumerate() {
                    pages.write(y * pitch as usize + (ch - 32) as usize * gw as usize, row);
                }
            }
        }
        pages.maintain(0, pitch as usize * 16, false);
        let fence = with_core(|c| c.device_fence()).ok_or(ENODEV)?;
        let d = exec_ok(
            |t| dev::transfer_to_host(t, ctx, fence, atlas, pages.clone(), 0, 0, gw * 95, 16, 0, pitch),
            ctx,
            &[atlas],
        )?;
        if !d.matches_fence(fence) {
            return Err(EIO);
        }
        FIXTURE.fetch_add((gw * 95 * 16) as u64, Ordering::Release);
        build.armed = false;
        Ok(Self {
            epoch,
            context: ctx,
            handle,
            target,
            atlas,
            scratch,
            vertices,
            glyph_width: gw,
        })
    }
    fn text(&self, s: &mut Vec<u32>, text: &[u8], y: i32) {
        let mut instances = Vec::new();
        for (i, &ch) in text.iter().enumerate() {
            if !(33..=126).contains(&ch) {
                continue;
            }
            let x = 16 + i as i32 * self.glyph_width as i32;
            let mut params = [0u32; 16];
            params[..8].copy_from_slice(&[
                x as u32,
                y as u32,
                (ch - 32) as u32 * self.glyph_width,
                0,
                self.glyph_width,
                16,
                1,
                1,
            ]);
            instances.push(abi::draw::Instance {
                rect: [x, y, x + self.glyph_width as i32, y + 16],
                params,
            });
        }
        if instances.is_empty() {
            return;
        }
        abi::draw::encode(
            s,
            abi::pipeline::BITMAP_TEXT,
            FG,
            self.atlas as u64,
            0,
            0,
            &instances,
        );
    }
    /// Draw the console's offscreen frame, so its target has a completed write.
    pub(super) fn draw(&mut self) -> Result<(), i64> {
        let mut s = Vec::new();
        stream::encode::begin(&mut s, self.target as u64);
        stream::encode::clear(&mut s, BG);
        self.text(&mut s, b"Breenix GPU console", 16);
        let requests: Vec<Request> = if dev::is_svga() {
            let mut o = with_core(|c| c.contexts.get(&self.context).map(|x| x.dx.clone()))
                .flatten()
                .ok_or(EIO)?;
            let extras = || svga::encode::Extras {
                scratch: [[WIDTH, HEIGHT], [0, 0]],
                data_width: 1,
                corrupt: None,
                suppress: None,
                swap: None,
            };
            let (sized, _) = svga::encode::translate(&mut o.clone(), &[], s.as_slice(), self, extras(), None, None).ok_or(ENOMEM)?;
            let backing = svga::encode::Backing::allocate(&sized.sizes).ok_or(ENOMEM)?;
            let (out, _) = svga::encode::translate(&mut o, &[], s.as_slice(), self, extras(), Some(backing), None).ok_or(ENOMEM)?;
            with_core(|c| {
                if let Some(ctx) = c.contexts.get_mut(&self.context) {
                    ctx.dx = o;
                }
            });
            let mut reqs = Vec::new();
            for chunk in out.chunks {
                let (serial, fence) = with_core(|c| (c.serial(), c.device_fence())).ok_or(ENODEV)?;
                reqs.push(dev::submit_svga(Tag::Sync(serial), self.context, Some(fence), chunk).ok_or(EIO)?);
            }
            reqs
        } else {
            let mut o = with_core(|c| c.contexts.get(&self.context).map(|x| x.objects.clone()))
                .flatten()
                .ok_or(EIO)?;
            let mut enc = encode::Enc::new();
            encode::translate(
                &mut enc,
                &mut o,
                s.as_slice(),
                self,
                encode::Extras {
                    vertex_buffer: self.vertices,
                    scratch: [self.scratch, 0],
                    data: 0,
                    data_width: 0,
                    flip_y: FLIP_Y,
                    corrupt: None,
                    suppress: None,
                    swap: None,
                },
            );
            let chunks = enc.finish().ok_or(ENOMEM)?;
            with_core(|c| {
                if let Some(ctx) = c.contexts.get_mut(&self.context) {
                    ctx.objects = o;
                }
            });
            let mut reqs = Vec::new();
            for words in chunks {
                let (serial, fence) = with_core(|c| (c.serial(), c.device_fence())).ok_or(ENODEV)?;
                reqs.push(dev::submit_virgl(Tag::Sync(serial), self.context, Some(fence), words));
            }
            reqs
        };
        let mut last = None;
        let device_ctx = dev::context_id(self.context);
        for req in requests {
            let Tag::Sync(serial) = req.tag() else { return Err(EIO) };
            let fence = req.fence();
            let d = exec_req(
                serial,
                req,
                self.context,
                &[self.target, self.atlas, self.scratch, self.vertices],
                &[],
            )?;
            if !dev::ok(d.answer) || !fence.is_some_and(|f| d.matches_fence(f)) {
                return Err(EIO);
            }
            last = Some(LastWrite {
                ctx: self.context,
                device_ctx,
                fence: d.echoed,
                serial,
                answer: d.answer,
                kind: dev::draw_kind(),
            });
        }
        with_core(|c| {
            if let Some(r) = c.resources.get_mut(&self.target) {
                r.last = last;
                r.generation += 1;
                r.invalid = false;
            }
        });
        Ok(())
    }
}
impl encode::Bindings for Console {
    fn target(&self, h: u64) -> Option<encode::TargetRes> {
        (h == self.target as u64).then_some(encode::TargetRes {
            id: self.target,
            format: wire::format::B8G8R8A8_UNORM,
            info: TargetInfo {
                width: WIDTH,
                height: HEIGHT,
                invalid: false,
                format: format::B8G8R8A8,
            },
        })
    }
    fn source(&self, h: u64) -> Option<encode::SourceRes> {
        (h == self.atlas as u64).then_some(encode::SourceRes {
            id: self.atlas,
            format: wire::format::R8_UNORM,
            width: self.glyph_width * 95,
            height: 16,
        })
    }
    fn buffer_rect(&self, _h: u64, _offset: u64) -> [i32; 4] {
        [0; 4]
    }
    fn buffer_word(&self, _h: u64, _offset: u64) -> u32 {
        0
    }
}

/// Called on the render thread once the core owns the display: prepares the panic screen
/// after the kernel's screen was first shown, and makes the restores ended owners asked for.
pub fn pump() {
    let _path = GpuPath::enter();
    if !display::active() {
        return;
    }
    let now = now_ns();
    // Preparation is attempted at most once per interval, construction failures included.
    // The private target is drawn offscreen during preparation. Prepare outside
    // display transitions, including while a process owns the display after reset.
    if now >= NEXT_DRAW.load(Ordering::Acquire) && HANDOFF.load(Ordering::Acquire) {
        NEXT_DRAW.store(now.saturating_add(DRAW_INTERVAL_NS), Ordering::Release);
        if let Some(epoch) = with_core(|c| {
            (!c.stalled && !c.resetting && !c.reset_wanted).then_some(c.epoch)
        }).flatten() {
            if PANIC_ATTEMPT_EPOCH.swap(epoch, Ordering::AcqRel) != epoch {
                PANIC_ATTEMPTS.store(0, Ordering::Release);
            }
            if PANIC_EPOCH.load(Ordering::Acquire) != epoch
                && PANIC_ATTEMPTS.load(Ordering::Acquire) < PANIC_MAX_ATTEMPTS
            {
                PANIC_ATTEMPTS.fetch_add(1, Ordering::AcqRel);
                if super::panic::prepare().is_ok() {
                    PANIC_EPOCH.store(epoch, Ordering::Release);
                }
                #[cfg(any(feature = "capture_panic_oracle", feature = "gpu_fault_acceptance"))]
                super::panic::preparation_finished();
            }
        }
    }
    // A pending restore is made on the next pass, not after the preparation interval: the
    // ended owner's frame stays bound until it is (#2110). A failed restore waits an interval.
    if RESTORE.load(Ordering::Acquire) && now >= NEXT_RESTORE.load(Ordering::Acquire) {
        if display::restore_pending().is_err() {
            NEXT_RESTORE.store(now.saturating_add(DRAW_INTERVAL_NS), Ordering::Release);
            return;
        }
        notify();
    }
}

#[cfg(any(feature = "capture_panic_oracle", feature = "gpu_fault_acceptance"))]
pub(super) fn panic_with_lock(f: impl FnOnce()) {
    if !HANDOFF.load(Ordering::Acquire) {
        return;
    }
    super::screen::with_lock(f);
}
