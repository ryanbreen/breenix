//! Live composite barriers, with forced pre-completion overlap and independent input receipts.
use super::settle::{launch, pixels, snapshot};
use breenix_suite::suite::{check, CaseResult};
use libbreenix::{
    input::{axis, key, kind, Reader},
    test_control, time,
    window::{self, abi},
};
use libuitest::{
    input::{self as script, LivePointer},
    settle::{self, Predicate},
    Session,
};
use std::time::{Duration, Instant};
const APP: &[u8] = b"/usr/libexec/breenix/uitest-app_test";
fn require(ok: bool, why: &str) -> Result<(), String> {
    if ok {
        Ok(())
    } else {
        Err(why.into())
    }
}
fn now_ns() -> Result<u64, String> {
    time::now_monotonic()
        .map(|t| t.as_nanos() as u64)
        .map_err(|e| e.to_string())
}
fn pause() -> Result<(), String> {
    time::sleep_ms(1).map_err(|e| e.to_string())
}
fn info(id: u64) -> Result<abi::WinInfo, String> {
    settle::presentation()?
        .windows
        .into_iter()
        .find(|w| w.id == id)
        .ok_or("settle window missing".into())
}
fn ready(session: &Session, targets: &[(u64, u64)]) -> Result<(), String> {
    settle::settle_session(session, targets, 0, &mut [])
}
fn wait(mut condition: impl FnMut() -> Result<bool, String>) -> Result<(), String> {
    let end = Instant::now() + Duration::from_secs(2);
    while !condition()? {
        require(Instant::now() < end, "composite fixture witness timed out")?;
        pause()?;
    }
    Ok(())
}
/// Chrome events are WM-handled. Observe their actual source serials rather
/// than treating the supervisor's injection acknowledgment as WM completion.
struct Mouse {
    reader: Reader,
}
impl Mouse {
    fn new() -> Result<Self, String> {
        Ok(Self {
            reader: Reader::open(true).map_err(|e| e.to_string())?,
        })
    }
    fn source(&mut self, session: &Session, k: u16, code: u16, value: i32) -> Result<u64, String> {
        let end = Instant::now() + Duration::from_secs(2);
        loop {
            let mut bytes = [0; 32768];
            match self.reader.read(&mut bytes) {
                Ok(records) => {
                    for r in records {
                        let e = r.map_err(|e| e.to_string())?.event;
                        if e.device == session.input_devices()[1]
                            && e.kind == k
                            && e.code == code
                            && e.value == value
                        {
                            script::wait_pointer_idle(e.serial)?;
                            return Ok(e.serial);
                        }
                    }
                }
                Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::EAGAIN)) => {}
                Err(e) => return Err(e.to_string()),
            }
            require(Instant::now() < end, "chrome input source missing")?;
            pause()?;
        }
    }
    fn motion(&mut self, session: &mut Session, dx: i32, dy: i32) -> Result<(), String> {
        session.raw_motion(dx, dy)?;
        // source() may consume the companion Y event; only X is needed as a
        // terminal witness because both axes are one ordered injected batch.
        self.source(session, kind::REL, axis::X, dx)?;
        Ok(())
    }
    fn position(&mut self, session: &mut Session, x: i32, y: i32) -> Result<(), String> {
        let mode = libbreenix::gpu::current_mode().map_err(|e| e.to_string())?;
        self.motion(session, -(mode.width as i32), -(mode.height as i32))?;
        self.motion(session, x, y)
    }
    fn button(&mut self, session: &mut Session, down: bool) -> Result<u64, String> {
        if down {
            session.raw_press(key::BUTTON_LEFT)?;
        } else {
            session.raw_release(key::BUTTON_LEFT)?;
        }
        self.source(session, kind::KEY, key::BUTTON_LEFT, i32::from(down))
    }
    fn click(&mut self, session: &mut Session, x: i32, y: i32) -> Result<u64, String> {
        self.position(session, x, y)?;
        let press = self.button(session, true)?;
        self.button(session, false)?;
        Ok(press)
    }
}
struct Release;
impl Drop for Release {
    fn drop(&mut self) {
        let _ = test_control::arm(abi::tc_point::RELEASE_STALL, 0, 0);
    }
}
fn held_composite(id: u64, change: impl FnOnce() -> Result<(), String>) -> Result<Release, String> {
    let before = test_control::fired(abi::tc_point::COMPOSITE_STALL).map_err(|e| e.to_string())?;
    test_control::arm(abi::tc_point::COMPOSITE_STALL, id, 0).map_err(|e| e.to_string())?;
    let release = Release;
    change()?;
    wait(|| {
        Ok(
            test_control::fired(abi::tc_point::COMPOSITE_STALL).map_err(|e| e.to_string())?
                > before,
        )
    })?;
    Ok(release)
}
fn release_in_settle(
    session: &Session,
    targets: &[(u64, u64)],
    release: Release,
) -> Result<(), String> {
    let mut release = Some(release);
    let mut polls = 0;
    let mut unblock = || {
        polls += 1;
        if polls == 2 {
            drop(release.take());
        }
        Ok(true)
    };
    settle::settle_session(
        session,
        targets,
        0,
        &mut [Predicate {
            name: "release-composite",
            check: &mut unblock,
        }],
    )?;
    require(
        polls >= 3,
        "settle returned before the held composite completed",
    )
}
fn run(which: u8) -> Result<(), String> {
    let began_ns = now_ns()?;
    let case_deadline_ns =
        began_ns.saturating_add(breenix_suite::suite::case_ms_left().saturating_mul(1_000_000));
    let mut wm_trace = String::new();
    let mut session = Session::desktop()?;
    let result: Result<(), String> = (|| {
        let (mut control, id) = launch(&mut session, false)?;
        ready(&session, &[(id, 0)])?;
        match which {
            0 => {
                // Align the second window over the first using the real chrome
                // drag. Client dimensions stay pixels; chrome uses mode scale.
                session.launch_app(&[APP, b"composite cover"], 0)?;
                let upper = session.wait_window("composite cover")?.id;
                let lower = info(id)?;
                let top = info(upper)?;
                let scale = libbreenix::gpu::current_mode()
                    .map_err(|e| e.to_string())?
                    .scale as i32;
                let mut mouse = Mouse::new()?;
                mouse.position(&mut session, top.x + 100, top.y - 16 * scale)?;
                mouse.button(&mut session, true)?;
                mouse.motion(&mut session, lower.x - top.x, lower.y - top.y)?;
                mouse.button(&mut session, false)?;
                wait(|| {
                    let top = info(upper)?;
                    Ok(top.x == lower.x && top.y == lower.y)
                })?;
                ready(&session, &[(id, 0), (upper, 0)])?;
                control.command("hold")?;
                let before = pixels(id)?;
                let release = held_composite(id, || control.command("pixel"))?;
                require(pixels(id)? > before, "fixture did not publish new pixels")?;
                let pending = settle::presentation()?;
                require(
                    pending.waiting(0, 0, &[]).is_some(),
                    "pending composite was accepted",
                )?;
                release_in_settle(&session, &[(id, 0), (upper, 0)], release)?;
                let done = settle::presentation()?;
                require(
                    done.waiting(0, 0, &[]).is_none(),
                    "settle missed a newest window generation",
                )?;
                let generation = pixels(id)?;
                require(
                    done.entries
                        .iter()
                        .any(|e| e.id == id && e.pixel_gen == generation && e.pixels == 0),
                    "occluded window lacks its zero-pixel completion",
                )?;
            }
            1 => {
                control.command("hold")?;
                ready(&session, &[(id, 0)])?;
                let before = snapshot(id)?;
                let generation = pixels(id)?;
                let composite = settle::presentation()?.status.composites;
                control.command("ack")?;
                ready(&session, &[(id, 0)])?;
                require(
                    snapshot(id)?.commit > before.commit && pixels(id)? == generation,
                    "metadata-only acknowledgment changed pixels",
                )?;
                require(
                    settle::presentation()?.status.composites == composite,
                    "no-change settle forced a composite",
                )?;
            }
            2 => {
                let before = settle::presentation()?.status.composites;
                let stamp = session.step(0)?;
                ready(&session, &[(id, 0)])?;
                let after = settle::presentation()?.status;
                require(
                    after.epoch == stamp.epoch
                        && after.flags == abi::compositor_ack_flags::NONE_NEEDED
                        && after.presented == 0
                        && after.composites == before,
                    "idle step did not settle through none-needed",
                )?;
            }
            3 => {
                control.command("hold")?;
                let mut live = LivePointer::new(&mut session)?;
                ready(live.session(), &[(id, 0)])?;
                let before = snapshot(id)?;
                let mut last = 0;
                for down in [true, false, true, false] {
                    last = live.wm_key(key::LEFT_SUPER, down)?;
                }
                settle::settle_session(live.session(), &[], last, &mut [])?;
                let ledger = std::fs::read_to_string("/proc/breenix/input-ledger")
                    .map_err(|e| e.to_string())?;
                require(
                    script::ledger_entry(&ledger, last)?
                        .is_some_and(|r| r.disposition == "wm-handled" && r.window == 0),
                    "hotkey lacks its WM-handled disposition",
                )?;
                let wait_begin_ns = now_ns()?;
                wm_trace.push_str(&format!(
                    "registration-wait source_serial={last} begin_ns={wait_begin_ns} deadline_ns={case_deadline_ns}\n",
                ));
                // LIST uses the raw chromeless title, including its marker.
                let registration = live
                    .session()
                    .wait_window_until("\u{1}Launcher", case_deadline_ns);
                wm_trace.push_str(&format!(
                    "registration-wait end_ns={} result={registration:?}\n",
                    now_ns()?,
                ));
                let launcher = registration?;
                // This is an observation bound, not a producer registration stamp.
                // WindowReady retains the kernel's completed-composite timestamp.
                wm_trace.push_str(&format!(
                    "registered-observation at_ns={} window={} registry={:?} presented_ns={}\n",
                    now_ns()?,
                    launcher.id,
                    live.info(launcher.id).map(|w| w.owner),
                    launcher.presented_ns,
                ));
                wm_trace.push_str(&format!("snapshot begin_ns={}\n", now_ns()?));
                let desktop = live.desktop_tree();
                wm_trace.push_str(&format!(
                    "snapshot end_ns={} scene_and_children={:?}\n",
                    now_ns()?,
                    desktop.as_ref().map(|tree| (
                        &tree.root.value,
                        tree.children
                            .iter()
                            .take(64)
                            .map(|n| n.key.as_str())
                            .collect::<Vec<_>>()
                    )),
                ));
                require(
                    desktop?.children.iter().any(|n| n.name == "Launcher"),
                    "hotkey did not open launcher",
                )?;
                require(
                    snapshot(id)? == before,
                    "WM-only hotkey involved the client",
                )?;
            }
            4 => {
                control.command("hold")?;
                ready(&session, &[(id, 0)])?;
                let before = (
                    snapshot(id)?,
                    pixels(id)?,
                    settle::presentation()?.status.composites,
                );
                for _ in 0..3 {
                    ready(&session, &[(id, 0)])?;
                }
                require(
                    before
                        == (
                            snapshot(id)?,
                            pixels(id)?,
                            settle::presentation()?.status.composites,
                        ),
                    "repeated settle caused extra frames",
                )?;
            }
            5 => {
                control.command("hold")?;
                let w = info(id)?;
                let mode = libbreenix::gpu::current_mode().map_err(|e| e.to_string())?;
                let s = mode.scale as i32;
                let mut mouse = Mouse::new()?;
                // Literal chrome button geometry: 20 size, 4 padding, 2 border.
                let serial =
                    mouse.click(&mut session, w.x + w.width as i32 - 40 * s, w.y - 17 * s)?;
                wait(|| Ok(info(id)?.flags & abi::win_flags::MINIMIZED != 0))?;
                let stamp = session.step(0)?;
                let generation = pixels(id)?;
                control.command("pixel")?;
                ready(&session, &[(id, 0)])?;
                let meta = window::read_meta(id).map_err(|e| e.to_string())?.1;
                require(
                    meta.epoch == stamp.epoch && pixels(id)? > generation,
                    "minimized app did not acknowledge and commit",
                )?;
                require(
                    script::client_processed(meta.processed_serial, serial),
                    "minimized app did not process its focus notice",
                )?;
                let scene = settle::presentation()?;
                let generation = pixels(id)?;
                require(
                    !scene
                        .entries
                        .iter()
                        .any(|e| e.id == id && e.pixel_gen >= generation),
                    "minimized fixture unexpectedly presented new pixels",
                )?;
                // First appbar button: left=4, height=28 at bottom, center within it.
                mouse.position(&mut session, 24 * s, mode.height as i32 - 18 * s)?;
                let release = held_composite(id, || {
                    session.raw_press(key::BUTTON_LEFT)?;
                    wait(|| Ok(info(id)?.flags & abi::win_flags::MINIMIZED == 0))
                })?;
                // Release is queued while the compositor is held; do not wait
                // for bwm to handle it until the completion is released.
                session.raw_release(key::BUTTON_LEFT)?;
                require(
                    settle::presentation()?
                        .waiting(stamp.epoch, 0, &[])
                        .is_some(),
                    "restore reused stale pixels",
                )?;
                release_in_settle(&session, &[(id, 0)], release)?;
            }
            _ => unreachable!(),
        }
        Ok(())
    })();
    if result.is_err() && which == 3 {
        eprintln!("WM_HANDLED_ORDERING\n{wm_trace}");
    }
    let teardown = session.teardown();
    result?;
    teardown?;
    Ok(())
}
pub fn composited() -> CaseResult {
    run(0).map_err(Into::into)
}
pub fn no_change_no_composite() -> CaseResult {
    run(1).map_err(Into::into)
}
pub fn bwm_ack_no_composite() -> CaseResult {
    run(2).map_err(Into::into)
}
pub fn wm_handled() -> CaseResult {
    run(3).map_err(Into::into)
}
pub fn repeated_settle() -> CaseResult {
    run(4).map_err(Into::into)
}
pub fn minimized_acks() -> CaseResult {
    run(5).map_err(Into::into)
}

fn animation(input: bool) -> CaseResult {
    let mut session = Session::desktop()?;
    let result: CaseResult = (|| {
        session.launch_app(&[APP, b"settle animation", b"--animate"], 0)?;
        let id = session.wait_window("settle animation")?.id;
        let mut live = LivePointer::new(&mut session)?;
        let mut expected_keys = Vec::new();
        let mut expected_serials = Vec::new();
        for code in [key::A, key::B, key::C] {
            let stamp = live.session().step(16)?;
            let last = if input {
                let press = live.send_key(code, true)?;
                script::delivered(press.serial)?;
                expected_keys.push(u32::from(code));
                expected_serials.push(press.serial);
                let release = live.send_key(code, false)?;
                script::delivered(release.serial)?;
                release.serial
            } else {
                0
            };
            settle::settle_session(live.session(), &[(id, last)], 0, &mut [])?;
            let page = libbreenix::ui_clock::ClockPage::open(
                &std::path::Path::new(live.session().config_root()).join("ui-clock"),
            )?;
            check(
                page.participants().iter().any(|p| {
                    p.id == id
                        && p.flags & libbreenix::ui_clock::ANIMATING != 0
                        && p.next_deadline_ns > stamp.now_ns
                }),
                "settle stopped animation or waited past its future deadline",
            )?;
            if input {
                let mut px = vec![0; 320 * 200 * 4];
                let mut tree = vec![0; abi::limits::DEFAULT_TREE_KIB as usize * 1024];
                window::capture(id, &mut px, Some(&mut tree)).map_err(|e| e.to_string())?;
                let end = tree
                    .iter()
                    .position(|b| *b == 0)
                    .ok_or("animation tree unterminated")?;
                let tree: serde_json::Value =
                    serde_json::from_slice(&tree[..end]).map_err(|e| e.to_string())?;
                check(
                    tree["value"]["keys"] == serde_json::json!(expected_keys)
                        && tree["value"]["serials"] == serde_json::json!(expected_serials),
                    "animation input arrived out of order",
                )?;
            }
        }
        Ok(())
    })();
    let teardown = session.teardown();
    result?;
    teardown?;
    Ok(())
}
pub fn animation_allowed() -> CaseResult {
    animation(false)
}
pub fn input_during_animation() -> CaseResult {
    animation(true)
}
