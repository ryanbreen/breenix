//! Delivery and isolation cases observe committed app and compositor trees.
use breenix_suite::suite::CaseResult;
use libbreenix::{
    input::{self, key, kind, Batch, Capability, Injector, Reader},
    process, window,
};
use libuitest::{
    input::{self as script, Keyboard, KeyboardBackend, LivePointer, Target},
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
fn entry(live: &LivePointer<'_>, id: u64) -> Result<serde_json::Value, String> {
    Ok(crate::tree::node(&live.tree(id)?, "entry")?.value.clone())
}
fn launch(session: &mut Session, pair: bool) -> Result<u64, String> {
    let mut args: Vec<&[u8]> = vec![
        b"/usr/libexec/breenix/uitest-app_test",
        b"delivery",
        b"--widgets",
    ];
    if pair {
        args.push(b"--two-windows");
    }
    let pid = session.launch_app(&args, 0)?;
    let id = session.wait_window("delivery")?.id;
    require(
        u64::from(window_info(id)?.owner) == pid,
        "delivery window owner differs from app",
    )?;
    Ok(id)
}
fn window_info(id: u64) -> Result<window::abi::WinInfo, String> {
    let mut rows = vec![window::abi::WinInfo::default(); 64];
    let n = window::list(&mut rows).map_err(|e| e.to_string())?;
    rows.get(..n)
        .ok_or("registry overflow")?
        .iter()
        .find(|w| w.id == id)
        .copied()
        .ok_or("window missing".into())
}
fn run(which: u8) -> Result<(), String> {
    let mut session = Session::desktop()?;
    let result: Result<(), String> = (|| {
        let id = launch(&mut session, which == 3)?;
        let second = if which == 3 {
            Some(session.wait_window("delivery second")?.id)
        } else {
            None
        };
        let mut live = LivePointer::new(&mut session)?;
        live.watch(id)?;
        live.focus(&Target::new(id, "entry"))?;
        match which {
            0 => {
                let event = live.send_key(key::A, true)?;
                let done = script::delivered(event.serial)?;
                require(
                    done.window == id && done.serial == event.serial,
                    format!("delivery {done:?} expected={id}"),
                )?;
                let (commit, meta) = window::read_meta(id).map_err(|e| e.to_string())?;
                require(
                    script::delivery_ready(commit, meta.processed_serial, event.serial),
                    "delivered returned before processed commit",
                )?;
                let release = live.send_key(key::A, false)?;
                script::delivered(release.serial)?;
                let (commit, meta) = window::read_meta(id).map_err(|e| e.to_string())?;
                require(
                    script::delivery_ready(commit, meta.processed_serial, release.serial),
                    "release returned before processed commit",
                )?;
                require(
                    entry(&live, id)? == "a",
                    "delivered did not publish entry value",
                )?;
            }
            1 => {
                let mut keyboard = Keyboard::new(live);
                keyboard.type_text(id, "aZλ")?;
                keyboard.chord(id, &[script::Modifier::Ctrl], key::A)?;
                keyboard.type_text(id, "done")?;
                live = keyboard.into_backend();
                require(
                    entry(&live, id)? == "done",
                    "disposition script did not execute",
                )?;
                require(
                    !live.serials().is_empty(),
                    "script recorded no source serials",
                )?;
                script::dispositions(live.serials())?;
            }
            2 => {
                let mut keyboard = Keyboard::new(live);
                keyboard.type_text(id, "root")?;
                live = keyboard.into_backend();
                require(
                    entry(&live, id)? == "root",
                    "root injection did not execute before UID drop",
                )?;
                nonroot(live.session())?;
                let mut keyboard = Keyboard::new(live);
                keyboard.type_text(id, "b")?;
                live = keyboard.into_backend();
                require(
                    entry(&live, id)? == "rootb",
                    "non-root input changed the root session or blocked recovery",
                )?;
            }
            3 => {
                let other = second.unwrap();
                require(
                    window_info(id)?.owner == window_info(other)?.owner,
                    "two windows must belong to one app",
                )?;
                live.watch(other)?;
                let mut keyboard = Keyboard::new(live);
                keyboard.focus_then_type(&Target::new(id, "entry"), "first")?;
                keyboard.focus_then_type(&Target::new(other, "entry"), "second")?;
                keyboard.focus_then_type(&Target::new(id, "entry"), "!")?;
                live = keyboard.into_backend();
                require(
                    entry(&live, id)? == "first!" && entry(&live, other)? == "second",
                    "input crossed the two app windows",
                )?;
            }
            4 => {
                require(
                    !live
                        .desktop_tree()?
                        .children
                        .iter()
                        .any(|n| n.name == "Launcher"),
                    "launcher open before hotkey",
                )?;
                for pressed in [true, false, true, false] {
                    live.wm_key(key::LEFT_SUPER, pressed)?;
                }
                let end = Instant::now() + Duration::from_secs(2);
                loop {
                    let tree = live.desktop_tree()?;
                    if let Some(node) = tree.children.iter().find(|n| n.name == "Launcher") {
                        require(
                            node.flags.contains(&libbui::tree::Flag::Visible),
                            "launcher tree node not visible",
                        )?;
                        break;
                    }
                    if Instant::now() >= end {
                        return Err("Super double tap did not open Launcher in bwm tree".into());
                    }
                    libbreenix::time::sleep_ms(1).map_err(|e| e.to_string())?;
                }
            }
            5 => excluded(&mut live, id)?,
            _ => unreachable!(),
        }
        script::dispositions(live.serials())?;
        Ok(())
    })();
    let teardown = session.teardown();
    result?;
    teardown?;
    Ok(())
}

fn nonroot(session: &mut Session) -> Result<(), String> {
    use libbreenix::{errno::Errno, error::Error};
    let device = Injector::create(
        &[Capability {
            kind: kind::KEY,
            code: 45,
            minimum: 0,
            maximum: 1,
        }],
        true,
    )
    .map_err(|e| e.to_string())?;
    match process::fork().map_err(|e| e.to_string())? {
        process::ForkResult::Child => {
            extern "C" {
                fn setresuid(r: u32, e: u32, s: u32) -> i32;
                fn geteuid() -> u32;
            }
            if unsafe { setresuid(1000, 1000, 1000) } != 0 || unsafe { geteuid() } != 1000 {
                process::exit(31);
            }
            let mut bytes = [0; 64];
            let mut batch = Batch::new(&mut bytes);
            if batch.event(kind::KEY, 45, 1, 0).is_err() || batch.finish(0).is_err() {
                process::exit(32);
            }
            let inherited = matches!(
                device.inject(&batch),
                Err(Error::Os(Errno::EPERM | Errno::EACCES))
            );
            let fresh = matches!(
                Injector::create(
                    &[Capability {
                        kind: kind::KEY,
                        code: 45,
                        minimum: 0,
                        maximum: 1
                    }],
                    true
                ),
                Err(Error::Os(Errno::EPERM | Errno::EACCES))
            );
            let control = session.raw_press(45).is_err_and(|e| e == "EPERM")
                && session.raw_motion(4, 5).is_err_and(|e| e == "EPERM")
                && session.raw_text("BAD").is_err_and(|e| e == "EPERM");
            process::exit(if inherited && fresh && control { 0 } else { 33 });
        }
        process::ForkResult::Parent(pid) => {
            let mut status = 0;
            process::waitpid(pid.raw() as i32, &mut status, 0).map_err(|e| e.to_string())?;
            require(
                process::wifexited(status) && process::wexitstatus(status) == 0,
                format!("non-root injection refusal status={status}"),
            )
        }
    }
}

fn outside(device: &Injector, code: u16, pressed: bool) -> Result<input::Event, String> {
    let reader = Reader::open(true).map_err(|e| e.to_string())?;
    let mut bytes = [0; 64];
    let mut batch = Batch::new(&mut bytes);
    batch
        .event(kind::KEY, code, i32::from(pressed), 0)
        .map_err(|e| e.to_string())?;
    batch.finish(0).map_err(|e| e.to_string())?;
    device.inject(&batch).map_err(|e| e.to_string())?;
    let end = Instant::now() + Duration::from_secs(2);
    loop {
        let mut bytes = [0; 4096];
        match reader.read(&mut bytes) {
            Ok(records) => {
                for record in records {
                    let e = record.map_err(|e| e.to_string())?.event;
                    if e.device == device.id()
                        && e.kind == kind::KEY
                        && e.code == code
                        && e.value == i32::from(pressed)
                    {
                        let ledger = std::fs::read_to_string("/proc/breenix/input-ledger")
                            .map_err(|e| e.to_string())?;
                        let row = script::ledger_entry(&ledger, e.serial)?
                            .ok_or("unselected source has no terminal disposition")?;
                        require(
                            row.disposition == "rejected" && row.reason == "not-selected",
                            format!("unselected source {row:?}"),
                        )?;
                        let label = format!("q-device-{}-key-{code}-{}-{}-{}", e.device,
                            if pressed { "down" } else { "up" }, row.disposition, row.reason);
                        breenix_suite::suite::value(&label,
                            i64::try_from(e.serial).map_err(|_| "input serial exceeds evidence range")?, "serial", None);
                        return Ok(e);
                    }
                }
            }
            Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::EAGAIN)) => {}
            Err(e) => return Err(e.to_string()),
        }
        if Instant::now() >= end {
            return Err("unselected keyboard source missing".into());
        }
        libbreenix::time::sleep_ms(1).map_err(|e| e.to_string())?;
    }
}
fn excluded(live: &mut LivePointer<'_>, id: u64) -> Result<(), String> {
    let q = Injector::create(
        &[key::LEFT_SHIFT, 45].map(|code| Capability {
            kind: kind::KEY,
            code,
            minimum: 0,
            maximum: 1,
        }),
        true,
    )
    .map_err(|e| e.to_string())?;
    // plan.md 22.5: Q holds Shift, K types a, Q releases, Q types x, K types b.
    outside(&q, key::LEFT_SHIFT, true)?;
    let a = live.send_key(key::A, true)?;
    require(
        a.flags & u32::from(input::modifier::SHIFT) == 0,
        "unselected Shift leaked into selected a flags",
    )?;
    breenix_suite::suite::value("selected-a-modifiers", i64::from(a.flags), "flags", None);
    script::delivered(a.serial)?;
    let release = live.send_key(key::A, false)?;
    script::delivered(release.serial)?;
    require(
        entry(live, id)? == "a",
        "K did not type lowercase a while Q held Shift",
    )?;
    outside(&q, key::LEFT_SHIFT, false)?;
    outside(&q, 45, true)?;
    outside(&q, 45, false)?;
    require(entry(live, id)? == "a", "Q changed selected entry")?;
    let b = live.send_key(key::B, true)?;
    script::delivered(b.serial)?;
    let release = live.send_key(key::B, false)?;
    script::delivered(release.serial)?;
    let field = entry(live, id)?;
    require(
        field == "ab",
        "K did not type ab after excluded Q events",
    )?;
    let bytes = field.as_str().ok_or("entry value is not text")?.as_bytes();
    breenix_suite::suite::value("field-length", bytes.len() as i64, "bytes", Some((2, 2)));
    breenix_suite::suite::value("field-byte-a", i64::from(bytes[0]), "byte", Some((97, 97)));
    breenix_suite::suite::value("field-byte-b", i64::from(bytes[1]), "byte", Some((98, 98)));
    Ok(())
}

pub fn delivered() -> CaseResult {
    run(0).map_err(Into::into)
}
pub fn dispositions_all() -> CaseResult {
    run(1).map_err(Into::into)
}
pub fn nonroot_denied() -> CaseResult {
    run(2).map_err(Into::into)
}
pub fn two_windows() -> CaseResult {
    run(3).map_err(Into::into)
}
pub fn wm_hotkey_scripted() -> CaseResult {
    run(4).map_err(Into::into)
}
pub fn real_input_excluded() -> CaseResult {
    run(5).map_err(Into::into)
}
