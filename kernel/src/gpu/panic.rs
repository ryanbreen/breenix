//! Terminal GPU diagnostics for ruling 40 (#1749). Preparation uses the normal
//! core; presentation uses preallocated requests outside that core.
//! Pixels are rendered by bitmap_text, not by a CPU framebuffer copy.
use super::console::{HEIGHT, WIDTH};
use super::*;
use alloc::boxed::Box;
use core::sync::atomic::{AtomicPtr, AtomicUsize};
#[cfg(feature = "capture_panic_oracle")]
use core::sync::atomic::AtomicU8;
#[cfg(feature = "capture_panic_oracle")]
use noto_sans_mono_bitmap::{get_raster, FontWeight, RasterHeight};
use virgl::emergency::{Command, Queue};

const COLS: usize = 80;
const ROWS: usize = 32;
const CELLS: usize = COLS * ROWS;
const BG: u32 = 0xff6e1216;
const FG: u32 = 0xffffebeb;
/// The glyph each cell was given, kept apart from the draw requests that carry it.
#[cfg(feature = "capture_panic_oracle")]
static SHOWN: [AtomicU8; CELLS] = [const { AtomicU8::new(b' ') }; CELLS];
#[cfg(feature = "capture_panic_oracle")]
static INITIALIZED: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "capture_panic_oracle")]
static PREPARATION_FINISHED: AtomicBool = AtomicBool::new(false);
static TERMINAL: AtomicBool = AtomicBool::new(false);
/// Fatal takeover intentionally ends scheduler progress for the rest of this boot.
pub fn terminal() -> bool {
    TERMINAL.load(Ordering::Acquire)
}

/// Stop a peer at its next timer interrupt after its raw device access drains.
/// Let an interrupted Access finish before parking its CPU.
pub(crate) fn stop_if_terminal() {
    if !terminal() {
        return;
    }
    let cpu = crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as usize;
    if USERS[cpu].load(Ordering::SeqCst) == 0 {
        park();
    }
}

static VALID: AtomicBool = AtomicBool::new(false);
static STATE: AtomicPtr<Screen> = AtomicPtr::new(core::ptr::null_mut());
static USERS: [AtomicUsize; crate::arch_impl::aarch64::constants::MAX_CPUS] =
    [const { AtomicUsize::new(0) }; crate::arch_impl::aarch64::constants::MAX_CPUS];

pub(crate) fn park() -> ! {
    unsafe {
        core::arch::asm!("msr daifset, #3", options(nomem, nostack));
    }
    loop {
        unsafe {
            core::arch::asm!("wfi", options(nomem, nostack));
        }
    }
}

/// Normal users stop even when a panicking CPU stranded the mutex they need.
pub(super) fn lock<T>(mutex: &spin::Mutex<T>) -> spin::MutexGuard<'_, T> {
    loop {
        if TERMINAL.load(Ordering::SeqCst) {
            park();
        }
        if let Some(guard) = mutex.try_lock() {
            if TERMINAL.load(Ordering::SeqCst) {
                park();
            }
            return guard;
        }
        core::hint::spin_loop();
    }
}

/// Acquire an IRQ-safe console lock without spinning past terminal takeover.
pub(super) fn lock_irq<T>(
    mutex: &crate::irq_safe_mutex::IrqSafeMutex<T>,
) -> crate::irq_safe_mutex::IrqSafeMutexGuard<'_, T> {
    loop {
        if TERMINAL.load(Ordering::SeqCst) {
            park();
        }
        if let Some(guard) = mutex.try_lock() {
            if TERMINAL.load(Ordering::SeqCst) {
                park();
            }
            return guard;
        }
        core::hint::spin_loop();
    }
}

/// Count raw queue/transport accesses and state publication, outside allocating
/// operations. Nesting is counted; preemption pins the counter to its CPU.
/// A panicking CPU can abandon its own operation partway through publication:
/// terminal output reads the actual avail index and uses disjoint descriptors.
pub(crate) struct Access(Option<usize>);
impl Access {
    pub(crate) fn enter() -> Self {
        // Probe-error recovery can run before per-CPU initialization; no panic
        // screen or competing producer exists at that point.
        if !crate::per_cpu::is_initialized() {
            return Self(None);
        }
        crate::per_cpu::preempt_disable();
        let cpu = crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as usize;
        USERS[cpu].fetch_add(1, Ordering::SeqCst);
        if TERMINAL.load(Ordering::SeqCst) {
            USERS[cpu].fetch_sub(1, Ordering::SeqCst);
            // Keep interrupted DMA owners on their stacks, including reset's Device.
            park();
        }
        Self(Some(cpu))
    }
}
/// Exception recovery abandons the current stack, so its Access guards cannot drop.
/// Called with IRQs masked before switching to the idle stack.
pub(crate) fn abandon_accesses() {
    let cpu = crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as usize;
    let count = USERS[cpu].swap(0, Ordering::SeqCst);
    for _ in 0..count {
        crate::per_cpu::preempt_enable();
    }
}

impl Drop for Access {
    fn drop(&mut self) {
        if let Some(cpu) = self.0 {
            USERS[cpu].fetch_sub(1, Ordering::SeqCst);
            crate::per_cpu::preempt_enable();
        }
    }
}

#[cfg(feature = "capture_panic_oracle")]
pub(super) fn initialized() {
    INITIALIZED.store(true, Ordering::Release);
}

pub(super) fn invalidate() {
    VALID.store(false, Ordering::Release);
}

struct Screen {
    #[cfg(feature = "capture_panic_oracle")]
    observed: AtomicBool,
    console: console::Console,
    queue: Queue,
    draws: Vec<Command>,
    // (draw request, source-x word) for each cell, in row order.
    glyphs: Vec<(usize, usize)>,
    bind: Command,
    flush: Command,
    #[cfg(feature = "capture_panic_oracle")]
    reads: [Command; 2],
    #[cfg(feature = "capture_panic_oracle")]
    backing: Arc<Pages>,
    #[cfg(feature = "capture_panic_oracle")]
    first: Pages,
    #[cfg(feature = "capture_panic_oracle")]
    pitch: usize,
}

/// Called after console handoff, again when the console recovers from a reset. A private context keeps ordinary console draws out of the panic state.
pub(super) fn prepare() -> Result<(), i64> {
    if VALID.load(Ordering::Acquire) {
        return Ok(());
    }
    let mut c = console::Console::new(false)?;
    c.draw(false)?;
    let (mut objects, queue) = with_core(|core| {
        Some((
            core.contexts.get(&c.context)?.objects.clone(),
            core.dev.as_ref()?.terminal_queue(),
        ))
    })
    .flatten()
    .ok_or(EIO)?;
    #[cfg(feature = "capture_panic_oracle")]
    let (backing, pitch) = with_core(|core| {
        let r = core.resources.get(&c.target)?;
        Some((r.backing.clone(), r.pitch as usize))
    })
    .flatten()
    .ok_or(EIO)?;
    let mut stream = Vec::new();
    stream::encode::begin(&mut stream, c.target as u64);
    stream::encode::clear(&mut stream, BG);
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
        &mut stream,
        abi::pipeline::BITMAP_TEXT,
        FG,
        c.atlas as u64,
        0,
        0,
        &instances,
    );
    let mut enc = encode::Enc::new();
    encode::translate(
        &mut enc,
        &mut objects,
        stream.as_slice(),
        &c,
        encode::Extras {
            vertex_buffer: c.vertices,
            scratch: [c.scratch, 0],
            data: 0,
            data_width: 0,
            flip_y: FLIP_Y,
            corrupt: None,
            suppress: None,
            swap: None,
        },
    );
    let mut draws = Vec::new();
    let mut glyphs = Vec::with_capacity(CELLS);
    for words in enc.finish().ok_or(ENOMEM)? {
        let mut at = 0;
        while at < words.len() {
            let header = words.word(at);
            let len = (header >> 16) as usize;
            // The catalogue encoder's SET_CONSTANT_BUFFER carries shader/index,
            // then params; param 2 is the atlas x origin (encode::translate).
            if header & 0xff == wire::ccmd::SET_CONSTANT_BUFFER && len == 34 {
                glyphs.push((draws.len(), at + 5));
            }
            at += 1 + len;
        }
        let fence = with_core(|core| core.device_fence()).ok_or(EIO)?;
        draws.push(Command::new(virgl::submit(
            Tag::Background,
            c.context,
            Some(fence),
            words,
        )));
    }
    if glyphs.len() != CELLS {
        return Err(EIO);
    }
    let fences = with_core(|core| {
        [
            core.device_fence(),
            core.device_fence(),
            core.device_fence(),
        ]
    })
    .ok_or(EIO)?;
    let bind = Command::new(virgl::set_scanout(
        Tag::Background,
        c.context,
        c.target,
        WIDTH,
        HEIGHT,
    ));
    let flush = Command::new(virgl::flush_resource(
        Tag::Background,
        c.context,
        fences[0],
        c.target,
        WIDTH,
        HEIGHT,
    ));
    #[cfg(feature = "capture_panic_oracle")]
    let reads = [1, 2].map(|i| {
        Command::new(virgl::transfer_from_host(
            Tag::Background,
            c.context,
            fences[i],
            c.target,
            backing.clone(),
            0,
            0,
            WIDTH,
            HEIGHT,
            0,
            pitch as u32,
        ))
    });
    #[cfg(feature = "capture_panic_oracle")]
    let first = Pages::new(backing.count()).ok_or(ENOMEM)?;
    let screen = Box::into_raw(Box::new(Screen {
        #[cfg(feature = "capture_panic_oracle")]
        observed: AtomicBool::new(false),
        console: c,
        queue,
        draws,
        glyphs,
        bind,
        flush,
        #[cfg(feature = "capture_panic_oracle")]
        reads,
        #[cfg(feature = "capture_panic_oracle")]
        backing,
        #[cfg(feature = "capture_panic_oracle")]
        first,
        #[cfg(feature = "capture_panic_oracle")]
        pitch,
    }));
    let old = with_core(|core| {
        // Publication shares the reset decision's lock, so stale resources cannot
        // be republished after reset invalidation.
        if core.epoch != unsafe { (*screen).console.epoch } {
            return None;
        }
        let _access = Access::enter();
        let old = STATE.swap(screen, Ordering::AcqRel);
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

/// The red screen's text layout. Input bytes are text, not pixel data.
impl Screen {
    fn line(&self, row: usize, text: &[u8]) {
        for (column, &byte) in text.iter().take(COLS).enumerate() {
            let glyph = if (32..=126).contains(&byte) {
                byte
            } else {
                b'?'
            };
            let (request, word) = self.glyphs[row * COLS + column];
            self.draws[request].patch(word, (glyph - 32) as u32 * self.console.glyph_width);
            #[cfg(feature = "capture_panic_oracle")]
            SHOWN[row * COLS + column].store(glyph, Ordering::Relaxed);
        }
    }
    fn show(
        &self,
        title: &str,
        headline: &str,
        details: &[(&str, &str)],
        stage: &str,
        log: &[u8],
        deadline: u64,
    ) -> bool {
        self.line(0, title.as_bytes());
        for (i, chunk) in headline.as_bytes().chunks(COLS).take(3).enumerate() {
            self.line(2 + i, chunk);
        }
        for (i, (label, value)) in details.iter().take(3).enumerate() {
            let mut line = Line::new();
            let _ = core::fmt::write(&mut line, format_args!("{label}: {value}"));
            self.line(6 + i, line.as_str().as_bytes());
        }
        self.line(10, stage.as_bytes());
        self.line(11, b"Recent kernel log (serial is the panic record):");
        // Match the diagnostics screen's tail selection without allocating strings.
        let mut end = log.len();
        while end > 0 && matches!(log[end - 1], b'\n' | b'\r') {
            end -= 1;
        }
        let mut start = end;
        let mut lines = 1;
        while start > 0 {
            start -= 1;
            if log[start] == b'\n' {
                if lines == 20 {
                    start += 1;
                    break;
                }
                lines += 1;
            }
        }
        for (i, line) in log[start..end].split(|b| *b == b'\n').take(20).enumerate() {
            self.line(12 + i, line);
        }
        self.draws.iter().all(|c| self.queue.exec(c, deadline))
            && self.queue.exec(&self.bind, deadline)
            && self.queue.exec(&self.flush, deadline)
    }
    /// The pixel the layout above puts at (x, y): each cell's glyph at scale 2,
    /// coverage-blended (gpu.md section 2) like the suite's console check.
    #[cfg(feature = "capture_panic_oracle")]
    fn expected(&self, x: usize, y: usize) -> u32 {
        let gw = self.console.glyph_width as usize;
        if x < 40 || y < 32 {
            return BG;
        }
        let (column, row) = ((x - 40) / (gw * 2), (y - 32) / 46);
        let (dx, dy) = ((x - 40) % (gw * 2) / 2, (y - 32) % 46 / 2);
        if column >= COLS || row >= ROWS || dy >= 16 {
            return BG;
        }
        let glyph = SHOWN[row * COLS + column].load(Ordering::Relaxed);
        let a = get_raster(glyph as char, FontWeight::Regular, RasterHeight::Size16)
            .and_then(|r| r.raster().get(dy).and_then(|l| l.get(dx)).copied())
            .unwrap_or(0) as u32;
        let (f, b) = (FG.to_le_bytes(), BG.to_le_bytes());
        let mut out = [0u8; 4];
        for i in 0..4 {
            out[i] = ((f[i] as u32 * a + b[i] as u32 * (255 - a) + 127) / 255) as u8;
        }
        u32::from_le_bytes(out)
    }
    /// Device readback of the bound panic target (gpu-abi.md, READBACK's two-poison
    /// observation), then each of the target's pixels against `expected`. Returns the
    /// mismatch count and the first mismatch when the two reads agree.
    #[cfg(feature = "capture_panic_oracle")]
    fn observe(&self, deadline: u64) -> Option<(u64, Option<(usize, usize, u32, u32)>)> {
        let bytes = self.pitch * HEIGHT as usize;
        let mut a = [0u8; PAGE];
        let mut b = [0u8; PAGE];
        for pass in 0..2 {
            self.backing
                .fill(0, bytes, if pass == 0 { POISON } else { !POISON });
            self.backing.maintain(0, bytes, false);
            if !self.queue.exec(&self.reads[pass], deadline) {
                return None;
            }
            self.backing.maintain(0, bytes, true);
            for offset in (0..bytes).step_by(PAGE) {
                if crate::arch_impl::aarch64::timer::rdtsc() >= deadline {
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
        let mut first = None;
        for offset in (0..bytes).step_by(PAGE) {
            if crate::arch_impl::aarch64::timer::rdtsc() >= deadline {
                return None;
            }
            let n = (bytes - offset).min(PAGE);
            self.first.read(offset, &mut a[..n]);
            for (i, px) in a[..n].chunks_exact(4).enumerate() {
                let at = offset + i * 4;
                let (x, y) = (at % self.pitch / 4, at / self.pitch);
                if x >= WIDTH as usize {
                    continue;
                }
                let got = u32::from_le_bytes([px[0], px[1], px[2], px[3]]);
                let want = self.expected(x, y);
                if got != want {
                    mismatches += 1;
                    first.get_or_insert((x, y, got, want));
                }
            }
        }
        (crate::arch_impl::aarch64::timer::rdtsc() < deadline).then_some((mismatches, first))
    }
}
struct Line {
    bytes: [u8; COLS],
    len: usize,
}
impl Line {
    fn new() -> Self {
        Self {
            bytes: [0; COLS],
            len: 0,
        }
    }
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("?")
    }
}
impl core::fmt::Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for b in s.bytes().take(COLS - self.len) {
            self.bytes[self.len] = if b.is_ascii() { b } else { b'?' };
            self.len += 1;
        }
        Ok(())
    }
}

/// Terminal, single-owner path. A failed or unresponsive device leaves serial as
/// the record; resources and requests remain pinned, including after a timeout.
pub fn show(title: &str, headline: &str, details: &[(&str, &str)], stage: &str, log: &[u8]) {
    if TERMINAL.swap(true, Ordering::SeqCst) {
        return;
    }
    let cpu = crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as usize;
    let frequency = crate::arch_impl::aarch64::timer::frequency_hz();
    let drain = crate::arch_impl::aarch64::timer::rdtsc() + frequency / 10;
    while USERS
        .iter()
        .enumerate()
        .any(|(i, n)| i != cpu && n.load(Ordering::SeqCst) != 0)
    {
        if crate::arch_impl::aarch64::timer::rdtsc() >= drain {
            return;
        }
        core::hint::spin_loop();
    }
    if !VALID.load(Ordering::Acquire) {
        return;
    }
    let screen = STATE.load(Ordering::Acquire);
    if screen.is_null() {
        return;
    }
    // SAFETY: the terminal latch excludes preparation and normal device writers.
    // This CPU owns the published state and retains it for post-mortem inspection.
    let s = unsafe { &*screen };
    let deadline = crate::arch_impl::aarch64::timer::rdtsc() + frequency * 2;
    if !(s.queue.drain(deadline) && s.show(title, headline, details, stage, log, deadline)) {
        #[cfg(feature = "capture_panic_oracle")]
        report(s, false, deadline);
        return;
    }
    #[cfg(feature = "capture_panic_oracle")]
    report(s, true, deadline);
}

/// The capture build's screen check, on serial after the panic record.
#[cfg(feature = "capture_panic_oracle")]
fn report(s: &Screen, shown: bool, deadline: u64) {
    let result = if shown { s.observe(deadline) } else { None };
    s.observed.store(result.is_some(), Ordering::Release);
    let mut line = crate::serial_line::Line::unattributed();
    line.text("[GPU_PANIC_SCREEN shown=");
    line.dec(shown as u64);
    line.text(" observed=");
    line.dec(result.is_some() as u64);
    if let Some((mismatches, first)) = result {
        line.text(" pixels=");
        line.dec(WIDTH as u64 * HEIGHT as u64);
        line.text(" mismatches=");
        line.dec(mismatches);
        if let Some((x, y, got, want)) = first {
            line.text(" first=");
            line.dec(x as u64);
            line.text(",");
            line.dec(y as u64);
            line.text(" got=");
            line.hex32(got);
            line.text(" want=");
            line.hex32(want);
        }
    }
    line.text("]");
    line.newline();
}

#[cfg(feature = "capture_panic_oracle")]
pub fn test_ready() -> bool {
    !crate::platform_config::is_qemu()
        || (INITIALIZED.load(Ordering::Acquire)
            && (DOORBELL.load(Ordering::Acquire) == 0
                || (console::active() && PREPARATION_FINISHED.load(Ordering::Acquire))))
}

#[cfg(feature = "capture_panic_oracle")]
pub fn test_with_locks(f: impl FnOnce()) {
    let Some(_core) = CORE.try_lock() else {
        return;
    };
    console::panic_with_lock(|| {
        let Some(fb) = crate::graphics::arm64_fb::SHELL_FRAMEBUFFER.get() else {
            return;
        };
        let Some(_fb) = fb.try_lock() else {
            return;
        };
        f();
    });
}

#[cfg(feature = "capture_panic_oracle")]
pub(super) fn preparation_finished() {
    PREPARATION_FINISHED.store(true, Ordering::Release);
}
