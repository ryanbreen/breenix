//! Process ownership for the shared session core. Evidence collection is layered
//! onto the separate collector socket by the collection unit.
use libbreenix::{error::Error, errno::Errno, fs, io, process, signal, time, types::Fd};
use breenix_suite::suite::{descendants::descendant_victims, spawn, Run};
use super::{Message, SessionEnvironment, StderrRing};
use crate::channel::{Channel, now};
use std::{collections::BTreeMap, path::Path};

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
    Ok((pid, Output { stdout: Some(r), stderr: Some(err_r), stdout_bytes: 0, ring: StderrRing::default(), status: None, stdin }))
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
fn reap(channel: &mut Channel, outputs: &mut BTreeMap<u64, Output>, connected: bool) -> Result<bool, String> {
    loop {
        let mut status = 0;
        match process::waitpid(-1, &mut status, process::WNOHANG) {
            Ok(pid) if pid.raw() != 0 => {
                if let Some(output) = outputs.get_mut(&pid.raw()) { output.status = Some(status); output.stdin.take(); }
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
fn teardown(channel: &mut Channel, outputs_state: &mut BTreeMap<u64, Output>, mut connected: bool) -> Result<(), String> {
    let end = now()? + 1000;
    loop {
        kill_descendants()?;
        outputs(outputs_state, channel, connected)?;
        let empty = reap(channel, outputs_state, connected)?;
        outputs(outputs_state, channel, connected)?;
        if connected && channel.flush().is_err() { connected = false; }
        if empty && outputs_state.values().all(|o| o.stdout.is_none() && o.stderr.is_none()) { break; }
        if now()? >= end { return Err("session descendants outlived SIGKILL".into()); }
        time::sleep_ms(1).map_err(|e| e.to_string())?;
    }
    Ok(())
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
    let result = (|| -> Result<(), String> {
        signal::sigaction(signal::SIGPIPE, Some(&signal::Sigaction::ignore()), None).map_err(|e| e.to_string())?;
        io::fcntl_setfl(collector, io::status_flags::O_NONBLOCK).map_err(|e| e.to_string())?;
        channel.send(&Message::Ready(process::getpid().map_err(|e| e.to_string())?.raw()))?;
        let mut ended = Vec::new();
        let mut stopped = false;
        let mut connected = true;
        let mut stderr_request = None;
        loop {
            // Collector is independent of the case: a dead case cannot hold cleanup.
            let mut bytes = [0; 128];
            match io::read(collector, &mut bytes) {
                Ok(0) => return Err("runner collector closed".into()),
                Ok(n) => {
                    ended.extend_from_slice(&bytes[..n]);
                    if ended.len() > 128 { return Err("invalid collector message".into()); }
                    if ended.ends_with(b"\n") {
                        if ended != b"end discard\n" { return Err("unsupported collector request in session core".into()); }
                        if !stopped { teardown(&mut channel, &mut output, false)?; }
                        if let Some(config) = environment.take() { std::fs::remove_dir_all(config.config_root()).map_err(|e| e.to_string())?; }
                        io::write(collector, b"done\n").map_err(|e| e.to_string())?;
                        return Ok(());
                    }
                }
                Err(Error::Os(Errno::EINTR | Errno::EAGAIN)) => {},
                Err(error) => return Err(error.to_string()),
            }
            if !stopped {
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
                        let env = environment.as_ref().ok_or("session not configured")?.environment();
                        let path = if kiosk { "/usr/libexec/breenix/kiosk_test" } else { "/usr/bin/bwm" };
                        let (pid, pipes) = launch(&[path.as_bytes().to_vec()], &env, 0, kiosk)?;
                        compositor = Some(pid);
                        output.insert(pid, pipes);
                        channel.queue(&Message::Launched(pid))?;
                    }
                    Ok(Some(Message::ReadStderr(pid))) => {
                        if stderr_request.is_some() || !output.contains_key(&pid) { return Err("invalid app stderr request".into()); }
                        stderr_request = Some(pid);
                    }
                    Ok(Some(Message::Teardown)) => { teardown(&mut channel, &mut output, true)?;
                        if let Some(config) = environment.take() { std::fs::remove_dir_all(config.config_root()).map_err(|e| e.to_string())?; }
                        stopped = true; channel.send(&Message::Stopped)?; }
                    Ok(Some(_)) => return Err("unexpected case control message".into()),
                    Ok(None) => {},
                    // Case exited: keep the collector alive until runner tells us to end.
                    Err(error) if error == "session supervisor/control peer closed" => connected = false,
                    Err(error) => return Err(error),
                }
                outputs(&mut output, &mut channel, connected)?;
                reap(&mut channel, &mut output, connected)?;
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
    let cleanup = if result.is_err() { teardown(&mut channel, &mut output, false) } else { Ok(()) };
    let root_cleanup = if let Some(config) = environment {
        std::fs::remove_dir_all(config.config_root()).map_err(|e| e.to_string())
    } else { Ok(()) };
    let _ = io::close(collector);
    i32::from(result.is_err() || cleanup.is_err() || root_cleanup.is_err())
}
