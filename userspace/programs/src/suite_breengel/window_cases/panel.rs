//! The window cases of panel (contract section 23).

use crate::*;

pub(crate) const PANEL: &[Case] = &[
    pictured!("handoff", "A display-taking child under probe takes the display, gives it back, and the panel is restored exactly", panel_handoff),
    pictured!("between-cases", "In a GUI suite the panel is drawn only between cases and restored exactly after each", panel_between_cases),
    pictured!("cursor-restore", "Moving the pointer over the panel leaves no cursor trail", panel_cursor_restore),
    pictured!("cursor-trail-detected", "With the panel cursor's save and restore disabled by a test control, the trail is detected", panel_cursor_trail_detected),
    pictured!("kernel-screens", "The kernel's console screen redraws exactly after the display is given back", panel_kernel_screens),
];

// ---------------------------------------------------------------------------------------
// panel: the suite panel around window cases (contract section 23)
// ---------------------------------------------------------------------------------------

use super::composition::{screen_size,give_back, paint, screen, take_over};
use super::kit::*;

fn first_difference(a: &[u8], b: &[u8]) -> Result<Option<(usize, usize)>,libbreenix::suite::CaseError> {
    let (width,height)=screen_size()?;
    crate::remember_frame(b,width,height,Some(a.to_vec()));
    Ok((0..width * height).find(|&i| a[i * 4..i * 4 + 4] != b[i * 4..i * 4 + 4]).map(|i| (i % width, i / width)))
}

// gpu-display: B7 replaces the child's retired framebuffer paint with GPU PRESENT; exact restoration stays.
fn panel_handoff() -> CaseResult {
    let (ret,modes)=libbreenix::gpu::modes();
    ensure(ret==0 && modes.binding_known==1,"23","gpu-display: panel handoff has no known GPU binding")?;
    interface()?;
    let (before, _) = screen()?;
    let f = || {
        if take_over() < 0 {
            return 2;
        }
        if paint([0x70, 0x10, 0x90, 0]).is_err() {
            return 3;
        }
        if give_back() < 0 {
            return 4;
        }
        0
    };
    let ended = child_end(5_000, || f()).cite("23")?;
    ensure(matches!(ended, Ended::Exit(0)), "23", "the display-taking child did not take, paint and give back the display")?;
    let (after, _) = screen()?;
    // The judged frame is the stage's own, which it samples itself (ruling B36).
    libbreenix::suite::judged_picture_stage();
    match first_difference(&before, &after)? {
        None => Ok(()),
        Some((x, y)) => at("23", format!("after the child gave the display back, the panel differs at ({x}, {y})")),
    }
}

fn panel_between_cases() -> CaseResult {
    interface()?;
    let (a, _) = screen()?;
    let _ = time::sleep_ms(300);
    let (b, _) = screen()?;
    libbreenix::suite::judged_picture_stage();
    match first_difference(&a, &b)? {
        None => Ok(()),
        Some((x, y)) => at("23", format!("the screen changed at ({x}, {y}) while this case ran; the panel is drawn only between cases")),
    }
}

/// Move the kernel's pointer to (x, y) with `move-pointer`, wait until the kernel has read it,
/// and give the panel cursor time to follow.
fn move_pointer(x: u32, y: u32) -> CaseResult {
    use abi::tc_point::MOVE_POINTER;
    let fired = || libbreenix::test_control::fired(MOVE_POINTER).map_err(|e| format!("FIRED: {e}"));
    let before = fired().cite("21")?;
    libbreenix::test_control::arm(MOVE_POINTER, 0, x as u64 | (y as u64) << 32).map_err(|e| format!("ARM move-pointer: {e}")).cite("21")?;
    let start = now_ms();
    while fired().cite("21")? == before {
        if since(start) > 500 {
            return at("21, 23", format!("move-pointer to ({x}, {y}) did not fire within 500 ms: nothing read the pointing device for the panel cursor"));
        }
        let _ = time::sleep_ms(5);
    }
    let _ = time::sleep_ms(50);
    Ok(())
}

/// Where the pointer goes: off the scene at the top right, then across the panel, then back.
const REST: (u32, u32) = (1200, 40);
const PATH: [(u32, u32); 5] = [(200, 300), (400, 320), (640, 400), (900, 500), (1100, 600)];

/// The pixels that differ between the screen with the pointer at rest and the screen after it
/// crossed the panel and came back to rest, with `panel-cursor-no-restore` armed for the
/// crossing when `no_restore`.
fn trail(no_restore: bool) -> Result<Vec<(u32, u32)>, libbreenix::suite::CaseError> {
    let (width,height)=screen_size()?;
    move_pointer(REST.0, REST.1)?;
    let (rest, _) = screen()?;
    if no_restore {
        libbreenix::test_control::arm(abi::tc_point::PANEL_CURSOR_NO_RESTORE, 0, 0)
            .map_err(|e| format!("ARM panel-cursor-no-restore: {e}")).cite("21")?;
    }
    let crossed = PATH.iter().try_for_each(|&(x, y)| move_pointer(x, y)).and_then(|()| move_pointer(REST.0, REST.1));
    let _ = libbreenix::test_control::disarm_all();
    crossed?;
    let (back, _) = screen()?;
    crate::remember_frame(&back,width,height,if no_restore{None}else{Some(rest.clone())});
    libbreenix::suite::judged_picture_stage();
    Ok((0..(width * height) as u32)
        .filter(|&i| rest[i as usize * 4..i as usize * 4 + 4] != back[i as usize * 4..i as usize * 4 + 4])
        .map(|i| (i % width as u32, i / width as u32))
        .collect())
}

fn panel_cursor_restore() -> CaseResult {
    interface()?;
    let left = trail(false)?;
    ensure(left.is_empty(), "23", format!(
        "after the pointer crossed the panel and came back, {} pixels differ from the panel it left, the first at {:?}: a cursor trail",
        left.len(), left.first()))
}

fn panel_cursor_trail_detected() -> CaseResult {
    interface()?;
    let left = trail(true)?;
    ensure(!left.is_empty(), "21, 23", "with panel-cursor-no-restore armed the pointer crossed the panel and left no trail: the restore was not disabled, or the trail is not seen")?;
    let near = |&(x, y): &(u32, u32)| PATH.iter().any(|&(px, py)| x.abs_diff(px) < 32 && y.abs_diff(py) < 32);
    ensure(left.iter().all(near), "21, 23", format!(
        "the trail left with panel-cursor-no-restore armed has pixels away from every place the pointer stopped, the first at {:?}",
        left.iter().find(|p| !near(p))))
}

// gpu-display: B7 replaces retired framebuffer painting; the GPU console must be restored byte for byte.
fn panel_kernel_screens() -> CaseResult {
    let (ret,modes)=libbreenix::gpu::modes();
    ensure(ret==0 && modes.console_resource!=0,"23","gpu-display: kernel screens have no GPU console resource")?;
    interface()?;
    // This runner owns the display; giving it back shows the kernel's console screen.
    let gave = give_back();
    ensure(gave >= 0, "18", format!("give_back_display by the runner {}", outcome(gave)))?;
    let shown = screen();
    let f = || {
        if take_over() < 0 {
            return 2;
        }
        if paint([0x05, 0x06, 0x07, 0]).is_err() {
            return 3;
        }
        if give_back() < 0 {
            return 4;
        }
        0
    };
    let ended = child_end(5_000, || f());
    let again = screen();
    let retook = take_over();
    let (console, _) = shown?;
    let (redrawn, _) = again?;
    ensure(retook >= 0, "18", format!("the runner's take_over_display {}", outcome(retook)))?;
    ensure(matches!(ended.cite("23")?, Ended::Exit(0)), "23", "the display-taking child did not take, paint and give back the display")?;
    match first_difference(&console, &redrawn)? {
        None => Ok(()),
        Some((x, y)) => at("23", format!("after a process took the display and gave it back, the kernel's console screen differs at ({x}, {y})")),
    }
}
