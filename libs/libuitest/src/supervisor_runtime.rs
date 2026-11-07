//! Process ownership and local collector handshake for the shared session core.
use libbreenix::{error::Error, errno::Errno, fs, io, process, signal, time, types::Fd};
use breenix_suite::suite::{descendants::descendant_victims, spawn, Run};
use super::devices::{runtime::Devices, RawInput};
use super::{Message, SessionEnvironment, StderrRing};
use super::ownership::{Custody, Event, Owner, Ownership};
use crate::channel::{Channel, now};
use breenix_suite::suite::{collection::Request, result::Accounting};
use std::{collections::BTreeMap, path::Path};
use crate::collect::Evidence;

struct Display;
impl Custody for Display {
    fn take_display(&mut self) -> Result<(), String> { libbreenix::graphics::take_over_display().map_err(|e| e.to_string()) }
    fn claim_compositor(&mut self) -> Result<(), String> { libbreenix::window::claim_compositor().map(|_| ()).map_err(|e| e.to_string()) }
    fn release_compositor(&mut self) -> Result<(), String> { libbreenix::window::release_compositor().map(|_| ()).map_err(|e| e.to_string()) }
    fn give_display(&mut self) -> Result<(), String> { libbreenix::graphics::give_back_display().map_err(|e| e.to_string()) }
}

struct Pipe(Fd);
impl Drop for Pipe { fn drop(&mut self) { let _ = io::close(self.0); } }
fn pipe() -> Result<(Pipe, Pipe), String> {
    let (r, w) = io::pipe2(io::status_flags::O_CLOEXEC).map_err(|e| e.to_string())?;
    let pair = (Pipe(r), Pipe(w));
    io::fcntl_setfl(r, io::status_flags::O_NONBLOCK).map_err(|e| e.to_string())?;
    Ok(pair)
}
struct Output {
    stdout: Option<Pipe>, stderr: Option<Pipe>, stdout_bytes: usize,
    ring: StderrRing, status: Option<i32>,
    stdin: Option<Pipe>,
    startup: Option<Vec<u8>>,
    input_ready: bool,
}
extern "C" {
    fn setgroups(count: usize, groups: *const u32) -> i32;
    fn setresgid(real: u32, effective: u32, saved: u32) -> i32;
    fn setresuid(real: u32, effective: u32, saved: u32) -> i32;
    fn getresgid(real: *mut u32, effective: *mut u32, saved: *mut u32) -> i32;
    fn getresuid(real: *mut u32, effective: *mut u32, saved: *mut u32) -> i32;
    fn setuid(uid: u32) -> i32;
}
fn child_identity(uid: u32) -> Result<(), String> {
    if uid == 0 { return Ok(()); }
    let mut u = [0; 3]; let mut g = [0; 3];
    let [ur, ue, us] = &mut u; let [gr, ge, gs] = &mut g;
    // Clear inherited root groups before permanently giving up root identity.
    let changed = unsafe {
        setgroups(0, core::ptr::null()) == 0 && setresgid(uid, uid, uid) == 0 && setresuid(uid, uid, uid) == 0
            && getresuid(ur, ue, us) == 0
            && getresgid(gr, ge, gs) == 0
    };
    if !changed || u != [uid; 3] || g != [uid; 3] || unsafe { setuid(0) } == 0 {
        return Err("app credential switch failed".into());
    }
    Ok(())
}
fn launch(argv: &[Vec<u8>], env: &[Vec<u8>], uid: u32, kiosk: bool) -> Result<(u64, Output), String> {
    let (r, w) = pipe()?;
    let (err_r, err_w) = pipe()?;
    let null = Pipe(fs::open("/dev/null", fs::O_RDWR).map_err(|e| e.to_string())?);
    let input = if kiosk { Some(pipe()?) } else { None };
    if let Some((read, _)) = &input {
        io::fcntl_setfl(read.0, 0).map_err(|e| e.to_string())?;
    }
    let stdin = input.as_ref().map_or(null.0, |(read, _)| read.0);
    let args: Vec<Vec<u8>> = argv.iter().map(|s| { let mut s = s.clone(); s.push(0); s }).collect();
    let vars: Vec<Vec<u8>> = env.iter().map(|s| { let mut s = s.clone(); s.push(0); s }).collect();
    let mut exec = || {
        if let Err(error) = child_identity(uid) {
            let _ = io::write(Fd::STDERR, error.as_bytes()); return 126;
        }
        let mut ap: Vec<*const u8> = args.iter().map(|s| s.as_ptr()).collect(); ap.push(core::ptr::null());
        let mut ep: Vec<*const u8> = vars.iter().map(|s| s.as_ptr()).collect(); ep.push(core::ptr::null());
        let _ = process::execve(&args[0], ap.as_ptr(), ep.as_ptr());
        127
    };
    let pid = spawn(&[(stdin, Fd::STDIN), (w.0, Fd::STDOUT), (err_w.0, Fd::STDERR)], Run::Call(&mut exec))
        .map_err(|e| e.to_string())?.raw();
    let stdin = if let Some((_, write)) = input {
        // The reply is a barrier after kiosk has opened its scene descriptor.
        // Keep the writer alive: kiosk's normal command loop exits on EOF.
        io::write(write.0, b"auto 1\n").map_err(|e| e.to_string())?;
        Some(write)
    } else { None };
    Ok((pid, Output { stdout: Some(r), stderr: Some(err_r), stdout_bytes: 0, ring: StderrRing::default(), status: None, stdin, startup: kiosk.then(Vec::new), input_ready: false }))
}

fn outputs(outputs: &mut BTreeMap<u64, Output>, channel: &mut Channel, connected: bool) -> Result<(), String> {
    for (&pid, output) in outputs.iter_mut() {
        for stderr in [false, true] {
            // A live flood cannot monopolize the control/collector loop.
            let mut budget = 64 * 1024;
            while budget != 0 {
                let pipe = if stderr { &mut output.stderr } else { &mut output.stdout };
                let Some(fd) = pipe.as_ref().map(|pipe| pipe.0) else { break };
                let mut chunk = [0; 4096];
                match io::read(fd, &mut chunk) {
                    Ok(0) => { pipe.take(); break; }
                    Ok(n) => {
                        budget -= n;
                        if stderr { output.ring.push(&chunk[..n]); }
                        else {
                            if let Some(startup) = &mut output.startup {
                                if startup.len() + n > 128 { return Err("invalid kiosk startup reply".into()); }
                                startup.extend_from_slice(&chunk[..n]);
                                if let Some(at) = startup.iter().position(|&b| b == b'\n') {
                                    output.input_ready = std::str::from_utf8(&startup[..at]).ok()
                                        .and_then(|s| s.strip_prefix("ready "))
                                        .and_then(|s| s.parse::<u64>().ok()).is_some_and(|epoch| epoch != 0);
                                    output.startup = None;
                                }
                            }
                            let keep = n.min((64 * 1024usize).saturating_sub(output.stdout_bytes));
                            output.stdout_bytes += keep;
                            if connected && keep != 0 { channel.queue(&Message::Output { pid, bytes: chunk[..keep].to_vec() })?; }
                        }
                    }
                    Err(Error::Os(Errno::EINTR)) => continue,
                    Err(Error::Os(Errno::EAGAIN)) => break,
                    Err(error) => return Err(error.to_string()),
                }
            }
        }
    }
    Ok(())
}
fn reap(channel: &mut Channel, outputs: &mut BTreeMap<u64, Output>, connected: bool, compositor: Option<u64>, ownership: &mut Ownership, recover: bool) -> Result<bool, String> {
    loop {
        let mut status = 0;
        match process::waitpid(-1, &mut status, process::WNOHANG) {
            Ok(pid) if pid.raw() != 0 => {
                if let Some(output) = outputs.get_mut(&pid.raw()) { output.status = Some(status); output.stdin.take(); }
                if compositor == Some(pid.raw()) {
                    ownership.apply(Event::Died, &mut Display)?;
                    if recover { ownership.apply(Event::Recover, &mut Display)?; }
                }
                // Exit is observable only after the supervisor has recovered custody.
                if connected { channel.queue(&Message::Exited { pid: pid.raw(), status })?; }
            }
            Ok(_) => return Ok(false),
            Err(Error::Os(Errno::EINTR)) => continue,
            Err(Error::Os(Errno::ECHILD)) => return Ok(true),
            Err(error) => return Err(error.to_string()),
        }
    }
}
fn kill_descendants() -> Result<(), String> {
    let own = process::getpid().map_err(|e| e.to_string())?.raw();
    let mut rows = Vec::new();
    for entry in std::fs::read_dir("/proc").map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let Some(pid) = entry.file_name().to_str().and_then(|s| s.parse::<u64>().ok()) else { continue };
        let stat = match std::fs::read_to_string(entry.path().join("stat")) {
            Ok(stat) => stat,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.to_string()),
        };
        let parent = stat.rsplit_once(") ").and_then(|(_, s)| s.split_whitespace().nth(1))
            .and_then(|s| s.parse().ok()).ok_or("bad process parent")?;
        rows.push((pid, parent));
    }
    for pid in descendant_victims(own, &rows) {
        match signal::kill(pid as i32, signal::SIGKILL) {
            Ok(()) | Err(Error::Os(Errno::ESRCH)) => {},
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(())
}
fn teardown(channel: &mut Channel, outputs_state: &mut BTreeMap<u64, Output>, mut connected: bool, compositor: Option<u64>, ownership: &mut Ownership, devices: &mut Option<Devices>) -> Result<(), String> {
    let end = now()? + 1000;
    loop {
        kill_descendants()?;
        outputs(outputs_state, channel, connected)?;
        let empty = reap(channel, outputs_state, connected, compositor, ownership, true)?;
        if ownership.owner() == Owner::SupervisorHolds {
            if let Some(pair) = devices.as_mut() {
                pair.recover(process::getpid().map_err(|e| e.to_string())?.raw())?;
                pair.drain()?;
            }
        }
        outputs(outputs_state, channel, connected)?;
        if connected && channel.flush().is_err() { connected = false; }
        if empty && outputs_state.values().all(|o| o.stdout.is_none() && o.stderr.is_none()) { break; }
        if now()? >= end { return Err("session descendants outlived SIGKILL".into()); }
        time::sleep_ms(1).map_err(|e| e.to_string())?;
    }
    // Reap recovers the compositor claim before releases can reach the console.
    let release = if let Some(mut pair) = devices.take() {
        let grabbed = if ownership.owner() == Owner::SupervisorHolds {
            pair.recover(process::getpid().map_err(|e| e.to_string())?.raw())
        } else { Ok(()) };
        let destroyed = pair.destroy();
        grabbed.and(destroyed)
    } else { Ok(()) };
    let handback = ownership.apply(Event::End, &mut Display);
    release.and(handback)
}

/// Runner callback. Its first operation registers the subreaper; failure refuses
/// startup without launching any process. spawn has already closed runner fds.
pub fn serve(control: Fd, collector: Fd) -> i32 {
    let registered = process::set_child_subreaper(true);
    let mut channel = match Channel::new(control) { Ok(channel) => channel, Err(_) => return 125 };
    if let Err(error) = registered {
        let _ = channel.send(&Message::Error(format!("session subreaper: {error}")));
        return 125;
    }
    let mut output = BTreeMap::new();
    let mut environment: Option<SessionEnvironment> = None;
    let mut compositor = None;
    let mut devices = None;
    let mut ownership = Ownership::default();
    let result = (|| -> Result<(), String> {
        signal::sigaction(signal::SIGPIPE, Some(&signal::Sigaction::ignore()), None).map_err(|e| e.to_string())?;
        io::fcntl_setfl(collector, io::status_flags::O_NONBLOCK).map_err(|e| e.to_string())?;
        let own = process::getpid().map_err(|e| e.to_string())?.raw();
        let mut evidence = Evidence::new()?;
        ownership.apply(Event::Begin, &mut Display)?;
        devices = Some(Devices::create()?);
        channel.send(&Message::Devices(devices.as_ref().unwrap().ids()))?;
        channel.send(&Message::Ready(process::getpid().map_err(|e| e.to_string())?.raw()))?;
        let mut ended = Vec::new();
        let mut stopped = false;
        let mut connected = true;
        let mut stderr_request = None;
        let mut pending_input: Option<(RawInput, i128)> = None;
        loop {
            if !stopped && ownership.owner() != Owner::SupervisorHolds {
                evidence.observe(own, &output.keys().copied().collect())?;
            }
            // Collector is independent of the case: a dead case cannot hold cleanup.
            let mut bytes = [0; 128];
            match io::read(collector, &mut bytes) {
                Ok(0) => return Err("runner collector closed".into()),
                Ok(n) => {
                    ended.extend_from_slice(&bytes[..n]);
                    if ended.len() > 128 { return Err("invalid collector message".into()); }
                    if ended.ends_with(b"\n") {
                        let request = Request::parse(std::str::from_utf8(&ended).map_err(|_| "invalid collector command UTF-8")?)?;
                        outputs(&mut output, &mut channel, false)?;
                        let record = if let Request::Collect(trigger) = request {
                            let suite = std::env::var("BREENIX_COLLECT_SUITE").map_err(|e| e.to_string())?;
                            let case = std::env::var("BREENIX_COLLECT_CASE").map_err(|e| e.to_string())?;
                            Accounting::new(&suite, &case, &[], true)?;
                            let stderr = output.iter().map(|(&pid, app)| (pid, app.ring.dropped(), app.ring.bytes())).collect::<Vec<_>>();
                            Some(evidence.collect(&suite, &case, trigger, &stderr))
                        } else { None };
                        if !stopped { teardown(&mut channel, &mut output, false, compositor, &mut ownership, &mut devices)?; }
                        if let Some(config) = environment.take() { std::fs::remove_dir_all(config.config_root()).map_err(|e| e.to_string())?; }
                        evidence.release_remnants();
                        let reply = format!("{}done\n", record.unwrap_or_default());
                        let mut left = reply.as_bytes();
                        while !left.is_empty() {
                            match io::write(collector, left) {
                                Ok(0) => return Err("collector reply closed".into()),
                                Ok(n) => left = &left[n..],
                                Err(Error::Os(Errno::EINTR | Errno::EAGAIN)) => { time::sleep_ms(1).map_err(|e| e.to_string())?; }
                                Err(error) => return Err(error.to_string()),
                            }
                        }
                        return Ok(());
                    }
                }
                Err(Error::Os(Errno::EINTR | Errno::EAGAIN)) => {},
                Err(error) => return Err(error.to_string()),
            }
            if !stopped {
                // Death is serviced before a queued input request, and re-grab is
                // completed before Exited can be flushed to the case.
                reap(&mut channel, &mut output, connected, compositor, &mut ownership, true)?;
                let pair = devices.as_mut().ok_or("session devices missing")?;
                if ownership.owner() == Owner::SupervisorHolds {
                    pair.recover(process::getpid().map_err(|e| e.to_string())?.raw())?;
                }
                let kiosk_ready = compositor.and_then(|pid| output.get(&pid)).is_some_and(|o| o.input_ready);
                pair.observe(compositor, kiosk_ready)?;
                pair.drain()?;
                match if connected { channel.receive() } else { Ok(None) } {
                    Ok(Some(Message::Launch { argv, env })) => match launch(&argv, &env, 0, false) {
                        Ok((pid, pipe)) => { output.insert(pid, pipe); channel.queue(&Message::Launched(pid))?; }
                        Err(error) => channel.queue(&Message::Error(error))?,
                    },
                    Ok(Some(Message::Configure { case_pid, seed, mode })) => {
                        if environment.is_some() || !output.is_empty() { return Err("session already configured or launched".into()); }
                        let config = SessionEnvironment::new(case_pid, seed, &mode)?;
                        // The parent directory is harness-owned. Refuse a symlink or
                        // an untrusted owner before creating a case-PID directory.
                        use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
                        match std::fs::DirBuilder::new().mode(0o755).create("/tmp/uitest") {
                            Ok(()) => {},
                            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {},
                            Err(e) => return Err(e.to_string()),
                        }
                        let meta = std::fs::symlink_metadata("/tmp/uitest").map_err(|e| e.to_string())?;
                        if !meta.is_dir() || meta.uid() != 0 || meta.mode() & 0o022 != 0 {
                            return Err("untrusted session root parent".into());
                        }
                        std::fs::set_permissions("/tmp/uitest", std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
                        // Keep the root in cleanup state even if setup fails partway.
                        environment = Some(config);
                        let root = Path::new(environment.as_ref().unwrap().config_root());
                        crate::fixtures::prepare(root)?;
                        crate::fixtures::verify(root)?;
                        evidence.configure(root)?;
                        channel.queue(&Message::Configured)?;
                    }
                    Ok(Some(Message::LaunchApp { argv, uid })) => {
                        let env = environment.as_ref().ok_or("session not configured")?.environment();
                        match launch(&argv, &env, uid, false) {
                            Ok((pid, pipe)) => { output.insert(pid, pipe); channel.queue(&Message::Launched(pid))?; }
                            Err(error) => channel.queue(&Message::Error(error))?,
                        }
                    }
                    Ok(Some(Message::StartCompositor { kiosk })) => {
                        if compositor.is_some() { return Err("session compositor already started".into()); }
                        let mut env = environment.as_ref().ok_or("session not configured")?.environment();
                        env.push(devices.as_ref().ok_or("session devices missing")?.environment());
                        let path = if kiosk { "/usr/libexec/breenix/kiosk_test" } else { "/usr/bin/bwm" };
                        let (pid, pipes) = launch(&[path.as_bytes().to_vec()], &env, 0, kiosk)?;
                        compositor = Some(pid);
                        ownership.apply(Event::Started, &mut Display)?;
                        output.insert(pid, pipes);
                        channel.queue(&Message::Launched(pid))?;
                    }
                    Ok(Some(Message::HoldWindow(id))) => {
                        evidence.observe(own, &output.keys().copied().collect())?;
                        if !evidence.holds(id) { channel.queue(&Message::Error(format!("window {id} is not a held session window")))?; }
                        else { channel.queue(&Message::WindowHeld(id))?; }
                    }
                    Ok(Some(Message::Input(event))) => {
                        if pending_input.is_some() { return Err("input request already pending".into()); }
                        event.validate()?;
                        pending_input = Some((event, now()? + 2000));
                    }
                    Ok(Some(Message::ReadStderr(pid))) => {
                        if stderr_request.is_some() || !output.contains_key(&pid) { return Err("invalid app stderr request".into()); }
                        stderr_request = Some(pid);
                    }
                    Ok(Some(Message::Teardown)) => { teardown(&mut channel, &mut output, true, compositor, &mut ownership, &mut devices)?;
                        if let Some(config) = environment.take() { std::fs::remove_dir_all(config.config_root()).map_err(|e| e.to_string())?; }
                        stopped = true; channel.send(&Message::Stopped)?; }
                    Ok(Some(_)) => return Err("unexpected case control message".into()),
                    Ok(None) => {},
                    // Case exited: keep the collector alive until runner tells us to end.
                    Err(error) if error == "session supervisor/control peer closed" => connected = false,
                    Err(error) => return Err(error),
                }
                outputs(&mut output, &mut channel, connected)?;
                if let Some((event, end)) = pending_input {
                    if stopped { pending_input = None; }
                    else if devices.as_mut().ok_or("session devices missing")?.inject(event)? {
                        pending_input = None;
                        channel.queue(&Message::InputDone)?;
                    } else if now()? >= end { return Err("input paused awaiting compositor grab".into()); }
                }
                if let Some(pid) = stderr_request {
                    let app = output.get(&pid).ok_or("unknown app stderr pid")?;
                    // A live snapshot is the ring now. After exit, finish draining
                    // stderr first; inherited pipes do not delay the exit event.
                    if app.status.is_none() || app.stderr.is_none() {
                        let bytes = app.ring.bytes();
                        if bytes.is_empty() {
                            channel.queue(&Message::Stderr { pid, dropped: app.ring.dropped(), bytes, done: true })?;
                        } else {
                            for (index, chunk) in bytes.chunks(4096).enumerate() {
                                channel.queue(&Message::Stderr { pid, dropped: app.ring.dropped(), bytes: chunk.to_vec(), done: (index+1)*4096 >= bytes.len() })?;
                            }
                        }
                        stderr_request = None;
                    }
                }
                if connected && channel.flush().is_err() { connected = false; }
            }
            time::sleep_ms(1).map_err(|e| e.to_string())?;
        }
    })();
    if let Err(error) = &result { let _ = channel.send(&Message::Error(error.clone())); }
    let cleanup = if result.is_err() && !matches!(ownership.owner(), Owner::Initial | Owner::CaseEnd | Owner::RunnerHolds) {
        teardown(&mut channel, &mut output, false, compositor, &mut ownership, &mut devices)
    } else { Ok(()) };
    let device_cleanup = if let Some(pair) = devices { pair.destroy() } else { Ok(()) };
    let root_cleanup = if let Some(config) = environment {
        std::fs::remove_dir_all(config.config_root()).map_err(|e| e.to_string())
    } else { Ok(()) };
    let _ = io::close(collector);
    i32::from(result.is_err() || cleanup.is_err() || device_cleanup.is_err() || root_cleanup.is_err())
}
