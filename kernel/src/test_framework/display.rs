//! Graphical progress display for boot tests
//!
//! Renders progress bars showing test execution status in real-time.
//! Works on both x86_64 and ARM64 whenever a framebuffer is available.
//!
//! On x86_64, the SHELL_FRAMEBUFFER requires the `interactive` feature since it
//! needs the double-buffered framebuffer setup. Without it, the raw framebuffer
//! is consumed by the basic text logger and no shell framebuffer exists.
//! On ARM64, VirtIO GPU provides the framebuffer regardless of feature flags.
//!
//! When no framebuffer is available, this module gracefully degrades to a no-op.

use core::sync::atomic::{AtomicBool, Ordering};

#[cfg(any(feature = "interactive", target_arch = "aarch64"))]
use super::progress::{get_progress, get_stage_progress, is_started};
#[cfg(any(feature = "interactive", target_arch = "aarch64"))]
use super::registry::{SubsystemId, TestStage};

/// Whether graphical display is available and initialized
static DISPLAY_READY: AtomicBool = AtomicBool::new(false);

// =============================================================================
// Public API (always available)
// =============================================================================

/// Initialize the display
///
/// Checks if a framebuffer is available and marks the display as ready.
/// Should be called after framebuffer initialization.
pub fn init() {
    let has_framebuffer = has_framebuffer_available();

    if has_framebuffer {
        DISPLAY_READY.store(true, Ordering::Release);
        log::debug!("[test_display] Graphical progress display initialized");
    } else {
        log::debug!("[test_display] No framebuffer available, graphical display disabled");
    }
}

/// Check if the display is ready for rendering
pub fn is_ready() -> bool {
    DISPLAY_READY.load(Ordering::Acquire)
}

/// Render current progress to the framebuffer
///
/// Draws the progress panel with bars for each subsystem.
/// Safe to call even if display is not ready (will be a no-op).
pub fn render_progress() {
    if !is_ready() {
        return;
    }

    render_to_framebuffer();
}

/// Request a display refresh
///
/// This is called by the executor after each test completes.
/// On ARM64 with VirtIO GPU, this triggers a flush to make changes visible.
#[inline]
pub fn request_refresh() {
    render_progress();
}

// =============================================================================
// Platform detection
// =============================================================================

/// Check if framebuffer is available (architecture-specific)
///
/// On x86_64, SHELL_FRAMEBUFFER only exists with the `interactive` feature.
/// On ARM64, SHELL_FRAMEBUFFER is always available when VirtIO GPU initializes.
fn has_framebuffer_available() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        #[cfg(feature = "interactive")]
        {
            crate::logger::SHELL_FRAMEBUFFER.get().is_some()
        }
        #[cfg(not(feature = "interactive"))]
        {
            false
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        crate::graphics::arm64_fb::SHELL_FRAMEBUFFER.get().is_some()
    }
}

// =============================================================================
// Graphical rendering implementation
// =============================================================================
//
// The full rendering code is compiled on platforms that can have a framebuffer:
// - x86_64 with interactive feature (SHELL_FRAMEBUFFER exists)
// - ARM64 (VirtIO GPU provides SHELL_FRAMEBUFFER)
//
// On x86_64 without interactive, a no-op stub is provided instead.

/// Render to the appropriate framebuffer based on platform
///
/// On ARM64 the panel shares the screen with the boot screen, which stays up
/// while a testing kernel runs (docs/console/contract.md, section 7): it draws
/// only in the band the boot screen leaves free (`boot_screen::runner_area`),
/// and not at all once the boot screen has given the screen away.
#[cfg(any(feature = "interactive", target_arch = "aarch64"))]
fn render_to_framebuffer() {
    use crate::graphics::primitives::{
        draw_rect, draw_text, fill_rect, Canvas, Color, Rect, TextStyle,
    };

    // Color scheme
    const COLOR_BACKGROUND: Color = Color::rgb(26, 26, 46);
    const COLOR_BORDER: Color = Color::rgb(74, 74, 106);
    const COLOR_TEXT: Color = Color::rgb(255, 255, 255);
    const COLOR_TITLE: Color = Color::rgb(100, 200, 255);
    const COLOR_PASS: Color = Color::rgb(0, 255, 0);
    const COLOR_FAIL: Color = Color::rgb(255, 0, 0);
    const COLOR_RUN: Color = Color::rgb(0, 191, 255);
    const COLOR_PEND: Color = Color::rgb(128, 128, 128);
    const COLOR_BAR_EMPTY: Color = Color::rgb(64, 64, 64);

    // Stage colors for progress bar segments
    const COLOR_STAGE_EARLY: Color = Color::rgb(0, 200, 100); // Green - EarlyBoot
    const COLOR_STAGE_SCHED: Color = Color::rgb(0, 150, 255); // Blue - PostScheduler
    const COLOR_STAGE_PROC: Color = Color::rgb(255, 200, 0); // Yellow - ProcessContext
    const COLOR_STAGE_USER: Color = Color::rgb(180, 100, 255); // Purple - Userspace

    // Layout constants
    const PANEL_PADDING: i32 = 6;
    const TITLE_HEIGHT: i32 = 24;
    const ROW_HEIGHT: i32 = 20;
    const SUMMARY_HEIGHT: i32 = 20;
    const COLUMN_GAP: i32 = 16;
    const NAME_WIDTH: i32 = 100;
    const BAR_HEIGHT: u32 = 12;
    const MIN_BAR_WIDTH: i32 = 80;
    const PERCENT_WIDTH: i32 = 48;
    const STATUS_WIDTH: i32 = 40;

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Status {
        NotApplicable,
        Pending,
        Running,
        Pass,
        Fail,
    }

    /// One subsystem's counters, read once per render so its row and the
    /// summary line describe the same moment.
    #[derive(Clone, Copy)]
    struct Row {
        completed: u32,
        total: u32,
        failed: u32,
        status: Status,
        stages: [(u32, u32); TestStage::COUNT],
    }

    fn snapshot(id: SubsystemId) -> Row {
        let (completed, total, failed) = get_progress(id);
        let status = if total == 0 {
            Status::NotApplicable
        } else if completed >= total {
            if failed > 0 {
                Status::Fail
            } else {
                Status::Pass
            }
        } else if !is_started(id) {
            Status::Pending
        } else {
            Status::Running
        };
        Row {
            completed,
            total,
            failed,
            status,
            stages: get_stage_progress(id),
        }
    }

    /// Render the progress panel into `area`, or skip the render when `area` is
    /// too small for its columns.
    fn render_panel<C: Canvas>(canvas: &mut C, area: Rect) {
        let mut rows = [None; SubsystemId::COUNT];
        for (idx, row) in rows.iter_mut().enumerate() {
            *row = SubsystemId::from_index(idx).map(|id| (id, snapshot(id)));
        }

        let inner_x = area.x + PANEL_PADDING;
        let inner_width = area.width as i32 - 2 * PANEL_PADDING;
        let rows_fit = (area.height as i32 - 2 * PANEL_PADDING - TITLE_HEIGHT - SUMMARY_HEIGHT)
            / ROW_HEIGHT;
        if rows_fit <= 0 {
            return;
        }
        let rows_fit = rows_fit as usize;
        let columns = SubsystemId::COUNT.div_ceil(rows_fit);
        let rows_per_column = SubsystemId::COUNT.div_ceil(columns);
        let column_width =
            (inner_width - (columns as i32 - 1) * COLUMN_GAP) / columns as i32;
        let bar_width = column_width - NAME_WIDTH - PERCENT_WIDTH - STATUS_WIDTH;
        if bar_width < MIN_BAR_WIDTH {
            return;
        }

        fill_rect(canvas, area, COLOR_BACKGROUND);
        draw_rect(canvas, area, COLOR_BORDER);

        let title_style = TextStyle::new().with_color(COLOR_TITLE);
        let advance = title_style.font.metrics().char_advance() as i32;
        let title = "BREENIX PARALLEL TEST RUNNER";
        let title_x = area.x + (area.width as i32 - title.len() as i32 * advance) / 2;
        let title_y = area.y + PANEL_PADDING;
        draw_text(canvas, title_x, title_y, title, &title_style);
        fill_rect(
            canvas,
            Rect {
                x: inner_x,
                y: title_y + TITLE_HEIGHT - 4,
                width: inner_width as u32,
                height: 1,
            },
            COLOR_BORDER,
        );

        let content_y = title_y + TITLE_HEIGHT;
        for (idx, entry) in rows.iter().enumerate() {
            if let Some((id, row)) = entry {
                let column = (idx / rows_per_column) as i32;
                let x = inner_x + column * (column_width + COLUMN_GAP);
                let y = content_y + (idx % rows_per_column) as i32 * ROW_HEIGHT;
                render_subsystem_row(canvas, x, y, bar_width, *id, row);
            }
        }

        let summary_y = content_y + rows_per_column as i32 * ROW_HEIGHT + 2;
        render_summary_line(canvas, inner_x, summary_y, &rows);
    }

    /// Render a single subsystem row
    fn render_subsystem_row<C: Canvas>(
        canvas: &mut C,
        x: i32,
        y: i32,
        bar_width: i32,
        id: SubsystemId,
        row: &Row,
    ) {
        let (status_text, status_color) = match row.status {
            Status::NotApplicable => ("N/A", COLOR_PEND),
            Status::Pending => ("PEND", COLOR_PEND),
            Status::Running => ("RUN", COLOR_RUN),
            Status::Pass => ("PASS", COLOR_PASS),
            Status::Fail => ("FAIL", COLOR_FAIL),
        };

        let name_style = TextStyle::new().with_color(COLOR_TEXT);
        draw_text(canvas, x, y, id.name(), &name_style);

        let bar_x = x + NAME_WIDTH;
        let bar_y = y + 2;
        render_progress_bar(canvas, bar_x, bar_y, bar_width as u32, row.total, &row.stages);

        let percent = if row.total > 0 {
            (row.completed.min(row.total) as u64 * 100 / row.total as u64) as u32
        } else {
            0
        };
        let mut percent_buf = [0u8; 4];
        let percent_x = bar_x + bar_width + 8;
        let percent_style = TextStyle::new().with_color(COLOR_TEXT);
        draw_text(
            canvas,
            percent_x,
            y,
            format_percent(percent, &mut percent_buf),
            &percent_style,
        );

        let status_x = percent_x + PERCENT_WIDTH;
        let status_style = TextStyle::new().with_color(status_color);
        draw_text(canvas, status_x, y, status_text, &status_style);
    }

    /// Render a progress bar with stage-colored segments
    ///
    /// Each stage gets its own color segment showing completed tests for that stage.
    fn render_progress_bar<C: Canvas>(
        canvas: &mut C,
        x: i32,
        y: i32,
        bar_width: u32,
        total: u32,
        stage_progress: &[(u32, u32); TestStage::COUNT],
    ) {
        // Stage colors in order
        const STAGE_COLORS: [Color; TestStage::COUNT] = [
            COLOR_STAGE_EARLY, // SerialBoot - Green
            COLOR_STAGE_EARLY, // EarlyBoot - Green
            COLOR_STAGE_SCHED, // PostScheduler - Blue
            COLOR_STAGE_PROC,  // ProcessContext - Yellow
            COLOR_STAGE_USER,  // Userspace - Purple
        ];

        // Draw background (empty bar)
        fill_rect(
            canvas,
            Rect {
                x,
                y,
                width: bar_width,
                height: BAR_HEIGHT,
            },
            COLOR_BAR_EMPTY,
        );

        // Draw colored segments for each stage
        if total > 0 {
            let mut current_x = x;

            for (stage_idx, &(completed, _stage_total)) in stage_progress.iter().enumerate() {
                if completed > 0 {
                    // Calculate width for this stage's completed tests
                    let segment_width =
                        ((completed as u64 * bar_width as u64) / total as u64) as u32;

                    if segment_width > 0 {
                        // Ensure we don't overflow the bar
                        let remaining = bar_width.saturating_sub((current_x - x) as u32);
                        let actual_width = segment_width.min(remaining);

                        if actual_width > 0 {
                            fill_rect(
                                canvas,
                                Rect {
                                    x: current_x,
                                    y,
                                    width: actual_width,
                                    height: BAR_HEIGHT,
                                },
                                STAGE_COLORS[stage_idx],
                            );
                            current_x += actual_width as i32;
                        }
                    }
                }
            }
        }

        // Draw bar border
        draw_rect(
            canvas,
            Rect {
                x,
                y,
                width: bar_width,
                height: BAR_HEIGHT,
            },
            COLOR_BORDER,
        );
    }

    /// Render the summary line from the same snapshot the rows were drawn from:
    /// the complete count is the number of rows showing PASS or FAIL.
    fn render_summary_line<C: Canvas>(
        canvas: &mut C,
        x: i32,
        y: i32,
        rows: &[Option<(SubsystemId, Row)>; SubsystemId::COUNT],
    ) {
        let (mut completed, mut total, mut failed, mut complete_count) = (0u32, 0u32, 0u32, 0u32);
        for (_, row) in rows.iter().flatten() {
            completed += row.completed;
            total += row.total;
            failed += row.failed;
            if matches!(row.status, Status::Pass | Status::Fail) {
                complete_count += 1;
            }
        }

        let summary = format_summary(completed, total, complete_count, failed);
        let style = TextStyle::new().with_color(COLOR_TEXT);
        draw_text(canvas, x, y, &summary, &style);
    }

    /// Format a percentage (0-100) right-aligned in four characters, e.g. " 37%".
    fn format_percent(percent: u32, buf: &mut [u8; 4]) -> &str {
        let percent = percent.min(100);
        *buf = *b"   %";
        let mut value = percent;
        let mut pos = 2;
        loop {
            buf[pos] = b'0' + (value % 10) as u8;
            value /= 10;
            if value == 0 || pos == 0 {
                break;
            }
            pos -= 1;
        }
        core::str::from_utf8(buf).unwrap_or("")
    }

    /// Format the summary line
    fn format_summary(
        completed: u32,
        total: u32,
        subsystems: u32,
        failed: u32,
    ) -> alloc::string::String {
        use alloc::format;

        if failed == 0 {
            format!(
                "Total: {}/{} tests | {} subsystems complete",
                completed, total, subsystems
            )
        } else {
            format!(
                "Total: {}/{} tests | {} subsystems | {} failures",
                completed, total, subsystems, failed
            )
        }
    }

    // Actual rendering dispatch based on architecture
    #[cfg(all(target_arch = "x86_64", feature = "interactive"))]
    {
        if let Some(fb) = crate::logger::SHELL_FRAMEBUFFER.get() {
            let mut guard = fb.lock();
            let area = Rect {
                x: 40,
                y: 40,
                width: 600,
                height: 400,
            };
            render_panel(&mut *guard, area);
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        if let Some(fb) = crate::graphics::arm64_fb::SHELL_FRAMEBUFFER.get() {
            let mut guard = fb.lock();
            // The console retires the firmware canvas under this lock.
            if crate::gpu::console::active() {
                return;
            }
            let Some(area) = crate::graphics::boot_screen::runner_area(&*guard) else {
                return;
            };
            crate::graphics::cursor::hide(&mut *guard);
            render_panel(&mut *guard, area);
            crate::graphics::cursor::show(&mut *guard);
            guard.flush_rect(area.x as u32, area.y as u32, area.width, area.height);
        }
    }
}

/// No-op rendering stub for x86_64 without interactive feature.
///
/// On x86_64, the SHELL_FRAMEBUFFER requires the `interactive` feature because
/// without it, the raw framebuffer is consumed by the basic text logger.
/// This stub ensures the code compiles cleanly with zero warnings.
#[cfg(all(target_arch = "x86_64", not(feature = "interactive")))]
fn render_to_framebuffer() {
    // No graphical display available - framebuffer is used by basic text logger
}
