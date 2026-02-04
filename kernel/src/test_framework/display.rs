//! Graphical progress display for boot tests
//!
//! Renders progress bars showing test execution status in real-time.
//! Works on both x86_64 (with interactive feature) and ARM64.
//!
//! On platforms without graphical framebuffer support, this module
//! gracefully degrades to a no-op.

use core::sync::atomic::{AtomicBool, Ordering};

use super::registry::{SubsystemId, TestStage};
use super::progress::{get_progress, get_stage_progress, is_started, is_complete, get_overall_progress};

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
fn has_framebuffer_available() -> bool {
    #[cfg(all(target_arch = "x86_64", feature = "interactive"))]
    {
        crate::logger::SHELL_FRAMEBUFFER.get().is_some()
    }

    #[cfg(all(target_arch = "x86_64", not(feature = "interactive")))]
    {
        // Without interactive feature, no graphical framebuffer is available
        false
    }

    #[cfg(target_arch = "aarch64")]
    {
        crate::graphics::arm64_fb::SHELL_FRAMEBUFFER.get().is_some()
    }
}

// =============================================================================
// Graphical rendering implementation (only when graphics available)
// =============================================================================

/// Render to the appropriate framebuffer based on platform
#[cfg(any(feature = "interactive", target_arch = "aarch64"))]
fn render_to_framebuffer() {
    use crate::graphics::primitives::{
        fill_rect, draw_text, draw_rect, Canvas, Color, Rect, TextStyle,
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
    const COLOR_STAGE_EARLY: Color = Color::rgb(0, 200, 100);    // Green - EarlyBoot
    const COLOR_STAGE_SCHED: Color = Color::rgb(0, 150, 255);    // Blue - PostScheduler
    const COLOR_STAGE_PROC: Color = Color::rgb(255, 200, 0);     // Yellow - ProcessContext
    const COLOR_STAGE_USER: Color = Color::rgb(180, 100, 255);   // Purple - Userspace

    // Layout constants
    const PANEL_MARGIN_X: i32 = 40;
    const PANEL_MARGIN_Y: i32 = 40;
    const PANEL_WIDTH: u32 = 600;
    const PANEL_HEIGHT: u32 = 400;
    const PANEL_PADDING: i32 = 20;
    const TITLE_HEIGHT: i32 = 40;
    const ROW_HEIGHT: i32 = 28;
    const NAME_WIDTH: i32 = 100;
    const BAR_WIDTH: u32 = 300;
    const BAR_HEIGHT: u32 = 16;
    const PERCENT_WIDTH: i32 = 50;

    /// Render the progress panel to a canvas
    fn render_panel<C: Canvas>(canvas: &mut C) {
        let panel_x = PANEL_MARGIN_X;
        let panel_y = PANEL_MARGIN_Y;

        // Draw panel background
        fill_rect(
            canvas,
            Rect {
                x: panel_x,
                y: panel_y,
                width: PANEL_WIDTH,
                height: PANEL_HEIGHT,
            },
            COLOR_BACKGROUND,
        );

        // Draw panel border
        draw_rect(
            canvas,
            Rect {
                x: panel_x,
                y: panel_y,
                width: PANEL_WIDTH,
                height: PANEL_HEIGHT,
            },
            COLOR_BORDER,
        );

        // Draw title
        let title_style = TextStyle::new().with_color(COLOR_TITLE);
        let title = "BREENIX PARALLEL TEST RUNNER";
        let title_x = panel_x + (PANEL_WIDTH as i32 - title.len() as i32 * 9) / 2;
        let title_y = panel_y + PANEL_PADDING;
        draw_text(canvas, title_x, title_y, title, &title_style);

        // Draw horizontal line under title
        let line_y = panel_y + PANEL_PADDING + TITLE_HEIGHT - 8;
        fill_rect(
            canvas,
            Rect {
                x: panel_x + PANEL_PADDING,
                y: line_y,
                width: PANEL_WIDTH - (PANEL_PADDING as u32 * 2),
                height: 1,
            },
            COLOR_BORDER,
        );

        // Draw each subsystem row
        let content_start_y = panel_y + PANEL_PADDING + TITLE_HEIGHT;
        let content_x = panel_x + PANEL_PADDING;

        for idx in 0..SubsystemId::COUNT {
            if let Some(id) = SubsystemId::from_index(idx) {
                let row_y = content_start_y + (idx as i32 * ROW_HEIGHT);
                render_subsystem_row(canvas, content_x, row_y, id);
            }
        }

        // Draw summary line at bottom
        let summary_y = content_start_y + (SubsystemId::COUNT as i32 * ROW_HEIGHT) + 10;
        render_summary_line(canvas, content_x, summary_y);
    }

    /// Render a single subsystem row
    fn render_subsystem_row<C: Canvas>(canvas: &mut C, x: i32, y: i32, id: SubsystemId) {
        let (completed, total, failed) = get_progress(id);
        let started = is_started(id);
        let complete = is_complete(id);

        // Determine status
        let (status_text, status_color) = if total == 0 {
            ("N/A ", COLOR_PEND)
        } else if !started {
            ("PEND", COLOR_PEND)
        } else if complete {
            if failed > 0 {
                ("FAIL", COLOR_FAIL)
            } else {
                ("PASS", COLOR_PASS)
            }
        } else {
            ("RUN ", COLOR_RUN)
        };

        // Draw subsystem name
        let name = id.name();
        let name_style = TextStyle::new().with_color(COLOR_TEXT);
        draw_text(canvas, x, y, name, &name_style);

        // Draw progress bar with stage-colored segments
        let bar_x = x + NAME_WIDTH;
        let bar_y = y + (ROW_HEIGHT - BAR_HEIGHT as i32) / 2 - 2;
        let stage_progress = get_stage_progress(id);
        render_progress_bar(canvas, bar_x, bar_y, total, &stage_progress);

        // Draw percentage
        let percent = if total > 0 {
            (completed * 100) / total
        } else {
            0
        };
        let percent_str = format_percent(percent);
        let percent_x = bar_x + BAR_WIDTH as i32 + 10;
        let percent_style = TextStyle::new().with_color(COLOR_TEXT);
        draw_text(canvas, percent_x, y, percent_str, &percent_style);

        // Draw status
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
        total: u32,
        stage_progress: &[(u32, u32); TestStage::COUNT],
    ) {
        // Stage colors in order
        const STAGE_COLORS: [Color; TestStage::COUNT] = [
            COLOR_STAGE_EARLY,  // EarlyBoot - Green
            COLOR_STAGE_SCHED,  // PostScheduler - Blue
            COLOR_STAGE_PROC,   // ProcessContext - Yellow
            COLOR_STAGE_USER,   // Userspace - Purple
        ];

        // Draw background (empty bar)
        fill_rect(
            canvas,
            Rect {
                x,
                y,
                width: BAR_WIDTH,
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
                        ((completed as u64 * BAR_WIDTH as u64) / total as u64) as u32;

                    if segment_width > 0 {
                        // Ensure we don't overflow the bar
                        let remaining = BAR_WIDTH.saturating_sub((current_x - x) as u32);
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
                width: BAR_WIDTH,
                height: BAR_HEIGHT,
            },
            COLOR_BORDER,
        );
    }

    /// Render the summary line at bottom of panel
    fn render_summary_line<C: Canvas>(canvas: &mut C, x: i32, y: i32) {
        let (completed, total, failed) = get_overall_progress();

        // Count complete subsystems
        let mut complete_count = 0u32;
        for idx in 0..SubsystemId::COUNT {
            if let Some(id) = SubsystemId::from_index(idx) {
                if is_complete(id) {
                    complete_count += 1;
                }
            }
        }

        // Format summary
        let summary = format_summary(completed, total, complete_count, failed);
        let style = TextStyle::new().with_color(COLOR_TEXT);
        draw_text(canvas, x, y, &summary, &style);
    }

    /// Format a percentage as a string
    fn format_percent(percent: u32) -> &'static str {
        match percent {
            0 => "  0%",
            5 => "  5%",
            10 => " 10%",
            15 => " 15%",
            20 => " 20%",
            25 => " 25%",
            30 => " 30%",
            35 => " 35%",
            40 => " 40%",
            45 => " 45%",
            50 => " 50%",
            55 => " 55%",
            60 => " 60%",
            65 => " 65%",
            70 => " 70%",
            75 => " 75%",
            80 => " 80%",
            85 => " 85%",
            90 => " 90%",
            95 => " 95%",
            100 => "100%",
            _ => {
                if percent < 10 {
                    "  ?%"
                } else if percent < 100 {
                    " ??%"
                } else {
                    "100%"
                }
            }
        }
    }

    /// Format the summary line
    fn format_summary(completed: u32, total: u32, subsystems: u32, failed: u32) -> alloc::string::String {
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
            render_panel(&mut *guard);
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        if let Some(fb) = crate::graphics::arm64_fb::SHELL_FRAMEBUFFER.get() {
            let mut guard = fb.lock();
            render_panel(&mut *guard);
            // Flush to display on ARM64
            guard.flush();
        }
    }
}

/// No-op rendering for platforms without graphics support
#[cfg(all(target_arch = "x86_64", not(feature = "interactive")))]
fn render_to_framebuffer() {
    // No graphical display available without interactive feature
}
