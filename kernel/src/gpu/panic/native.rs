//! Prepared GK208 diagnostics under the shared panic latch and text layout.
use super::*;
use crate::gk208::{self, emergency, encode as enc, prepared};

static NATIVE: AtomicPtr<Screen> = AtomicPtr::new(core::ptr::null_mut());
struct Screen {
    console: console::Console,
    queue: emergency::Queue,
    display: emergency::Display,
    frame: Arc<Pages>,
    glyphs: Vec<usize>,
    packets: Vec<emergency::Packet>,
    // Both mappings and the encoded frame's target/source holds stay pinned.
    _commands: Arc<Pages>,
    _command_hold: gk208::resource::Hold,
    _draws: Vec<gk208::adapter::EncodedDraw>,
    #[cfg(any(feature = "capture_panic_oracle", feature = "gpu_fault_acceptance"))]
    reads: [emergency::Packet; 2],
    #[cfg(any(feature = "capture_panic_oracle", feature = "gpu_fault_acceptance"))]
    backing: Arc<Pages>,
    #[cfg(any(feature = "capture_panic_oracle", feature = "gpu_fault_acceptance"))]
    first: Pages,
}
struct Bindings<'a>(&'a console::Console);
impl stream::Resolver for Bindings<'_> {
    fn target(&self, h: u64) -> Option<stream::TargetInfo> {
        (h == u64::from(self.0.target)).then_some(stream::TargetInfo {
            width: WIDTH,
            height: HEIGHT,
            invalid: false,
            format: format::B8G8R8A8,
        })
    }
    fn source(&self, h: u64) -> Option<stream::SourceInfo> {
        (h == u64::from(self.0.atlas)).then_some(stream::SourceInfo {
            width: self.0.glyph_width * 95,
            height: 16,
            invalid: false,
            format: format::R8,
        })
    }
    fn buffer_size(&self, _: u64) -> Option<u64> {
        None
    }
    fn buffer_rect(&self, _: u64, _: u64) -> [i32; 4] {
        [0; 4]
    }
}
impl enc::Bindings for Bindings<'_> {
    fn render_target(&self, h: u64) -> Option<enc::Target> {
        (h == u64::from(self.0.target))
            .then(|| enc::target(self.0.target, format::B8G8R8A8))
            .flatten()
    }
    fn sampled_source(&self, h: u64) -> Option<enc::Target> {
        (h == u64::from(self.0.atlas))
            .then(|| enc::target(self.0.atlas, format::R8))
            .flatten()
    }
    fn blend_scratch(&self, _: u32) -> Option<enc::Target> {
        enc::target(self.0.scratch, format::B8G8R8A8)
    }
}

pub(super) fn prepare() -> Result<(), i64> {
    let c = console::Console::new()?;
    let words = prepared::text_stream(c.target, c.atlas, c.glyph_width);
    let mut frame = enc::translate(words.as_slice(), &Bindings(&c))
        .map_err(|e| if e == enc::NO_MEMORY { ENOMEM } else { EIO })?;
    let glyphs = prepared::glyph_offsets(
        frame.chunks.iter().filter(|c| c.source.is_some() && c.copy.is_none())
            .map(|c| c.words.as_slice()), frame.data.len()).map_err(|_| EIO)?;
    frame.batch_draws(1400).map_err(|_| ENOMEM)?;
    let backing = Arc::new(Pages::new(frame.data.len().div_ceil(PAGE)).ok_or(ENOMEM)?);
    backing.write(0, &frame.data);
    backing.maintain(0, backing.bytes(), false);
    let draws = enc::prepare(frame, backing.clone()).map_err(|_| EIO)?;
    let queue = with_core(|core| match core.dev.as_ref()? {
        dev::Device::Gk208(d) => d.terminal_queue().ok(),
        _ => None,
    })
    .flatten()
    .ok_or(EIO)?;
    let display = gk208::ce_terminal_display(c.target).map_err(|_| EIO)?;
    let mut encoded = Vec::new();
    // First idle release establishes device execution drain, after GP_GET.
    encoded.push(queue.draw(&[], 1).map_err(|_| EIO)?);
    for d in &draws {
        let nonce = encoded.len() as u32 + 1;
        encoded.push(if let Some(plan) = d.transfer {
            gk208::copy::encode_plan(plan, queue.report_va, queue.epoch, nonce).map_err(|_| EIO)?
        } else {
            queue.draw(&d.words, nonce).map_err(|_| EIO)?
        });
    }
    #[cfg(any(feature = "capture_panic_oracle", feature = "gpu_fault_acceptance"))]
    let pixels = with_core(|core| core.resources.get(&c.target).map(|r| r.backing.clone()))
        .flatten()
        .ok_or(EIO)?;
    #[cfg(any(feature = "capture_panic_oracle", feature = "gpu_fault_acceptance"))]
    {
        let pages: Vec<_> = (0..pixels.count()).map(|i| pixels.page_addr(i)).collect();
        let plan = gk208::emergency::readback(c.target, &pixels, &pages).map_err(|_| EIO)?;
        for _ in 0..2 {
            encoded.push(
                gk208::copy::encode_plan(
                    plan,
                    queue.report_va,
                    queue.epoch,
                    encoded.len() as u32 + 1,
                )
                .map_err(|_| EIO)?,
            );
        }
    }
    // New mappings must be attached before the final channel-directory bind.
    let bytes = encoded.iter().map(|w| w.len() * 4).sum::<usize>();
    let commands = Arc::new(Pages::new(bytes.div_ceil(PAGE)).ok_or(ENOMEM)?);
    let (address, hold) = gk208::emergency::map_commands(commands.clone()).map_err(|_| EIO)?;
    // Bind the command and readback mappings before publishing the immutable snapshot.
    with_core(|core| match core.dev.as_ref() {
        Some(dev::Device::Gk208(d)) => d.terminal_queue().map(|_| ()),
        _ => Err("native device absent"),
    })
    .ok_or(EIO)?
    .map_err(|_| EIO)?;
    let mut packets = Vec::new();
    let mut offset = 0;
    for (i, w) in encoded.iter().enumerate() {
        for v in w {
            commands.write(offset, &v.to_le_bytes());
            offset += 4;
        }
        packets.push(
            queue
                .packet(
                    address + (offset - w.len() * 4) as u64,
                    w.len(),
                    i as u32 + 1,
                )
                .map_err(|_| EIO)?,
        );
    }
    commands.maintain(0, commands.bytes(), false);
    #[cfg(any(feature = "capture_panic_oracle", feature = "gpu_fault_acceptance"))]
    let reads = [
        packets.remove(packets.len() - 2),
        packets.remove(packets.len() - 1),
    ];
    let screen = Box::into_raw(Box::new(Screen {
        console: c,
        queue,
        display,
        frame: backing,
        glyphs,
        packets,
        _commands: commands,
        _command_hold: hold,
        _draws: draws,
        #[cfg(any(feature = "capture_panic_oracle", feature = "gpu_fault_acceptance"))]
        first: Pages::new(pixels.count()).ok_or(ENOMEM)?,
        #[cfg(any(feature = "capture_panic_oracle", feature = "gpu_fault_acceptance"))]
        backing: pixels,
        #[cfg(any(feature = "capture_panic_oracle", feature = "gpu_fault_acceptance"))]
        reads,
    }));
    let old = with_core(|core| {
        let prepared = unsafe { &*screen };
        if core.epoch != prepared.console.epoch
            || !matches!(core.dev.as_ref(), Some(dev::Device::Gk208(d))
                if d.terminal_identity() == Some((prepared.queue.instance, prepared.queue.epoch)))
        {
            return None;
        }
        let _access = Access::enter();
        let old = NATIVE.swap(screen, Ordering::AcqRel);
        VALID.store(true, Ordering::Release);
        Some(old)
    })
    .flatten();
    match old {
        Some(old) if !old.is_null() => unsafe {
            drop(Box::from_raw(old));
        },
        Some(_) => {}
        None => {
            unsafe {
                drop(Box::from_raw(screen));
            }
            return Err(EIO);
        }
    }
    Ok(())
}
#[cfg(any(feature = "capture_panic_oracle", feature = "gpu_fault_acceptance"))]
pub(super) fn ready() -> bool {
    console::active() && VALID.load(Ordering::Acquire) && !NATIVE.load(Ordering::Acquire).is_null()
}
pub(super) fn show(
    title: &str, headline: &str, details: &[(&str, &str)], stage: &str, log: &[u8],
) {
    let mut runtime = Runtime { title, headline, details, stage, log, used: false };
    let outcome = prepared::attempt(&mut runtime);
    #[cfg(any(feature = "capture_panic_oracle", feature = "gpu_fault_acceptance"))]
    if runtime.used { runtime.screen().report(outcome); }
    #[cfg(not(any(feature = "capture_panic_oracle", feature = "gpu_fault_acceptance")))]
    let _ = outcome;
}

struct Runtime<'a> {
    title: &'a str, headline: &'a str, details: &'a [(&'a str, &'a str)],
    stage: &'a str, log: &'a [u8], used: bool,
}
impl Runtime<'_> {
    fn screen(&self) -> &Screen {
        // SAFETY: only called after terminal latching, peer drain and ready().
        // No producer can replace/free the published screen after that point.
        unsafe { &*NATIVE.load(Ordering::Acquire) }
    }
}
impl prepared::Terminal for Runtime<'_> {
    fn counter(&self) -> u64 { arch::counter() }
    fn frequency(&self) -> u64 { arch::frequency_hz() }
    fn latch(&mut self) -> bool { !TERMINAL.swap(true, Ordering::SeqCst) }
    fn peers_idle(&self) -> bool {
        let cpu = arch::cpu_id();
        USERS.iter().enumerate().all(|(i, n)| i == cpu || n.load(Ordering::SeqCst) == 0)
    }
    fn ready(&self) -> bool {
        VALID.load(Ordering::Acquire) && !NATIVE.load(Ordering::Acquire).is_null()
    }
    fn patch(&mut self, deadline: u64) -> bool {
        self.used = true;
        let s = self.screen();
        layout(|row, text| {
            for (col, &byte) in text.iter().take(COLS).enumerate() {
                if arch::counter() >= deadline { return; }
                let glyph = if (32..=126).contains(&byte) { byte } else { b'?' };
                s.frame.write(s.glyphs[row * COLS + col],
                    &((glyph - 32) as u32 * s.console.glyph_width).to_le_bytes());
                #[cfg(any(feature = "capture_panic_oracle", feature = "gpu_fault_acceptance"))]
                SHOWN[row * COLS + col].store(glyph, Ordering::Relaxed);
            }
        }, self.title, self.headline, self.details, self.stage, self.log);
        s.frame.maintain(0, s.frame.bytes(), false);
        arch::counter() < deadline
    }
    fn drain(&mut self, deadline: u64) -> bool { self.screen().queue.drain(deadline) }
    fn draws(&self) -> usize { self.screen().packets.len() }
    fn draw(&mut self, index: usize, deadline: u64) -> bool {
        let s = self.screen();
        s.queue.exec(&s.packets[index], deadline)
    }
    fn flip(&mut self, deadline: u64) -> bool { self.screen().display.show(deadline) }
    fn observe(&mut self, deadline: u64) -> Option<u64> {
        #[cfg(any(feature = "capture_panic_oracle", feature = "gpu_fault_acceptance"))]
        { self.screen().observe(deadline) }
        #[cfg(not(any(feature = "capture_panic_oracle", feature = "gpu_fault_acceptance")))]
        { let _ = deadline; None }
    }
}
#[cfg(any(feature = "capture_panic_oracle", feature = "gpu_fault_acceptance"))]
impl Screen {
    fn observe(&self, deadline: u64) -> Option<u64> {
        let bytes = WIDTH as usize * HEIGHT as usize * 4;
        let mut a = [0u8; PAGE];
        let mut b = [0u8; PAGE];
        if !self.display.bound() {
            return None;
        }
        for pass in 0..2 {
            self.backing
                .fill(0, bytes, if pass == 0 { 0x55 } else { 0xaa });
            self.backing.maintain(0, bytes, false);
            if !self.queue.exec(&self.reads[pass], deadline) {
                return None;
            }
            self.backing.maintain(0, bytes, true);
            for offset in (0..bytes).step_by(PAGE) {
                if arch::counter() >= deadline {
                    return None;
                }
                let n = (bytes - offset).min(PAGE);
                self.backing.read(offset, &mut a[..n]);
                if pass == 0 {
                    self.first.write(offset, &a[..n]);
                } else {
                    self.first.read(offset, &mut b[..n]);
                    if a[..n] != b[..n] {
                        return None;
                    }
                }
            }
        }
        let mut mismatches = 0;
        for offset in (0..bytes).step_by(PAGE) {
            if arch::counter() >= deadline {
                return None;
            }
            let n = (bytes - offset).min(PAGE);
            self.first.read(offset, &mut a[..n]);
            for (i, pixel) in a[..n].chunks_exact(4).enumerate() {
                let at = offset / 4 + i;
                if u32::from_le_bytes(pixel.try_into().ok()?)
                    != expected(
                        self.console.glyph_width,
                        at % WIDTH as usize,
                        at / WIDTH as usize,
                    )
                {
                    mismatches += 1;
                }
            }
        }
        (arch::counter() < deadline && self.display.bound()).then_some(mismatches)
    }
    fn report(&self, outcome: prepared::Outcome) {
        let prepared::Outcome { shown, observation: result } = outcome;
        let mut line = crate::capture::record::Writer::new();
        line.text("[GPU_PANIC_SCREEN shown=");
        line.dec(shown as u64);
        line.text(" observed=");
        line.dec(result.is_some() as u64);
        if let Some(mismatches) = result {
            line.text(" pixels=4096000 mismatches=");
            line.dec(mismatches);
        }
        line.text("]");
        line.text("\r\n");
        line.text("[GPU_PANIC_NATIVE target=");
        line.dec(u64::from(self.console.target));
        line.text(" bound=");
        line.dec(if self.display.bound() {
            u64::from(self.display.target)
        } else {
            0
        });
        line.text(" poisons=55,aa equal=");
        line.dec(result.is_some() as u64);
        line.text(" firmware_writes=");
        line.dec(arch::firmware_writes());
        line.text("]");
        line.text("\r\n");
    }
}
