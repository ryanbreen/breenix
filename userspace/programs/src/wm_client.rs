//! Independent E18 GPU client; commands control its own buffers and behaviour.
#[path = "wm_suite/event_log.rs"]
pub mod event_log;
#[path = "wm_suite/protocol.rs"]
pub mod protocol;
use event_log::EventLog;
use libbreenix::{io, signal, time, window};
use protocol::*;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use window::abi::{self, op, Plain};

struct Options {
    control: bool,
    seed: u32,
    flags: u32,
    crash_after: u32,
    slow: u32,
}
impl Options {
    fn parse() -> Result<Self, String> {
        let mut o = Self {
            control: false,
            seed: 1,
            flags: 0,
            crash_after: 0,
            slow: 10,
        };
        let mut args = std::env::args().skip(1).peekable();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--control" => o.control = true,
                "--ignore-close" => o.flags |= flags::IGNORE_CLOSE,
                "--slow-commit" => {
                    o.flags |= flags::SLOW_COMMIT;
                    if args.peek().is_some_and(|v| !v.starts_with('-')) {
                        o.slow = args
                            .next()
                            .ok_or("missing slow period")?
                            .parse()
                            .map_err(|_| "invalid slow period")?;
                        if o.slow == 0 {
                            return Err("slow period must be positive".into());
                        }
                    }
                }
                "--crash-after" => {
                    o.crash_after = args
                        .next()
                        .ok_or("missing crash event count")?
                        .parse()
                        .map_err(|_| "invalid crash event count")?;
                    if o.crash_after == 0 {
                        return Err("crash event count must be positive".into());
                    }
                    o.flags |= flags::CRASH_AFTER;
                }
                "--hang" => o.flags |= flags::HANG,
                "--two-windows" => o.flags |= flags::TWO_WINDOWS,
                "--animate" => o.flags |= flags::ANIMATE,
                "--refuse-resize" => o.flags |= flags::REFUSE_RESIZE,
                "--never-read-events" => o.flags |= flags::NEVER_READ,
                "--chromeless" => o.flags |= flags::CHROMELESS,
                _ => {
                    if let Some(v) = arg.strip_prefix("--crash-after=") {
                        o.crash_after = v.parse().map_err(|_| "invalid --crash-after")?;
                        if o.crash_after == 0 {
                            return Err("--crash-after must be positive".into());
                        }
                        o.flags |= flags::CRASH_AFTER;
                    } else if let Some(v) = arg.strip_prefix("--slow-commit=") {
                        o.slow = v.parse().map_err(|_| "invalid --slow-commit")?;
                        if o.slow == 0 {
                            return Err("--slow-commit must be positive".into());
                        }
                        o.flags |= flags::SLOW_COMMIT;
                    } else if let Some(v) = arg.strip_prefix("--seed=") {
                        o.seed = v.parse().map_err(|_| "invalid --seed")?;
                    } else {
                        return Err(format!("unknown wm-client flag: {arg}"));
                    }
                }
            }
        }
        Ok(o)
    }
}

struct ClientWindow {
    c: abi::Create,
    fd: i32,
    seed: u32,
    commit: u64,
    frame_commit: u64,
    commit_ns: u64,
    handling_commit: u64,
    processed: u64,
    painted_processed: u64,
    epoch: u64,
    frame: u64,
    counter: bool,
}
impl ClientWindow {
    fn new(seed: u32, options: &Options, renderer: &mut bgl::Context) -> Result<Self, String> {
        Self::new_shape(seed, options, renderer, 240, 160, 3)
    }
    fn new_shape(
        seed: u32,
        options: &Options,
        renderer: &mut bgl::Context,
        width: u32,
        height: u32,
        buffers: u32,
    ) -> Result<Self, String> {
        let c = window::create(width, height, buffers, 4).map_err(|e| {
            let libbreenix::error::Error::Os(n) = e;
            format!("CREATE errno {}: {n:?}", n as i32)
        })?;
        let result = (|| {
            let title = title(seed, options.flags);
            if options.flags & flags::UNREGISTERED == 0 {
                window::register(c.id, title.as_bytes()).map_err(|e| format!("REGISTER: {e}"))?;
            }
            let fd =
                window::with_id(op::EVENT_FD, c.id).map_err(|e| format!("EVENT_FD: {e}"))? as i32;
            let mut w = Self {
                c,
                fd,
                seed,
                commit: 0,
                frame_commit: 0,
                commit_ns: 0,
                handling_commit: 0,
                processed: 0,
                painted_processed: 0,
                epoch: 0,
                frame: 0,
                counter: false,
            };
            w.paint(renderer)?;
            Ok(w)
        })();
        if result.is_err() {
            let _ = window::destroy(c.id);
        }
        result
    }
    fn meta(&self, processed: u64) -> abi::Meta {
        let mut m = abi::Meta::ZERO;
        m.processed_serial = processed;
        m.epoch = self.epoch;
        m.ui_time_ms = libbreenix::ui_clock::now_ns() / 1_000_000;
        m
    }
    fn paint(&mut self, renderer: &mut bgl::Context) -> Result<(), String> {
        let a = window::acquire(self.c.id, 1_000_000_000).map_err(|e| format!("ACQUIRE: {e}"))?;
        renderer.begin(
            self.c.buffer[a.buffer as usize],
            self.c.width,
            self.c.height,
        );
        for (x, y) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            let (left, top) = (x * self.c.width / 2, y * self.c.height / 2);
            let (right, bottom) = (
                if x == 0 {
                    self.c.width / 2
                } else {
                    self.c.width
                },
                if y == 0 {
                    self.c.height / 2
                } else {
                    self.c.height
                },
            );
            if right > left && bottom > top {
                renderer.solid(
                    colour(self.seed, x + 2 * y),
                    &[[left as i32, top as i32, right as i32, bottom as i32]],
                );
            }
            if self.c.width as u64 * self.c.height as u64 > 4_194_304 {
                renderer
                    .submit()
                    .and_then(|f| f.wait(1_000_000_000))
                    .map_err(|e| format!("GPU large quadrant: {e}"))?;
                if (x, y) != (1, 1) || self.counter {
                    renderer.begin(
                        self.c.buffer[a.buffer as usize],
                        self.c.width,
                        self.c.height,
                    );
                }
            }
        }
        if self.counter {
            draw_counter(renderer, self.frame, self.c.width, self.c.height);
        }
        if self.c.width as u64 * self.c.height as u64 <= 4_194_304 || self.counter {
            renderer
                .submit()
                .and_then(|f| f.wait(1_000_000_000))
                .map_err(|e| format!("GPU paint: {e}"))?;
        }
        self.commit_ns = time::now_monotonic().map_err(|e| e.to_string())?.as_nanos() as u64;
        self.commit = window::commit(
            self.c.id,
            a.buffer,
            &[abi::Rect::new(
                0,
                0,
                self.c.width as i32,
                self.c.height as i32,
            )],
            Some(&self.meta(self.processed)),
        )
        .map_err(|e| format!("COMMIT: {e}"))?
        .commit_id;
        self.frame_commit = self.commit;
        self.painted_processed = self.processed;
        Ok(())
    }
    fn no_change(&mut self) -> Result<(), String> {
        self.commit = window::commit_no_change(self.c.id, Some(&self.meta(self.painted_processed)))
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    fn resize(&mut self, width: u32, height: u32) -> Result<(), libbreenix::error::Error> {
        let r = window::resize(self.c.id, width, height)?;
        self.c.width = r.width;
        self.c.height = r.height;
        self.c.stride = r.stride;
        self.c.buffer = r.buffer;
        Ok(())
    }
}
impl Drop for ClientWindow {
    fn drop(&mut self) {
        let _ = io::close(libbreenix::types::Fd::from_raw(self.fd as u64));
    }
}
fn title(seed: u32, flags: u32) -> String {
    format!(
        "{}WM client {seed}",
        if flags & flags::CHROMELESS != 0 {
            "\u{1}"
        } else {
            ""
        }
    )
}
fn colour(seed: u32, q: u32) -> u32 {
    0xff00_0000
        | (((seed.wrapping_mul(37) + q * 61) & 255) << 16)
        | (((seed.wrapping_mul(71) + q * 43) & 255) << 8)
        | ((seed.wrapping_mul(17) + q * 97) & 255)
}
fn draw_counter(renderer: &mut bgl::Context, mut frame: u64, width: u32, height: u32) {
    const DIGITS: [[u8; 5]; 10] = [
        [7, 5, 5, 5, 7],
        [2, 6, 2, 2, 7],
        [7, 1, 7, 4, 7],
        [7, 1, 7, 1, 7],
        [5, 5, 7, 1, 1],
        [7, 4, 7, 1, 7],
        [7, 4, 7, 5, 7],
        [7, 1, 1, 1, 1],
        [7, 5, 7, 5, 7],
        [7, 5, 7, 1, 7],
    ];
    let mut digits = [0usize; 20];
    let mut count = 0;
    loop {
        digits[count] = (frame % 10) as usize;
        count += 1;
        frame /= 10;
        if frame == 0 {
            break;
        }
    }
    let mut rects = Vec::new();
    for digit in (0..count).rev() {
        for y in 0..5 {
            for x in 0..3 {
                let px = 2 + (count - 1 - digit) * 4 + x;
                let py = 2 + y;
                if px < width as usize
                    && py < height as usize
                    && DIGITS[digits[digit]][y] & (4 >> x) != 0
                {
                    rects.push([px as i32, py as i32, px as i32 + 1, py as i32 + 1]);
                }
            }
        }
    }
    if !rects.is_empty() {
        renderer.solid(0xffff_ffff, &rects);
    }
}
fn respond(
    replies: &mut Option<File>,
    windows: &[ClientWindow],
    active: usize,
    log: &EventLog,
) -> Result<(), String> {
    let Some(out) = replies.as_mut() else {
        return Ok(());
    };
    let w = &windows[active];
    let published =
        u32::try_from(log.published()).map_err(|_| "event log response index exceeds u32")?;
    let mut h = [0; RESPONSE_SIZE];
    h[28..32].copy_from_slice(&published.to_le_bytes());
    for (offset, v) in [
        (0, w.c.id),
        (8, w.commit),
        (32, w.commit_ns),
        (40, w.handling_commit),
        (48, windows.get(1).map_or(0, |w| w.c.id)),
        (56, if w.counter { w.frame + 1 } else { 0 }),
        (64, w.frame_commit),
    ] {
        h[offset..offset + 8].copy_from_slice(&v.to_le_bytes());
    }
    for (offset, v) in [(16, w.c.width), (20, w.c.height), (24, w.seed)] {
        h[offset..offset + 4].copy_from_slice(&v.to_le_bytes());
    }
    out.write_all(&h).map_err(|e| e.to_string())
}
fn reply_error(replies: &mut Option<File>, text: &str) -> Result<(), String> {
    let out = replies.as_mut().ok_or_else(|| text.to_string())?;
    let bytes = text.as_bytes();
    let n = bytes.len().min(1024);
    let mut h = [0; RESPONSE_SIZE];
    h[..8].copy_from_slice(&u64::MAX.to_le_bytes());
    h[16..20].copy_from_slice(&(n as u32).to_le_bytes());
    out.write_all(&h)
        .and_then(|()| out.write_all(&bytes[..n]))
        .map_err(|e| e.to_string())
}
fn clipboard(set: bool, id: u64, text: &[u8], mime: u32) -> Result<(u64, Vec<u8>), i32> {
    let errno = |e: libbreenix::error::Error| match e {
        libbreenix::error::Error::Os(n) => -(n as i32),
    };
    if set {
        let mut a = abi::ClipboardSet::ZERO;
        a.id = id;
        a.ptr = text.as_ptr() as u64;
        a.len = text.len() as u64;
        a.mime = mime;
        window::call(op::CLIPBOARD_SET, &mut a).map_err(errno)?;
        Ok((a.serial, Vec::new()))
    } else {
        let mut a = abi::ClipboardGet::ZERO;
        a.id = id;
        a.mime = mime;
        window::call(op::CLIPBOARD_GET, &mut a).map_err(errno)?;
        if a.len > abi::CLIPBOARD_MAX_BYTES {
            return Err(-75);
        }
        let mut out = vec![0; a.len as usize];
        a.dst = out.as_mut_ptr() as u64;
        a.capacity = a.len;
        if a.len != 0 {
            window::call(op::CLIPBOARD_GET, &mut a).map_err(errno)?;
        }
        out.truncate(a.len as usize);
        Ok((a.serial, out))
    }
}
fn run(
    mut options: Options,
    commands: &mut Option<File>,
    replies: &mut Option<File>,
) -> Result<(), String> {
    if let Some(command) = commands.as_mut() {
        let mut first = [0; COMMAND_SIZE];
        command.read_exact(&mut first).map_err(|e| e.to_string())?;
        options.seed = word(&first, 4);
        options.flags |= word(&first, 8);
    }
    if options.flags & !flags::MASK != 0 {
        return Err("unknown client behaviour flags".into());
    }
    let log = if options.control {
        EventLog::map(5)?
    } else {
        let (log, file) = EventLog::create()?;
        drop(file);
        log
    };
    let mut renderer = bgl::Context::new().map_err(|e| format!("GPU context: {e}"))?;
    let mut windows = vec![ClientWindow::new(options.seed, &options, &mut renderer)?];
    if options.flags & flags::TWO_WINDOWS != 0 {
        windows.push(ClientWindow::new(
            options.seed.wrapping_add(1),
            &options,
            &mut renderer,
        )?);
    }
    let mut active = 0;
    let mut received = 0u32;
    let mut continuous = !options.control && options.flags & flags::ANIMATE != 0;
    let session_clock = std::env::var_os("BREENIX_UI_CLOCK").is_some();
    let mut ticks = 0u64;
    respond(replies, &windows, active, &log)?;
    loop {
        if options.flags & flags::HANG != 0 {
            time::sleep_ms(1000).map_err(|e| e.to_string())?;
            continue;
        }
        let mut fds = Vec::new();
        if let Some(command) = commands.as_ref() {
            fds.push(io::PollFd {
                fd: command.as_raw_fd(),
                events: 1,
                revents: 0,
            });
        }
        let event_start = fds.len();
        if options.flags & flags::NEVER_READ == 0 {
            fds.extend(windows.iter().map(|w| io::PollFd {
                fd: w.fd,
                events: 1,
                revents: 0,
            }));
        }
        let slow_pending = !options.control
            && options.flags & flags::SLOW_COMMIT != 0
            && windows.iter().any(|w| w.processed != w.painted_processed);
        io::poll(
            &mut fds,
            if continuous {
                if options.control {
                    0
                } else {
                    16
                }
            } else if slow_pending {
                16
            } else {
                -1
            },
        )
        .map_err(|e| e.to_string())?;
        if options.flags & flags::NEVER_READ == 0 {
            let mut closed = Vec::new();
            for i in 0..windows.len() {
                if fds[event_start + i].revents & 1 == 0 {
                    continue;
                }
                let mut batch = [abi::WindowEvent::ZERO; 64];
                let w = &mut windows[i];
                let n =
                    window::read_events(w.c.id, &mut batch, false, 0).map_err(|e| e.to_string())?;
                for e in &batch[..n] {
                    log.push(w.c.id, *e)?;
                    received = received.saturating_add(1);
                    if options.flags & flags::CRASH_AFTER != 0
                        && options.crash_after != 0
                        && received >= options.crash_after
                    {
                        signal::kill(
                            libbreenix::process::getpid()
                                .map_err(|e| e.to_string())?
                                .raw() as i32,
                            signal::SIGSEGV,
                        )
                        .map_err(|e| e.to_string())?;
                        return Err("SIGSEGV did not terminate crash-after client".into());
                    }
                    w.processed = w.processed.max(e.serial);
                    if e.kind == abi::event_kind::CLOSE_REQUESTED
                        && options.flags & flags::IGNORE_CLOSE == 0
                    {
                        closed.push(i);
                    }
                    if e.kind == abi::event_kind::RESIZE_REQUESTED
                        && options.flags & flags::REFUSE_RESIZE == 0
                    {
                        if let Err(error) = w.resize(e.width, e.height) {
                            if !matches!(
                                error,
                                libbreenix::error::Error::Os(libbreenix::errno::Errno::ENOMEM)
                            ) {
                                return Err(format!("owner RESIZE: {error}"));
                            }
                            // RESIZE retained the old buffers and queued ResizeFailed.
                        }
                    }
                }
                if n != 0 && options.flags & flags::SLOW_COMMIT == 0 {
                    w.paint(&mut renderer)?;
                    w.handling_commit = w.commit;
                }
            }
            for i in closed.into_iter().rev() {
                window::destroy(windows[i].c.id).map_err(|e| e.to_string())?;
                windows.remove(i);
            }
            if windows.is_empty() {
                return Ok(());
            }
            active = active.min(windows.len() - 1);
        }
        if continuous || slow_pending {
            ticks = ticks.wrapping_add(1);
            for w in &mut windows {
                if !options.control && session_clock {
                    let epoch = libbreenix::ui_clock::epoch();
                    if w.epoch == epoch {
                        continue;
                    }
                    w.epoch = epoch;
                    if options.flags & flags::SLOW_COMMIT != 0 && epoch % options.slow as u64 != 0 {
                        w.no_change()?;
                        continue;
                    }
                } else if options.flags & flags::SLOW_COMMIT != 0
                    && ticks % options.slow as u64 != 0
                {
                    continue;
                }
                if continuous {
                    w.counter = true;
                    w.frame = w.frame.wrapping_add(1);
                }
                w.paint(&mut renderer)?;
                w.handling_commit = w.commit;
            }
        }
        let command_ready = commands.is_some() && fds[0].revents & 1 != 0;
        if !command_ready {
            continue;
        }
        let mut bytes = [0; COMMAND_SIZE];
        let command = commands.as_mut().ok_or("missing client command pipe")?;
        command.read_exact(&mut bytes).map_err(|e| e.to_string())?;
        let (which, a, b) = (word(&bytes, 0), word(&bytes, 4), word(&bytes, 8));
        if matches!(which, CLIP_SET | CLIP_GET) {
            if a as usize > 16 * 1024 * 1024 + 1 {
                return Err("clipboard command exceeds bound".into());
            }
            let mut text = vec![0; a as usize];
            command.read_exact(&mut text).map_err(|e| e.to_string())?;
            let (status, serial, text) = match clipboard(
                which == CLIP_SET,
                windows[active].c.id,
                &text,
                if b == 0 { 1 } else { b },
            ) {
                Ok((serial, text)) => (0, serial, text),
                Err(e) => (e, 0, Vec::new()),
            };
            let mut h = [0; 16];
            h[..4].copy_from_slice(&status.to_le_bytes());
            h[4..8].copy_from_slice(&(text.len() as u32).to_le_bytes());
            h[8..].copy_from_slice(&serial.to_le_bytes());
            let out = replies
                .as_mut()
                .ok_or("clipboard command without reply pipe")?;
            out.write_all(&h)
                .and_then(|()| out.write_all(&text))
                .map_err(|e| e.to_string())?;
            continue;
        }
        if which == SELECT_WINDOW {
            if a as usize >= windows.len() {
                return Err("client window selector out of range".into());
            }
            active = a as usize;
        }
        if which == CREATE_WINDOW {
            let dimensions = word(&bytes, 12);
            let (width, height) = if dimensions == 0 {
                (240, 160)
            } else {
                (dimensions & 65535, dimensions >> 16)
            };
            match ClientWindow::new_shape(
                a,
                &options,
                &mut renderer,
                width,
                height,
                if b == 0 { 3 } else { b },
            ) {
                Ok(w) => {
                    windows.push(w);
                    active = windows.len() - 1;
                    respond(replies, &windows, active, &log)?;
                }
                Err(error) => reply_error(replies, &error)?,
            }
            continue;
        }
        let w = &mut windows[active];
        match which {
            SNAPSHOT | CLEAR_EVENTS | SELECT_WINDOW => {}
            REGISTER => {
                window::register(w.c.id, title(w.seed, options.flags).as_bytes())
                    .map_err(|e| e.to_string())?;
            }
            PAINT => {
                w.seed = a;
                w.paint(&mut renderer)?;
            }
            RESIZE => {
                if let Err(error) = w
                    .resize(a, b)
                    .map_err(|error| {
                        let libbreenix::error::Error::Os(n) = error;
                        format!("owner RESIZE errno {}: {n:?}", n as i32)
                    })
                    .and_then(|()| w.paint(&mut renderer))
                {
                    reply_error(replies, &error)?;
                    continue;
                }
            }
            REFUSE => {
                if a != 0 {
                    options.flags |= flags::REFUSE_RESIZE;
                } else {
                    options.flags &= !flags::REFUSE_RESIZE;
                }
            }
            ANIMATE => continuous = a != 0,
            SET_FLAGS => {
                if a & !flags::MASK != 0 || (a & flags::CRASH_AFTER != 0 && b == 0) {
                    return Err("invalid client flags or crash count".into());
                }
                options.flags = a;
                options.crash_after = b;
            }
            SET_SLOW => {
                if a == 0 {
                    return Err("slow period must be positive".into());
                }
                options.slow = a;
            }
            SET_TITLE => {
                if a > 64 {
                    return Err("title exceeds window ABI limit".into());
                }
                let mut text = vec![0; a as usize];
                command.read_exact(&mut text).map_err(|e| e.to_string())?;
                window::set_title(w.c.id, &text).map_err(|e| e.to_string())?;
            }
            CLIP_SUBSCRIBE => {
                let mut r = abi::ClipboardSubscribe::ZERO;
                r.id = w.c.id;
                r.flags = a;
                window::call(op::CLIPBOARD_SUBSCRIBE, &mut r)
                    .map_err(|e| format!("CLIPBOARD_SUBSCRIBE: {e}"))?;
            }
            STEP => {
                w.epoch = a as u64;
                if b != 0 && (options.flags & flags::SLOW_COMMIT == 0 || a % options.slow == 0) {
                    w.counter = true;
                    w.frame += 1;
                    w.paint(&mut renderer)?;
                    w.handling_commit = w.commit;
                } else {
                    w.no_change()?;
                }
            }
            EXIT => {
                window::destroy(w.c.id).map_err(|e| e.to_string())?;
                windows.remove(active);
                if windows.is_empty() {
                    return Ok(());
                }
                active = active.min(windows.len() - 1);
            }
            EXEC => {
                let argv = [c"true".as_ptr().cast::<u8>(), core::ptr::null()];
                let _ = libbreenix::process::execv(b"/bin/true\0", argv.as_ptr());
                return Err("exec /bin/true failed".into());
            }
            _ => return Err("unknown client command".into()),
        }
        respond(replies, &windows, active, &log)?;
    }
}
fn main() {
    let options = Options::parse().unwrap_or_else(|e| {
        eprintln!("wm-client_test: {e}");
        std::process::exit(1)
    });
    let controlled = options.control;
    // The harness passes exactly these descriptors with suite::spawn.
    let (mut commands, mut replies) = if controlled {
        unsafe { (Some(File::from_raw_fd(3)), Some(File::from_raw_fd(4))) }
    } else {
        (None, None)
    };
    if let Err(e) = run(options, &mut commands, &mut replies) {
        if let Some(out) = replies.as_mut() {
            let bytes = e.as_bytes();
            let n = bytes.len().min(1024);
            let mut h = [0; RESPONSE_SIZE];
            h[..8].copy_from_slice(&u64::MAX.to_le_bytes());
            h[16..20].copy_from_slice(&(n as u32).to_le_bytes());
            let _ = out.write_all(&h).and_then(|()| out.write_all(&bytes[..n]));
        } else {
            eprintln!("wm-client_test: {e}");
        }
        std::process::exit(1);
    }
}
