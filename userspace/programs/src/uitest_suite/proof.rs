//! Planted faults are accepted only after the same scenario passes cleanly.
use breenix_suite::suite::{check, CaseResult};
use libuitest::{
    arena::{Slot, SlotRead},
    capture, classify,
    compare::{self, Format, Frame, Options, Rect},
    input::LivePointer,
    Classification, Expected, FailureClass, ScenarioOutcome, ScenarioSpec, Session,
};

const APP: &[u8] = b"/usr/libexec/breenix/uitest-app_test";
const WRONG_OPERATION: &str = "expect_pixels(submit)";

fn pixels(session: &mut Session, id: u64) -> Result<(u32, u32, Vec<u8>), String> {
    let record = capture::window(session.arena_mut(), "submit", id)?;
    match session.arena_mut().read(Slot::Frame)? {
        SlotRead::Final { bytes, .. } => Ok((record.width, record.height, bytes)),
        other => Err(format!("incomplete fault capture: {other:?}")),
    }
}
fn submit_color(session: &mut Session, id: u64, scale: usize) -> Result<ScenarioOutcome, String> {
    let (width, height, got) = pixels(session, id)?;
    // A literal flat patch inside the button, away from its border and text.
    let want = [60, 60, 60, 255].repeat(width as usize * height as usize);
    let regions = [Rect {
        x: 282 * scale,
        y: 44 * scale,
        w: 4 * scale,
        h: 4 * scale,
    }];
    let verdict = compare::compare(
        Frame::new(
            &want,
            width as usize,
            height as usize,
            width as usize * 4,
            Format::Bgrx,
        )
        .map_err(|e| format!("reference: {e:?}"))?,
        Frame::new(
            &got,
            width as usize,
            height as usize,
            width as usize * 4,
            Format::Bgrx,
        )
        .map_err(|e| format!("capture: {e:?}"))?,
        Options {
            regions: &regions,
            ..Options::default()
        },
    )
    .map_err(|e| format!("comparison: {e:?}"))?;
    Ok(match verdict.comparison.first {
        None => ScenarioOutcome::pass(),
        Some(first) => ScenarioOutcome::fail(
            FailureClass::PixelMismatch,
            WRONG_OPERATION,
            &libuitest::messages::pixel_details(first.x, first.y, first.actual, first.expected),
        ),
    })
}
fn wrong_color_scenario(fault: bool) -> Result<ScenarioOutcome, String> {
    let mut session = Session::desktop()?;
    let mut argv = vec![APP, b"planted color".as_slice(), b"--widgets".as_slice()];
    if fault {
        argv.push(b"--wrong-color");
    }
    let pid = session.launch_app(&argv, 0)?;
    let result = (|| {
        let id = session.wait_window("planted color")?.id;
        let mut live = LivePointer::new(&mut session)?;
        live.watch(id)?;
        let scale = libbreenix::gpu::current_mode()
            .map_err(|e| e.to_string())?
            .scale as usize;
        submit_color(live.session(), id, scale)
    })();
    let exits = session.teardown()?;
    reject_crash(pid, &exits)?;
    result
}

pub fn fault_wrong_color() -> CaseResult {
    let scale = libbreenix::gpu::current_mode()
        .map_err(|e| e.to_string())?
        .scale as usize;
    for fault in [false, true] {
        let outcome = libuitest::run_scenario(ScenarioSpec {
            suite: "ui-testing",
            case: "proof/fault-wrong-color",
            checks: &[],
            limit_ms: 6500,
            run: &mut |_| {
                wrong_color_scenario(fault).unwrap_or_else(|e| {
                    ScenarioOutcome::fail(FailureClass::Infrastructure, "wrong-color-scenario", &e)
                })
            },
        });
        let evidence = format!(
            "first={},{} got=#d02040 want=#3c3c3c",
            282 * scale,
            44 * scale
        );
        let expected = if fault {
            Expected::Fault {
                class: FailureClass::PixelMismatch,
                operation: WRONG_OPERATION,
                evidence: &evidence,
            }
        } else {
            Expected::Pass
        };
        check(
            classify(&expected, &outcome)
                == if fault {
                    Classification::ExpectedFault
                } else {
                    Classification::CleanPass
                },
            &format!("wrong-color fault={fault}: {outcome:?}"),
        )?;
    }
    Ok(())
}

const ENTRY_OPERATION: &str = "expect_value(entry)";
const STALE_OPERATION: &str = "expect_pixels(entry)";
const HANG_OPERATION: &str = "step(16)";
const STALE_EVIDENCE: &str = "node=entry field=value tree=\"ab\" pixels=\"\" expected=\"ab\"";
const HANG_EVIDENCE: &str = "app=planted hang epoch=1 bound_ns=2000000000";

fn entry_value(tree: &libbui::tree::Tree) -> Result<serde_json::Value, String> {
    Ok(libuitest::tree::find(
        tree,
        &libuitest::tree::Query {
            key: Some("entry"),
            ..Default::default()
        },
    )?
    .value
    .clone())
}
fn entry_reference(width: u32, height: u32, text: &str) -> Result<Vec<u8>, String> {
    let mut surface = libgfx::surface::Surface::new(
        width as usize,
        height as usize,
        libgfx::surface::Format::BGRX,
    )
    .map_err(|e| e.to_string())?;
    let mut field =
        libbui::widget::text_field::TextField::new(libbui::Rect::new(20, 42, 240, 28), 256);
    field.set_focused(true);
    if !field.insert_str(text) && !text.is_empty() {
        return Err("reference text refused".into());
    }
    surface.batch(|s| field.draw(s, &libbui::Theme::dark()));
    let mut pixels = vec![0; width as usize * height as usize * 4];
    surface
        .read(&mut pixels, width as usize * 4)
        .map_err(|e| e.to_string())?;
    Ok(pixels)
}
fn entry_comparison(
    want: &[u8],
    got: &[u8],
    width: u32,
    height: u32,
    scale: usize,
) -> Result<compare::Verdict, String> {
    let regions = [Rect {
        x: 20 * scale,
        y: 42 * scale,
        w: 240 * scale,
        h: 28 * scale,
    }];
    compare::compare(
        Frame::new(
            want,
            width as usize,
            height as usize,
            width as usize * 4,
            Format::Bgrx,
        )
        .map_err(|e| format!("reference: {e:?}"))?,
        Frame::new(
            got,
            width as usize,
            height as usize,
            width as usize * 4,
            Format::Bgrx,
        )
        .map_err(|e| format!("capture: {e:?}"))?,
        Options {
            regions: &regions,
            ..Default::default()
        },
    )
    .map_err(|e| format!("entry comparison: {e:?}"))
}
fn text_scenario(flag: &str, fault: bool) -> Result<ScenarioOutcome, String> {
    use libuitest::input::{Keyboard, Target};
    let mut session = Session::desktop()?;
    let mut argv = vec![APP, b"planted text".as_slice(), b"--widgets".as_slice()];
    if fault {
        argv.push(flag.as_bytes());
    }
    let pid = session.launch_app(&argv, 0)?;
    let result = (|| {
        let id = session.wait_window("planted text")?.id;
        let mut live = LivePointer::new(&mut session)?;
        live.watch(id)?;
        let mut keys = Keyboard::new(live);
        keys.focus_then_type(&Target::new(id, "entry"), "ab")?;
        let mut live = keys.into_backend();
        let scale = libbreenix::gpu::current_mode()
            .map_err(|e| e.to_string())?
            .scale as usize;
        // Capture tree and pixels together, after the final key has been processed.
        let (width, height, got) = pixels(live.session(), id)?;
        let tree = match live.session().arena_mut().read(Slot::Tree)? {
            SlotRead::Final { bytes, .. } => libbui::tree::Tree::parse(&bytes)?,
            other => return Err(format!("incomplete text tree: {other:?}")),
        };
        let value = entry_value(&tree)?;
        if value != "ab" {
            return Ok(ScenarioOutcome::fail(
                FailureClass::TreeMismatch,
                ENTRY_OPERATION,
                &libuitest::messages::tree(
                    ENTRY_OPERATION,
                    "entry",
                    "value",
                    &serde_json::json!("ab"),
                    &value,
                ),
            ));
        }
        let want = entry_reference(width, height, "ab")?;
        let verdict = entry_comparison(&want, &got, width, height, scale)?;
        if verdict.comparison.passed() {
            return Ok(ScenarioOutcome::pass());
        }
        // A stale frame must equal the independently rendered empty field exactly;
        // arbitrary corruption or a partial render cannot stand in for this fault.
        let empty = entry_reference(width, height, "")?;
        if !entry_comparison(&empty, &got, width, height, scale)?
            .comparison
            .passed()
        {
            return Err("entry pixels match neither current nor initial rendering".into());
        }
        Ok(ScenarioOutcome::fail(
            FailureClass::PixelMismatch,
            STALE_OPERATION,
            STALE_EVIDENCE,
        ))
    })();
    let exits = session.teardown()?;
    reject_crash(pid, &exits)?;
    result
}
fn reject_crash(pid: u64, exits: &[libuitest::ProcessExit]) -> Result<(), String> {
    let exit = exits
        .iter()
        .find(|e| e.pid == pid)
        .ok_or("app exit not witnessed during teardown")?;
    if !libbreenix::process::wifsignaled(exit.status)
        || libbreenix::process::wtermsig(exit.status) != libbreenix::signal::SIGKILL
    {
        return Err(format!("app exited before teardown status={}", exit.status));
    }
    Ok(())
}
fn hang_scenario(fault: bool) -> Result<ScenarioOutcome, String> {
    let mut session = Session::desktop()?;
    let mut argv = vec![APP, b"planted hang".as_slice(), b"--widgets".as_slice()];
    if fault {
        argv.push(b"--hang");
    }
    let pid = session.launch_app(&argv, 0)?;
    let result = (|| {
        let id = session.wait_window("planted hang")?.id;
        let mut live = LivePointer::new(&mut session)?;
        live.watch(id)?;
        let began = libbreenix::time::now_monotonic()
            .map_err(|e| e.to_string())?
            .as_nanos();
        let step = live.session().step(16);
        let elapsed = libbreenix::time::now_monotonic()
            .map_err(|e| e.to_string())?
            .as_nanos()
            - began;
        match step {
            Ok(stamp) if stamp.epoch == 1 && stamp.now_ns == 16_000_000 => {
                Ok(ScenarioOutcome::pass())
            }
            Err(error)
                if error == format!("clock-step-timeout epoch=1 window={id} waiting=[{id}]")
                    && elapsed >= 2_000_000_000 =>
            {
                let (_, meta) = libbreenix::window::read_meta(id).map_err(|e| e.to_string())?;
                if meta.epoch != 0 {
                    return Err("timed-out widget acknowledged the step".into());
                }
                let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))
                    .map_err(|e| e.to_string())?;
                let state = stat
                    .rsplit_once(") ")
                    .and_then(|(_, tail)| tail.split_whitespace().next());
                if !matches!(state, Some("S" | "R")) {
                    return Err(format!("hung app is not live: {state:?}"));
                }
                Ok(ScenarioOutcome::fail(
                    FailureClass::AppHung,
                    HANG_OPERATION,
                    HANG_EVIDENCE,
                ))
            }
            other => Err(format!(
                "unexpected clock step elapsed_ns={elapsed}: {other:?}"
            )),
        }
    })();
    let exits = session.teardown()?;
    reject_crash(pid, &exits)?;
    result
}
fn fault_pair(
    case: &str,
    operation: &str,
    class: FailureClass,
    evidence: &str,
    mut body: impl FnMut(bool) -> Result<ScenarioOutcome, String>,
) -> CaseResult {
    for fault in [false, true] {
        let outcome = libuitest::run_scenario(ScenarioSpec {
            suite: "ui-testing",
            case,
            checks: &[],
            limit_ms: 6500,
            run: &mut |_| {
                body(fault).unwrap_or_else(|e| {
                    ScenarioOutcome::fail(FailureClass::Infrastructure, "fault-scenario", &e)
                })
            },
        });
        let expected = if fault {
            Expected::Fault {
                class,
                operation,
                evidence,
            }
        } else {
            Expected::Pass
        };
        check(
            classify(&expected, &outcome)
                == if fault {
                    Classification::ExpectedFault
                } else {
                    Classification::CleanPass
                },
            &format!("{case} fault={fault}: {outcome:?}"),
        )?;
    }
    Ok(())
}
pub fn fault_drop_key() -> CaseResult {
    fault_pair("proof/fault-drop-key",ENTRY_OPERATION,FailureClass::TreeMismatch,
        "tree-mismatch check=\"expect_value(entry)\" node=\"entry\" field=value expected=String(\"ab\") reported=String(\"b\")",
        |fault|text_scenario("--drop-key",fault))
}
pub fn fault_stale_pixels() -> CaseResult {
    fault_pair(
        "proof/fault-stale-pixels",
        STALE_OPERATION,
        FailureClass::PixelMismatch,
        STALE_EVIDENCE,
        |fault| text_scenario("--stale-pixels", fault),
    )
}
pub fn fault_hang() -> CaseResult {
    fault_pair(
        "proof/fault-hang",
        HANG_OPERATION,
        FailureClass::AppHung,
        HANG_EVIDENCE,
        hang_scenario,
    )
}

#[path = "proof_app_tree.rs"]
mod app_tree;
const REFERENCES: &str = "/usr/share/breenix/uitest/frames";

fn app_checkpoint(
    live: &mut LivePointer<'_>,
    id: u64,
    case: &str,
    stage: &str,
    scale: u32,
    client: &mut libuitest::RecordClient<'_>,
    expect: impl FnOnce(&libbui::tree::Tree) -> Result<(), String>,
) -> Result<(), String> {
    use libuitest::{frames::Bindings, input::Backend};
    let info = live.info(id)?;
    live.move_to((info.x + 600 * scale as i32, info.y + 350 * scale as i32))?;
    libuitest::settle::settle_session(live.session(), &[(id, 0)], 0, &mut [])?;
    live.session().next_checkpoint()?;
    let record = capture::window(live.session().arena_mut(), stage, id)?;
    let tree = match live.session().arena_mut().read(Slot::Tree)? {
        SlotRead::Final { bytes, .. } => libbui::tree::Tree::parse(&bytes)?,
        other => return Err(format!("incomplete scenario tree: {other:?}")),
    };
    expect(&tree)?;
    let scenario = case
        .strip_prefix("proof/")
        .ok_or("scenario case prefix missing")?
        .strip_suffix("-scenario")
        .ok_or("scenario case suffix missing")?;
    let path = format!("ui-testing/{scenario}-{stage}-s{scale}.png");
    let provenance: serde_json::Value = serde_json::from_slice(
        &std::fs::read(format!("{REFERENCES}/PROVENANCE.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let rows: Vec<_> = provenance["references"]
        .as_array()
        .ok_or("missing references")?
        .iter()
        .filter(|row| row["path"] == path)
        .collect();
    if rows.len() != 1 {
        return Err(format!(
            "scenario reference pin missing or duplicate: {path}"
        ));
    }
    let pin = rows[0]["sha256"].as_str().ok_or("scenario pin missing")?;
    let definition = serde_json::json!({"version":1,"case":case,"check":stage,"kind":"stored",
        "source":{"kind":"window","window":"widgets"},"at":{"checkpoint":stage,"requires":[]},
        "reference":{"path":path,"sha256":pin},
        "regions":[{"name":"all","x":0,"y":0,"w":640*scale,"h":400*scale}],
        "mask":[],"tolerance":{"channel":0,"maxDiffering":0}});
    let mut bindings = Bindings::default();
    bindings.window("widgets");
    bindings.checkpoint(stage);
    let verdict = live.session().check_stored(
        "ui-testing",
        &definition,
        &bindings,
        std::path::Path::new(REFERENCES),
        &[],
        client,
    )?;
    if !verdict.comparison.passed() {
        return Err(format!(
            "pixel-mismatch check={stage} commit={} first={:?}",
            record.commit, verdict.comparison.first
        ));
    }
    Ok(())
}

pub fn form_scenario() -> CaseResult {
    use libuitest::input::{Keyboard, Pointer, Target};
    const CASE: &str = "proof/form-scenario";
    const CHECKS: &[&str] = &["initial", "typed", "checked", "slider", "submitted"];
    let mut client = libuitest::RecordClient::outer("ui-testing", CASE, CHECKS)?;
    let mut session = Session::desktop()?;
    let pid = session.launch_app(&[APP, b"form scenario", b"--widgets"], 0)?;
    let result: Result<(), String> = (|| {
        let id = session.wait_window("form scenario")?.id;
        let scale = libbreenix::gpu::current_mode()
            .map_err(|e| e.to_string())?
            .scale;
        let mut live = LivePointer::new(&mut session)?;
        live.watch(id)?;
        app_checkpoint(&mut live, id, CASE, "initial", scale, &mut client, |t| {
            app_tree::form(t, 0, scale)
        })?;
        let mut keyboard = Keyboard::new(live);
        keyboard.focus_then_type(&Target::new(id, "entry"), "hello")?;
        live = keyboard.into_backend();
        app_checkpoint(&mut live, id, CASE, "typed", scale, &mut client, |t| {
            app_tree::form(t, 1, scale)
        })?;
        let mut pointer = Pointer::new(live);
        for (index, key) in ["accept", "amount", "submit"].into_iter().enumerate() {
            pointer.click(&Target::new(id, key))?;
            app_checkpoint(
                pointer.backend_mut(),
                id,
                CASE,
                CHECKS[index + 2],
                scale,
                &mut client,
                |t| app_tree::form(t, index + 2, scale),
            )?;
        }
        libuitest::input::dispositions(pointer.backend_mut().serials())?;
        Ok(())
    })();
    let exits = session.teardown()?;
    reject_crash(pid, &exits)?;
    result?;
    Ok(())
}

fn selection_scenario(menu: bool) -> CaseResult {
    use libuitest::input::{Pointer, Target};
    let (case, title, stages) = if menu {
        (
            "proof/menu-scenario",
            "menu scenario",
            ["initial", "opened", "chosen"],
        )
    } else {
        (
            "proof/list-scenario",
            "list scenario",
            ["initial", "scrolled", "selected"],
        )
    };
    let mut client = libuitest::RecordClient::outer("ui-testing", case, &stages)?;
    let mut session = Session::desktop()?;
    let pid = session.launch_app(&[APP, title.as_bytes(), b"--widgets"], 0)?;
    let result: Result<(), String> = (|| {
        let id = session.wait_window(title)?.id;
        let scale = libbreenix::gpu::current_mode()
            .map_err(|e| e.to_string())?
            .scale;
        let mut live = LivePointer::new(&mut session)?;
        live.watch(id)?;
        app_checkpoint(&mut live, id, case, stages[0], scale, &mut client, |t| {
            if menu {
                app_tree::menu(t, 0, scale)
            } else {
                app_tree::list(t, 0, scale)
            }
        })?;
        let mut pointer = Pointer::new(live);
        if menu {
            pointer.click(&Target::new(id, "menu/0"))?;
        } else {
            pointer.scroll(&Target::new(id, "list"), -2)?;
        }
        app_checkpoint(
            pointer.backend_mut(),
            id,
            case,
            stages[1],
            scale,
            &mut client,
            |t| {
                if menu {
                    app_tree::menu(t, 1, scale)
                } else {
                    app_tree::list(t, 1, scale)
                }
            },
        )?;
        pointer.click(&Target::new(
            id,
            if menu { "menu/0/0" } else { "list/item-07" },
        ))?;
        app_checkpoint(
            pointer.backend_mut(),
            id,
            case,
            stages[2],
            scale,
            &mut client,
            |t| {
                if menu {
                    app_tree::menu(t, 2, scale)
                } else {
                    app_tree::list(t, 2, scale)
                }
            },
        )?;
        libuitest::input::dispositions(pointer.backend_mut().serials())?;
        Ok(())
    })();
    let exits = session.teardown()?;
    reject_crash(pid, &exits)?;
    result?;
    Ok(())
}
pub fn menu_scenario() -> CaseResult {
    selection_scenario(true)
}
pub fn list_scenario() -> CaseResult {
    selection_scenario(false)
}


pub fn wm_focus_scenario() -> CaseResult {
    use libuitest::input::Keyboard;
    wm_scenario(|mut live, ids, owners, scale, screen| {
        wm_checkpoint(&mut live, ids, owners, scale, screen, 1, ["", ""])?;
        wm_click(&mut live, ids[0], "entry")?;
        wm_checkpoint(&mut live, ids, owners, scale, screen, 0, ["", ""])?;
        let mut keyboard = Keyboard::new(live);
        keyboard.type_text(ids[0], "alpha")?;
        wm_checkpoint(
            keyboard.backend_mut(),
            ids,
            owners,
            scale,
            screen,
            0,
            ["alpha", ""],
        )?;
        wm_focus_next(keyboard.backend_mut())?;
        wm_checkpoint(
            keyboard.backend_mut(),
            ids,
            owners,
            scale,
            screen,
            1,
            ["alpha", ""],
        )?;
        keyboard.type_text(ids[1], "beta")?;
        wm_checkpoint(
            keyboard.backend_mut(),
            ids,
            owners,
            scale,
            screen,
            1,
            ["alpha", "beta"],
        )
    })
}

pub fn wm_stack_scenario() -> CaseResult {
    wm_scenario(|mut live, ids, owners, scale, screen| {
        wm_checkpoint(&mut live, ids, owners, scale, screen, 1, ["", ""])?;
        wm_click(&mut live, ids[0], "entry")?;
        wm_checkpoint(&mut live, ids, owners, scale, screen, 0, ["", ""])?;
        wm_click(&mut live, ids[1], "root")?;
        wm_checkpoint(&mut live, ids, owners, scale, screen, 1, ["", ""])
    })
}

#[path = "proof_wm_checks.rs"]
mod wm_checks;

fn wm_scenario(
    run: impl FnOnce(LivePointer<'_>, [u64; 2], [u64; 2], u32, [u32; 2]) -> Result<(), String>,
) -> CaseResult {
    let mut session = Session::desktop()?;
    let mut owners = [0; 2];
    let result: Result<(), String> = (|| {
        let mut ids = [0; 2];
        for (index, title) in ["wm scenario lower", "wm scenario upper"]
            .into_iter()
            .enumerate()
        {
            owners[index] = session.launch_app(&[APP, title.as_bytes(), b"--widgets"], 0)?;
            ids[index] = session.wait_window(title)?.id;
        }
        let mode = libbreenix::gpu::current_mode().map_err(|e| e.to_string())?;
        let mut live = LivePointer::new(&mut session)?;
        for id in ids {
            live.watch(id)?;
        }
        run(live, ids, owners, mode.scale, [mode.width, mode.height])
    })();
    let exits = session.teardown()?;
    for pid in owners.into_iter().filter(|pid| *pid != 0) {
        reject_crash(pid, &exits)?;
    }
    result?;
    Ok(())
}

fn wm_click(live: &mut LivePointer<'_>, id: u64, key: &str) -> Result<(), String> {
    use libuitest::input::{Backend, Target};
    let resolved = live.resolve(&Target::new(id, key))?;
    live.move_to(resolved.point)?;
    live.confirm(&resolved)?;
    let press = live.button(true, id);
    let release = live.button(false, id);
    press?;
    release?;
    Ok(())
}
fn wm_focus_next(live: &mut LivePointer<'_>) -> Result<(), String> {
    use libbreenix::input::key;
    let alt = live.wm_key(key::LEFT_ALT, true);
    let tab = match &alt {
        Ok(_) => live.wm_key(key::TAB, true),
        Err(error) => Err(error.clone()),
    };
    let tab_up = live.wm_key(key::TAB, false);
    let alt_up = live.wm_key(key::LEFT_ALT, false);
    alt?;
    tab?;
    tab_up?;
    alt_up?;
    Ok(())
}
fn wm_checkpoint(
    live: &mut LivePointer<'_>,
    ids: [u64; 2],
    owners: [u64; 2],
    scale: u32,
    screen: [u32; 2],
    top: usize,
    text: [&str; 2],
) -> Result<(), String> {
    use libuitest::input::Backend;
    // Park beyond the two frames so hover state cannot color the structural patch.
    live.move_to((1100 * scale as i32, 700 * scale as i32))?;
    // Client capture and scanout are separate observations. A publication between
    // them supersedes the whole snapshot, including its retained trees.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    'snapshot: loop {
        libuitest::settle::settle_session(live.session(), &[(ids[0], 0), (ids[1], 0)], 0, &mut [])?;
        wm_checks::desktop(&live.desktop_tree()?, ids, owners, scale, screen, top)?;
        let mut commits = [(0, 0, 0); 2];
        for (index, id) in ids.into_iter().enumerate() {
            let before = wm_window(id)?;
            live.session().next_checkpoint()?;
            let record = capture::window(live.session().arena_mut(), "wm-client", id)?;
            let after = wm_window(id)?;
            if before.pixel_gen != after.pixel_gen {
                if std::time::Instant::now() >= deadline {
                    return Err(format!("WM client front did not stabilize window={id}"));
                }
                continue 'snapshot;
            }
            let generation = after.pixel_gen;
            commits[index] = (id, record.commit, generation);
            match live.session().arena_mut().read(Slot::Tree)? {
                SlotRead::Final { bytes, .. } => {
                    wm_checks::entry(&libbui::tree::Tree::parse(&bytes)?, text[index], scale)?
                }
                other => return Err(format!("incomplete WM app tree: {other:?}")),
            }
        }
        live.session().next_checkpoint()?;
        let record = capture::screen(
            live.session().arena_mut(),
            "wm-screen",
            libbreenix::window::ObservationKind::Composed,
        )?;
        let observed = capture::screen_windows(&record)?;
        let entries = wm_composite(record.composite)?;
        for (id, commit, generation) in commits {
            let after = wm_window(id)?;
            let projected = entries.iter().find(|entry| entry.id == id);
            if !projected.is_some_and(|entry| {
                wm_front_matches(commit, generation, after.pixel_gen,
                    observed.iter().find(|row| row.0 == id).map(|row| row.1), entry)
            }) {
                if std::time::Instant::now() < deadline {
                    continue 'snapshot;
                }
                return Err(format!(
                    "WM screen missing compared window={id} commit={commit} generation={generation} current={} composite={} observed={observed:?} projected={projected:?}",
                    after.pixel_gen, record.composite
                ));
            }
        }
        match live.session().arena_mut().read(Slot::Screen)? {
            SlotRead::Final { bytes, .. } => wm_checks::screen(
                Frame::new(
                    &bytes,
                    record.width as usize,
                    record.height as usize,
                    record.width as usize * 4,
                    Format::Bgrx,
                )
                .map_err(|e| format!("WM screen frame: {e:?}"))?,
                scale,
                top,
            )?,
            other => return Err(format!("incomplete WM screen: {other:?}")),
        }
        return libuitest::input::dispositions(live.serials());
    }
}

fn wm_front_matches(captured: u64, before: u64, after: u64, observed: Option<u64>,
    entry: &libbreenix::window::abi::CompositeEntry) -> bool {
    // entry.pixels is work in this update; unchanged visible pixels need no work.
    observed == Some(entry.commit_id)
        && wm_checks::front_matches(captured, before, after, entry.commit_id, entry.pixel_gen)
}

fn wm_window(id: u64) -> Result<libbreenix::window::abi::WinInfo, String> {
    let mut windows = vec![libbreenix::window::abi::WinInfo::default(); 64];
    let count = libbreenix::window::list(&mut windows).map_err(|e| e.to_string())?;
    if count > windows.len() {
        return Err("WM window registry exceeds scenario bound".into());
    }
    windows.truncate(count);
    windows.into_iter().find(|entry| entry.id == id)
        .ok_or_else(|| format!("WM window missing from registry window={id}"))
}

/// Bind scanout to the retained composite named by its observation serial.
fn wm_composite(serial: u64) -> Result<Vec<libbreenix::window::abi::CompositeEntry>, String> {
    use libbreenix::window::{self, abi};
    if serial == 0 {
        return Err("WM screen has no composite serial".into());
    }
    let head = core::mem::size_of::<abi::CompositeRecordHeader>();
    let size = core::mem::size_of::<abi::CompositeEntry>();
    // The two-client scenario fits within 64 entries, exceeding the observation's
    // 16-row limit. Missing and oversized records are errors.
    let mut bytes = vec![0u8; head + 64 * size];
    let mut query = abi::RecordQuery {
        serial, ptr: bytes.as_mut_ptr() as u64, len: bytes.len() as u32, reserved: 0,
    };
    window::call(abi::op::COMPOSITE_RECORD, &mut query)
        .map_err(|e| format!("WM composite record serial={serial}: {e}"))?;
    // SAFETY: the successful copy supplies plain ABI values, read unaligned
    // from byte storage. Validate the header before accessing any entry.
    let header = unsafe { core::ptr::read_unaligned(bytes.as_ptr().cast::<abi::CompositeRecordHeader>()) };
    if header.serial != serial || header.flags != 0 || header.count as usize > (bytes.len() - head) / size {
        return Err(format!("WM invalid composite record serial={serial} header={header:?}"));
    }
    let mut entries: Vec<abi::CompositeEntry> = Vec::with_capacity(header.count as usize);
    for index in 0..header.count as usize {
        let entry = unsafe { core::ptr::read_unaligned(bytes.as_ptr().add(head + index * size).cast::<abi::CompositeEntry>()) };
        if entry.id == 0 || entries.iter().any(|prior| prior.id == entry.id) {
            return Err(format!("WM duplicate/invalid composite window serial={serial} window={}", entry.id));
        }
        entries.push(entry);
    }
    Ok(entries)
}
