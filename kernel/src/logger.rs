use crate::log_serial_println;
#[cfg(feature = "interactive")]
use bootloader_api::info::{FrameBufferInfo, PixelFormat};
use bootloader_x86_64_common::logger::LockedLogger;
use conquer_once::spin::OnceCell;
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicU64, Ordering};
use log::{Level, LevelFilter, Log, Metadata, Record};
#[cfg(feature = "interactive")]
use noto_sans_mono_bitmap::{get_raster, get_raster_width, FontWeight, RasterHeight, RasterizedChar};
use spin::Mutex;

pub static FRAMEBUFFER_LOGGER: OnceCell<LockedLogger> = OnceCell::uninit();

/// Shell framebuffer for direct shell output (interactive mode only)
#[cfg(feature = "interactive")]
pub static SHELL_FRAMEBUFFER: OnceCell<Mutex<ShellFrameBuffer>> = OnceCell::uninit();

/// Font constants for shell framebuffer
#[cfg(feature = "interactive")]
mod shell_font {
    use super::*;

    /// Height of each char raster
    pub const CHAR_RASTER_HEIGHT: RasterHeight = RasterHeight::Size16;

    /// Width of each char
    pub const CHAR_RASTER_WIDTH: usize = get_raster_width(FontWeight::Regular, CHAR_RASTER_HEIGHT);

    /// Font weight
    pub const FONT_WEIGHT: FontWeight = FontWeight::Regular;

    /// Backup character
    pub const BACKUP_CHAR: char = '?';
}

/// Additional vertical space between lines
#[cfg(feature = "interactive")]
const LINE_SPACING: usize = 2;
/// Additional horizontal space between characters
#[cfg(feature = "interactive")]
const LETTER_SPACING: usize = 0;
/// Padding from the border
#[cfg(feature = "interactive")]
const BORDER_PADDING: usize = 1;

/// Returns the raster of the given char or a backup char
#[cfg(feature = "interactive")]
fn get_char_raster(c: char) -> RasterizedChar {
    fn get(c: char) -> Option<RasterizedChar> {
        get_raster(
            c,
            shell_font::FONT_WEIGHT,
            shell_font::CHAR_RASTER_HEIGHT,
        )
    }
    get(c).unwrap_or_else(|| get(shell_font::BACKUP_CHAR).expect("Should get raster of backup char"))
}

/// Shell framebuffer writer for direct text output
#[cfg(feature = "interactive")]
pub struct ShellFrameBuffer {
    /// Pointer to the framebuffer memory
    buffer_ptr: *mut u8,
    /// Length of the framebuffer
    buffer_len: usize,
    /// Framebuffer info (dimensions, pixel format, etc.)
    info: FrameBufferInfo,
    /// Current x position (in pixels)
    x_pos: usize,
    /// Current y position (in pixels)
    y_pos: usize,
    /// Whether the cursor is currently visible (for blinking)
    cursor_visible: bool,
}

#[cfg(feature = "interactive")]
unsafe impl Send for ShellFrameBuffer {}
#[cfg(feature = "interactive")]
unsafe impl Sync for ShellFrameBuffer {}

#[cfg(feature = "interactive")]
impl ShellFrameBuffer {
    /// Create a new shell framebuffer from raw pointer and info
    ///
    /// # Safety
    /// The caller must ensure that:
    /// - buffer_ptr points to valid framebuffer memory
    /// - buffer_len is the correct length
    /// - The memory remains valid for the lifetime of this struct
    pub unsafe fn new(buffer_ptr: *mut u8, buffer_len: usize, info: FrameBufferInfo) -> Self {
        let mut fb = Self {
            buffer_ptr,
            buffer_len,
            info,
            x_pos: BORDER_PADDING,
            y_pos: BORDER_PADDING,
            cursor_visible: true,
        };
        fb.clear();
        fb
    }

    /// Clear the framebuffer (fill with black)
    pub fn clear(&mut self) {
        self.x_pos = BORDER_PADDING;
        self.y_pos = BORDER_PADDING;
        unsafe {
            core::ptr::write_bytes(self.buffer_ptr, 0, self.buffer_len);
        }
    }

    /// Get framebuffer width
    fn width(&self) -> usize {
        self.info.width
    }

    /// Get framebuffer height
    fn height(&self) -> usize {
        self.info.height
    }

    /// Move to next line
    fn newline(&mut self) {
        self.y_pos += shell_font::CHAR_RASTER_HEIGHT.val() + LINE_SPACING;
        self.carriage_return();

        // Check if we need to scroll
        let new_ypos = self.y_pos + shell_font::CHAR_RASTER_HEIGHT.val() + BORDER_PADDING;
        if new_ypos >= self.height() {
            self.scroll();
        }
    }

    /// Return to beginning of line
    fn carriage_return(&mut self) {
        self.x_pos = BORDER_PADDING;
    }

    /// Scroll the screen up by one line
    fn scroll(&mut self) {
        let line_height = shell_font::CHAR_RASTER_HEIGHT.val() + LINE_SPACING;
        let bytes_per_pixel = self.info.bytes_per_pixel;
        let stride = self.info.stride;

        // Calculate the number of bytes in one row of pixels
        let row_bytes = stride * bytes_per_pixel;
        let scroll_bytes = line_height * row_bytes;

        // Move everything up
        unsafe {
            let src = self.buffer_ptr.add(scroll_bytes);
            let remaining_bytes = self.buffer_len.saturating_sub(scroll_bytes);
            core::ptr::copy(src, self.buffer_ptr, remaining_bytes);

            // Clear the bottom line
            let clear_start = self.buffer_ptr.add(remaining_bytes);
            core::ptr::write_bytes(clear_start, 0, scroll_bytes.min(self.buffer_len - remaining_bytes));
        }

        // Adjust y position
        self.y_pos = self.y_pos.saturating_sub(line_height);
    }

    /// Write a single character to the framebuffer
    pub fn write_char(&mut self, c: char) {
        // Hide cursor before writing
        self.draw_cursor(false);

        match c {
            '\n' => self.newline(),
            '\r' => self.carriage_return(),
            '\x08' => {
                // Backspace: move cursor back and clear the character
                if self.x_pos > BORDER_PADDING {
                    self.x_pos = self.x_pos.saturating_sub(shell_font::CHAR_RASTER_WIDTH + LETTER_SPACING);
                    // Clear the character by writing a space
                    let raster = get_char_raster(' ');
                    self.write_rendered_char(raster);
                    // Move back again since write_rendered_char advances x_pos
                    self.x_pos = self.x_pos.saturating_sub(shell_font::CHAR_RASTER_WIDTH + LETTER_SPACING);
                }
            }
            c => {
                let new_xpos = self.x_pos + shell_font::CHAR_RASTER_WIDTH;
                if new_xpos >= self.width() {
                    self.newline();
                }
                let new_ypos = self.y_pos + shell_font::CHAR_RASTER_HEIGHT.val() + BORDER_PADDING;
                if new_ypos >= self.height() {
                    self.scroll();
                }
                self.write_rendered_char(get_char_raster(c));
            }
        }

        // Show cursor after writing and reset blink state
        self.cursor_visible = true;
        self.draw_cursor(true);
    }

    /// Write a rendered character to the framebuffer
    fn write_rendered_char(&mut self, rendered_char: RasterizedChar) {
        for (y, row) in rendered_char.raster().iter().enumerate() {
            for (x, byte) in row.iter().enumerate() {
                self.write_pixel(self.x_pos + x, self.y_pos + y, *byte);
            }
        }
        self.x_pos += rendered_char.width() + LETTER_SPACING;
    }

    /// Write a pixel at the specified coordinates
    fn write_pixel(&mut self, x: usize, y: usize, intensity: u8) {
        let pixel_offset = y * self.info.stride + x;
        let color = match self.info.pixel_format {
            PixelFormat::Rgb => [intensity, intensity, intensity / 2, 0],
            PixelFormat::Bgr => [intensity / 2, intensity, intensity, 0],
            PixelFormat::U8 => [if intensity > 200 { 0xf } else { 0 }, 0, 0, 0],
            other => {
                // Unsupported format - just use RGB and hope for the best
                let _ = other;
                [intensity, intensity, intensity / 2, 0]
            }
        };
        let bytes_per_pixel = self.info.bytes_per_pixel;
        let byte_offset = pixel_offset * bytes_per_pixel;

        if byte_offset + bytes_per_pixel <= self.buffer_len {
            unsafe {
                let dest = self.buffer_ptr.add(byte_offset);
                for (i, &byte) in color[..bytes_per_pixel].iter().enumerate() {
                    dest.add(i).write_volatile(byte);
                }
            }
        }
    }

    /// Write a string to the framebuffer
    pub fn write_str(&mut self, s: &str) {
        for c in s.chars() {
            self.write_char(c);
        }
    }

    /// Draw the cursor at the current position
    ///
    /// The cursor is rendered as an underscore character at the current position.
    fn draw_cursor(&mut self, visible: bool) {
        let intensity = if visible { 255 } else { 0 };

        // Draw an underscore-style cursor at the current position
        // The cursor is drawn at the bottom of the character cell
        let cursor_height = 2; // 2 pixels tall
        let cursor_y = self.y_pos + shell_font::CHAR_RASTER_HEIGHT.val() - cursor_height;

        for dy in 0..cursor_height {
            for dx in 0..shell_font::CHAR_RASTER_WIDTH {
                self.write_pixel(self.x_pos + dx, cursor_y + dy, intensity);
            }
        }
    }

    /// Toggle the cursor visibility (for blinking effect)
    ///
    /// This should be called from the timer interrupt at ~500ms intervals.
    pub fn toggle_cursor(&mut self) {
        // Hide the current cursor
        self.draw_cursor(false);

        // Toggle visibility state
        self.cursor_visible = !self.cursor_visible;

        // Draw cursor in new state
        self.draw_cursor(self.cursor_visible);
    }

    /// Show the cursor (if it was hidden)
    ///
    /// Part of public cursor API for future TTY control sequences.
    #[allow(dead_code)]
    pub fn show_cursor(&mut self) {
        if !self.cursor_visible {
            self.cursor_visible = true;
            self.draw_cursor(true);
        }
    }

    /// Hide the cursor
    ///
    /// Part of public cursor API for future TTY control sequences.
    #[allow(dead_code)]
    pub fn hide_cursor(&mut self) {
        if self.cursor_visible {
            self.draw_cursor(false);
            self.cursor_visible = false;
        }
    }

    /// Ensure cursor is visible and reset blink timer
    ///
    /// Part of public cursor API for future shell integration.
    #[allow(dead_code)]
    pub fn reset_cursor_blink(&mut self) {
        self.cursor_visible = true;
        self.draw_cursor(true);
    }
}

#[cfg(feature = "interactive")]
impl fmt::Write for ShellFrameBuffer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_str(s);
        Ok(())
    }
}

const BUFFER_SIZE: usize = 8192;

/// Counting sink to suppress TRACE logs while preserving timing
/// This prevents log spam while maintaining the synchronization side effects
#[allow(dead_code)]
struct CountingSink(AtomicU64);

impl CountingSink {
    #[allow(dead_code)]
    const fn new() -> Self {
        CountingSink(AtomicU64::new(0))
    }
}

impl Log for CountingSink {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() == Level::Trace
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            // Increment counter but don't output
            self.0.fetch_add(1, Ordering::Relaxed);

            // Preserve all the timing side effects of log processing
            let _level = record.level();
            let _target = record.target();
            let _args = record.args();

            // This format_args call is crucial - it forces evaluation of the arguments
            // which preserves the timing behavior that prevents the race condition
            let _ = format_args!("{}", _args);
        }
    }

    fn flush(&self) {
        // No-op for counting sink
    }
}

#[allow(dead_code)]
static COUNTING_SINK: CountingSink = CountingSink::new();

/// Buffer for storing log messages before serial is initialized
struct LogBuffer {
    buffer: [u8; BUFFER_SIZE],
    position: usize,
}

impl LogBuffer {
    const fn new() -> Self {
        Self {
            buffer: [0; BUFFER_SIZE],
            position: 0,
        }
    }

    fn write_str(&mut self, s: &str) -> fmt::Result {
        let bytes = s.as_bytes();
        let remaining = BUFFER_SIZE - self.position;

        if bytes.len() > remaining {
            // Buffer is full, drop oldest messages to make room
            // Simple strategy: just keep the newer messages
            return Ok(());
        }

        self.buffer[self.position..self.position + bytes.len()].copy_from_slice(bytes);
        self.position += bytes.len();

        Ok(())
    }

    fn contents(&self) -> &str {
        core::str::from_utf8(&self.buffer[..self.position]).unwrap_or("<invalid UTF-8>")
    }
}

impl Write for LogBuffer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_str(s)
    }
}

/// State of the logger
enum LoggerState {
    /// Buffering messages until serial is ready
    Buffering,
    /// Serial is initialized, flush buffer and start outputting
    SerialReady,
    /// Fully initialized with framebuffer
    FullyInitialized,
}

pub struct CombinedLogger {
    buffer: Mutex<LogBuffer>,
    state: Mutex<LoggerState>,
}

impl CombinedLogger {
    const fn new() -> Self {
        CombinedLogger {
            buffer: Mutex::new(LogBuffer::new()),
            state: Mutex::new(LoggerState::Buffering),
        }
    }

    /// Call this after serial is initialized
    pub fn serial_ready(&self) {
        let mut state = self.state.lock();
        let buffer = self.buffer.lock();

        // Flush buffered messages to log serial (COM2)
        if buffer.position > 0 {
            log_serial_println!("=== Buffered Boot Messages ===");
            log_serial_println!("{}", buffer.contents());
            log_serial_println!("=== End Buffered Messages ===");
        }

        *state = LoggerState::SerialReady;
    }

    /// Call this after framebuffer logger is initialized
    pub fn fully_ready(&self) {
        let mut state = self.state.lock();
        *state = LoggerState::FullyInitialized;
    }
}

impl Log for CombinedLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Trace
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            // SUPPRESS TRACE LOGS TO REDUCE OUTPUT WHILE PRESERVING TIMING
            if record.level() == Level::Trace {
                // Process the log record to preserve timing but don't output
                let _level = record.level();
                let _target = record.target();
                let _args = record.args();
                let _ = format_args!("{}", _args);
                return;
            }
            // Format the message manually to avoid allocation
            let level = record.level();
            let target = record.target();
            let args = record.args();

            // Use try_lock to avoid deadlocks from interrupt context
            let state = match self.state.try_lock() {
                Some(state) => state,
                None => {
                    // If we can't acquire the lock, fall back to basic log serial output (COM2)
                    // This prevents deadlocks when logging from interrupt handlers
                    log_serial_println!("[INTR] {}: {}", target, args);
                    return;
                }
            };

            // Get current timestamp if available
            // TEMPORARILY DISABLE TIMESTAMPS TO DEBUG TIMER INTERRUPT HANG
            let timestamp = 0;

            match *state {
                LoggerState::Buffering => {
                    // Buffer the message without timestamp (we don't have time yet)
                    drop(state); // Release lock before acquiring buffer lock
                    match self.buffer.try_lock() {
                        Some(mut buffer) => {
                            // Format directly into buffer
                            let _ = write!(&mut *buffer, "[{:>5}] {}: {}\n", level, target, args);
                        }
                        None => {
                            // Fall back to log serial (COM2) if buffer is locked
                            log_serial_println!("[BUFF] {}: {}", target, args);
                        }
                    }
                }
                LoggerState::SerialReady => {
                    // Output to log serial (COM2) only with timestamp if available
                    drop(state); // Release lock before serial I/O
                    if timestamp > 0 {
                        log_serial_println!(
                            "{} - [{:>5}] {}: {}",
                            timestamp,
                            record.level(),
                            record.target(),
                            record.args()
                        );
                    } else {
                        log_serial_println!(
                            "[{:>5}] {}: {}",
                            record.level(),
                            record.target(),
                            record.args()
                        );
                    }
                }
                LoggerState::FullyInitialized => {
                    // Output to log serial (COM2) - always
                    drop(state); // Release lock before I/O

                    if timestamp > 0 {
                        log_serial_println!(
                            "{} - [{:>5}] {}: {}",
                            timestamp,
                            record.level(),
                            record.target(),
                            record.args()
                        );
                    } else {
                        log_serial_println!(
                            "[{:>5}] {}: {}",
                            record.level(),
                            record.target(),
                            record.args()
                        );
                    }

                    // In interactive mode, kernel logs go to COM2 ONLY (not framebuffer)
                    // Shell output uses SHELL_FRAMEBUFFER directly
                    #[cfg(feature = "interactive")]
                    {
                        // Kernel logs skip the framebuffer entirely in interactive mode
                        // This keeps the QEMU screen clean for shell output only
                    }

                    // In non-interactive mode, also write to framebuffer
                    #[cfg(not(feature = "interactive"))]
                    {
                        // Write to framebuffer
                        // CRITICAL: Don't write to framebuffer if we're in interrupt/exception context
                        // or using a process page table, as the framebuffer might not be mapped
                        // Check if we're in interrupt context (IRQ) or exception context (preempt disabled)
                        // But only if per-CPU is initialized (otherwise assume we're safe)
                        let skip_framebuffer = if crate::per_cpu::is_initialized() {
                            // Skip if in IRQ context OR if preemption is disabled (exception context)
                            crate::per_cpu::in_interrupt() || crate::per_cpu::preempt_count() > 0
                        } else {
                            false // Early boot, safe to use framebuffer
                        };

                        if !skip_framebuffer {
                            // TODO: Add proper synchronization to prevent rendering conflicts
                            // For now, we'll accept occasional visual glitches rather than deadlock
                            if let Some(fb_logger) = FRAMEBUFFER_LOGGER.get() {
                                fb_logger.log(record);
                            }
                        }
                    }
                }
            }
        }
    }

    fn flush(&self) {
        if let Some(fb_logger) = FRAMEBUFFER_LOGGER.get() {
            fb_logger.flush();
        }
    }
}

pub static COMBINED_LOGGER: CombinedLogger = CombinedLogger::new();

/// Initialize the logger early - can be called before serial is ready
pub fn init_early() {
    // Set up the combined logger with TRACE level
    // The CombinedLogger already suppresses TRACE logs while preserving timing
    log::set_logger(&COMBINED_LOGGER).expect("Logger already set");
    log::set_max_level(LevelFilter::Trace);
}

/// Call after serial port is initialized
pub fn serial_ready() {
    COMBINED_LOGGER.serial_ready();
}

/// Complete initialization with framebuffer
pub fn init_framebuffer(buffer: &'static mut [u8], info: bootloader_api::info::FrameBufferInfo) {
    // In interactive mode, initialize the SHELL_FRAMEBUFFER FIRST
    // by capturing the raw pointer before passing to LockedLogger
    #[cfg(feature = "interactive")]
    {
        // Store the raw pointer and info for shell output
        // SAFETY: We're storing a pointer to the same buffer that LockedLogger will use.
        // This is intentionally sharing the framebuffer memory between kernel logs (LockedLogger)
        // and shell output (ShellFrameBuffer). In interactive mode, kernel logs go to COM2 only,
        // so there's no actual conflict - only shell output uses the framebuffer.
        let buffer_ptr = buffer.as_mut_ptr();
        let buffer_len = buffer.len();
        let _ = SHELL_FRAMEBUFFER.get_or_init(|| {
            Mutex::new(unsafe { ShellFrameBuffer::new(buffer_ptr, buffer_len, info) })
        });
    }

    // Initialize framebuffer logger (used for non-interactive mode)
    let _fb_logger =
        FRAMEBUFFER_LOGGER.get_or_init(move || LockedLogger::new(buffer, info, true, false));

    // Mark logger as fully ready
    COMBINED_LOGGER.fully_ready();

    log::info!("Logger fully initialized - output to both framebuffer and serial");
}

/// Write raw text to framebuffer console (for shell output in interactive mode)
///
/// This writes directly to the SHELL_FRAMEBUFFER, bypassing the logging system.
/// Shell output appears on the QEMU window without log prefixes.
#[cfg(feature = "interactive")]
pub fn write_to_framebuffer(s: &str) {
    if let Some(fb) = SHELL_FRAMEBUFFER.get() {
        if let Some(mut guard) = fb.try_lock() {
            guard.write_str(s);
        }
    }
}

/// Write a single character to framebuffer console (for keyboard echo in interactive mode)
#[cfg(feature = "interactive")]
pub fn write_char_to_framebuffer(byte: u8) {
    if let Some(fb) = SHELL_FRAMEBUFFER.get() {
        if let Some(mut guard) = fb.try_lock() {
            guard.write_char(byte as char);
        }
    }
}

/// Toggle cursor visibility for blinking effect (called from timer interrupt)
///
/// This is called from the timer interrupt at ~500ms intervals to create
/// a blinking cursor effect. Uses try_lock to avoid blocking in interrupt context.
#[cfg(feature = "interactive")]
pub fn toggle_cursor_blink() {
    if let Some(fb) = SHELL_FRAMEBUFFER.get() {
        if let Some(mut guard) = fb.try_lock() {
            guard.toggle_cursor();
        }
    }
}
