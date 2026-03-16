//! Window management for Breengel applications.
//!
//! Wraps the kernel window buffer syscalls into a high-level API.
//! Includes automatic font management via an internal FontWatcher.

use libfont::CachedFont;
use libbreenix::error::Error;
use libbreenix::graphics::{self, WindowInputEvent};
use libgfx::framebuf::FrameBuf;

use crate::event::Event;
use crate::font::FontWatcher;

/// A GUI window backed by a kernel-managed shared pixel buffer.
///
/// Created via [`Window::new`], which allocates a pixel buffer in the kernel
/// and registers it with the compositor. The window is immediately visible.
///
/// Draw into the window via [`framebuf`](Window::framebuf), then call
/// [`present`](Window::present) to signal the compositor.
///
/// Font management is automatic: the window polls `/etc/fonts.conf` during
/// [`poll_events`](Window::poll_events) and emits [`Event::FontChanged`] when
/// the system fonts change. Use [`take_mono_font`](Window::take_mono_font)
/// to get the loaded font for rendering.
pub struct Window {
    buffer_id: u32,
    fb: FrameBuf,
    width: u32,
    height: u32,
    font_watcher: FontWatcher,
}

impl Window {
    /// Create a new window and register it with the compositor.
    ///
    /// The window is immediately visible at a compositor-chosen position.
    /// System fonts are loaded automatically from `/etc/fonts.conf`.
    pub fn new(title: &[u8], width: u32, height: u32) -> Result<Self, Error> {
        let win = graphics::create_window(width, height)?;
        graphics::register_window(win.id, title)?;

        let bpp = 4usize;
        let stride = width as usize * bpp;
        let fb = unsafe {
            FrameBuf::from_raw(
                win.pixels as *mut u8,
                width as usize,
                height as usize,
                stride,
                bpp,
                true, // BGRA for compositor
            )
        };

        Ok(Self {
            buffer_id: win.id,
            fb,
            width,
            height,
            font_watcher: FontWatcher::new(),
        })
    }

    /// Get a mutable reference to the framebuffer for drawing.
    pub fn framebuf(&mut self) -> &mut FrameBuf {
        &mut self.fb
    }

    /// Signal the compositor that this window's pixels have changed.
    ///
    /// Call after drawing a frame. This may block until the compositor
    /// consumes the previous frame (back-pressure / frame pacing).
    pub fn present(&self) -> Result<(), Error> {
        graphics::mark_window_dirty(self.buffer_id)
    }

    /// Poll for pending input events (non-blocking).
    ///
    /// Also polls the system font config — if it changed, an
    /// `Event::FontChanged` is appended to the returned events.
    ///
    /// Resize events are handled automatically: the window buffer is
    /// reallocated to the new dimensions before the `Event::Resized` is
    /// returned.
    pub fn poll_events(&mut self) -> Vec<Event> {
        let mut raw = [WindowInputEvent::default(); 16];
        let mut events = match graphics::read_window_input(self.buffer_id, &mut raw, false) {
            Ok(n) => self.process_raw_events(&raw[..n]),
            Err(_) => Vec::new(),
        };

        if self.font_watcher.poll() {
            events.push(Event::FontChanged);
        }

        events
    }

    /// Wait for at least one input event (blocking).
    ///
    /// Blocks until the compositor delivers an event or a 100ms timeout
    /// expires (in which case the returned vec may be empty).
    /// Also polls font config on each call.
    pub fn wait_event(&mut self) -> Vec<Event> {
        let mut raw = [WindowInputEvent::default(); 16];
        let mut events = match graphics::read_window_input(self.buffer_id, &mut raw, true) {
            Ok(n) => self.process_raw_events(&raw[..n]),
            Err(_) => Vec::new(),
        };

        if self.font_watcher.poll() {
            events.push(Event::FontChanged);
        }

        events
    }

    /// Convert raw events, auto-handling resize internally.
    fn process_raw_events(&mut self, raw: &[WindowInputEvent]) -> Vec<Event> {
        let mut events = Vec::with_capacity(raw.len());
        for r in raw {
            let event = Event::from_raw(r);
            if let Event::Resized { width, height } = &event {
                let _ = self.apply_resize(*width, *height);
            }
            events.push(event);
        }
        events
    }

    // ── Font accessors ──────────────────────────────────────────────────

    /// Take the monospace font out of the window for rendering.
    ///
    /// Returns the loaded `CachedFont`, leaving `None` internally. The font
    /// is yours to use with `ttf_font::draw_char()` etc. When the font config
    /// changes (`Event::FontChanged`), the window loads the new font internally —
    /// call this again to get the updated font.
    pub fn take_mono_font(&mut self) -> Option<CachedFont> {
        self.font_watcher.take_mono_font()
    }

    /// Return a previously taken mono font. Call before taking a new one
    /// on `FontChanged`, or the old font is simply dropped.
    pub fn put_mono_font(&mut self, font: Option<CachedFont>) {
        self.font_watcher.put_mono_font(font);
    }

    /// Take the display font out of the window for rendering.
    pub fn take_display_font(&mut self) -> Option<CachedFont> {
        self.font_watcher.take_display_font()
    }

    /// Return a previously taken display font.
    pub fn put_display_font(&mut self, font: Option<CachedFont>) {
        self.font_watcher.put_display_font(font);
    }

    /// The current monospace font size in pixels.
    pub fn mono_size(&self) -> f32 {
        self.font_watcher.mono_size()
    }

    /// The current monospace font file path.
    pub fn mono_path(&self) -> &str {
        self.font_watcher.mono_path()
    }

    /// The current display font size in pixels.
    pub fn display_size(&self) -> f32 {
        self.font_watcher.display_size()
    }

    /// The current display font file path.
    pub fn display_path(&self) -> &str {
        self.font_watcher.display_path()
    }

    /// Set how many `poll_events()` calls between font config file checks.
    /// Default is 20 (at 50ms sleep = ~1 second).
    pub fn set_font_poll_interval(&mut self, interval: u32) {
        self.font_watcher.set_poll_interval(interval);
    }

    // ── Window metadata ─────────────────────────────────────────────────

    /// The kernel-assigned buffer ID for this window.
    pub fn id(&self) -> u32 {
        self.buffer_id
    }

    /// Window width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Window height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Resize the window buffer to new dimensions.
    ///
    /// Called after receiving `Event::Resized`. The kernel allocates new pages,
    /// copies the intersection of old content, and returns a new mmap pointer.
    /// The internal FrameBuf is recreated at the new dimensions.
    pub fn apply_resize(&mut self, new_width: u32, new_height: u32) -> Result<(), Error> {
        let win = graphics::resize_window(self.buffer_id, new_width, new_height)?;

        let bpp = 4usize;
        let stride = new_width as usize * bpp;
        self.fb = unsafe {
            FrameBuf::from_raw(
                win.pixels as *mut u8,
                new_width as usize,
                new_height as usize,
                stride,
                bpp,
                true,
            )
        };
        self.width = new_width;
        self.height = new_height;

        Ok(())
    }
}
