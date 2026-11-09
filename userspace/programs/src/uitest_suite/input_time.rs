//! Input timing observed at the client in a stepped desktop session.
use breenix_suite::suite::{check, CaseResult};
use libbreenix::{input::key, time};
use libuitest::{
    input::{self, KeyboardBackend, LivePointer, Pointer, Target},
    Session,
};

pub fn hold_repeat() -> CaseResult {
    let mut session = Session::desktop()?;
    session.launch_app(
        &[
            b"/usr/libexec/breenix/uitest-app_test",
            b"held input",
            b"--widgets",
        ],
        0,
    )?;
    let id = session.wait_window("held input")?.id;
    let mut live = LivePointer::new(&mut session)?;
    live.watch(id)?;
    live.focus(&Target::new(id, "entry"))?;
    let press = live.send_key(key::A, true)?;
    input::delivered(press.serial)?;
    live.session().step(600)?;
    let deadline = time::now_monotonic().map_err(|e| e.to_string())?.as_nanos() + 2_000_000_000;
    loop {
        let tree = live.tree(id)?;
        let value = &crate::tree::node(&tree, "entry")?.value;
        if value == "aaaa" {
            break;
        }
        check(
            value == "a" || value == "aa" || value == "aaa",
            &format!("unexpected repeat result {value}"),
        )?;
        check(
            time::now_monotonic().map_err(|e| e.to_string())?.as_nanos() < deadline,
            "three repeats did not arrive",
        )?;
        time::sleep_ms(1).map_err(|e| e.to_string())?;
    }
    let release = live.send_key(key::A, false)?;
    input::delivered(release.serial)?;
    check(
        crate::tree::node(&live.tree(id)?, "entry")?.value == "aaaa",
        "repeat count after release",
    )?;
    drop(live);
    session.teardown()?;
    Ok(())
}

fn clicks(check_window: bool) -> CaseResult {
    let mut session = Session::desktop()?;
    let pid = session.launch_app(
        &[
            b"/usr/libexec/breenix/uitest-app_test",
            b"timed clicks",
            b"--pointer-probe",
        ],
        0,
    )?;
    let id = session.wait_window("timed clicks")?.id;
    let mut pointer = Pointer::new(LivePointer::new(&mut session)?);
    pointer.backend_mut().watch(id)?;
    let target = Target::new(id, "accept");
    let first = pointer.click(&target)?;
    if check_window {
        // Real waiting cannot age a click while the session clock is frozen.
        time::sleep_ms(450).map_err(|e| e.to_string())?;
    }
    pointer.backend_mut().session().step(300)?;
    let second = pointer.click(&target)?;
    for (serial, stamp, count, pressed) in [
        (first.press.serial, 0, 1, true),
        (first.release.serial, 0, 1, false),
        (second.press.serial, 300_000_000, 2, true),
        (second.release.serial, 300_000_000, 2, false),
    ] {
        let expected = format!(
            "UITEST-POINTER serial={serial} time_ns={stamp} clicks={count} pressed={pressed}"
        );
        loop {
            let line = pointer.backend_mut().session().read_line(pid)?;
            if line == expected {
                break;
            }
            check(
                !line.starts_with("UITEST-POINTER "),
                &format!("expected {expected}, got {line}"),
            )?;
        }
    }
    if check_window {
        pointer.backend_mut().session().step(401)?;
        let third = pointer.click(&target)?;
        for (serial, pressed) in [(third.press.serial, true), (third.release.serial, false)] {
            let expected = format!(
                "UITEST-POINTER serial={serial} time_ns=701000000 clicks=1 pressed={pressed}"
            );
            loop {
                let line = pointer.backend_mut().session().read_line(pid)?;
                if line == expected {
                    break;
                }
                check(
                    !line.starts_with("UITEST-POINTER "),
                    &format!("expected {expected}, got {line}"),
                )?;
            }
        }
    }
    drop(pointer);
    session.teardown()?;
    Ok(())
}
pub fn double_click() -> CaseResult {
    clicks(false)
}
pub fn input_time() -> CaseResult {
    clicks(true)
}
