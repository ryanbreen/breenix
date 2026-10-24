//! Independent GPU client for the window-manager suite. No compositor policy lives here.
#[path = "wm_suite/protocol.rs"]
mod protocol;

use libbreenix::window;
use protocol::*;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::sync::atomic::{AtomicU64, Ordering};
use window::abi::{self, op, Plain};

static COMMIT_NS: AtomicU64 = AtomicU64::new(0);
static HANDLING_COMMIT: AtomicU64 = AtomicU64::new(0);
static EPOCH: AtomicU64 = AtomicU64::new(0);

fn run(commands: &mut File, replies: &mut File) -> Result<(), String> {
    let mut first = [0; COMMAND_SIZE];
    commands.read_exact(&mut first).map_err(|e| e.to_string())?;
    let mut seed = word(&first, 4);
    let mut refuse = word(&first, 8) & 1 != 0;
    let mut c = window::create(240, 160, 3, 4).map_err(|e| format!("CREATE: {e}"))?;
    let title = format!("WM client {seed}");
    if word(&first, 8) & 2 == 0 {
        window::register(c.id, title.as_bytes()).map_err(|e| format!("REGISTER: {e}"))?;
    }
    let event_fd =
        window::with_id(op::EVENT_FD, c.id).map_err(|e| format!("EVENT_FD: {e}"))? as i32;
    let mut renderer = bgl::Context::new().map_err(|e| format!("GPU context: {e}"))?;
    let mut events = Vec::new();
    let mut processed = 0;
    let mut animate = false;
    let mut commit = paint(&c, &mut renderer, seed, processed)?;
    respond(replies, &c, seed, commit, &events)?;
    loop {
        let mut fds = [
            libbreenix::io::PollFd {
                fd: commands.as_raw_fd(),
                events: 1,
                revents: 0,
            },
            libbreenix::io::PollFd {
                fd: event_fd,
                events: 1,
                revents: 0,
            },
        ];
        libbreenix::io::poll(&mut fds, if animate { 0 } else { -1 }).map_err(|e| e.to_string())?;
        let mut batch = [abi::WindowEvent::ZERO; 64];
        let n = window::read_events(c.id, &mut batch, false, 0).map_err(|e| e.to_string())?;
        if n != 0 {
            if events.len() + n > 8192 {
                return Err("client event log overflow".into());
            }
            for e in &batch[..n] {
                events.push(*e);
                processed = processed.max(e.serial);
                if e.kind == abi::event_kind::RESIZE_REQUESTED && !refuse {
                    let r = window::resize(c.id, e.width, e.height)
                        .map_err(|e| format!("owner RESIZE: {e}"))?;
                    c.width = r.width;
                    c.height = r.height;
                    c.stride = r.stride;
                    c.buffer = r.buffer;
                }
            }
            // A handling commit is made only after the whole batch's handlers returned.
            commit = paint(&c, &mut renderer, seed, processed)?;
            HANDLING_COMMIT.store(commit, Ordering::Relaxed);
        }
        if animate {
            seed = seed.wrapping_add(1);
            commit = paint(&c, &mut renderer, seed, processed)?;
        }
        if fds[0].revents == 0 {
            continue;
        }
        let mut bytes = [0; COMMAND_SIZE];
        commands.read_exact(&mut bytes).map_err(|e| e.to_string())?;
        let a = word(&bytes, 4);
        let b = word(&bytes, 8);
        if matches!(word(&bytes, 0), CLIP_SET | CLIP_GET) {
            if a as usize > 16 * 1024 * 1024 + 1 {
                return Err("client clipboard command exceeds bound".into());
            }
            let mut text = vec![0; a as usize];
            commands.read_exact(&mut text).map_err(|e| e.to_string())?;
            let answer = clipboard(word(&bytes, 0) == CLIP_SET, c.id, &text);
            let (status, serial, text) = match answer {
                Ok((serial, text)) => (0, serial, text),
                Err(e) => (e, 0, Vec::new()),
            };
            let mut header = Vec::new();
            header.extend_from_slice(&status.to_le_bytes());
            header.extend_from_slice(&(text.len() as u32).to_le_bytes());
            header.extend_from_slice(&serial.to_le_bytes());
            replies
                .write_all(&header)
                .and_then(|()| replies.write_all(&text))
                .map_err(|e| e.to_string())?;
            continue;
        }
        match word(&bytes, 0) {
            SNAPSHOT => {}
            REGISTER => {
                window::register(c.id, title.as_bytes()).map_err(|e| format!("REGISTER: {e}"))?;
            }
            PAINT => {
                seed = a;
                commit = paint(&c, &mut renderer, seed, processed)?;
            }
            RESIZE => {
                let r = window::resize(c.id, a, b).map_err(|e| format!("owner RESIZE: {e}"))?;
                c.width = r.width;
                c.height = r.height;
                c.stride = r.stride;
                c.buffer = r.buffer;
                commit = paint(&c, &mut renderer, seed, processed)?;
            }
            REFUSE => refuse = a != 0,
            CLEAR_EVENTS => events.clear(),
            ANIMATE => animate = a != 0,
            STEP => {
                EPOCH.store(a as u64, Ordering::Relaxed);
                if b != 0 {
                    seed = seed.wrapping_add(1);
                    commit = paint(&c, &mut renderer, seed, processed)?;
                } else {
                    let mut meta = abi::Meta::ZERO;
                    meta.epoch = a as u64;
                    meta.processed_serial = processed;
                    meta.ui_time_ms = libbreenix::ui_clock::now_ns() / 1_000_000;
                    commit =
                        window::commit_no_change(c.id, Some(&meta)).map_err(|e| e.to_string())?;
                }
            }
            EXIT => {
                window::destroy(c.id).map_err(|e| e.to_string())?;
                return Ok(());
            }
            EXEC => {
                let argv = [c"true".as_ptr().cast::<u8>(), core::ptr::null()];
                let _ = libbreenix::process::execv(b"/bin/true\0", argv.as_ptr());
                return Err("exec /bin/true failed".into());
            }
            _ => return Err("unknown client command".into()),
        }
        respond(replies, &c, seed, commit, &events)?;
    }
}

fn clipboard(set: bool, id: u64, text: &[u8]) -> Result<(u64, Vec<u8>), i32> {
    use std::os::unix::net::UnixStream;
    let io_error = |e: std::io::Error| -e.raw_os_error().unwrap_or(5);
    let mut socket = UnixStream::connect("/run/bwm/clipboard").map_err(io_error)?;
    let mut request = b"BCLIP001".to_vec();
    request.extend_from_slice(&(if set { 1u32 } else { 2 }).to_le_bytes());
    request.extend_from_slice(&(text.len() as u32).to_le_bytes());
    request.extend_from_slice(&id.to_le_bytes());
    socket
        .write_all(&request)
        .and_then(|()| socket.write_all(text))
        .map_err(io_error)?;
    let mut header = [0; 16];
    socket.read_exact(&mut header).map_err(io_error)?;
    let status = i32::from_le_bytes(header[0..4].try_into().unwrap());
    if status != 0 {
        return Err(status);
    }
    let n = word(&header, 4) as usize;
    if n > 16 * 1024 * 1024 {
        return Err(-75);
    }
    let serial = u64::from_le_bytes(header[8..16].try_into().unwrap());
    let mut result = vec![0; n];
    socket.read_exact(&mut result).map_err(io_error)?;
    Ok((serial, result))
}

fn paint(
    c: &abi::Create,
    renderer: &mut bgl::Context,
    seed: u32,
    processed: u64,
) -> Result<u64, String> {
    let a = window::acquire(c.id, 1_000_000_000).map_err(|e| format!("ACQUIRE: {e}"))?;
    renderer.begin(c.buffer[a.buffer as usize], c.width, c.height);
    renderer.clear(colour(seed, 0, 0, c.width, c.height));
    for (x, y) in [(1, 0), (0, 1), (1, 1)] {
        let (left, top) = (x * c.width / 2, y * c.height / 2);
        let (right, bottom) = (
            if x == 0 { c.width / 2 } else { c.width },
            if y == 0 { c.height / 2 } else { c.height },
        );
        renderer.solid(
            colour(seed, left, top, c.width, c.height),
            &[[left as i32, top as i32, right as i32, bottom as i32]],
        );
    }
    renderer
        .submit()
        .and_then(|f| f.wait(1_000_000_000))
        .map_err(|e| format!("GPU paint: {e}"))?;
    let mut meta = abi::Meta::ZERO;
    meta.processed_serial = processed;
    meta.epoch = EPOCH.load(Ordering::Relaxed);
    meta.ui_time_ms = libbreenix::ui_clock::now_ns() / 1_000_000;
    let began = libbreenix::time::now_monotonic()
        .map_err(|e| e.to_string())?
        .as_nanos() as u64;
    let committed = window::commit(
        c.id,
        a.buffer,
        &[abi::Rect::new(0, 0, c.width as i32, c.height as i32)],
        Some(&meta),
    )
    .map_err(|e| format!("COMMIT: {e}"))?;
    COMMIT_NS.store(began, Ordering::Relaxed);
    Ok(committed.commit_id)
}

fn colour(seed: u32, x: u32, y: u32, width: u32, height: u32) -> u32 {
    let q = u32::from(x >= width / 2) + 2 * u32::from(y >= height / 2);
    0xff00_0000
        | (((seed * 37 + q * 61) & 255) << 16)
        | (((seed * 71 + q * 43) & 255) << 8)
        | ((seed * 17 + q * 97) & 255)
}

fn respond(
    out: &mut File,
    c: &abi::Create,
    seed: u32,
    commit: u64,
    events: &[abi::WindowEvent],
) -> Result<(), String> {
    let mut h = [0; RESPONSE_SIZE];
    h[0..8].copy_from_slice(&c.id.to_le_bytes());
    h[8..16].copy_from_slice(&commit.to_le_bytes());
    for (offset, value) in [
        (16, c.width),
        (20, c.height),
        (24, seed),
        (28, events.len() as u32),
    ] {
        h[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    h[32..40].copy_from_slice(&COMMIT_NS.load(Ordering::Relaxed).to_le_bytes());
    h[40..48].copy_from_slice(&HANDLING_COMMIT.load(Ordering::Relaxed).to_le_bytes());
    out.write_all(&h).map_err(|e| e.to_string())?;
    // SAFETY: WindowEvent is the plain 320-byte wire ABI with initialized reserved fields.
    let bytes = unsafe {
        core::slice::from_raw_parts(events.as_ptr().cast::<u8>(), core::mem::size_of_val(events))
    };
    out.write_all(bytes).map_err(|e| e.to_string())
}

fn main() {
    // SAFETY: suite::spawn installs these two client-owned descriptors.
    let (mut commands, mut replies) = unsafe { (File::from_raw_fd(3), File::from_raw_fd(4)) };
    if let Err(e) = run(&mut commands, &mut replies) {
        let text = e.as_bytes();
        let n = text.len().min(1024);
        let mut header = [0; RESPONSE_SIZE];
        header[0..8].copy_from_slice(&u64::MAX.to_le_bytes());
        header[16..20].copy_from_slice(&(n as u32).to_le_bytes());
        let _ = replies
            .write_all(&header)
            .and_then(|()| replies.write_all(&text[..n]));
        std::process::exit(1);
    }
}
