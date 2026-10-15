//! The library cases of composition and panel (fbinfo, fb_mmap and the suite panel,
//! contract sections 18 and 23), and the run showcase.

use crate::*;

pub(crate) const COMPOSITION: &[Case] = &[
    case("fbinfo-geometry", "fbinfo's stride, bytes per pixel and pixel format agree with the mode", composition_fbinfo_geometry),
    case("fbmmap-pane", "fb_mmap gives the display owner the whole screen", composition_fbmmap_pane),
];

pub(crate) const PANEL: &[Case] = &[
    case("counts", "Fixed pass, fail and count fixtures draw the expected digits in the expected colours", panel_counts),
    case("fail-rows", "Failed cases are listed with their names in the failure rows", panel_fail_rows),
    case("layout", "The category bars and score occupy the contract's rectangles at this mode", panel_layout),
    case("stage-layout", "Stage fixture header, wall, spotlight and ribbon occupy the contract rectangles", panel_stage_layout),
    case("tile-stamps", "Tile order, milestone grouping, stamp states and key numbers match the fixture", panel_tile_stamps),
    case("tile-thumbnail", "Every thumbnail is the integer box filter of the judged frame", panel_tile_thumbnail),
    case("spotlight-picture", "Expected, actual and difference panes show the first mismatch and comparison bound", panel_spotlight_picture),
    case("spotlight-timing", "Frame-time bars and the budget line match the fixture numbers", panel_spotlight_timing),
    case("milestone-ribbon", "Milestone segments light exactly when their cases have passed", panel_milestone_ribbon),
    case("finale", "The summary shows counts and the slowest case and holds for five seconds of UI time", panel_finale),
    case("deterministic", "The same results at the same UI time draw byte-identical stage pixels without real clock reads", panel_deterministic),
    case("judged-frame-shown", "Every picture tile shows the frame whose SHA-256 its CASE record printed, after detecting substitution", panel_judged_frame_shown),
    case("serial-agreement", "Stage scanout tile ids, stamps and numbers agree with CASE bytes after detecting stage-lie", panel_serial_agreement),
    case("quiet-window", "No runner composite occurs inside a measured case window after detecting stage-draw-in-window", panel_quiet_window),
    case("boot-ribbon", "The restored kernel ribbon matches producer boot-stage markers after detecting ribbon-drop-stage", panel_boot_ribbon),
    case("boundary-notice", "The stage observation notice agrees with the backend flags and boundary code", panel_boundary_notice),
];

#[repr(C)]
#[derive(Default)]
pub(crate) struct FbInfo {
    pub(crate) width: u64,
    pub(crate) height: u64,
    pub(crate) stride: u64,
    pub(crate) bytes_per_pixel: u64,
    pub(crate) pixel_format: u64,
}

pub(crate) fn fbinfo() -> Result<FbInfo, String> {
    let mut info = FbInfo::default();
    // SAFETY: the kernel writes one FbInfo into this live value.
    let r = unsafe { raw::syscall1(SYS_FB_INFO, &mut info as *mut FbInfo as u64) as i64 };
    if r < 0 { Err(format!("fbinfo {}", outcome(r))) } else { Ok(info) }
}

fn composition_fbinfo_geometry() -> CaseResult {
    let i = fbinfo().cite("18")?;
    value("fb-width", i.width as i64, "px", Some((1280, 1280)));
    value("fb-height", i.height as i64, "px", Some((800, 800)));
    ensure((i.width, i.height) == (1280, 800), "18", format!("fbinfo reports {}x{}; the contract requires 1280x800 in a GUI suite boot", i.width, i.height))?;
    ensure(i.bytes_per_pixel == 4 && i.stride >= 1280 && i.pixel_format <= 1, "18", format!(
        "fbinfo reports a stride of {} pixels, {} bytes per pixel and pixel format {}; the contract gives 4 bytes per pixel, a stride of at least 1280 pixels and format 0 or 1",
        i.stride, i.bytes_per_pixel, i.pixel_format))
}

fn composition_fbmmap_pane() -> CaseResult {
    let info = fbinfo().cite("18")?;
    // SAFETY: neither call reads user memory.
    let took = unsafe { raw::syscall0(SYS_TAKE_OVER_DISPLAY) as i64 };
    ensure(took >= 0, "18", format!("take_over_display {}", outcome(took)))?;
    let mut width: u64 = 0;
    // SAFETY: the kernel writes the mapped width into this live value.
    let addr = unsafe { raw::syscall1(SYS_FB_MMAP, &mut width as *mut u64 as u64) as i64 };
    let _ = unsafe { raw::syscall0(SYS_GIVE_BACK_DISPLAY) };
    ensure(addr > 0, "18", format!("fb_mmap for the display owner {}", outcome(addr)))?;
    value("mapped-width", width as i64, "px", Some((info.width as i64, info.width as i64)));
    ensure(width == info.width, "18", format!(
        "fb_mmap gave the display owner {width} of the screen's {} columns; the owner gets the whole screen", info.width))
}

// ---------------------------------------------------------------------------------------
// panel: suite & probe panels (contract section 23)
// ---------------------------------------------------------------------------------------

const PANEL_BG: Color = Color::rgb(12, 19, 31);
const PANEL_PASS: Color = Color::rgb(26, 112, 77);
const PANEL_FAIL: Color = Color::rgb(145, 50, 58);
const PANEL_PENDING: Color = Color::rgb(57, 67, 80);

/// The contract's fixture panel drawn on a 1280 × 800 surface.
fn fixture_panel() -> Guarded {
    use diagnostics::{Check, CheckState, Group, Panel, Verdict};
    let alpha = [Check { name: "a1", state: CheckState::Ok("") }, Check { name: "a2", state: CheckState::Fail("bad pixel") },
        Check { name: "a3", state: CheckState::Ok("") }];
    let beta = [Check { name: "b1", state: CheckState::Fail("no frame") }, Check { name: "b2", state: CheckState::Pending },
        Check { name: "b3", state: CheckState::Ok("") }];
    let groups = [Group { title: "alpha", checks: &alpha }, Group { title: "beta", checks: &beta }];
    let panel = Panel { title: "BREENGEL PANEL", subtitle: "fixture", groups: &groups, output: &[], verdict: Verdict::Running("run"),
        scored: true, live: None };
    let g = Guarded::bgrx(1280, 800);
    diagnostics::draw(&mut g.fb(), &panel);
    g
}

/// The region (x, y, w, h) of `got` equals a `bg` region with `draw` applied, the expected
/// text drawn with the 5×7 font that text/bitmap-glyph and bitmap-scale certify.
fn region_is(got: &Guarded, (x0, y0, w, h): (usize, usize, usize, usize), bg: Color, what: &str, draw: impl FnOnce(&mut FrameBuf)) -> CaseResult {
    let mut want = Guarded::bgrx(got.width, got.height);
    want.reset(bg);
    draw(&mut want.fb());
    for y in y0..y0 + h {
        for x in x0..x0 + w {
            if got.color(x, y) != want.color(x, y) {
                return at("23", format!("{what}: pixel ({x},{y}) is {}, the contract's panel gives {}", hex(got.color(x, y)), hex(want.color(x, y))));
            }
        }
    }
    Ok(())
}

fn panel_counts() -> CaseResult {
    let g = fixture_panel();
    region_is(&g, (32, 124, 1216, 28), PANEL_BG, "the score row", |fb| {
        font5x7::draw_text(fb, b"3 / 6 PASSED  50%", 32, 124, WHITE, 4);
        let counts = b"2 failed  0 skipped  1 to run";
        font5x7::draw_text(fb, counts, 1248 - counts.len() * 12, 138, Color::rgb(161, 183, 204), 2);
    })?;
    region_is(&g, (32, 162, 1216, 18), PANEL_BG, "the score bar", |fb| {
        shapes::fill_rect(fb, 32, 162, 608, 18, PANEL_PASS);
        shapes::fill_rect(fb, 640, 162, 405, 18, PANEL_FAIL);
        shapes::fill_rect(fb, 1045, 162, 203, 18, PANEL_PENDING);
    })
}

fn panel_fail_rows() -> CaseResult {
    let g = fixture_panel();
    for (x, k, name, reason) in [(32usize, 1usize, "a2", "bad pixel"), (652, 0, "b1", "no frame")] {
        let y = 248 + 52 * k;
        region_is(&g, (x, y, 596, 46), PANEL_FAIL, &format!("the failed card {name}"), |fb| {
            font5x7::draw_text(fb, name.as_bytes(), x + 12, y + 4, WHITE, 2);
            font5x7::draw_text(fb, b"FAIL", x + 584 - 48, y + 4, WHITE, 2);
            font5x7::draw_text(fb, reason.as_bytes(), x + 12, y + 26, WHITE, 2);
        })?;
    }
    Ok(())
}

fn panel_layout() -> CaseResult {
    let g = fixture_panel();
    region_is(&g, (0, 0, 1280, 4), Color::rgb(24, 39, 59), "the banner's top rows", |_| {})?;
    region_is(&g, (0, 108, 1280, 16), PANEL_BG, "the rows between the banner and the score", |_| {})?;
    for (x, edges) in [(32usize, [(32usize, 429usize, PANEL_PASS), (429, 628, PANEL_FAIL), (628, 628, PANEL_PENDING)]),
        (652, [(652, 850, PANEL_PASS), (850, 1049, PANEL_FAIL), (1049, 1248, PANEL_PENDING)])] {
        region_is(&g, (x, 224, 596, 12), PANEL_BG, &format!("the bar of the group at x = {x}"), |fb| {
            for (a, b, c) in edges {
                shapes::fill_rect(fb, a as i32, 224, (b - a) as i32, 12, c);
            }
        })?;
    }
    region_is(&g, (0, 724, 1280, 76), Color::rgb(48, 67, 96), "the verdict bar", |fb| {
        font5x7::draw_text(fb, b"run", 32, 746, WHITE, 2);
    })
}

fn panel_stage_layout() -> CaseResult { missing("the run stage", "23.2") }
fn panel_tile_stamps() -> CaseResult { missing("the run stage", "23.3") }
fn panel_tile_thumbnail() -> CaseResult { missing("the run stage", "23.3") }
fn panel_spotlight_picture() -> CaseResult { missing("the run stage", "23.4") }
fn panel_spotlight_timing() -> CaseResult { missing("the run stage", "23.4") }
fn panel_milestone_ribbon() -> CaseResult { missing("the run stage", "23.5") }
fn panel_finale() -> CaseResult { missing("the run stage", "23.6") }
fn panel_deterministic() -> CaseResult { missing("the run stage", "23.2") }
fn panel_judged_frame_shown() -> CaseResult { missing("the run stage", "23.8") }
fn panel_serial_agreement() -> CaseResult { missing("the run stage", "23.8") }
fn panel_quiet_window() -> CaseResult { missing("the run stage", "23.8") }
fn panel_boot_ribbon() -> CaseResult { missing("the run stage", "23.7") }
fn panel_boundary_notice() -> CaseResult { missing("the run stage", "23.8") }
