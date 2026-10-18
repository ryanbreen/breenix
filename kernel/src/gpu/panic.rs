//! Terminal GPU diagnostics for ruling 40 (#1749). Preparation uses the normal
//! core; presentation uses preallocated requests outside that core.
//! Pixels are rendered by bitmap_text, not by a CPU framebuffer copy.
use super::*;
use alloc::boxed::Box;
use core::sync::atomic::{AtomicPtr, AtomicUsize};
use virgl::emergency::{Command, Queue};

const COLS: usize = 80;
const ROWS: usize = 32;
const CELLS: usize = COLS * ROWS;
#[cfg(feature = "capture_panic_oracle")]
static INITIALIZED: AtomicBool = AtomicBool::new(false);
static TERMINAL: AtomicBool = AtomicBool::new(false);
static VALID: AtomicBool = AtomicBool::new(false);
static STATE: AtomicPtr<Screen> = AtomicPtr::new(core::ptr::null_mut());
static USERS: [AtomicUsize; crate::arch_impl::aarch64::constants::MAX_CPUS] =
    [const { AtomicUsize::new(0) }; crate::arch_impl::aarch64::constants::MAX_CPUS];

fn park() -> ! {
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
    observed: AtomicBool,
    console: console::Console,
    queue: Queue,
    draws: Vec<Command>,
    // (draw request, source-x word) for each cell, in row order.
    glyphs: Vec<(usize, usize)>,
    bind: Command,
    flush: Command,
    reads: [Command; 2],
    backing: Arc<Pages>,
    first: Pages,
    pitch: usize,
}

/// Called before firmware display retirement, again when the console recovers from
/// a reset. A private context keeps ordinary console draws out of the panic state.
pub(super) fn prepare() -> Result<(), i64> {
    if VALID.load(Ordering::Acquire) {
        return Ok(());
    }
    let mut c = console::Console::new()?;
    c.draw(false)?;
    let (mut objects, backing, pitch, queue) = with_core(|core| {
        Some((
            core.contexts.get(&c.context)?.objects.clone(),
            core.resources.get(&c.target)?.backing.clone(),
            core.resources.get(&c.target)?.pitch as usize,
            core.dev.as_ref()?.terminal_queue(),
        ))
    })
    .flatten()
    .ok_or(EIO)?;
    let mut stream = Vec::new();
    stream::encode::begin(&mut stream, c.target as u64);
    stream::encode::clear(&mut stream, 0xff6e1216);
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
        0xffffebeb,
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
        2560,
        1600,
    ));
    let flush = Command::new(virgl::flush_resource(
        Tag::Background,
        c.context,
        fences[0],
        c.target,
        2560,
        1600,
    ));
    let reads = [1, 2].map(|i| {
        Command::new(virgl::transfer_from_host(
            Tag::Background,
            c.context,
            fences[i],
            c.target,
            backing.clone(),
            0,
            0,
            2560,
            1600,
            0,
            pitch as u32,
        ))
    });
    let first = Pages::new(backing.count()).ok_or(ENOMEM)?;
    let screen = Box::into_raw(Box::new(Screen {
        observed: AtomicBool::new(false),
        console: c,
        queue,
        draws,
        glyphs,
        bind,
        flush,
        reads,
        backing,
        first,
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
    fn line(&self, row: usize, text: &str) {
        for (column, byte) in text.bytes().take(COLS).enumerate() {
            let glyph = if (32..=126).contains(&byte) {
                byte
            } else {
                b'?'
            };
            let (request, word) = self.glyphs[row * COLS + column];
            self.draws[request].patch(word, (glyph - 32) as u32 * self.console.glyph_width);
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
        self.line(0, title);
        for (i, chunk) in headline.as_bytes().chunks(COLS).take(3).enumerate() {
            self.line(2 + i, core::str::from_utf8(chunk).unwrap_or("?"));
        }
        for (i, (label, value)) in details.iter().take(3).enumerate() {
            let mut line = Line::new();
            let _ = core::fmt::write(&mut line, format_args!("{label}: {value}"));
            self.line(6 + i, line.as_str());
        }
        self.line(10, stage);
        self.line(11, "Recent kernel log (serial is the panic record):");
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
            self.line(12 + i, core::str::from_utf8(line).unwrap_or("?"));
        }
        self.draws.iter().all(|c| self.queue.exec(c, deadline))
            && self.queue.exec(&self.bind, deadline)
            && self.queue.exec(&self.flush, deadline)
    }
    fn observe(&self, deadline: u64) -> bool {
        let bytes = self.pitch * 1600;
        let mut a = [0u8; PAGE];
        let mut b = [0u8; PAGE];
        for pass in 0..2 {
            self.backing
                .fill(0, bytes, if pass == 0 { POISON } else { !POISON });
            self.backing.maintain(0, bytes, false);
            if !self.queue.exec(&self.reads[pass], deadline) {
                return false;
            }
            self.backing.maintain(0, bytes, true);
            for offset in (0..bytes).step_by(PAGE) {
                let n = (bytes - offset).min(PAGE);
                self.backing.read(offset, &mut a[..n]);
                if pass == 0 {
                    self.first.write(offset, &a[..n]);
                } else {
                    self.first.read(offset, &mut b[..n]);
                    if a[..n] != b[..n] {
                        return false;
                    }
                }
            }
        }
        true
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
    if s.queue.drain(deadline) && s.show(title, headline, details, stage, log, deadline) {
        s.observed.store(s.observe(deadline), Ordering::Release);
    }
}

#[cfg(feature = "capture_panic_oracle")]
pub fn test_ready() -> bool {
    INITIALIZED.load(Ordering::Acquire)
        && (DOORBELL.load(Ordering::Acquire) == 0
            || (console::active() && VALID.load(Ordering::Acquire)))
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
