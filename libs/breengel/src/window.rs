//! Window management for Breengel applications.
//!
//! Wraps the kernel window buffer syscalls into a high-level API.

use libbreenix::error::Error;
use libbreenix::graphics::{self, WindowInputEvent};
use libgfx::framebuf::FrameBuf;

use crate::event::Event;

/// A GUI window backed by a kernel-managed shared pixel buffer.
///
/// Created via [`Window::new`], which allocates a pixel buffer in the kernel
/// and registers it with the compositor. The window is immediately visible.
///
/// Draw into the window via [`framebuf`](Window::framebuf), then call
/// [`present`](Window::present) to signal the compositor.
pub struct Window {
    buffer_id: u32,
    fb: FrameBuf,
    width: u32,
    height: u32,
}

impl Window {
    /// Create a new window and register it with the compositor.
    ///
    /// The window is immediately visible at a compositor-chosen position.
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
    /// Resize events are handled automatically: the window buffer is
    /// reallocated to the new dimensions before the `Event::Resized` is
    /// returned. Applications only need to update their own layout state
    /// (e.g. recalculate visible lines) — they do NOT need to call
    /// `apply_resize()`.
    pub fn poll_events(&mut self) -> Vec<Event> {
        let mut raw = [WindowInputEvent::default(); 16];
        match graphics::read_window_input(self.buffer_id, &mut raw, false) {
            Ok(n) => self.process_raw_events(&raw[..n]),
            Err(_) => Vec::new(),
        }
    }

    /// Wait for at least one input event (blocking).
    ///
    /// Blocks until the compositor delivers an event or a 100ms timeout
    /// expires (in which case the returned vec may be empty).
    /// Resize events are handled automatically (see [`poll_events`]).
    pub fn wait_event(&mut self) -> Vec<Event> {
        let mut raw = [WindowInputEvent::default(); 16];
        match graphics::read_window_input(self.buffer_id, &mut raw, true) {
            Ok(n) => self.process_raw_events(&raw[..n]),
            Err(_) => Vec::new(),
        }
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
