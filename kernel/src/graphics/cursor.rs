//! Mouse cursor rendering with background save/restore.
//!
//! Provides a simple arrow cursor sprite that can be drawn on top of any
//! Canvas. The background pixels under the cursor are saved before drawing
//! and restored when the cursor moves, avoiding corruption of the underlying
//! framebuffer content. Both the restored and the newly drawn rectangle are
//! marked dirty, so the display shows the restore as well as the new sprite.
//!
//! The kernel's drawers that paint while the cursor is up (the boot screen,
//! the boot-test runner panel and the log console) call `hide` before drawing
//! and `show` after, so the saved pixels are the ones under the sprite.
//!
//! The cursor state is guarded by the framebuffer lock: call these functions
//! with the canvas's lock held, from thread context (not interrupt context).

use super::primitives::{Canvas, Color};

/// Cursor width in pixels
pub const CURSOR_W: usize = 12;
/// Cursor height in pixels
pub const CURSOR_H: usize = 18;

/// Arrow cursor bitmap (1 = white, 2 = black outline, 0 = transparent).
/// Standard arrow pointer shape.
const CURSOR_BITMAP: [[u8; CURSOR_W]; CURSOR_H] = [
    [2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [2, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [2, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [2, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0],
    [2, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0],
    [2, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0],
    [2, 1, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0],
    [2, 1, 1, 1, 1, 1, 1, 2, 0, 0, 0, 0],
    [2, 1, 1, 1, 1, 1, 1, 1, 2, 0, 0, 0],
    [2, 1, 1, 1, 1, 1, 1, 1, 1, 2, 0, 0],
    [2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 0],
    [2, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 0],
    [2, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0],
    [2, 1, 1, 2, 1, 1, 2, 0, 0, 0, 0, 0],
    [2, 1, 2, 0, 2, 1, 1, 2, 0, 0, 0, 0],
    [2, 2, 0, 0, 2, 1, 1, 2, 0, 0, 0, 0],
    [2, 0, 0, 0, 0, 2, 1, 2, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 2, 2, 0, 0, 0, 0, 0],
];

const CURSOR_WHITE: Color = Color::WHITE;
const CURSOR_BLACK: Color = Color::rgb(0, 0, 0);

/// Saved background pixels under the cursor (CURSOR_W * CURSOR_H max).
/// Stored as packed u32 BGRA values to avoid Color overhead.
static mut SAVED_BG: [u32; CURSOR_W * CURSOR_H] = [0; CURSOR_W * CURSOR_H];

/// Whether we currently have a cursor drawn (and thus saved background)
static mut CURSOR_DRAWN: bool = false;

/// Last drawn cursor position
static mut LAST_X: usize = 0;
static mut LAST_Y: usize = 0;

/// The cursor was taken down by `hide` and is put back by `show`.
static mut CURSOR_HIDDEN: bool = false;

/// Erase the cursor by restoring saved background pixels.
///
/// Call with the framebuffer lock held.
pub fn erase_cursor(canvas: &mut impl Canvas) {
    unsafe {
        if !CURSOR_DRAWN {
            return;
        }
        let cx = LAST_X;
        let cy = LAST_Y;
        let w = canvas.width();
        let h = canvas.height();

        for row in 0..CURSOR_H {
            let py = cy + row;
            if py >= h {
                break;
            }
            for col in 0..CURSOR_W {
                let px = cx + col;
                if px >= w {
                    break;
                }
                if CURSOR_BITMAP[row][col] != 0 {
                    let saved = SAVED_BG[row * CURSOR_W + col];
                    let r = ((saved >> 16) & 0xFF) as u8;
                    let g = ((saved >> 8) & 0xFF) as u8;
                    let b = (saved & 0xFF) as u8;
                    canvas.set_pixel(px as i32, py as i32, Color::rgb(r, g, b));
                }
            }
        }

        CURSOR_DRAWN = false;
        canvas.mark_dirty_region(cx, cy, CURSOR_W, CURSOR_H);
    }
}

/// Draw the cursor at the given position, saving the background first.
///
/// Call with the framebuffer lock held.
pub fn draw_cursor(canvas: &mut impl Canvas, x: usize, y: usize) {
    let w = canvas.width();
    let h = canvas.height();

    unsafe {
        // Save background pixels
        for row in 0..CURSOR_H {
            let py = y + row;
            if py >= h {
                break;
            }
            for col in 0..CURSOR_W {
                let px = x + col;
                if px >= w {
                    break;
                }
                if CURSOR_BITMAP[row][col] != 0 {
                    let color = canvas
                        .get_pixel(px as i32, py as i32)
                        .unwrap_or(Color::rgb(0, 0, 0));
                    // Pack as 0x00RRGGBB
                    let packed =
                        ((color.r as u32) << 16) | ((color.g as u32) << 8) | (color.b as u32);
                    SAVED_BG[row * CURSOR_W + col] = packed;
                }
            }
        }

        // Draw cursor pixels
        for row in 0..CURSOR_H {
            let py = y + row;
            if py >= h {
                break;
            }
            for col in 0..CURSOR_W {
                let px = x + col;
                if px >= w {
                    break;
                }
                match CURSOR_BITMAP[row][col] {
                    1 => canvas.set_pixel(px as i32, py as i32, CURSOR_WHITE),
                    2 => canvas.set_pixel(px as i32, py as i32, CURSOR_BLACK),
                    _ => {} // transparent
                }
            }
        }

        LAST_X = x;
        LAST_Y = y;
        CURSOR_DRAWN = true;
        CURSOR_HIDDEN = false;
    }
    canvas.mark_dirty_region(x, y, CURSOR_W, CURSOR_H);
}

/// Take the cursor down before drawing into the framebuffer, restoring the
/// pixels under it. `show` puts it back once the drawing is done.
///
/// Call with the framebuffer lock held.
pub fn hide(canvas: &mut impl Canvas) {
    unsafe {
        if CURSOR_DRAWN {
            erase_cursor(canvas);
            CURSOR_HIDDEN = true;
        }
    }
}

/// Put the cursor back where `hide` took it down, saving what is now under it.
///
/// Call with the framebuffer lock held.
pub fn show(canvas: &mut impl Canvas) {
    unsafe {
        if CURSOR_HIDDEN {
            draw_cursor(canvas, LAST_X, LAST_Y);
        }
    }
}

/// Update the cursor position. Erases the old cursor and draws at the new position.
///
/// Call with the framebuffer lock held.
pub fn update_cursor(canvas: &mut impl Canvas, new_x: usize, new_y: usize) {
    unsafe {
        if CURSOR_DRAWN && LAST_X == new_x && LAST_Y == new_y {
            return; // No movement
        }
    }

    erase_cursor(canvas);
    draw_cursor(canvas, new_x, new_y);
}
