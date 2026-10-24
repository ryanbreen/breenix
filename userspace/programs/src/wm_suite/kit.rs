pub(super) use super::{desktop::*, input::*, measure, protocol::*};
pub(super) use breenix_suite::suite::{check, hostkeys, value, CaseResult};
pub(super) use libbreenix::{signal, time, window};
pub(super) use window::abi::{self, event_kind as kind, win_flags, Plain};

pub(super) fn placed(c: &Client) -> Result<(), String> {
    until(2000, "bwm assigned registered client geometry", || {
        let w = c.info()?;
        Ok(w.x > 0 && w.y > 0)
    })
}

pub(super) fn one() -> Result<(Desktop, Client), String> {
    let d = Desktop::start()?;
    let c = Client::start(11)?;
    placed(&c)?;
    Ok((d, c))
}
pub(super) fn twin() -> Result<(Desktop, Client, Client), String> {
    let d = Desktop::start()?;
    let a = Client::start(11)?;
    placed(&a)?;
    let b = Client::start(23)?;
    placed(&b)?;
    Ok((d, a, b))
}
pub(super) fn key(c: &mut Client, chords: &str, code: u32) -> Result<abi::WindowEvent, String> {
    hostkeys(chords)?;
    c.await_event(
        &format!("bwm routing of {chords}, Linux keycode {code}"),
        |e| e.kind == kind::KEY_PRESS && e.keycode == code,
    )
}
pub(super) fn parked(d: &Desktop) -> Result<(), String> {
    d.pointer(d.width as i32 - 40, d.height as i32 - 100, 0, 0)?;
    time::sleep_ms(150).map_err(|e| e.to_string())
}
pub(super) fn content(d: &mut Desktop, c: &Client) -> CaseResult {
    parked(d)?;
    let commit = c.barrier()?;
    d.shown_after(c, commit)?.client(c)
}

pub(super) fn minimize(d: &Desktop, c: &Client) -> Result<(), String> {
    let w = c.info()?;
    d.click(w.x + w.width as i32 - 39 * d.scale, w.y - 16 * d.scale)?;
    until(1000, "minimized visibility published", || {
        Ok(c.info()?.flags & win_flags::MINIMIZED != 0)
    })
}
pub(super) fn drag(d: &Desktop, from: (i32, i32), to: (i32, i32)) -> Result<(), String> {
    d.pointer(from.0, from.1, 0, 0)?;
    time::sleep_ms(150).map_err(|e| e.to_string())?;
    d.pointer(from.0, from.1, 1, 0)?;
    time::sleep_ms(150).map_err(|e| e.to_string())?;
    d.pointer(to.0, to.1, 1, 0)?;
    time::sleep_ms(150).map_err(|e| e.to_string())?;
    d.pointer(to.0, to.1, 0, 0)?;
    time::sleep_ms(150).map_err(|e| e.to_string())
}
pub(super) fn resize_case(bottom: bool) -> CaseResult {
    let (mut d, mut c) = one()?;
    let w = c.info()?;
    let from = if bottom {
        (w.x + 80, w.y + w.height as i32 + 1)
    } else {
        (w.x + w.width as i32 + 1, w.y + 80)
    };
    let to = if bottom {
        (from.0, from.1 + 60)
    } else {
        (from.0 + 80, from.1)
    };
    drag(&d, from, to)?;
    c.await_event("owner-mediated ResizeRequested", |e| {
        e.kind == kind::RESIZE_REQUESTED
    })?;
    until(1000, "owner resized buffers published", || {
        c.snapshot()?;
        Ok((c.width, c.height)
            == if bottom {
                (w.width, w.height + 60)
            } else {
                (w.width + 80, w.height)
            })
    })?;
    let after = c.info()?;
    check(
        after.x == w.x && after.y == w.y,
        "right/bottom resize moved content origin",
    )?;
    content(&mut d, &c)
}
pub(super) fn disappear(sig: Option<i32>, exec: bool) -> CaseResult {
    let (mut d, a, mut b) = twin()?;
    content(&mut d, &b)?;
    if let Some(sig) = sig {
        b.child.signal(sig)?;
    } else {
        b.send(if exec { EXEC } else { EXIT }, 0, 0)?;
    }
    let mut status = None;
    until(1000, "client completed its exit, exec or crash", || {
        status = b.child.status()?;
        Ok(status.is_some())
    })?;
    let status = status.unwrap();
    if let Some(sig) = sig {
        check(
            status & 127 == sig,
            &format!("client did not die from expected signal {sig}: wait status {status}"),
        )?;
    } else {
        check(
            status == 0,
            &format!(
                "client {} failed: wait status {status}",
                if exec {
                    "exec of /bin/true"
                } else {
                    "normal exit"
                }
            ),
        )?;
    }
    until(1000, "dead/exec client window removed", || {
        Ok(!listed()?.iter().any(|w| w.id == b.id))
    })?;
    content(&mut d, &a)
}
pub(super) fn launch() -> Result<(Desktop, Client, abi::WinInfo), String> {
    let (mut d, mut c) = one()?;
    d.focus(&mut c)?;
    let before = listed()?.iter().map(|w| w.id).collect::<Vec<_>>();
    hostkeys("super-enter")?;
    let mut launched = None;
    until(
        5000,
        "Super+Return launches a real GUI client and its first commit",
        || {
            launched = listed()?
                .into_iter()
                .find(|w| !before.contains(&w.id) && w.commit_id > 0);
            Ok(launched.is_some())
        },
    )?;
    let w = launched.unwrap();
    until(2000, "launched client first commit composed by bwm", || {
        Ok(Screen::read().is_ok_and(|s| s.contains(w.id, w.commit_id)))
    })?;
    Ok((d, c, w))
}
pub(super) fn children(parent: i32, name: &str) -> Result<Vec<i32>, String> {
    let mut result = Vec::new();
    for entry in std::fs::read_dir("/proc")
        .map_err(|e| e.to_string())?
        .flatten()
    {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<i32>().ok())
        else {
            continue;
        };
        let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
        let wanted = status.lines().any(|s| {
            s.strip_prefix("PPid:")
                .is_some_and(|s| s.trim() == parent.to_string())
        });
        let named = name.is_empty()
            || status
                .lines()
                .any(|s| s.strip_prefix("Name:").is_some_and(|s| s.trim() == name));
        if wanted && named {
            result.push(pid);
        }
    }
    result.sort_unstable();
    Ok(result)
}

pub(super) fn clip_ok(c: &mut Client, set: bool, text: &[u8]) -> Result<(u64, Vec<u8>), String> {
    let (status, serial, text) = c.clipboard(set, text)?;
    if status != 0 {
        return Err(format!(
            "kernel CLIPBOARD_{} failed: errno {}; requires syscall 414 and a registered window",
            if set { "SET" } else { "GET" },
            -status
        ));
    }
    if serial == 0 {
        return Err("bwm clipboard omitted its change serial".into());
    }
    Ok((serial, text))
}
pub(super) fn clip_roundtrip(text: &[u8]) -> CaseResult {
    let (mut d, mut a, mut b) = twin()?;
    d.shown(&b)?;
    let (serial, _) = clip_ok(&mut a, true, text)?;
    let (got, bytes) = clip_ok(&mut b, false, &[])?;
    check(
        got == serial && bytes == text,
        "clipboard did not transfer exact bytes and serial between independent clients",
    )
}
pub(super) fn rss(pid: i32) -> Result<i64, String> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/status")).map_err(|e| e.to_string())?;
    text.lines()
        .find_map(|s| s.strip_prefix("VmRSS:"))
        .and_then(|s| s.split_whitespace().next())
        .and_then(|s| s.parse().ok())
        .ok_or("VmRSS is missing".into())
}

pub(super) fn input_latency(pointer: bool, animation: bool) -> CaseResult {
    measure::receipts()?;
    let mut device = Virtual::create()?;
    let mut d = Desktop::start()?;
    let mut animators = Vec::new();
    if animation {
        for n in 0..3 {
            let mut c = Client::start(60 + n)?;
            c.command(ANIMATE, 1, 0)?;
            animators.push(c);
        }
    }
    let mut c = Client::start(11)?;
    d.shown(&c)?;
    if animation {
        c.command(ANIMATE, 1, 0)?;
    }
    let mut samples = Vec::new();
    let mut receipt_serial = 0;
    let w = c.info()?;
    for n in 0..110 {
        c.clear()?;
        let begin = now();
        if pointer {
            let x = (w.x + 20 + (n % 100)) as u64;
            let y = (w.y + 40) as u64;
            device.batch(
                &[
                    (3, 0, (x * 32767 / (d.width - 1) as u64) as i32),
                    (3, 1, (y * 32767 / (d.height - 1) as u64) as i32),
                ],
                begin,
            )?;
        } else {
            device.batch(&[(1, 30, 1), (1, 30, 0)], begin)?;
        }
        c.await_event("timed input delivered through bwm", |e| {
            e.kind
                == if pointer {
                    kind::POINTER_MOVE
                } else {
                    kind::KEY_PRESS
                }
        })?;
        let r = measure::completed_commit(&c, c.handling_commit)?;
        receipt_serial = receipt_serial.max(r.serial);
        check(r.ns >= begin, "completion predates injected input")?;
        if n >= 10 {
            samples.push((r.ns - begin) / 1000);
        }
        let spent = (now() - begin) / 1_000_000;
        if spent < 50 {
            time::sleep_ms(50 - spent)?;
        }
    }
    measure::observed(&mut d, &[&c], receipt_serial)?;
    samples.sort_unstable();
    let median = samples[50];
    let max = *samples.last().unwrap();
    let (median_bound, max_bound) = if animation {
        (33000, 100000)
    } else {
        (20000, 50000)
    };
    value(
        "latency-median",
        median as i64,
        "us",
        Some((0, median_bound)),
    );
    value("latency-max", max as i64, "us", Some((0, max_bound)));
    check(
        median <= median_bound as u64 && max <= max_bound as u64,
        "input-to-composited-ack latency exceeds the GUI plan bound",
    )
}
pub(super) fn cpu_ticks(pid: i32) -> Result<u64, String> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).map_err(|e| e.to_string())?;
    let tail = text
        .rsplit_once(')')
        .ok_or("malformed proc stat")?
        .1
        .split_whitespace()
        .collect::<Vec<_>>();
    let user = tail
        .get(11)
        .and_then(|v| v.parse::<u64>().ok())
        .ok_or("proc stat utime missing")?;
    let sys = tail
        .get(12)
        .and_then(|v| v.parse::<u64>().ok())
        .ok_or("proc stat stime missing")?;
    Ok(user + sys)
}

pub(super) fn child_check(mut run: impl FnMut() -> i32) -> CaseResult {
    let pid = breenix_suite::suite::spawn(&[], breenix_suite::suite::Run::Call(&mut run))
        .map_err(|e| e.to_string())?;
    let mut status = 0;
    libbreenix::process::waitpid(pid.raw() as i32, &mut status, 0).map_err(|e| e.to_string())?;
    check(
        status == 0,
        &format!("assertion child returned wait status {status}"),
    )
}
pub(super) fn ledger() -> Result<Vec<Vec<String>>, String> {
    Ok(std::fs::read_to_string("/proc/breenix/input-ledger")
        .map_err(|e| format!("input disposition ledger: {e}"))?
        .lines()
        .map(|l| l.split_whitespace().map(str::to_owned).collect())
        .collect())
}
pub(super) fn disposition(serial: u64, expected: &str, reason: Option<&str>) -> CaseResult {
    until(1000, "terminal disposition", || {
        let rows = ledger()?;
        let matches: Vec<_> = rows
            .iter()
            .filter(|r| r.first().is_some_and(|v| v == &serial.to_string()))
            .collect();
        if matches.is_empty() {
            return Ok(false);
        }
        if matches.len() != 1 {
            return Err("source serial has duplicate terminal dispositions".into());
        }
        if !matches[0].get(1).is_some_and(|v| v == expected) {
            return Err("serial has wrong terminal disposition".into());
        }
        if let Some(reason) = reason {
            if !matches[0].get(3).is_some_and(|v| v == reason) {
                return Err("serial has wrong rejection reason".into());
            }
        }
        Ok(true)
    })?;
    Ok(())
}
pub(super) fn status() -> Result<abi::CompositorStatus, String> {
    let mut s = abi::CompositorStatus::default();
    window::call(abi::op::COMPOSITOR_STATUS, &mut s)
        .map_err(|e| format!("COMPOSITOR_STATUS: {e}"))?;
    Ok(s)
}

pub(super) struct InputClaim;
impl InputClaim {
    pub(super) fn new() -> Result<Self, String> {
        window::claim_compositor().map_err(|e| e.to_string())?;
        Ok(Self)
    }
}
impl Drop for InputClaim {
    fn drop(&mut self) {
        let _ = window::release_compositor();
    }
}
pub(super) fn console_bytes(keys: &[(u16, i32)], expected: &[u8]) -> CaseResult {
    use libbreenix::{io, termios, types::Fd};
    struct Restore(termios::Termios);
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = termios::tcsetattr(Fd::STDIN, termios::TCSANOW, &self.0);
        }
    }
    let mut saved = termios::Termios::default();
    termios::tcgetattr(Fd::STDIN, &mut saved).map_err(|e| e.to_string())?;
    let _restore = Restore(saved);
    let mut raw = saved;
    termios::cfmakeraw(&mut raw);
    termios::tcsetattr(Fd::STDIN, termios::TCSANOW, &raw).map_err(|e| e.to_string())?;
    let mut v = Virtual::create()?;
    let batch: Vec<_> = keys.iter().map(|&(code, value)| (1, code, value)).collect();
    v.batch(&batch, 100)?;
    let mut got = Vec::new();
    until(1000, "console keymap bytes", || {
        let mut p = [io::PollFd {
            fd: 0,
            events: 1,
            revents: 0,
        }];
        io::poll(&mut p, 10).map_err(|e| e.to_string())?;
        if p[0].revents & 1 != 0 {
            let mut b = [0; 128];
            let n = io::read(Fd::STDIN, &mut b).map_err(|e| e.to_string())?;
            got.extend_from_slice(&b[..n]);
        }
        Ok(got.len() >= expected.len())
    })?;
    check(
        got == expected,
        &format!("console bytes {got:?}, expected {expected:?}"),
    )
}

pub(super) fn pattern(seed: u32, quadrant: u32) -> u32 {
    0xff000000
        | (((seed * 37 + quadrant * 61) % 256) << 16)
        | (((seed * 71 + quadrant * 43) % 256) << 8)
        | ((seed * 17 + quadrant * 97) % 256)
}
pub(super) fn background(x: i32, y: i32, height: i32, scale: i32) -> u32 {
    let t = y * 255 / height;
    let grid = i32::from(x % (64 * scale) == 0 || y % (64 * scale) == 0);
    0xff000000
        | ((12 + t / 12 + 6 * grid) as u32) << 16
        | ((16 + t / 20 + 8 * grid) as u32) << 8
        | (38 + t / 6 + 12 * grid) as u32
}
pub(super) fn title_pixels(d: &mut Desktop, c: &Client) -> CaseResult {
    use libgfx_ref::{bitmap_font as font, color::Color, framebuf::FrameBuf};
    parked(d)?;
    let s = d.shown(c)?;
    let w = c.info()?;
    let scale = d.scale as usize;
    let height = if scale == 2 {
        font::RasterHeight::Size32
    } else {
        font::RasterHeight::Size16
    };
    let metrics = font::metrics_at(height);
    let mut expected = FrameBuf::new(
        w.width as usize,
        32 * scale,
        libgfx_ref::framebuf::Format::BGRX,
    )
    .map_err(|e| format!("title oracle surface: {e:?}"))?;
    expected.clear(Color::rgb(40, 100, 220));
    let max = (w.width as usize).saturating_sub(60 * scale) / metrics.char_width;
    let len = (w.title_len as usize).min(max);
    for (i, b) in w.title[..len].iter().enumerate() {
        font::draw_char_at(
            &mut expected,
            *b as char,
            6 * scale + i * metrics.char_width,
            8 * scale,
            Color::rgb(255, 255, 255),
            height,
        );
    }
    for y in 8 * scale..(8 * scale + metrics.char_height) {
        for x in 6 * scale..6 * scale + len * metrics.char_width {
            let colour = expected.get_pixel(x, y);
            let expected =
                0xff000000 | ((colour.r as u32) << 16) | ((colour.g as u32) << 8) | colour.b as u32;
            check(
                s.pixel(w.x + x as i32, w.y - 34 * d.scale + y as i32)? == expected,
                "title glyph differs from certified bitmap renderer at contract position",
            )?;
        }
    }
    Ok(())
}

pub(super) struct FileOverride {
    path: String,
    previous: Option<Vec<u8>>,
}
impl FileOverride {
    pub(super) fn new(path: &str, contents: &[u8]) -> Result<Self, String> {
        // A killed case cannot run Drop. Configuration therefore lives beneath
        // the child's own root, rather than mutating another case's defaults.
        let isolated;
        let path = if path.starts_with("/etc/") {
            let root = format!("/tmp/wm-suite-config-{}", std::process::id());
            std::fs::create_dir_all(format!("{root}/etc")).map_err(|e| e.to_string())?;
            std::env::set_var("BREENIX_CONFIG_ROOT", &root);
            isolated = format!("{root}{path}");
            isolated.as_str()
        } else {
            path
        };
        let previous = match std::fs::read(path) {
            Ok(b) => Some(b),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.to_string()),
        };
        std::fs::write(path, contents).map_err(|e| e.to_string())?;
        Ok(Self {
            path: path.to_owned(),
            previous,
        })
    }
}
impl Drop for FileOverride {
    fn drop(&mut self) {
        if let Some(b) = &self.previous {
            let _ = std::fs::write(&self.path, b);
        } else {
            let _ = std::fs::remove_file(&self.path);
        }
        if self.path.starts_with("/tmp/wm-suite-config-") {
            if let Some(etc) = std::path::Path::new(&self.path).parent() {
                let _ = std::fs::remove_dir(etc);
                if let Some(root) = etc.parent() {
                    let _ = std::fs::remove_dir(root);
                }
            }
        }
    }
}

pub(super) fn appbar(d: &Desktop, id: u64) -> Result<(), String> {
    let mut ws = listed()?;
    ws.retain(|w| w.title.first() != Some(&1));
    ws.sort_by_key(|w| w.id);
    let mut x = 4 * d.scale;
    for w in ws {
        let width =
            (w.title_len as i32 * 9 * d.scale + 16 * d.scale).clamp(60 * d.scale, 180 * d.scale);
        if w.id == id {
            return d.click(x + width / 2, d.height as i32 - 18 * d.scale);
        }
        x += width + 2 * d.scale;
    }
    Err("appbar target absent".into())
}
pub(super) fn focused(c: &Client) -> Result<bool, String> {
    Ok(c.info()?.flags & win_flags::FOCUSED != 0)
}

pub(super) struct AddressLimit {
    pid: i32,
    previous: [u64; 2],
}
impl AddressLimit {
    pub(super) fn new(pid: i32, extra: u64) -> Result<Self, String> {
        let text =
            std::fs::read_to_string(format!("/proc/{pid}/status")).map_err(|e| e.to_string())?;
        let mapped = text
            .lines()
            .find_map(|l| l.strip_prefix("VmSize:"))
            .and_then(|s| s.split_whitespace().next())
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or("client VmSize missing")?
            * 1024;
        let mut previous = [0; 2];
        let r = unsafe {
            libbreenix::syscall::raw::syscall4(
                libbreenix::syscall::nr::PRLIMIT64,
                pid as u64,
                9,
                0,
                previous.as_mut_ptr() as u64,
            )
        } as i64;
        if r < 0 {
            return Err(format!("query client RLIMIT_AS errno {}", -r));
        }
        let limit = [mapped + extra, previous[1]];
        let r = unsafe {
            libbreenix::syscall::raw::syscall4(
                libbreenix::syscall::nr::PRLIMIT64,
                pid as u64,
                9,
                limit.as_ptr() as u64,
                0,
            )
        } as i64;
        if r < 0 {
            return Err(format!("limit client RLIMIT_AS errno {}", -r));
        }
        Ok(Self { pid, previous })
    }
}
impl Drop for AddressLimit {
    fn drop(&mut self) {
        let _ = unsafe {
            libbreenix::syscall::raw::syscall4(
                libbreenix::syscall::nr::PRLIMIT64,
                self.pid as u64,
                9,
                self.previous.as_ptr() as u64,
                0,
            )
        };
    }
}

pub(super) fn visible_content(d: &mut Desktop, c: &Client) -> CaseResult {
    parked(d)?;
    let s = d.shown(c)?;
    let w = c.info()?;
    let mut checked = 0usize;
    for y in 0..w.height {
        for x in 0..w.width {
            let sx = w.x + x as i32;
            let sy = w.y + y as i32;
            if sx < 0
                || sy < 28 * d.scale
                || sx >= d.width as i32
                || sy >= d.height as i32 - 36 * d.scale
            {
                continue;
            }
            let q = u32::from(x >= w.width / 2) + 2 * u32::from(y >= w.height / 2);
            check(
                s.pixel(sx, sy)? == pattern(c.seed, q),
                "visible clipped client pixel differs from its quadrant pattern",
            )?;
            checked += 1;
        }
    }
    check(checked > 0, "clipped client has no visible content pixels")
}

pub(super) fn counter(name: &str) -> Result<u64, String> {
    let text = std::fs::read_to_string("/proc/trace/counters").map_err(|e| e.to_string())?;
    text.lines()
        .find_map(|l| {
            let mut fields = l.split_whitespace();
            if fields.next()?.trim_end_matches(':') == name {
                fields.next()?.parse().ok()
            } else {
                None
            }
        })
        .ok_or_else(|| format!("required trace counter {name} is absent"))
}
