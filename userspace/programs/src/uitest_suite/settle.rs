//! App acknowledgments witnessed independently from accepted commit metadata.
use breenix_suite::suite::CaseResult;
use libbreenix::{
    input::key,
    time,
    window::{self, abi},
};
use libuitest::{
    input::{self, LivePointer},
    settle::{self, Barrier, Observation, Predicate, Window},
    Session,
};
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    time::{Duration, Instant},
};

fn require(ok: bool, message: impl Into<String>) -> Result<(), String> {
    if ok {
        Ok(())
    } else {
        Err(message.into())
    }
}
struct Control(BufReader<UnixStream>);
impl Control {
    fn line(&mut self) -> Result<String, String> {
        let mut line = String::new();
        if self.0.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
            return Err("settle app closed control channel".into());
        }
        Ok(line.trim_end().into())
    }
    fn command(&mut self, command: &str) -> Result<(), String> {
        writeln!(self.0.get_mut(), "{command}").map_err(|e| e.to_string())?;
        loop {
            let line = self.line()?;
            if line == format!("ok {command}") {
                return Ok(());
            }
            require(
                line.starts_with("drained "),
                format!("unexpected control reply {line}"),
            )?;
        }
    }
    fn drained(&mut self, source: u64) -> Result<(), String> {
        loop {
            let line = self.line()?;
            let serial = line
                .strip_prefix("drained ")
                .and_then(|s| s.parse::<u64>().ok())
                .ok_or_else(|| format!("invalid drain witness {line}"))?;
            if libbreenix::input::abi::source_order(serial).is_some_and(|(s, _)| s == source) {
                breenix_suite::suite::value("drained-source", source as i64, "serial", None);
                return Ok(());
            }
        }
    }
}

fn launch(session: &mut Session, drain: bool) -> Result<(Control, u64), String> {
    let path = std::path::Path::new(session.config_root()).join("settle-control.sock");
    let listener = UnixListener::bind(&path).map_err(|e| e.to_string())?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let mut args: Vec<&[u8]> = vec![
        b"/usr/libexec/breenix/uitest-app_test",
        b"settle app",
        b"--settle-control",
    ];
    if drain {
        args.push(b"--drain-no-render");
    }
    session.launch_app(&args, 0)?;
    let end = Instant::now() + Duration::from_secs(2);
    let socket = loop {
        match listener.accept() {
            Ok((socket, _)) => break socket,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e.to_string()),
        }
        require(Instant::now() < end, "settle app did not connect")?;
        time::sleep_ms(1).map_err(|e| e.to_string())?;
    };
    socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|e| e.to_string())?;
    socket
        .set_write_timeout(Some(Duration::from_secs(2)))
        .map_err(|e| e.to_string())?;
    let mut control = Control(BufReader::new(socket));
    let ready = control.line()?;
    let id = ready
        .strip_prefix("ready ")
        .and_then(|s| s.parse::<u64>().ok())
        .ok_or("settle ready missing window")?;
    require(
        session.wait_window("settle app")?.id == id,
        "settle control names another window",
    )?;
    Ok((control, id))
}
fn snapshot(id: u64) -> Result<Window, String> {
    let (commit, meta) = window::read_meta(id).map_err(|e| e.to_string())?;
    Ok(Window {
        id,
        commit,
        processed_serial: meta.processed_serial,
    })
}
fn pixels(id: u64) -> Result<u64, String> {
    let mut infos = vec![abi::WinInfo::default(); 64];
    let n = window::list(&mut infos).map_err(|e| e.to_string())?;
    infos
        .get(..n)
        .ok_or("settle registry overflow")?
        .iter()
        .find(|w| w.id == id)
        .map(|w| w.pixel_gen)
        .ok_or("settle window absent".into())
}
fn tree(id: u64) -> Result<libuitest::tree::Tree, String> {
    let mut pixels = vec![0; 320 * 200 * 4];
    let mut tree = vec![0; abi::limits::DEFAULT_TREE_KIB as usize * 1024];
    window::capture(id, &mut pixels, Some(&mut tree)).map_err(|e| e.to_string())?;
    libuitest::tree::Tree::parse(&tree)
}
fn routed(serial: u64, id: u64) -> Result<(), String> {
    input::wait_pointer_idle(serial)?;
    let ledger =
        std::fs::read_to_string("/proc/breenix/input-ledger").map_err(|e| e.to_string())?;
    let row = input::ledger_entry(&ledger, serial)?.ok_or("settle source disposition missing")?;
    require(
        row.disposition == "delivered" && row.window == id,
        format!("settle source routed to {row:?}"),
    )
}
fn timeout(result: Result<(), String>, id: u64, serial: u64, began: Instant) -> Result<(), String> {
    let error = result
        .err()
        .ok_or("settle accepted an uncommitted serial")?;
    require(
        error.starts_with("settle-timeout ")
            && error.contains(&format!("window={id}"))
            && error.contains(&format!("serial={serial}")),
        format!("unnamed settle timeout {error}"),
    )?;
    // The exact boundary is judged off-hardware in ui_settle; here require that
    // the negative actually spent its real bound, with the UI clock still frozen.
    require(
        began.elapsed() >= Duration::from_secs(2),
        "settle timed out before its real bound",
    )?;
    Ok(())
}

fn run(which: u8) -> Result<(), String> {
    let mut session = Session::desktop()?;
    let result: Result<(), String> = (|| {
        let (mut control, id) = launch(&mut session, which == 2)?;
        let mut live = LivePointer::new(&mut session)?;
        settle::settle(&[(id, 0)], &mut [])?;
        match which {
            0 => {
                control.command("hold")?;
                let before = snapshot(id)?;
                let generation = pixels(id)?;
                settle::settle(&[(id, 0)], &mut [])?;
                require(
                    snapshot(id)? == before && pixels(id)? == generation,
                    "idle settle changed the app",
                )?;
            }
            1 | 2 | 3 | 6 => {
                control.command(if which == 3 {
                    "no-change"
                } else if which == 6 {
                    "hang"
                } else {
                    "hold"
                })?;
                let before = snapshot(id)?;
                let generation = pixels(id)?;
                let event = live.send_key(key::A, true)?;
                routed(event.serial, id)?;
                if which != 6 {
                    control.drained(event.serial)?;
                }
                let consumed = snapshot(id)?;
                require(
                    !input::delivery_ready(
                        consumed.commit,
                        consumed.processed_serial,
                        event.serial,
                    ),
                    "draining alone acknowledged the serial",
                )?;
                let mut barrier = Barrier::new(0, vec![(id, event.serial)], vec![]);
                for now in [1, 2] {
                    require(
                        !barrier.observe(now, &Observation::new(vec![consumed.clone()], vec![]))?,
                        "settle accepted the drain witness without a commit",
                    )?;
                }
                if which == 2 || which == 6 {
                    let began = Instant::now();
                    timeout(
                        settle::settle(&[(id, event.serial)], &mut []),
                        id,
                        event.serial,
                        began,
                    )?;
                    require(
                        snapshot(id)? == consumed,
                        "unresponsive app unexpectedly committed",
                    )?;
                } else {
                    let mut polls = 0;
                    let mut release = || {
                        polls += 1;
                        if polls == 2 {
                            control.command(if which == 3 { "ack" } else { "pixel" })?;
                        }
                        Ok(true)
                    };
                    settle::settle(
                        &[(id, event.serial)],
                        &mut [Predicate {
                            name: "release-commit",
                            check: &mut release,
                        }],
                    )?;
                    require(
                        polls >= 3,
                        "settle returned before the controlled acknowledgment",
                    )?;
                    let after = snapshot(id)?;
                    require(
                        after.commit > before.commit
                            && input::delivery_ready(
                                after.commit,
                                after.processed_serial,
                                event.serial,
                            ),
                        "settle returned without a processed commit",
                    )?;
                    require(
                        if which == 3 {
                            pixels(id)? == generation
                        } else {
                            pixels(id)? > generation
                        },
                        "pixel generation disagrees with acknowledgment kind",
                    )?;
                }
                // Sending a release does not depend on the hung client's processing.
                live.send_key(key::A, false)?;
            }
            4 => {
                control.command("hold")?;
                require(
                    tree(id)?.root.value == "Waiting",
                    "async status did not start pending",
                )?;
                let mut polls = 0;
                let mut saw_pending = false;
                let mut saved = || {
                    polls += 1;
                    let holds = tree(id)?.root.value == "Saved";
                    saw_pending |= !holds;
                    if polls == 2 {
                        control.command("save")?;
                    }
                    Ok(holds)
                };
                settle::settle(
                    &[(id, 0)],
                    &mut [Predicate {
                        name: "status-Saved",
                        check: &mut saved,
                    }],
                )?;
                require(
                    saw_pending && polls >= 3 && tree(id)?.root.value == "Saved",
                    "settle did not wait for committed async tree",
                )?;
            }
            5 => {
                control.command("hold")?;
                let missing = u64::MAX - 1;
                let began = Instant::now();
                timeout(settle::settle(&[(missing, 0)], &mut []), missing, 0, began)?;
                let mut saved = || Ok(tree(id)?.root.value == "Saved");
                let began = Instant::now();
                let error = settle::settle(
                    &[(id, 0)],
                    &mut [Predicate {
                        name: "status-Saved",
                        check: &mut saved,
                    }],
                )
                .err()
                .ok_or("settle accepted a false predicate")?;
                require(
                    error.starts_with("settle-timeout ")
                        && error.contains("predicate=\"status-Saved\"")
                        && began.elapsed() >= Duration::from_secs(2),
                    format!("unnamed predicate timeout {error}"),
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
pub fn idle() -> CaseResult {
    run(0).map_err(Into::into)
}
pub fn processed_not_consumed() -> CaseResult {
    run(1).map_err(Into::into)
}
pub fn drained_not_rendered() -> CaseResult {
    run(2).map_err(Into::into)
}
pub fn no_change_ack() -> CaseResult {
    run(3).map_err(Into::into)
}
pub fn async_predicate() -> CaseResult {
    run(4).map_err(Into::into)
}
pub fn timeout_names() -> CaseResult {
    run(5).map_err(Into::into)
}
pub fn undelivered_fails() -> CaseResult {
    run(6).map_err(Into::into)
}
