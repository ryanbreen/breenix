//! Process ownership for the shared session core. Evidence collection is layered
//! onto the separate collector socket by the collection unit.
use libbreenix::{error::Error, errno::Errno, fs, io, process, signal, time, types::Fd};
use breenix_suite::suite::{descendants::descendant_victims, spawn, Run};
use super::Message;
use crate::channel::{Channel, now};

struct Output { pid: u64, fd: Fd, bytes: usize }
impl Drop for Output { fn drop(&mut self) { let _ = io::close(self.fd); } }

fn launch(argv: &[Vec<u8>], env: &[Vec<u8>]) -> Result<(u64, Output), String> {
    let (r, w) = io::pipe2(io::status_flags::O_CLOEXEC).map_err(|e| e.to_string())?;
    let mut reader = Output { pid: 0, fd: r, bytes: 0 };
    if let Err(error) = io::fcntl_setfl(r, io::status_flags::O_NONBLOCK) { let _ = io::close(w); return Err(error.to_string()); }
    let null = match fs::open("/dev/null", fs::O_RDWR) {
        Ok(fd) => fd,
        Err(error) => { let _ = io::close(w); return Err(error.to_string()); }
    };
    let args: Vec<Vec<u8>> = argv.iter().map(|s| { let mut s = s.clone(); s.push(0); s }).collect();
    let vars: Vec<Vec<u8>> = env.iter().map(|s| { let mut s = s.clone(); s.push(0); s }).collect();
    let mut exec = || {
        let mut ap: Vec<*const u8> = args.iter().map(|s| s.as_ptr()).collect(); ap.push(core::ptr::null());
        let mut ep: Vec<*const u8> = vars.iter().map(|s| s.as_ptr()).collect(); ep.push(core::ptr::null());
        let _ = process::execve(&args[0], ap.as_ptr(), ep.as_ptr());
        127
    };
    let result = spawn(&[(null, Fd::STDIN), (w, Fd::STDOUT), (null, Fd::STDERR)], Run::Call(&mut exec));
    let _ = (io::close(w), io::close(null));
    let pid = result.map_err(|e| e.to_string())?.raw();
    reader.pid = pid;
    Ok((pid, reader))
}

fn outputs(outputs: &mut Vec<Output>, channel: &mut Channel, connected: bool) -> Result<(), String> {
    let mut at = 0;
    while at < outputs.len() {
        let mut chunk = [0; 4096];
        match io::read(outputs[at].fd, &mut chunk) {
            Ok(0) => { outputs.remove(at); continue; }
            Ok(n) => {
                // Stdout uses a separate bounded diagnostic channel.
                let keep = n.min((64 * 1024usize).saturating_sub(outputs[at].bytes));
                outputs[at].bytes += keep;
                if connected && keep != 0 { channel.queue(&Message::Output { pid: outputs[at].pid, bytes: chunk[..keep].to_vec() })?; }
            }
            Err(Error::Os(Errno::EINTR | Errno::EAGAIN)) => {},
            Err(error) => return Err(error.to_string()),
        }
        at += 1;
    }
    Ok(())
}
fn reap(channel: &mut Channel, connected: bool) -> Result<bool, String> {
    loop {
        let mut status = 0;
        match process::waitpid(-1, &mut status, process::WNOHANG) {
            Ok(pid) if pid.raw() != 0 => {
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
fn teardown(channel: &mut Channel, outputs_state: &mut Vec<Output>, mut connected: bool) -> Result<(), String> {
    let end = now()? + 1000;
    loop {
        kill_descendants()?;
        outputs(outputs_state, channel, connected)?;
        let empty = reap(channel, connected)?;
        if connected && channel.flush().is_err() { connected = false; }
        if empty { break; }
        if now()? >= end { return Err("session descendants outlived SIGKILL".into()); }
        time::sleep_ms(1).map_err(|e| e.to_string())?;
    }
    if connected { channel.send(&Message::Stopped)?; }
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
    let mut output = Vec::new();
    let result = (|| -> Result<(), String> {
        signal::sigaction(signal::SIGPIPE, Some(&signal::Sigaction::ignore()), None).map_err(|e| e.to_string())?;
        io::fcntl_setfl(collector, io::status_flags::O_NONBLOCK).map_err(|e| e.to_string())?;
        channel.send(&Message::Ready(process::getpid().map_err(|e| e.to_string())?.raw()))?;
        let mut ended = Vec::new();
        let mut stopped = false;
        let mut connected = true;
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
                        io::write(collector, b"done\n").map_err(|e| e.to_string())?;
                        return Ok(());
                    }
                }
                Err(Error::Os(Errno::EINTR | Errno::EAGAIN)) => {},
                Err(error) => return Err(error.to_string()),
            }
            if !stopped {
                match if connected { channel.receive() } else { Ok(None) } {
                    Ok(Some(Message::Launch { argv, env })) => match launch(&argv, &env) {
                        Ok((pid, pipe)) => { output.push(pipe); channel.queue(&Message::Launched(pid))?; }
                        Err(error) => channel.queue(&Message::Error(error))?,
                    },
                    Ok(Some(Message::Teardown)) => { teardown(&mut channel, &mut output, true)?; stopped = true; }
                    Ok(Some(_)) => return Err("unexpected case control message".into()),
                    Ok(None) => {},
                    // Case exited: keep the collector alive until runner tells us to end.
                    Err(error) if error == "session supervisor/control peer closed" => connected = false,
                    Err(error) => return Err(error),
                }
                outputs(&mut output, &mut channel, connected)?;
                reap(&mut channel, connected)?;
                if connected && channel.flush().is_err() { connected = false; }
            }
            time::sleep_ms(1).map_err(|e| e.to_string())?;
        }
    })();
    if let Err(error) = &result { let _ = channel.send(&Message::Error(error.clone())); }
    let cleanup = if result.is_err() { teardown(&mut channel, &mut output, false) } else { Ok(()) };
    let _ = io::close(collector);
    i32::from(result.is_err() || cleanup.is_err())
}
