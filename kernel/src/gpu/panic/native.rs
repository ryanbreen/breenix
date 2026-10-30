//! Prepared GK208 diagnostics under the shared panic latch and text layout.
use super::*;
use crate::gk208::{self, emergency, encode as enc};

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
    let mut words = Vec::new();
    stream::encode::begin(&mut words, u64::from(c.target));
    stream::encode::clear(&mut words, BG);
    let mut instances = Vec::with_capacity(CELLS);
    for row in 0..ROWS {
        for col in 0..COLS {
            let x = 40 + col as u32 * c.glyph_width * 2;
            let y = 32 + row as u32 * 46;
            let mut params = [0; 16];
            params[..8].copy_from_slice(&[x, y, 0, 0, c.glyph_width, 16, 2, 1]);
            instances.push(abi::draw::Instance {
                rect: [
                    x as i32,
                    y as i32,
                    (x + c.glyph_width * 2) as i32,
                    (y + 32) as i32,
                ],
                params,
            });
        }
    }
    abi::draw::encode(
        &mut words,
        abi::pipeline::BITMAP_TEXT,
        FG,
        u64::from(c.atlas),
        0,
        0,
        &instances,
    );
    let mut frame = enc::translate(words.as_slice(), &Bindings(&c)).map_err(|_| EIO)?;
    let mut glyphs = Vec::with_capacity(CELLS);
    // Each bitmap quad has one CB0 selector (size, high address, low address).
    // Patch only param[2], the atlas x origin, in that quad's input data.
    // CLEAR and the GPU destination snapshot precede these chunks.
    for chunk in frame
        .chunks
        .iter()
        .filter(|c| c.source.is_some() && c.copy.is_none())
    {
        let mut at = 0;
        while at < chunk.words.len() {
            let count = ((chunk.words[at] >> 16) & 0x1fff) as usize;
            if chunk.words[at] & 0x1fff == 0x2380 / 4 && count == 3 {
                let offset = ((u64::from(chunk.words[at + 2]) << 32)
                    | u64::from(chunk.words[at + 3])) as usize;
                glyphs.push(offset + 8);
                break;
            }
            at += 1 + count;
        }
    }
    if glyphs.len() != CELLS {
        return Err(EIO);
    }
    frame.batch_draws(1400);
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
        if core.epoch != unsafe { (*screen).console.epoch } {
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
    title: &str,
    headline: &str,
    details: &[(&str, &str)],
    stage: &str,
    log: &[u8],
    deadline: u64,
) -> bool {
    let screen = NATIVE.load(Ordering::Acquire);
    if screen.is_null() {
        return false;
    }
    // SAFETY: shared terminal latch and Access drain exclude preparation/reset.
    let s = unsafe { &*screen };
    layout(
        |row, text| {
            for (col, &byte) in text.iter().take(COLS).enumerate() {
                let glyph = if (32..=126).contains(&byte) {
                    byte
                } else {
                    b'?'
                };
                s.frame.write(
                    s.glyphs[row * COLS + col],
                    &((glyph - 32) as u32 * s.console.glyph_width).to_le_bytes(),
                );
                #[cfg(any(feature = "capture_panic_oracle", feature = "gpu_fault_acceptance"))]
                SHOWN[row * COLS + col].store(glyph, Ordering::Relaxed);
            }
        },
        title,
        headline,
        details,
        stage,
        log,
    );
    s.frame.maintain(0, s.frame.bytes(), false);
    let shown = s.queue.drain(deadline)
        && s.packets.iter().all(|c| s.queue.exec(c, deadline))
        && s.display.show(deadline);
    #[cfg(any(feature = "capture_panic_oracle", feature = "gpu_fault_acceptance"))]
    s.report(shown, deadline);
    #[cfg(not(any(feature = "capture_panic_oracle", feature = "gpu_fault_acceptance")))]
    let _ = shown;
    true
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
    fn report(&self, shown: bool, deadline: u64) {
        let result = if shown { self.observe(deadline) } else { None };
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
