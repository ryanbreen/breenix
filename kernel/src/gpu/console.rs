//! Kernel console rendered with the core's bitmap_text pipeline and a fixture glyph atlas.
use super::*;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use noto_sans_mono_bitmap::{get_raster, get_raster_width, FontWeight, RasterHeight};

const WIDTH: u32 = 2560;
const HEIGHT: u32 = 1600;
const BG: u32 = 0xff181512;
const FG: u32 = 0xffe8edf2;
const LINES: usize = 16;
const COLUMNS: usize = 96;
const VB_BYTES: u32 = 128 * 1024;
static CONSOLE: spin::Mutex<Option<Console>> = spin::Mutex::new(None);
static RESTORE: AtomicBool = AtomicBool::new(false);
static RETIRED: AtomicBool = AtomicBool::new(false);
static HANDOFF: AtomicBool = AtomicBool::new(false);
static HASH: AtomicU64 = AtomicU64::new(0);
static DRAWN: AtomicU64 = AtomicU64::new(0);
static FIXTURE: AtomicU64 = AtomicU64::new(0);
static RESOURCE: AtomicU64 = AtomicU64::new(0);

struct Console {
    epoch: u64,
    context: u32,
    target: u32,
    atlas: u32,
    scratch: u32,
    vertices: u32,
    glyph_width: u32,
    lines: [[u8; COLUMNS]; LINES],
    row: usize,
    column: usize,
}

pub fn active() -> bool {
    RETIRED.load(Ordering::Acquire)
}
pub(super) fn request_restore() {
    RESTORE.store(true, Ordering::Release);
}
pub(super) fn restoring() -> bool {
    RESTORE.load(Ordering::Acquire)
}
pub(super) fn restored_pending() {
    RESTORE.store(false, Ordering::Release);
}

pub(super) fn describe(a: &mut abi::Modes) {
    a.console_resource = RESOURCE.load(Ordering::Acquire);
    a.console_draws = DRAWN.load(Ordering::Acquire);
    a.console_fixture_bytes = FIXTURE.load(Ordering::Acquire);
    a.handoff = HANDOFF.load(Ordering::Acquire) as u64;
    a.firmware_hash_at_handoff = HASH.load(Ordering::Acquire);
    a.firmware_hash_now = if active() { firmware_hash() } else { 0 };
}

/// Inspect the actual framebuffer memory while its writer lock is held.
fn firmware_hash() -> u64 {
    let Some(fb) = crate::graphics::arm64_fb::SHELL_FRAMEBUFFER.get() else {
        return 0;
    };
    let guard = fb.lock();
    guard.firmware_hash()
}

fn quiesce() {
    if RETIRED.load(Ordering::Acquire) {
        return;
    }
    crate::graphics::boot_screen::stop_drawing();
    crate::graphics::log_console::stop();
    // Quiesce direct canvas writers under the framebuffer lock before retiring it.
    if let Some(fb) = crate::graphics::arm64_fb::SHELL_FRAMEBUFFER.get() {
        let _guard = fb.lock();
        RETIRED.store(true, Ordering::Release);
    } else {
        RETIRED.store(true, Ordering::Release);
    }
    HASH.store(firmware_hash(), Ordering::Release);
}

fn handoff() {
    HANDOFF.store(true, Ordering::Release);
    crate::graphics::log_capture::init();
}

/// A console resource has no process handle; its backing is frames and its device holds
/// use the same resource table as clients, so reset and DMA lifetime use the core's rules.
fn resource(
    ctx: u32,
    kind_: u32,
    fmt: u32,
    w: u32,
    h: u32,
    scanout: bool,
    backed: bool,
) -> Result<u32, i64> {
    let pitch = if kind_ == kind::BUFFER {
        w
    } else {
        (w * format::bytes(fmt)).next_multiple_of(256)
    };
    let bytes = if backed {
        pitch as usize * h as usize
    } else {
        0
    };
    let pages = Arc::new(Pages::new(bytes.div_ceil(PAGE)).ok_or(ENOMEM)?);
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
    let target = if kind_ == kind::BUFFER {
        wire::target::BUFFER
    } else {
        wire::target::TEXTURE_2D
    };
    let bind = if kind_ == kind::BUFFER {
        wire::bind::VERTEX_BUFFER
    } else {
        wire::bind::SAMPLER_VIEW
            | if kind_ == kind::TARGET {
                wire::bind::RENDER_TARGET
            } else {
                0
            }
            | if scanout { wire::bind::SCANOUT } else { 0 }
    };
    let result = (|| {
        exec_ok(
            |t| {
                Some(virgl::resource_create(
                    t,
                    id,
                    target,
                    virgl_format(fmt),
                    bind,
                    w,
                    h,
                ))
            },
            0,
            &[id],
        )?;
        if backed {
            exec_ok(|t| virgl::attach_backing(t, id, pages.clone()), 0, &[id])?;
        }
        exec_ok(|t| Some(virgl::ctx_attach(t, ctx, id)), ctx, &[id])?;
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
            c.reclaim(0);
        });
    }
}

impl Console {
    fn new() -> Result<Self, i64> {
        let (handle, _) = create_context(0)?;
        let mut build = Building {
            ids: Vec::new(),
            armed: true,
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
            true,
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
            |t| {
                Some(virgl::transfer_to_host(
                    t,
                    ctx,
                    fence,
                    atlas,
                    pages.clone(),
                    0,
                    0,
                    gw * 95,
                    16,
                    0,
                    pitch,
                ))
            },
            ctx,
            &[atlas],
        )?;
        if d.echoed != fence || d.resp.u32_at(wire::HDR_FLAGS) & wire::FLAG_FENCE == 0 {
            return Err(EIO);
        }
        FIXTURE.fetch_add((gw * 95 * 16) as u64, Ordering::Release);
        RESOURCE.store(target as u64, Ordering::Release);
        build.armed = false;
        Ok(Self {
            epoch,
            context: ctx,
            target,
            atlas,
            scratch,
            vertices,
            glyph_width: gw,
            lines: [[b' '; COLUMNS]; LINES],
            row: 0,
            column: 0,
        })
    }
    fn live(&self) -> bool {
        with_core(|c| {
            c.epoch == self.epoch
                && c.contexts
                    .get(&self.context)
                    .is_some_and(|x| x.state == CtxState::Live && x.device_alive)
                && c.resources.get(&self.target).is_some_and(|r| !r.lost)
        })
        .unwrap_or(false)
    }
    fn text(&self, s: &mut Vec<u32>, text: &[u8], y: i32) -> u64 {
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
            return 0;
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
        1
    }
    fn draw(&mut self) -> Result<(), i64> {
        let mut s = Vec::new();
        stream::encode::begin(&mut s, self.target as u64);
        stream::encode::clear(&mut s, BG);
        let mut draws = self.text(&mut s, b"Breenix GPU console", 16);
        for (i, line) in self.lines.iter().enumerate() {
            draws += self.text(&mut s, line, 48 + i as i32 * 18);
        }
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
        let mut last = None;
        for words in chunks {
            let (serial, fence) = with_core(|c| (c.serial(), c.device_fence())).ok_or(ENODEV)?;
            let d = exec_req(
                serial,
                virgl::submit(Tag::Sync(serial), self.context, Some(fence), words),
                self.context,
                &[self.target, self.atlas, self.scratch, self.vertices],
                &[],
            )?;
            if !virgl::ok(d.answer)
                || d.echoed != fence
                || d.resp.u32_at(wire::HDR_FLAGS) & wire::FLAG_FENCE == 0
            {
                return Err(EIO);
            }
            last = Some(LastWrite {
                ctx: self.context,
                fence,
                serial,
                answer: d.answer,
                kind: wire::cmd::SUBMIT_3D,
            });
        }
        with_core(|c| {
            if let Some(r) = c.resources.get_mut(&self.target) {
                r.last = last;
                r.generation += 1;
                r.invalid = false;
            }
        });
        quiesce();
        self.bind(display::Binding {
            id: self.target,
            fence: 0,
            answer: 0,
        })?;
        DRAWN.fetch_add(draws, Ordering::Release);
        handoff();
        Ok(())
    }
    fn bind(&self, mut b: display::Binding) -> Result<(), i64> {
        let (w, h) = with_core(|c| {
            c.resources
                .get(&b.id)
                .filter(|r| !r.lost)
                .map(|r| (r.width, r.height))
        })
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
        exec_ok(
            |t| Some(virgl::set_scanout(t, self.context, b.id, w, h)),
            self.context,
            &[b.id],
        )?;
        let fence = with_core(|c| c.device_fence()).ok_or(ENODEV)?;
        let d = exec_ok(
            |t| Some(virgl::flush_resource(t, self.context, fence, b.id, w, h)),
            self.context,
            &[b.id],
        )?;
        if d.echoed != fence || d.resp.u32_at(wire::HDR_FLAGS) & wire::FLAG_FENCE == 0 {
            return Err(EIO);
        }
        b.answer = d.answer;
        display::restored(b);
        Ok(())
    }
    fn bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            if b == b'\r' {
                continue;
            }
            if b == b'\n' || self.column == COLUMNS {
                self.column = 0;
                if self.row + 1 == LINES {
                    self.lines.copy_within(1.., 0);
                    self.lines[LINES - 1] = [b' '; COLUMNS];
                } else {
                    self.row += 1;
                }
                if b == b'\n' {
                    continue;
                }
            }
            self.lines[self.row][self.column] = if (32..=126).contains(&b) { b } else { b'?' };
            self.column += 1;
        }
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

pub(super) fn ensure() -> Result<(), i64> {
    let mut guard = CONSOLE.lock();
    if guard.as_ref().is_none_or(|c| !c.live()) {
        // A reset retires these old device objects; confirmed-reset resources are safe
        // to release through the normal dropped-resource path.
        if let Some(old) = guard.take() {
            RESOURCE.store(0, Ordering::Release);
            with_core(|c| {
                for id in [old.target, old.atlas, old.scratch, old.vertices] {
                    if let Some(r) = c.resources.get_mut(&id) {
                        r.dropped = true;
                    }
                    c.unhold(id);
                }
                c.reclaim(0);
            });
        }
        let mut console = Console::new()?;
        console.draw()?;
        *guard = Some(console);
    }
    Ok(())
}
pub(super) fn redraw() -> Result<(), i64> {
    ensure()?;
    CONSOLE.lock().as_mut().ok_or(EIO)?.draw()
}
pub(super) fn bind_saved(b: display::Binding) -> Result<(), i64> {
    ensure()?;
    CONSOLE.lock().as_ref().ok_or(EIO)?.bind(b)
}
pub(super) fn observation_context(id: u32) -> Result<u32, i64> {
    ensure()?;
    let ctx = CONSOLE.lock().as_ref().ok_or(EIO)?.context;
    exec_ok(|t| Some(virgl::ctx_attach(t, ctx, id)), ctx, &[id])?;
    Ok(ctx)
}
/// Called on the render thread. It replaces its legacy CPU console loop after handoff.
pub fn pump() {
    let _path = GpuPath::enter();
    if !display::active() {
        return;
    }
    if RESTORE.load(Ordering::Acquire) {
        let _ = display::restore_pending();
        notify();
    }
    let Some(_transition) = display::Transition::for_console() else {
        return;
    };
    if ensure().is_err() {
        return;
    }
    let Some(mut guard) = CONSOLE.try_lock() else {
        return;
    };
    let Some(c) = guard.as_mut() else {
        return;
    };
    let mut bytes = [0u8; 1024];
    let n = crate::graphics::log_capture::drain(&mut bytes);
    if n > 0 {
        c.bytes(&bytes[..n]);
        let _ = c.draw();
    }
}
