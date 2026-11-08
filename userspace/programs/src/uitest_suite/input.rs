//! Pointer cases judge real libbui state and source-serial delivery through bwm.
use breenix_suite::suite::CaseResult;
use libbreenix::{
    input::{axis, key, kind, Batch, Capability, Injector},
    window,
};
use libbui::tree::{Flag, Tree};
use libuitest::{
    input::{Delivery, LivePointer, Pointer, Target},
    Session,
};
use std::time::{Duration, Instant};

fn require(ok: bool, detail: impl Into<String>) -> Result<(), String> {
    if ok {
        Ok(())
    } else {
        Err(detail.into())
    }
}
fn value(tree: &Tree, key: &str) -> Result<serde_json::Value, String> {
    Ok(crate::tree::node(tree, key)?.value.clone())
}
fn delivered(d: Delivery, id: u64) -> Result<(), String> {
    require(
        d.serial != 0 && d.window == id,
        format!("pointer delivery {d:?} expected window={id}"),
    )
}
fn launch(session: &mut Session, title: &[u8]) -> Result<u64, String> {
    session.launch_app(
        &[
            b"/usr/libexec/breenix/uitest-app_test",
            title,
            b"--pointer-probe",
        ],
        0,
    )?;
    Ok(session.wait_window(std::str::from_utf8(title).unwrap())?.id)
}
fn run(which: u8) -> Result<(), String> {
    let mut session = Session::connect()?;
    // A second relative endpoint moves chrome while the primary endpoint holds content.
    let mover = Injector::create(
        &[
            Capability {
                kind: kind::REL,
                code: axis::X,
                minimum: i32::MIN,
                maximum: i32::MAX,
            },
            Capability {
                kind: kind::REL,
                code: axis::Y,
                minimum: i32::MIN,
                maximum: i32::MAX,
            },
            Capability {
                kind: kind::KEY,
                code: key::BUTTON_LEFT,
                minimum: 0,
                maximum: 1,
            },
        ],
        true,
    )
    .map_err(|e| e.to_string())?;
    session.start_compositor_with_pointer(mover.id())?;
    let result: Result<(), String> = (|| {
        let id = launch(&mut session, b"pointer lower")?;
        let mut p = Pointer::new(LivePointer::new(&mut session)?);
        p.backend_mut().watch(id)?;
        let scale = libbreenix::gpu::current_mode()
            .map_err(|e| e.to_string())?
            .scale as i32;
        match which {
            0 => {
                let click = p.click(&Target::new(id, "accept"))?;
                delivered(click.press, id)?;
                delivered(click.release, id)?;
                require(
                    value(&p.backend_mut().tree(id)?, "accept")? == true,
                    "click-node did not toggle accept",
                )?;
            }
            1 => {
                let other = launch(p.backend_mut().session(), b"pointer upper")?;
                p.backend_mut().watch(other)?;
                let lower = p.backend_mut().info(id)?;
                let upper = p.backend_mut().info(other)?;
                // The cascade exposes the left of the checkbox and covers its right.
                let left = lower.x + 20 * scale;
                let right = left + 180 * scale;
                require(
                    upper.x - 2 * scale > left && upper.x - 2 * scale < right,
                    "second window must partly cover accept",
                )?;
                let click = p.click(&Target::new(id, "accept"))?;
                require(
                    click.point.0 >= left && click.point.0 < upper.x - 2 * scale,
                    "click missed exposed checkbox strip",
                )?;
                delivered(click.press, id)?;
                delivered(click.release, id)?;
                require(
                    value(&p.backend_mut().tree(id)?, "accept")? == true,
                    "lower accept unchanged",
                )?;
                require(
                    value(&p.backend_mut().tree(other)?, "accept")? == false,
                    "upper accept changed",
                )?;
            }
            2 => {
                delivered(p.scroll(&Target::new(id, "list"), -1)?, id)?;
                let tree = p.backend_mut().tree(id)?;
                let row = crate::tree::node(&tree, "list/item-06")?;
                require(
                    row.bounds == [20, 230, 200, 20].map(|v| i64::from(v * scale))
                        && row.clip == [20, 230, 200, 15].map(|v| i64::from(v * scale)),
                    format!("partial row geometry {row:?}"),
                )?;
                let click = p.click(&Target::new(id, "list/item-06"))?;
                let info = p.backend_mut().info(id)?;
                require(
                    click.point.1 >= info.y + 230 * scale && click.point.1 < info.y + 245 * scale,
                    "click outside partial row",
                )?;
                delivered(click.press, id)?;
                delivered(click.release, id)?;
                require(
                    value(&p.backend_mut().tree(id)?, "list")? == "item-06",
                    "clipped item not selected",
                )?;
                let error = p.click(&Target::new(id, "list/item-00")).unwrap_err();
                require(
                    error.contains("list/item-00") && error.contains("no visible area"),
                    error,
                )?;
            }
            3 => {
                delivered(p.scroll(&Target::new(id, "list"), -2)?, id)?;
                let tree = p.backend_mut().tree(id)?;
                require(
                    tree.root.value["pointer"]["wheel"] == -80,
                    format!("wheel amount not delivered: {:?}", tree.root.value),
                )?;
                let row = crate::tree::node(&tree, "list/item-04")?;
                require(
                    row.bounds == [20, 150, 200, 20].map(|v| i64::from(v * scale))
                        && row.flags.contains(&Flag::Visible),
                    "scroll did not move the fourth row to list top",
                )?;
                require(
                    !crate::tree::node(&tree, "list/item-00")?
                        .flags
                        .contains(&Flag::Visible),
                    "scrolled-out row stayed visible",
                )?;
            }
            4 => {
                let before = p.backend_mut().info(id)?;
                let mut expected = Vec::new();
                let start = (before.x + 340 * scale, before.y + 102 * scale);
                let mut physical = start;
                let drag = p.drag(
                    &Target::new(id, "amount"),
                    &[(3 * scale, 0); 10],
                    |index, backend| {
                        if index != 0 {
                            physical.0 += 3 * scale;
                            expected.push((physical.0 - before.x, physical.1 - before.y));
                        }
                        if index == 5 {
                            let from = (before.x + 60 * scale, before.y - 16 * scale);
                            let to = (from.0 + 40 * scale, from.1 + 30 * scale);
                            move_window(
                                &mover,
                                from,
                                to,
                                id,
                                (before.x + 40 * scale, before.y + 30 * scale),
                            )?;
                            physical = to;
                        }
                        let tree = backend.tree(id)?;
                        require(
                            tree.root.value["pointer"]["layout_changes"]
                                == if index < 5 { 1 } else { 2 },
                            format!(
                                "layout-change witness missing at step={index}: {:?}",
                                tree.root.value
                            ),
                        )?;
                        Ok(())
                    },
                )?;
                delivered(drag.press, id)?;
                delivered(drag.release, id)?;
                require(drag.motions.len() == 10, "drag must deliver ten motions")?;
                let after = p.backend_mut().info(id)?;
                require(
                    (after.x, after.y) == (before.x + 40 * scale, before.y + 30 * scale),
                    "window did not move during capture",
                )?;
                let tree = p.backend_mut().tree(id)?;
                let trace = &tree.root.value["pointer"];
                let motions = trace["motions"]
                    .as_array()
                    .ok_or("missing app motion log")?;
                require(
                    motions.len() == 10,
                    format!("app saw {} drag motions", motions.len()),
                )?;
                for (i, (motion, d)) in motions.iter().zip(&drag.motions).enumerate() {
                    delivered(*d, id)?;
                    let (mut x, mut y) = expected[i];
                    if i >= 5 {
                        x -= 40 * scale;
                        y -= 30 * scale;
                    }
                    require(
                        *motion == serde_json::json!([d.serial, x / scale, y / scale]),
                        format!(
                            "motion {i} expected serial={} at=({x},{y}), got={motion}",
                            d.serial
                        ),
                    )?;
                }
                require(
                    trace["presses"] == 1
                        && trace["releases"] == 1
                        && trace["completed"] == 1
                        && trace["held"] == false
                        && trace["captured"] == false,
                    format!("drag completion/held state: {trace}"),
                )?;
                require(
                    !crate::tree::node(&tree, "amount")?
                        .flags
                        .contains(&Flag::Pressed),
                    "slider still pressed",
                )?;
                // Require a sibling click after releasing toolkit and WM capture.
                delivered(p.click(&Target::new(id, "accept"))?.release, id)?;
                require(
                    value(&p.backend_mut().tree(id)?, "accept")? == true,
                    "capture survived drag release",
                )?;
            }
            _ => unreachable!(),
        }
        Ok(())
    })();
    let teardown = session.teardown();
    result?;
    teardown?;
    Ok(())
}
fn inject(mover: &Injector, events: &[(u16, u16, i32)]) -> Result<(), String> {
    let reader = libbreenix::input::Reader::open(true).map_err(|e| e.to_string())?;
    let mut bytes = [0; 160];
    let mut batch = Batch::new(&mut bytes);
    for &(kind, code, value) in events {
        batch
            .event(kind, code, value, 0)
            .map_err(|e| e.to_string())?;
    }
    batch.finish(0).map_err(|e| e.to_string())?;
    mover.inject(&batch).map_err(|e| e.to_string())?;
    let end = Instant::now() + Duration::from_secs(2);
    let mut seen = 0;
    loop {
        let mut buf = [0; 4096];
        match reader.read(&mut buf) {
            Ok(records) => {
                for record in records {
                    let event = record.map_err(|e| e.to_string())?.event;
                    if event.device == mover.id() && event.kind != libbreenix::input::kind::SYN {
                        seen += 1;
                        libuitest::input::wait_pointer_idle(event.serial)?;
                    }
                }
            }
            Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::EAGAIN)) => {}
            Err(e) => return Err(e.to_string()),
        }
        if seen == events.len() {
            return Ok(());
        }
        if Instant::now() >= end {
            return Err("auxiliary pointer source event timeout".into());
        }
        libbreenix::time::sleep_ms(1).map_err(|e| e.to_string())?;
    }
}
fn move_window(
    mover: &Injector,
    from: (i32, i32),
    to: (i32, i32),
    id: u64,
    want: (i32, i32),
) -> Result<(), String> {
    // Separate batches and a compositor watermark prevent home/target coalescing.
    for point in [(i32::MIN / 2, i32::MIN / 2), from] {
        inject(
            mover,
            &[(kind::REL, axis::X, point.0), (kind::REL, axis::Y, point.1)],
        )?;
    }
    let pressed = inject(mover, &[(kind::KEY, key::BUTTON_LEFT, 1)]);
    let moved = pressed.and_then(|_| {
        inject(
            mover,
            &[
                (kind::REL, axis::X, to.0 - from.0),
                (kind::REL, axis::Y, to.1 - from.1),
            ],
        )
    });
    let released = inject(mover, &[(kind::KEY, key::BUTTON_LEFT, 0)]);
    moved?;
    released?;
    let end = Instant::now() + Duration::from_secs(2);
    loop {
        let mut infos = vec![window::abi::WinInfo::default(); 64];
        let n = window::list(&mut infos).map_err(|e| e.to_string())?;
        if infos[..n].iter().any(|w| w.id == id && (w.x, w.y) == want) {
            return Ok(());
        }
        if Instant::now() >= end {
            return Err("title-bar move did not publish expected geometry".into());
        }
        libbreenix::time::sleep_ms(1).map_err(|e| e.to_string())?;
    }
}
pub fn click_node() -> CaseResult {
    run(0).map_err(Into::into)
}
pub fn click_partly_covered() -> CaseResult {
    run(1).map_err(Into::into)
}
pub fn click_clipped() -> CaseResult {
    run(2).map_err(Into::into)
}
pub fn scroll_node() -> CaseResult {
    run(3).map_err(Into::into)
}
pub fn drag_steps() -> CaseResult {
    run(4).map_err(Into::into)
}


fn keyboard_case(which: u8) -> Result<(), String> {
    use libuitest::input::{Keyboard, KeyboardBackend, Modifier};
    let mut session = Session::connect()?;
    session.start_compositor(false)?;
    let result: Result<(),String> = (|| {
        session.launch_app(&[b"/usr/libexec/breenix/uitest-app_test",b"keyboard",b"--widgets"],0)?;
        let id=session.wait_window("keyboard")?.id;
        let mut k=Keyboard::new(LivePointer::new(&mut session)?);
        k.backend_mut().watch(id)?;
        let entry=Target::new(id,"entry");
        k.backend_mut().focus(&entry)?;
        let expected=match which {
            0=>{k.type_text(id,"hello world")?;"hello world"},
            1=>{k.type_text(id,"AZ!@#$%^&*()_+{}:\"~|<>?")?;"AZ!@#$%^&*()_+{}:\"~|<>?"},
            2=>{k.type_text(id,"héλ🙂界")?;"héλ🙂界"},
            3=>{
                k.type_text(id,"one\n\ttwo")?;
                let tree=k.backend_mut().tree(id)?;
                require(crate::tree::node(&tree,"submit")?.flags.contains(&Flag::Focused),"Tab did not focus submit")?;
                k.type_text(id,"\n")?;
                require(k.backend_mut().tree(id)?.root.value["submissions"]==1,"Enter did not activate focused submit")?;
                "one"
            },
            4=>{
                k.type_text(id,"replace me")?;
                k.chord(id,&[Modifier::Ctrl],key::A)?;
                k.type_text(id,"selected")?;
                "selected"
            },
            5=>{
                k.backend_mut().focus(&Target::new(id,"submit"))?;
                k.focus_then_type(&entry,"focused")?;
                for key in ["status","disabled"] {
                    let error=k.focus_then_type(&Target::new(id,key),"BAD").unwrap_err();
                    require(error.contains(key),format!("unnamed focus failure: {error}"))?;
                }
                "focused"
            },
            _=>unreachable!(),
        };
        require(value(&k.backend_mut().tree(id)?,"entry")?==expected,format!("entry value differs from {expected:?}"))?;
        Ok(())
    })();
    let teardown=session.teardown();result?;teardown?;Ok(())
}
pub fn type_ascii()->CaseResult {keyboard_case(0).map_err(Into::into)}
pub fn type_shifted()->CaseResult {keyboard_case(1).map_err(Into::into)}
pub fn type_utf8()->CaseResult {keyboard_case(2).map_err(Into::into)}
pub fn type_control()->CaseResult {keyboard_case(3).map_err(Into::into)}
pub fn chord()->CaseResult {keyboard_case(4).map_err(Into::into)}
pub fn focus_then_type()->CaseResult {keyboard_case(5).map_err(Into::into)}
