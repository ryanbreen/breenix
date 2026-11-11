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
use libgfx::ttf_font::CachedFont;
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
    stamp: crate::clock::Stamp,
}

/// A GUI window on the kernel window interface.
pub struct Window {
    id: u64,
    width: u32,
    height: u32,
    buffers: [u64; 3],
    tree: u64,
    tree_bytes: usize,
    tree_readers: abi::MapTree,
    semantic_tree: Option<crate::tree::Tree>,
    tree_serializations: u64,
    held: Option<Held>,
    gpu: Option<libgfx::surface::Surface>,
    font_watcher: FontWatcher,
    /// The highest serial whose handler has returned.
    handled: u64,
    /// Serials handled since the last pixel commit, with what their handlers returned.
    since_pixel: Vec<(u64, Handled)>,
    /// The processed watermark last published.
    published: u64,
    ui_stamp: crate::clock::Stamp,
    clock_ack_stamp: crate::clock::Stamp,
    clock_dispatched: crate::clock::Stamp,
    clock_redraw: bool,
    timers: crate::timer::Timers,
    animating: bool,
    clock_participant: Option<crate::clock::Participant>,
    last: Option<Committed>,
    pending: Option<(Committed, Option<libgfx::surface::Surface>, crate::clock::Stamp)>,
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
        let tree_readers = match window::map_tree_readers(c.id) {
            Ok(mapping) => mapping,
            Err(error) => { let _ = window::destroy(c.id); return Err(error); }
        };
        let clock_participant = match crate::clock::Participant::configured(c.id) {
            Ok(participant) => participant,
            Err(_) => {
                let _ = libbreenix::memory::munmap(tree_readers.addr as *mut u8, tree_readers.len as usize);
                let _ = window::destroy(c.id);
                return Err(Error::Os(Errno::EINVAL));
            }
        };
        let ui_stamp = crate::clock::stamp();
        Ok(Self {
            id: c.id,
            width,
            height,
            buffers: c.buffer,
            tree: c.tree,
            tree_bytes: c.tree_kib as usize * 1024,
            tree_readers,
            semantic_tree: None,
            tree_serializations: 0,
            held: None,
            gpu: None,
            font_watcher,
            handled: 0,
            since_pixel: Vec::new(),
            published: 0,
            ui_stamp,
            clock_ack_stamp: ui_stamp,
            clock_dispatched: ui_stamp,
            clock_redraw: false,
            timers: crate::timer::Timers::new(ui_stamp.now_ns / 1_000_000),
            animating: false,
            clock_participant,
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
        if let Some((done, _, stamp)) = self.pending.take() {
            if a.base >= done.commit_id { self.published(done, stamp); }
        }
        let info = Acquired {
            buffer: a.buffer,
            base: a.base,
            replay_full: a.replay_full != 0,
            replay: a.replay[..(a.replay_count as usize).min(16)].to_vec(),
        };
        self.gpu = Some(self.surface_over(a.buffer)?);
        let mut stamp = crate::clock::stamp();
        // A caller acquiring before dispatch cannot acknowledge timer work that
        // has not run yet. A frame rendered after dispatch uses the new stamp.
        if self.has_clock_work(stamp) && stamp != self.clock_dispatched {
            stamp = self.clock_dispatched;
        }
        self.held = Some(Held { info: info.clone(), handled_at_acquire: self.handled, stamp });
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

    /// Replace the semantic state to serialize with the next pixel commit.
    /// Updating state does not serialize it or acquire a pixel buffer.
    pub fn set_tree(&mut self, tree: crate::tree::Tree) {
        self.semantic_tree = Some(tree);
    }

    /// Report application-managed geometry changes alongside libbui's layout calls.
    pub fn layout_changed(&mut self) { libbui::layout::layout_changed(); }

    pub fn layout_version(&self) -> u64 { libbui::layout::layout_version() }

    /// The live MAP_TREE count, read from the page mapped once at construction.
    pub fn tree_reader_count(&self) -> u64 {
        // The kernel keeps this read-only, aligned mapping alive until munmap.
        let state = unsafe { &*(self.tree_readers.addr as *const abi::TreeReaderState) };
        state.readers.load(core::sync::atomic::Ordering::Acquire)
    }

    /// Write serialized tree bytes into the currently acquired buffer's page.
    /// ACQUIRE makes only this page writable; COMMIT seals it with the pixels.
    pub fn write_tree(&mut self, bytes: &[u8]) -> Result<(), Error> {
        if bytes.len() > self.tree_bytes { return Err(Error::Os(Errno::EINVAL)); }
        let buffer = self.acquire(u64::MAX)?.buffer as usize;
        // The window owns CREATE's mapping and the acquisition excludes readers
        // and other writers of this buffer until commit or cancellation.
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), (self.tree as *mut u8).add(buffer * self.tree_bytes), bytes.len()); }
        Ok(())
    }

    /// Metadata describes the epoch rendered by the frame, sampled before drawing.
    fn meta(&self, processed_serial: u64, stamp: crate::clock::Stamp) -> abi::Meta {
        let mut m = <abi::Meta as abi::Plain>::ZERO;
        m.processed_serial = processed_serial;
        m.epoch = stamp.epoch;
        m.ui_time_ms = stamp.now_ns / 1_000_000;
        m.real_clock_reads = crate::clock::real_clock_reads();
        m.tree_serializations = self.tree_serializations;
        m.layout_version = libbui::layout::layout_version();
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
        if let (Some(h), Some(tree)) = (&self.held, &self.semantic_tree) {
            let readers = self.tree_reader_count();
            crate::tree_commit::serialize_for_commit(readers, &mut self.tree_serializations, || {
                // The acquired buffer's tree page is writable until COMMIT seals it.
                let page = unsafe { core::slice::from_raw_parts_mut(
                    (self.tree as *mut u8).add(h.info.buffer as usize * self.tree_bytes), self.tree_bytes) };
                tree.write(page).map(|_| ())
            }).map_err(|_| Error::Os(Errno::EINVAL))?;
        }
        let Some(h) = self.held.take() else {
            return self.no_change();
        };
        let processed = h.handled_at_acquire;
        let meta = self.meta(processed, h.stamp);
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
        self.pending = Some((done, surface, h.stamp));
        Ok(done)
    }

    fn published(&mut self, done: Committed, stamp: crate::clock::Stamp) {
        self.published = done.processed_serial;
        self.since_pixel.retain(|&(serial, _)| libbreenix::input::abi::source_order(serial)
            > libbreenix::input::abi::source_order(done.processed_serial));
        self.last = Some(done);
        if stamp.epoch >= self.clock_dispatched.epoch && stamp.now_ns >= self.clock_dispatched.now_ns {
            self.clock_redraw = false;
        }
        self.clock_ack_stamp = stamp;
    }

    /// Confirm publication before advancing library metadata. Holding a non-owning
    /// Surface here retains fence identities, with no additional kernel buffer pin.
    fn finish_pending(&mut self) -> Result<(), Error> {
        let Some((done, surface, _)) = &self.pending else { return Ok(()); };
        if let Some(surface) = surface {
            surface.wait().map_err(|e| Error::Os(Errno::from_raw(-e)))?;
        }
        let (published, _) = window::read_meta(self.id)?;
        if published < done.commit_id { return Err(Error::Os(Errno::EIO)); }
        let (done, _, stamp) = self.pending.take().expect("pending publication checked above");
        self.published(done, stamp);
        if let Some(participant) = &self.clock_participant {
            let (_, meta) = window::read_meta(self.id)?;
            participant.accepted(crate::clock::Stamp { epoch: meta.epoch, now_ns: meta.ui_time_ms * 1_000_000 });
            self.clock_ack_stamp = crate::clock::Stamp { epoch: meta.epoch, now_ns: meta.ui_time_ms * 1_000_000 };
        }
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
            processed = libbreenix::input::abi::advance_watermark(processed, serial);
        }
        // An explicit metadata-only commit cannot stand in for unfinished timer
        // callbacks or a callback's requested redraw.
        let stamp = if self.clock_redraw || (self.has_clock_work(self.ui_stamp) && self.clock_dispatched != self.ui_stamp) {
            self.clock_ack_stamp
        } else { self.ui_stamp };
        let meta = self.meta(processed, stamp);
        let commit_id = window::commit_no_change(self.id, Some(&meta))?;
        if let Some(participant) = &self.clock_participant { participant.accepted(stamp); }
        self.clock_ack_stamp = stamp;
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

    /// Set the next UI timer deadline and animation state used by the step driver.
    pub fn clock_schedule(&self, next_deadline_ns: u64, animating: bool) {
        if let Some(participant) = &self.clock_participant { participant.schedule(next_deadline_ns, animating); }
    }

    /// Register a timer relative to UI time. A zero period is a one-shot.
    /// Timer callbacks are delivered through `dispatch` and `wait_dispatch`.
    /// (claim-lint:ok: tests/ui_clock_timers.rs zero_step_fires_only_due_timers.)
    pub fn timer(&mut self, id: u64, delay_ms: u64, period_ms: u64) {
        let deadline = (crate::clock::now_ns() / 1_000_000).checked_add(delay_ms)
            .expect("UI timer deadline overflow");
        self.timers.register(id, deadline, period_ms);
        self.schedule_timers();
    }

    /// Enable fixed 1 ms integration events, rendering after the whole dispatch.
    pub fn set_animating(&mut self, animating: bool) {
        self.animating = animating;
        self.schedule_timers();
    }

    fn schedule_timers(&self) {
        self.clock_schedule(self.timers.next_deadline().saturating_mul(1_000_000), self.animating);
    }

    fn has_clock_work(&self, stamp: crate::clock::Stamp) -> bool {
        self.animating || self.timers.next_deadline() <= stamp.now_ns / 1_000_000
    }

    // Failure-only context: preserve the errno while distinguishing an event
    // timeout from a pending GPU publication failure.
    fn read_error(&self, path: &str, block: bool, timeout_ns: u64, error: Error) -> Error {
        let at_ns = libbreenix::time::now_monotonic().map(|t| t.as_nanos());
        eprintln!("WINDOW_READ_ERROR pid={} window={} path={path} participant={} clock_configured={} next_timer_ms={} animating={} block={block} timeout_ns={timeout_ns} at_ns={} error={error}",
            std::process::id(), self.id, self.clock_participant.is_some(), crate::clock::configured(),
            self.timers.next_deadline(), self.animating,
            at_ns.map_or_else(|e| format!("unavailable:{e}"), |ns| ns.to_string()));
        error
    }

    fn read(&mut self, block: bool, timeout_ns: u64) -> Result<Vec<abi::WindowEvent>, Error> {
        // Complete any prior asynchronous publication before dispatch can mark
        // new timer work dirty.
        if self.animating || self.timers.next_deadline() != u64::MAX || self.clock_redraw {
            self.finish_pending().map_err(|e| self.read_error("finish-pending", block, timeout_ns, e))?;
        }
        let mut raw = vec![<abi::WindowEvent as abi::Plain>::ZERO; 64];
        if self.clock_participant.is_none() {
            if self.timers.next_deadline() == u64::MAX && !self.animating {
                let n = window::read_events(self.id, &mut raw, block, timeout_ns)
                    .map_err(|e| self.read_error("idle-read-events", block, timeout_ns, e))?;
                raw.truncate(n);
                return Ok(raw);
            }
            let now = crate::clock::stamp();
            let until_timer = self.timers.next_deadline().saturating_mul(1_000_000).saturating_sub(now.now_ns);
            let timeout = timeout_ns.min(until_timer).min(if self.animating { 16_000_000 } else { u64::MAX });
            let n = match window::read_events(self.id, &mut raw, block, timeout) {
                Ok(n) => n,
                Err(Error::Os(Errno::ETIMEDOUT)) => 0, // The timer wait expired normally.
                Err(error) => return Err(self.read_error("timer-read-events", block, timeout, error)),
            };
            self.ui_stamp = crate::clock::stamp();
            raw.truncate(n);
            return Ok(raw);
        }
        let began = std::time::Instant::now();
        loop {
            let generation = self.clock_participant.as_ref().unwrap().page().wake_generation();
            let stamp = crate::clock::stamp();
            let changed = stamp.epoch != self.ui_stamp.epoch;
            self.ui_stamp = stamp;
            let n = window::read_events(self.id, &mut raw, false, 0)
                .map_err(|e| self.read_error("participant-read-events", false, 0, e))?;
            if n != 0 || changed || !block {
                raw.truncate(n);
                return Ok(raw);
            }
            let elapsed = began.elapsed().as_nanos().min(u64::MAX as u128) as u64;
            if elapsed >= timeout_ns { return Ok(Vec::new()); }
            self.clock_participant.as_ref().unwrap().page().wait(generation, (timeout_ns - elapsed).min(1_000_000));
        }
    }

    fn deliver_clock(&mut self, handler: &mut dyn FnMut(&Delivered) -> Handled) -> usize {
        if !self.animating && self.timers.next_deadline() == u64::MAX { return 0; }
        let mut count = 0;
        let mut redraw = false;
        let elapsed = (self.ui_stamp.now_ns / 1_000_000).saturating_sub(self.timers.now());
        let fired = if self.animating {
            self.timers.advance(elapsed, |time_ms| {
                let event = Delivered { serial: 0, time_ns: time_ms * 1_000_000, clicks: 0,
                    event: Event::AnimationTick { time_ms } };
                redraw |= handler(&event) == Handled::Redraw;
                count += 1;
            })
        } else { self.timers.step(elapsed) };
        for (id, missed) in fired {
            let event = Delivered { serial: 0, time_ns: self.ui_stamp.now_ns, clicks: 0,
                event: Event::Timer { id, missed } };
            redraw |= handler(&event) == Handled::Redraw;
            count += 1;
        }
        self.clock_redraw |= redraw;
        self.clock_dispatched = self.ui_stamp;
        self.schedule_timers();
        count
    }

    fn acknowledge_clock(&mut self) -> Result<(), Error> {
        if self.clock_participant.is_some() && self.clock_ack_stamp.epoch != self.ui_stamp.epoch
            && !self.clock_redraw && (!self.has_clock_work(self.ui_stamp) || self.clock_dispatched == self.ui_stamp)
            && self.held.is_none() && (self.last.is_some() || self.pending.is_some())
            && !self.since_pixel.iter().any(|(_, handled)| *handled == Handled::Redraw) {
            self.no_change()?;
        }
        Ok(())
    }

    fn deliver(&mut self, raw: &[abi::WindowEvent], handler: &mut dyn FnMut(&Delivered) -> Handled) -> usize {
        let mut count = self.deliver_clock(handler);
        for r in raw {
            let d = Delivered::from_raw(r);
            let handled = handler(&d);
            count += 1;
            if d.serial != 0 {
                self.handled = libbreenix::input::abi::advance_watermark(self.handled, d.serial);
                self.since_pixel.push((d.serial, handled));
            }
        }
        if matches!(self.font_watcher.poll(), Ok(Some(_))) {
            handler(&Delivered { serial: 0, time_ns: 0, clicks: 0, event: Event::FontChanged });
            count += 1;
        }
        count
    }

    /// Run `handler` on every queued event, without waiting. A serial counts as handled once
    /// its handler has returned. Returns how many events were delivered.
    pub fn dispatch(&mut self, mut handler: impl FnMut(&Delivered) -> Handled) -> Result<usize, Error> {
        let raw = self.read(false, 0)?;
        let count = self.deliver(&raw, &mut handler);
        self.acknowledge_clock()?;
        Ok(count)
    }

    /// Like `dispatch`, waiting up to `timeout_ns` for the first event.
    pub fn wait_dispatch(&mut self, timeout_ns: u64, mut handler: impl FnMut(&Delivered) -> Handled) -> Result<usize, Error> {
        let raw = self.read(true, timeout_ns)?;
        let count = self.deliver(&raw, &mut handler);
        self.acknowledge_clock()?;
        Ok(count)
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
        if matches!(self.font_watcher.poll(), Ok(Some(_))) {
            events.push(Event::FontChanged);
        }
        // Polling alone does not handle timer callbacks. Idle legacy pollers can
        // still acknowledge epochs when they have no timer or animation work.
        let _ = self.acknowledge_clock();
        events
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
        self.layout_changed();
        Ok(())
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        let _ = libbreenix::memory::munmap(self.tree_readers.addr as *mut u8, self.tree_readers.len as usize);
        let _ = window::destroy(self.id);
    }
}
