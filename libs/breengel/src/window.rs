//! Breengel windows on the window interface, version 2 (`docs/gui/breengel.md` sections 17
//! and 19).
//!
//! A `Window` owns a kernel window of two buffers. Drawing goes into a buffer the kernel hands
//! out with `ACQUIRE`, already brought up to date with the newest committed frame inside the
//! kernel, so Breengel never copies pixels; `present` commits it. Events come through
//! `dispatch`, which runs the app's handler on each and keeps the processed watermark of
//! section 19.4 for the commit metadata. Dropping the window destroys it.

use libbreenix::errno::Errno;
use libbreenix::error::Error;
use libbreenix::window::{self, abi};
use libfont::CachedFont;
use libgfx::surface::{Format, Surface};

use crate::event::{Delivered, Event};
use crate::font::FontWatcher;

/// What a handler did with an event: changed something the window draws, or not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handled {
    Redraw,
    NoChange,
}

/// The buffer `acquire` handed out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Acquired {
    /// The buffer's index in the window's set.
    pub buffer: u32,
    /// The commit whose pixels it holds, 0 for none.
    pub base: u64,
    /// Whether the kernel replayed the whole window into it.
    pub replay_full: bool,
    /// The rectangles the kernel replayed.
    pub replay: Vec<abi::Rect>,
}

/// A commit identity and its processed watermark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Committed {
    pub commit_id: u64,
    pub pixel_gen: u64,
    pub processed_serial: u64,
}

struct Held {
    info: Acquired,
    /// The highest serial whose handler had returned when the buffer was acquired.
    handled_at_acquire: u64,
}

/// A GUI window on the kernel window interface.
pub struct Window {
    id: u64,
    width: u32,
    height: u32,
    buffers: [u64; 3],
    held: Option<Held>,
    gpu: Option<libgfx::surface::Surface>,
    font_watcher: FontWatcher,
    /// The highest serial whose handler has returned.
    handled: u64,
    /// Serials handled since the last pixel commit, with what their handlers returned.
    since_pixel: Vec<(u64, Handled)>,
    /// The processed watermark last published.
    published: u64,
    /// The compositor epoch of the last `CompositorChanged`, 0 before one.
    epoch: u64,
    last: Option<Committed>,
    pending: Option<(Committed, Option<libgfx::surface::Surface>)>,
}

impl Window {
    /// Create and register a window. When registration fails, the window just created is
    /// destroyed before the error is returned.
    pub fn new(title: &[u8], width: u32, height: u32) -> Result<Self, Error> {
        Self::with_watcher(title, width, height, FontWatcher::new())
    }

    /// A window that neither loads nor polls the system fonts.
    pub fn new_without_fonts(title: &[u8], width: u32, height: u32) -> Result<Self, Error> {
        Self::with_watcher(title, width, height, FontWatcher::disabled())
    }

    fn with_watcher(title: &[u8], width: u32, height: u32, font_watcher: FontWatcher) -> Result<Self, Error> {
        Self::validate_size(width, height)?;
        let c = window::create(width, height, 2, 0)?;
        if let Err(e) = window::register(c.id, title) {
            let _ = window::destroy(c.id);
            return Err(e);
        }
        Ok(Self {
            id: c.id,
            width,
            height,
            buffers: c.buffer,
            held: None,
            gpu: None,
            font_watcher,
            handled: 0,
            since_pixel: Vec::new(),
            published: 0,
            epoch: 0,
            last: None,
            pending: None,
        })
    }

    /// Validate the GPU surface layout before allocating the window targets.
    fn validate_size(width: u32, height: u32) -> Result<(), Error> {
        Surface::validate_layout(width as usize, height as usize, width as usize * 4, Format::BGRX)
            .map_err(|_| Error::Os(Errno::EINVAL))
    }

    /// Take a buffer to draw the next frame into, waiting up to `timeout_ns` (`u64::MAX`
    /// forever). The buffer already holds the newest committed frame. Returns the buffer
    /// held now when one is.
    pub fn acquire(&mut self, timeout_ns: u64) -> Result<Acquired, Error> {
        if let Some(h) = &self.held {
            return Ok(h.info.clone());
        }
        let a = match window::acquire(self.id, timeout_ns) {
            Ok(a) => a,
            Err(e) => {
                if matches!(e, Error::Os(Errno::EIO)) { self.pending = None; }
                return Err(e);
            }
        };
        if let Some((done, _)) = self.pending.take() {
            if a.base >= done.commit_id { self.published(done); }
        }
        let info = Acquired {
            buffer: a.buffer,
            base: a.base,
            replay_full: a.replay_full != 0,
            replay: a.replay[..(a.replay_count as usize).min(16)].to_vec(),
        };
        self.gpu = Some(self.surface_over(a.buffer)?);
        self.held = Some(Held { info: info.clone(), handled_at_acquire: self.handled });
        Ok(info)
    }

    fn surface_over(&self, buffer:u32) -> Result<libgfx::surface::Surface,Error> {
        libgfx::surface::Surface::from_window_target(self.buffers[buffer as usize],self.width as usize,self.height as usize)
            .map_err(|e| match e {
                libgfx::surface::SurfaceError::Device(r)=>Error::Os(Errno::from_raw(-r)),
                _=>Error::Os(Errno::EINVAL),
            })
    }

    /// The acquired GPU target. Errors preserve ACQUIRE's semantics; no scratch image
    /// hides destruction, render failure or memory pressure.
    pub fn surface(&mut self) -> Result<&mut libgfx::surface::Surface, Error> {
        if self.held.is_none() { self.acquire(u64::MAX)?; }
        Ok(self.gpu.as_mut().expect("GPU view accompanies acquisition"))
    }

    /// A commit's metadata (section 17.1): the processed watermark, the compositor epoch the
    /// window last heard of, and the time on the UI clock with its real-clock read count.
    fn meta(&self, processed_serial: u64) -> abi::Meta {
        let mut m = <abi::Meta as abi::Plain>::ZERO;
        m.processed_serial = processed_serial;
        m.epoch = self.epoch;
        m.ui_time_ms = crate::clock::now_ns() / 1_000_000;
        m.real_clock_reads = crate::clock::real_clock_reads();
        m
    }

    /// Commit the acquired buffer with `damage` (at most 16 rectangles covering every pixel
    /// that differs from the previous frame). Its processed serial is the watermark recorded
    /// when the buffer was acquired (section 19.4): an event handled after that is acknowledged
    /// by a later frame, never by this one. A commit the kernel refuses while the buffer stays
    /// acquired (bad damage, say) keeps it held, so drawing can go on and commit again.
    pub fn commit(&mut self, damage: &[abi::Rect]) -> Result<Committed, Error> {
        let done = self.commit_async(damage)?;
        self.finish_pending()?;
        Ok(done)
    }

    /// Accept the acquired frame without waiting for its rendering. Kernel publication
    /// retains the previous front until the accepted writers complete; ACQUIRE waits for
    /// that decision and reports a late failure once as EIO.
    pub fn commit_async(&mut self, damage: &[abi::Rect]) -> Result<Committed, Error> {
        if let Some(e)=self.gpu.as_ref().and_then(|s|s.error()) {
            return Err(Error::Os(Errno::from_raw(-e)));
        }
        let Some(h) = self.held.take() else {
            return self.no_change();
        };
        let processed = h.handled_at_acquire;
        let meta = self.meta(processed);
        let surface = self.gpu.take();
        let c = match window::commit(self.id, h.info.buffer, damage, Some(&meta)) {
            Ok(c) => c,
            Err(e) => {
                // ENOENT: the window is gone; EBUSY and EPERM: the buffer is not this
                // window's acquisition any more. Anything else left it acquired.
                if !matches!(e, Error::Os(Errno::ENOENT | Errno::EBUSY | Errno::EPERM)) {
                    self.gpu = surface;
                    self.held = Some(h);
                }
                return Err(e);
            }
        };
        let done = Committed { commit_id: c.commit_id, pixel_gen: c.pixel_gen, processed_serial: processed };
        self.pending = Some((done, surface));
        Ok(done)
    }

    fn published(&mut self, done: Committed) {
        self.published = done.processed_serial;
        self.since_pixel.retain(|&(serial, _)| serial > done.processed_serial);
        self.last = Some(done);
    }

    /// Confirm publication before advancing library metadata. Holding a non-owning
    /// Surface here retains fence identities, with no additional kernel buffer pin.
    fn finish_pending(&mut self) -> Result<(), Error> {
        let Some((done, surface)) = &self.pending else { return Ok(()); };
        if let Some(surface) = surface {
            surface.wait().map_err(|e| Error::Os(Errno::from_raw(-e)))?;
        }
        let (published, _) = window::read_meta(self.id)?;
        if published < done.commit_id { return Err(Error::Os(Errno::EIO)); }
        let (done, _) = self.pending.take().expect("pending publication checked above");
        self.published(done);
        Ok(())
    }

    /// Commit the frame drawn into `surface` with whole-window damage, or new metadata alone
    /// when no buffer was acquired.
    pub fn present(&mut self) -> Result<(), Error> {
        let whole = abi::Rect::new(0, 0, self.width as i32, self.height as i32);
        self.commit(&[whole]).map(|_| ())
    }

    fn no_change(&mut self) -> Result<Committed, Error> {
        self.finish_pending()?;
        // Every serial up to the first `Redraw` handled since the last pixel commit is
        // finished; a `Redraw` holds the watermark until a frame acquired after it commits.
        let mut processed = self.published;
        for &(serial, handled) in self.since_pixel.iter() {
            if handled == Handled::Redraw {
                break;
            }
            processed = processed.max(serial);
        }
        let meta = self.meta(processed);
        let commit_id = window::commit_no_change(self.id, Some(&meta))?;
        self.published = processed;
        let pixel_gen = self.last.map_or(0, |c| c.pixel_gen);
        let done = Committed { commit_id, pixel_gen, processed_serial: processed };
        self.last = Some(done);
        Ok(done)
    }

    /// Publish metadata against the current frame's pixels: no pixels change and no composite
    /// follows.
    pub fn commit_no_change(&mut self) -> Result<Committed, Error> {
        self.no_change()
    }

    /// What the last commit published.
    pub fn last_commit(&self) -> Option<Committed> {
        self.last
    }

    fn read(&mut self, block: bool, timeout_ns: u64) -> Result<Vec<abi::WindowEvent>, Error> {
        let mut raw = vec![<abi::WindowEvent as abi::Plain>::ZERO; 64];
        let n = window::read_events(self.id, &mut raw, block, timeout_ns)?;
        raw.truncate(n);
        Ok(raw)
    }

    fn deliver(&mut self, raw: &[abi::WindowEvent], handler: &mut dyn FnMut(&Delivered) -> Handled) -> usize {
        let mut count = 0;
        for r in raw {
            let d = Delivered::from_raw(r);
            self.note(&d.event);
            let handled = handler(&d);
            count += 1;
            if d.serial != 0 {
                self.handled = self.handled.max(d.serial);
                self.since_pixel.push((d.serial, handled));
            }
        }
        if matches!(self.font_watcher.poll(), Ok(Some(_))) {
            handler(&Delivered { serial: 0, time_ns: 0, event: Event::FontChanged });
            count += 1;
        }
        count
    }

    /// Run `handler` on every queued event, without waiting. A serial counts as handled once
    /// its handler has returned. Returns how many events were delivered.
    pub fn dispatch(&mut self, mut handler: impl FnMut(&Delivered) -> Handled) -> Result<usize, Error> {
        let raw = self.read(false, 0)?;
        Ok(self.deliver(&raw, &mut handler))
    }

    /// Like `dispatch`, waiting up to `timeout_ns` for the first event.
    pub fn wait_dispatch(&mut self, timeout_ns: u64, mut handler: impl FnMut(&Delivered) -> Handled) -> Result<usize, Error> {
        let raw = self.read(true, timeout_ns)?;
        Ok(self.deliver(&raw, &mut handler))
    }

    /// The queued events, without waiting. Polled events are not handled: they never move the
    /// processed watermark (use `dispatch` for that).
    pub fn poll_events(&mut self) -> Vec<Event> {
        let raw = self.read(false, 0).unwrap_or_default();
        self.decode(raw)
    }

    /// The queued events, waiting up to 100 ms for one. Like `poll_events`, these are not
    /// handled.
    pub fn wait_event(&mut self) -> Vec<Event> {
        let raw = self.read(true, 100_000_000).unwrap_or_default();
        self.decode(raw)
    }

    fn decode(&mut self, raw: Vec<abi::WindowEvent>) -> Vec<Event> {
        let mut events: Vec<Event> = raw.iter().map(Event::from_raw).collect();
        for e in events.iter() {
            self.note(e);
        }
        if matches!(self.font_watcher.poll(), Ok(Some(_))) {
            events.push(Event::FontChanged);
        }
        events
    }

    /// Take what the window needs to know from an event: the compositor's epoch. The window's
    /// size is never taken from an event: it changes only with the buffers, in `apply_resize`,
    /// which is what `Resized` reports.
    fn note(&mut self, event: &Event) {
        if let Event::CompositorChanged { epoch } = *event {
            self.epoch = epoch;
        }
    }

    // ── Fonts ──────────────────────────────────────────────────────────

    /// Take the monospace font out of the window for rendering; on `FontChanged` take it again
    /// for the new one.
    pub fn take_mono_font(&mut self) -> Option<CachedFont> {
        self.font_watcher.take_mono_font()
    }

    /// Return a font taken with `take_mono_font`.
    pub fn put_mono_font(&mut self, font: Option<CachedFont>) {
        self.font_watcher.put_mono_font(font);
    }

    pub fn take_display_font(&mut self) -> Option<CachedFont> {
        self.font_watcher.take_display_font()
    }

    pub fn put_display_font(&mut self, font: Option<CachedFont>) {
        self.font_watcher.put_display_font(font);
    }

    pub fn mono_size(&self) -> f32 {
        self.font_watcher.mono_size()
    }

    pub fn mono_path(&self) -> &str {
        self.font_watcher.mono_path()
    }

    pub fn display_size(&self) -> f32 {
        self.font_watcher.display_size()
    }

    pub fn display_path(&self) -> &str {
        self.font_watcher.display_path()
    }

    pub fn set_font_poll_interval(&mut self, interval: u32) {
        self.font_watcher.set_poll_interval(interval);
    }

    pub fn disable_font_polling(&mut self) {
        self.font_watcher.disable_polling();
    }

    // ── The window ─────────────────────────────────────────────────────

    /// The kernel's id for this window.
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Resize the window's buffers. The new buffers hold the old frame where the sizes
    /// overlap; a buffer held for drawing is given up. The kernel reports `Resized` (or
    /// `ResizeFailed`) through the window's events.
    pub fn apply_resize(&mut self, new_width: u32, new_height: u32) -> Result<(), Error> {
        Self::validate_size(new_width, new_height)?;
        let r = window::resize(self.id, new_width, new_height)?;
        self.held = None;
        self.gpu = None;
        self.buffers = r.buffer;
        self.width = new_width;
        self.height = new_height;
        Ok(())
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        let _ = window::destroy(self.id);
    }
}
