use crate::log_serial_println;
#[cfg(feature = "interactive")]
use crate::graphics::DoubleBufferedFrameBuffer;
#[cfg(feature = "interactive")]
use crate::graphics::primitives::{Canvas, Color};
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

/// ANSI escape sequence parser state
#[cfg(feature = "interactive")]
#[derive(Debug, Clone, Copy, PartialEq)]
enum AnsiState {
    Normal,
    Escape, // Saw ESC (0x1B)
    Csi,    // Saw ESC[
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
    /// Pointer to hardware framebuffer memory (from bootloader)
    buffer_ptr: *mut u8,
    /// Length of hardware buffer in bytes
    buffer_len: usize,
    /// Framebuffer metadata (dimensions, pixel format, stride)
    info: FrameBufferInfo,
    /// Double buffer (Some after heap init, None before)
    double_buffer: Option<DoubleBufferedFrameBuffer>,
    /// Current x position (in pixels)
    x_pos: usize,
    /// Current y position (in pixels)
    y_pos: usize,
    /// Whether the cursor is currently visible (for blinking)
    cursor_visible: bool,
    /// ANSI escape sequence parser state
    ansi_state: AnsiState,
    /// ANSI escape sequence parameters
    ansi_params: [u8; 16],
    /// Current ANSI parameter index
    ansi_param_idx: usize,
}

#[cfg(feature = "interactive")]
unsafe impl Send for ShellFrameBuffer {}
#[cfg(feature = "interactive")]
unsafe impl Sync for ShellFrameBuffer {}

#[cfg(feature = "interactive")]
impl ShellFrameBuffer {
    /// Create a new shell framebuffer with direct hardware writes.
    ///
    /// Used during early boot before the heap is initialized.
    /// Call `upgrade_to_double_buffer()` after heap init for tear-free rendering.
    ///
    /// # Safety
    /// The caller must ensure that buffer_ptr points to valid framebuffer memory.
    pub fn new_direct(buffer_ptr: *mut u8, buffer_len: usize, info: FrameBufferInfo) -> Self {
        let mut fb = Self {
            buffer_ptr,
            buffer_len,
            info,
            double_buffer: None,
            x_pos: BORDER_PADDING,
            y_pos: BORDER_PADDING,
            cursor_visible: true,
            ansi_state: AnsiState::Normal,
            ansi_params: [0; 16],
            ansi_param_idx: 0,
        };
        fb.clear();
        fb
    }

    /// Upgrade from direct writes to double-buffered mode.
    ///
    /// This allocates a shadow buffer on the heap, so it must be called
    /// after the heap allocator is initialized.
    pub fn upgrade_to_double_buffer(&mut self) {
        if self.double_buffer.is_none() {
            let stride = self.info.stride * self.info.bytes_per_pixel;
            let height = self.info.height;
            let double_buffer =
                DoubleBufferedFrameBuffer::new(self.buffer_ptr, self.buffer_len, stride, height);
            self.double_buffer = Some(double_buffer);
            log::info!("Shell framebuffer upgraded to double buffering");
        }
    }

    /// Get mutable access to the double buffer (if available).
    ///
    /// Returns None if double buffering has not been enabled yet.
    pub fn double_buffer_mut(&mut self) -> Option<&mut DoubleBufferedFrameBuffer> {
        self.double_buffer.as_mut()
    }

    /// Clear the framebuffer (fill with black)
    pub fn clear(&mut self) {
        self.x_pos = BORDER_PADDING;
        self.y_pos = BORDER_PADDING;
        if let Some(db) = &mut self.double_buffer {
            db.buffer_mut().fill(0);
            db.flush_full();
        } else {
            unsafe {
                core::ptr::write_bytes(self.buffer_ptr, 0, self.buffer_len);
            }
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
        let height = self.info.height;
        let row_bytes = stride * bytes_per_pixel;
        let scroll_bytes = line_height * row_bytes;

        if let Some(db) = &mut self.double_buffer {
            db.flush_if_dirty();
            let buffer_len = db.buffer_mut().len();
            if scroll_bytes < buffer_len {
                {
                    let buffer = db.buffer_mut();
                    buffer.copy_within(scroll_bytes..buffer_len, 0);
                    let clear_start = buffer_len - scroll_bytes;
                    buffer[clear_start..].fill(0);
                }
                db.scroll_hardware_up(scroll_bytes);

                let clear_start_y = height.saturating_sub(line_height);
                for y in clear_start_y..height {
                    db.mark_region_dirty(y, 0, row_bytes);
                }
                db.flush();
            } else {
                {
                    let buffer = db.buffer_mut();
                    buffer.fill(0);
                }
                db.flush_full();
            }
        } else {
            unsafe {
                let src = self.buffer_ptr.add(scroll_bytes);
                let remaining_bytes = self.buffer_len.saturating_sub(scroll_bytes);
                core::ptr::copy(src, self.buffer_ptr, remaining_bytes);

                let clear_start = self.buffer_ptr.add(remaining_bytes);
                core::ptr::write_bytes(
                    clear_start,
                    0,
                    scroll_bytes.min(self.buffer_len - remaining_bytes),
                );
            }
        }

        // Adjust y position
        self.y_pos = self.y_pos.saturating_sub(line_height);
    }

    /// Write a single character to the framebuffer (with ANSI escape sequence parsing)
    ///
    /// This is the public API for single character input (e.g., keyboard).
    /// Flushes the double buffer after processing to make changes visible.
    pub fn write_char(&mut self, c: char) {
        self.write_char_internal(c);
        self.flush_if_needed();
    }

    /// Flush the double buffer if in double-buffered mode and dirty
    fn flush_if_needed(&mut self) {
        if let Some(db) = &mut self.double_buffer {
            db.flush_if_dirty();
        }
        // Direct mode doesn't need flushing - writes go directly to hardware
    }

    /// Internal character write without automatic flush.
    ///
    /// Used by write_str to batch multiple characters before flushing.
    fn write_char_internal(&mut self, c: char) {
        let byte = c as u8;

        match self.ansi_state {
            AnsiState::Normal => {
                if byte == 0x1B {
                    // ESC character - start escape sequence
                    self.ansi_state = AnsiState::Escape;
                    return;
                }
                self.write_char_normal(byte);
            }
            AnsiState::Escape => {
                if byte == b'[' {
                    // CSI sequence (ESC[)
                    self.ansi_state = AnsiState::Csi;
                    self.ansi_param_idx = 0;
                    self.ansi_params = [0; 16];
                    return;
                }
                // Not a CSI sequence, output both and return to normal
                self.ansi_state = AnsiState::Normal;
                self.write_char_normal(0x1B);
                self.write_char_normal(byte);
            }
            AnsiState::Csi => {
                if byte >= b'0' && byte <= b'9' {
                    // Accumulate parameter digit
                    if self.ansi_param_idx < 16 {
                        self.ansi_params[self.ansi_param_idx] = self
                            .ansi_params[self.ansi_param_idx]
                            .saturating_mul(10)
                            .saturating_add(byte - b'0');
                    }
                    return;
                }
                if byte == b';' {
                    // Next parameter
                    self.ansi_param_idx = (self.ansi_param_idx + 1).min(15);
                    return;
                }
                // Command byte - execute and return to normal
                self.ansi_state = AnsiState::Normal;
                self.execute_csi(byte);
            }
        }
    }

    /// Write a normal character (not part of escape sequence)
    fn write_char_normal(&mut self, byte: u8) {
        // Hide cursor before writing
        self.draw_cursor(false);

        match byte {
            b'\n' => self.newline(),
            b'\r' => self.carriage_return(),
            0x08 => {
                // Backspace: move cursor back and clear the character
                if self.x_pos > BORDER_PADDING {
                    self.x_pos =
                        self.x_pos
                            .saturating_sub(shell_font::CHAR_RASTER_WIDTH + LETTER_SPACING);
                    // Clear the character by writing a space
                    let raster = get_char_raster(' ');
                    self.write_rendered_char(raster);
                    // Move back again since write_rendered_char advances x_pos
                    self.x_pos =
                        self.x_pos
                            .saturating_sub(shell_font::CHAR_RASTER_WIDTH + LETTER_SPACING);
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
                self.write_rendered_char(get_char_raster(c as char));
            }
        }

        // Show cursor after writing and reset blink state
        self.cursor_visible = true;
        self.draw_cursor(true);
    }

    /// Execute a CSI (Control Sequence Introducer) command
    fn execute_csi(&mut self, cmd: u8) {
        let param1 = self.ansi_params[0] as usize;

        match cmd {
            b'J' => {
                // Erase in Display
                match param1 {
                    2 => self.clear_screen(), // Clear entire screen
                    _ => {}                   // Ignore other modes for now
                }
            }
            b'H' => {
                // Cursor Position - move to home (or specified position)
                self.x_pos = BORDER_PADDING;
                self.y_pos = BORDER_PADDING;
            }
            b'K' => {
                // Erase in Line - clear to end of line
                self.clear_to_eol();
            }
            _ => {
                // Unknown command - ignore
            }
        }
    }

    /// Clear the entire screen and reset cursor to home
    fn clear_screen(&mut self) {
        // Fill entire framebuffer with background color (black)
        for y in 0..self.height() {
            for x in 0..self.width() {
                self.write_pixel(x, y, 0x00);
            }
        }
        // Reset cursor position
        self.x_pos = BORDER_PADDING;
        self.y_pos = BORDER_PADDING;
    }

    /// Clear from cursor to end of line
    fn clear_to_eol(&mut self) {
        let char_height = shell_font::CHAR_RASTER_HEIGHT.val();
        for y in self.y_pos..(self.y_pos + char_height) {
            for x in self.x_pos..self.width() {
                self.write_pixel(x, y, 0x00);
            }
        }
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
                let _ = other;
                [intensity, intensity, intensity / 2, 0]
            }
        };
        let bytes_per_pixel = self.info.bytes_per_pixel;
        let byte_offset = pixel_offset * bytes_per_pixel;
        let x_byte_offset = x * bytes_per_pixel;

        if let Some(db) = &mut self.double_buffer {
            let buffer = db.buffer_mut();
            if byte_offset + bytes_per_pixel <= buffer.len() {
                for (i, &byte) in color[..bytes_per_pixel].iter().enumerate() {
                    buffer[byte_offset + i] = byte;
                }
                db.mark_region_dirty(
                    y,
                    x_byte_offset,
                    x_byte_offset + bytes_per_pixel,
                );
            }
        } else if byte_offset + bytes_per_pixel <= self.buffer_len {
            unsafe {
                let dest = self.buffer_ptr.add(byte_offset);
                for (i, &byte) in color[..bytes_per_pixel].iter().enumerate() {
                    dest.add(i).write_volatile(byte);
                }
            }
        }
    }

    /// Write a string to the framebuffer
    ///
    /// Batches all character writes and flushes once at the end for efficiency.
    pub fn write_str(&mut self, s: &str) {
        for c in s.chars() {
            self.write_char_internal(c);
        }
        self.flush_if_needed();
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
        self.flush_if_needed();
    }

    /// Show the cursor (if it was hidden)
    ///
    /// Part of public cursor API for future TTY control sequences.
    #[allow(dead_code)]
    pub fn show_cursor(&mut self) {
        if !self.cursor_visible {
            self.cursor_visible = true;
            self.draw_cursor(true);
            self.flush_if_needed();
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
            self.flush_if_needed();
        }
    }

    /// Ensure cursor is visible and reset blink timer
    ///
    /// Part of public cursor API for future shell integration.
    #[allow(dead_code)]
    pub fn reset_cursor_blink(&mut self) {
        self.cursor_visible = true;
        self.draw_cursor(true);
        self.flush_if_needed();
    }
}

#[cfg(feature = "interactive")]
impl Canvas for ShellFrameBuffer {
    fn width(&self) -> usize {
        self.info.width
    }

    fn height(&self) -> usize {
        self.info.height
    }

    fn bytes_per_pixel(&self) -> usize {
        self.info.bytes_per_pixel
    }

    fn stride(&self) -> usize {
        self.info.stride
    }

    fn is_bgr(&self) -> bool {
        true
    }

    fn set_pixel(&mut self, x: i32, y: i32, color: Color) {
        if x < 0 || y < 0 {
            return;
        }
        let x = x as usize;
        let y = y as usize;
        if x >= self.info.width || y >= self.info.height {
            return;
        }

        let bytes_per_pixel = self.info.bytes_per_pixel;
        let pixel_bytes = color.to_pixel_bytes(bytes_per_pixel, self.is_bgr());

        let pixel_offset = y * self.info.stride + x;
        let byte_offset = pixel_offset * bytes_per_pixel;

        if let Some(db) = &mut self.double_buffer {
            let buffer = db.buffer_mut();
            if byte_offset + bytes_per_pixel <= buffer.len() {
                for (i, &byte) in pixel_bytes[..bytes_per_pixel].iter().enumerate() {
                    buffer[byte_offset + i] = byte;
                }
                let x_byte_offset = x * bytes_per_pixel;
                db.mark_region_dirty(y, x_byte_offset, x_byte_offset + bytes_per_pixel);
            }
        } else if byte_offset + bytes_per_pixel <= self.buffer_len {
            unsafe {
                let dest = self.buffer_ptr.add(byte_offset);
                for (i, &byte) in pixel_bytes[..bytes_per_pixel].iter().enumerate() {
                    *dest.add(i) = byte;
                }
            }
        }
    }

    fn get_pixel(&self, x: i32, y: i32) -> Option<Color> {
        if x < 0 || y < 0 {
            return None;
        }
        let x = x as usize;
        let y = y as usize;
        if x >= self.info.width || y >= self.info.height {
            return None;
        }

        let bytes_per_pixel = self.info.bytes_per_pixel;
        let pixel_offset = y * self.info.stride + x;
        let byte_offset = pixel_offset * bytes_per_pixel;

        let buffer = self.buffer();
        if byte_offset + bytes_per_pixel > buffer.len() {
            return None;
        }

        Some(Color::from_pixel_bytes(
            &buffer[byte_offset..byte_offset + bytes_per_pixel],
            bytes_per_pixel,
            self.is_bgr(),
        ))
    }

    fn buffer_mut(&mut self) -> &mut [u8] {
        if let Some(db) = &mut self.double_buffer {
            db.buffer_mut()
        } else {
            unsafe { core::slice::from_raw_parts_mut(self.buffer_ptr, self.buffer_len) }
        }
    }

    fn buffer(&self) -> &[u8] {
        if let Some(db) = &self.double_buffer {
            db.buffer()
        } else {
            unsafe { core::slice::from_raw_parts(self.buffer_ptr, self.buffer_len) }
        }
    }

    fn mark_dirty_region(&mut self, x: usize, y: usize, width: usize, height: usize) {
        if let Some(db) = &mut self.double_buffer {
            let bpp = self.info.bytes_per_pixel;
            let x_start = x * bpp;
            let x_end = (x + width) * bpp;
            // Mark each row in the region as dirty
            for row in y..y + height {
                db.mark_region_dirty(row, x_start, x_end);
            }
        }
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

                    // Route log output to Logs terminal (F2) in interactive mode
                    // This uses the terminal_manager's proper routing to the Logs pane
                    #[cfg(feature = "interactive")]
                    if crate::graphics::terminal_manager::is_terminal_manager_active() {
                        let msg = if timestamp > 0 {
                            alloc::format!(
                                "{} - [{:>5}] {}: {}\n",
                                timestamp,
                                record.level(),
                                record.target(),
                                record.args()
                            )
                        } else {
                            alloc::format!(
                                "[{:>5}] {}: {}\n",
                                record.level(),
                                record.target(),
                                record.args()
                            )
                        };
                        // Route to Logs terminal (F2) - not Shell (F1)
                        write_to_logs_terminal(&msg);
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
///
/// Note: In interactive mode, this does basic initialization only.
/// Call `upgrade_to_double_buffer()` after heap is initialized to enable
/// double buffering for tear-free rendering.
pub fn init_framebuffer(buffer: &'static mut [u8], info: bootloader_api::info::FrameBufferInfo) {
    // In interactive mode, store framebuffer info for later double buffer upgrade
    // We can't allocate the shadow buffer yet because the heap isn't initialized
    #[cfg(feature = "interactive")]
    {
        // Store the framebuffer parameters for later use
        let buffer_ptr = buffer.as_mut_ptr();
        let buffer_len = buffer.len();

        // Initialize with direct hardware writes (no double buffering yet)
        let _ = SHELL_FRAMEBUFFER.get_or_init(|| {
            Mutex::new(ShellFrameBuffer::new_direct(buffer_ptr, buffer_len, info))
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
///
/// When the render thread is active, bytes are queued for deferred rendering
/// to avoid deep stack usage in interrupt context.
#[cfg(feature = "interactive")]
#[allow(dead_code)] // Public API - available for shell output bypass
pub fn write_to_framebuffer(s: &str) {
    // If render queue is ready, use deferred rendering
    if crate::graphics::render_queue::is_ready() {
        crate::graphics::render_queue::queue_bytes(s.as_bytes());
        return;
    }

    // Fall back to direct rendering during early boot
    if let Some(fb) = SHELL_FRAMEBUFFER.get() {
        if let Some(mut guard) = fb.try_lock() {
            guard.write_str(s);
        }
    }
}

/// Write a single character to framebuffer console (for keyboard echo in interactive mode)
///
/// In terminal manager mode, this routes to the shell terminal.
/// Otherwise falls back to split-screen mode or full framebuffer.
#[cfg(feature = "interactive")]
#[allow(dead_code)]
pub fn write_char_to_framebuffer(byte: u8) {
    // Try terminal manager first (multi-terminal mode)
    if crate::graphics::terminal_manager::write_char_to_shell(byte as char) {
        return;
    }

    // Try split-screen mode as fallback
    if crate::graphics::split_screen::write_char_to_terminal(byte as char) {
        return;
    }

    // Fallback to full framebuffer
    if let Some(fb) = SHELL_FRAMEBUFFER.get() {
        if let Some(mut guard) = fb.try_lock() {
            guard.write_char(byte as char);
        }
    }
}

/// Write a byte slice to framebuffer console (batched version for efficient output)
///
/// This is more efficient than calling write_char_to_framebuffer per character
/// as it acquires locks once for the entire buffer and uses a shallower call stack.
///
/// In terminal manager mode, routes to the shell terminal using batched string write.
/// Otherwise falls back to split-screen mode or full framebuffer.
#[cfg(feature = "interactive")]
#[allow(dead_code)]
pub fn write_bytes_to_framebuffer(bytes: &[u8]) {
    // Try terminal manager first (multi-terminal mode) - uses batched write
    if crate::graphics::terminal_manager::write_bytes_to_shell(bytes) {
        return;
    }

    // Try split-screen mode as fallback
    if crate::graphics::split_screen::is_split_screen_active() {
        for &byte in bytes {
            if !crate::graphics::split_screen::write_char_to_terminal(byte as char) {
                break;
            }
        }
        return;
    }

    // Fallback to full framebuffer
    if let Some(fb) = SHELL_FRAMEBUFFER.get() {
        if let Some(mut guard) = fb.try_lock() {
            for &byte in bytes {
                guard.write_char(byte as char);
            }
        }
    }
}

/// Toggle cursor visibility for blinking effect (called from timer interrupt)
///
/// This is called from the timer interrupt at ~500ms intervals to create
/// a blinking cursor effect. Uses try_lock to avoid blocking in interrupt context.
///
/// In terminal manager mode, this toggles the active terminal's cursor.
#[cfg(feature = "interactive")]
pub fn toggle_cursor_blink() {
    // Try terminal manager first (multi-terminal mode)
    if crate::graphics::terminal_manager::is_terminal_manager_active() {
        crate::graphics::terminal_manager::toggle_cursor();
        return;
    }

    // Try split-screen mode as fallback
    if crate::graphics::split_screen::is_split_screen_active() {
        crate::graphics::split_screen::toggle_terminal_cursor();
        return;
    }

    // Fallback to full framebuffer cursor
    if let Some(fb) = SHELL_FRAMEBUFFER.get() {
        if let Some(mut guard) = fb.try_lock() {
            guard.toggle_cursor();
        }
    }
}

/// Write a log message to the logs terminal (interactive mode only)
///
/// This is called by the logger to route kernel log output to the
/// Logs terminal pane in multi-terminal mode.
#[cfg(feature = "interactive")]
#[allow(dead_code)]
pub fn write_to_logs_terminal(s: &str) {
    let _ = crate::graphics::terminal_manager::write_str_to_logs(s);
}

/// Upgrade the shell framebuffer to double-buffered mode.
///
/// This must be called AFTER the heap allocator is initialized, as it allocates
/// a shadow buffer on the heap. Double buffering provides tear-free rendering
/// by writing to a shadow buffer and then copying to hardware in one operation.
///
/// Safe to call multiple times - will only upgrade once.
#[cfg(feature = "interactive")]
pub fn upgrade_to_double_buffer() {
    if let Some(fb) = SHELL_FRAMEBUFFER.get() {
        let mut guard = fb.lock();
        guard.upgrade_to_double_buffer();
    }
}

/// Draw a simple test pattern with graphics primitives.
/// This is a demo function for testing the graphics primitives API.
#[cfg(feature = "interactive")]
#[allow(dead_code)]
pub fn draw_test_pattern() {
    use crate::graphics::primitives::{fill_rect, Rect, Color};

    if let Some(fb) = SHELL_FRAMEBUFFER.get() {
        let mut guard = fb.lock();
        fill_rect(
            &mut *guard,
            Rect {
                x: 10,
                y: 10,
                width: 50,
                height: 30,
            },
            Color::RED,
        );

        if let Some(db) = &mut guard.double_buffer {
            db.flush();
        }
    }
}
