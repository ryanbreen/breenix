//! Desktop process lifecycle, kept out of input policy and the desk model.
use super::*;
use libbreenix::{error::Error, errno::Errno, window::{self, abi, abi::Plain}};
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};

const LAUNCH_TIMEOUT_NS: u64 = 5_000_000_000;
const CLOSE_KILL_DELAY_NS: u64 = 2_000_000_000;
static TERMINATE: AtomicBool = AtomicBool::new(false);

struct PendingLaunch {
    pid: u32,
    path: Vec<u8>,
    deadline: u64,
}
struct PendingClose {
    window: u64,
    owner: u32,
    kill_at: Option<u64>,
}
#[derive(Default)]
struct Lifecycle {
    pending: Vec<PendingLaunch>,
    closing: Vec<PendingClose>,
    error: Option<breengel::Window>,
}
thread_local! {
    static LIFECYCLE: RefCell<Lifecycle> = RefCell::new(Lifecycle::default());
}

extern "C" fn terminate(_: i32) {
    TERMINATE.store(true, Ordering::Relaxed);
}

pub(super) fn initialize() {
    if let Err(error) = signal::sigaction(signal::SIGTERM, Some(&signal::Sigaction::new(terminate)), None) {
        eprintln!("[bwm] cannot install shutdown handler: {error}");
        process::exit(1);
    }
}

impl Lifecycle {
    fn close(&mut self, window: u64, owner: u32) {
        if let Some(close) = self.closing.iter_mut().find(|c| c.window == window) {
            // Further clicks never postpone an already running SIGKILL deadline.
            if close.owner == owner && close.kill_at.is_none() && owner != 0 {
                if signal::kill(owner as i32, signal::SIGTERM).is_ok() {
                    close.kill_at = Some(libbreenix::ui_clock::now_ns()
                        .saturating_add(CLOSE_KILL_DELAY_NS));
                }
            }
            return;
        }
        let mut backlog = INPUT_BACKLOG.lock().unwrap();
        let admission = backlog.push(window, Input::Close, |_, _| false);
        if admission == input_backlog::Admission::Disabled {
            backlog.request_close(window, Input::Close);
        }
        drop(backlog);
        flush_input();
        self.closing.push(PendingClose { window, owner, kill_at: None });
    }

    fn escalate(&mut self) {
        if self.closing.is_empty() { return; }
        let mut windows = Vec::new();
        if session::list_windows(&mut windows).is_err() { return; }
        let now = libbreenix::ui_clock::now_ns();
        self.closing.retain(|close| {
            if let Some(deadline) = close.kill_at {
                // W30 follows the owner's lifetime once TERM was sent, even if
                // it closes the window. Kernel PIDs increase without reuse.
                let sig = if now >= deadline { signal::SIGKILL } else { 0 };
                return match signal::kill(close.owner as i32, sig) {
                    Ok(()) => sig == 0,
                    Err(Error::Os(Errno::ESRCH)) => false,
                    Err(_) => true,
                };
            }
            windows.iter().any(|w| w.id == close.window && w.owner_pid == close.owner)
        });
    }

    fn show_error(&mut self, title: &str, path: &[u8], reason: &str) {
        let result = (|| -> Result<(), Error> {
            if let Some(error) = self.error.as_mut() {
                window::set_title(error.id(), title.as_bytes())?;
            } else {
                self.error = Some(breengel::Window::new_without_fonts(title.as_bytes(), 640, 140)?);
            }
            let error = self.error.as_mut().expect("error window created above");
            error.acquire(0)?;
            let surface = error.surface()?;
            fill_rect(surface, 0, 0, 640, 140, Color::rgb(45, 24, 30));
            draw_text_at(surface, reason.as_bytes(), 16, 16, Color::rgb(255, 225, 225), None);
            draw_text_at(surface, path, 16, 64, Color::rgb(255, 255, 255), None);
            error.present()
        })();
        if let Err(error) = result {
            eprintln!("[bwm] cannot show launch error: {error}");
        }
    }

    fn launch(&mut self, path: &[u8]) {
        // Never silently truncate a configured executable path.
        if path.is_empty() || path.contains(&0) {
            self.show_error("Launch error", path, "Invalid executable path");
            return;
        }
        let mut executable = path.to_vec();
        executable.push(0);
        match libbreenix::process::spawn(&executable) {
            Ok(pid) => self.pending.push(PendingLaunch {
                pid: pid.raw() as u32,
                path: path.to_vec(),
                deadline: libbreenix::ui_clock::now_ns().saturating_add(LAUNCH_TIMEOUT_NS),
            }),
            Err(error) => {
                let (title, reason) = match error {
                    Error::Os(Errno::ENOENT) => ("Launch error: missing program", "Executable does not exist"),
                    Error::Os(Errno::EAGAIN | Errno::ENOMEM) => ("Launch error: fork failed", "Cannot create the program's process"),
                    _ => ("Launch error", "Cannot start the executable"),
                };
                self.show_error(title, path, reason);
            }
        }
    }

    fn poll_error(&mut self) {
        let Some(error) = self.error.as_ref() else { return; };
        let mut events = [abi::WindowEvent::ZERO; abi::limits::EVENT_RING];
        if let Ok(count) = window::read_events(error.id(), &mut events, false, 0) {
            if events[..count].iter().any(|e| e.kind == abi::event_kind::CLOSE_REQUESTED) {
                self.error = None;
            }
        }
    }

    fn acknowledge(&mut self, serial: u64) {
        if self.pending.is_empty() || serial == 0 { return; }
        #[repr(C)]
        struct Record {
            header: abi::CompositeRecordHeader,
            entries: [abi::CompositeEntry; abi::limits::RECORD_WINDOWS],
        }
        let mut record = Record {
            header: abi::CompositeRecordHeader::default(),
            entries: [abi::CompositeEntry::default(); abi::limits::RECORD_WINDOWS],
        };
        let mut query = abi::RecordQuery::ZERO;
        query.serial = serial;
        query.ptr = (&mut record as *mut Record) as u64;
        query.len = core::mem::size_of::<Record>() as u32;
        if window::call(abi::op::COMPOSITE_RECORD, &mut query).is_err() || record.header.flags != 0 {
            return;
        }
        let mut infos = vec![abi::WinInfo::ZERO; 32];
        let Ok(count) = window::list(&mut infos) else { return; };
        if count > infos.len() {
            infos.resize(count, abi::WinInfo::ZERO);
            if window::list(&mut infos).is_err() { return; }
        } else { infos.truncate(count); }
        let entries = &record.entries[..(record.header.count as usize).min(record.entries.len())];
        self.pending.retain(|launch| !infos.iter().any(|w| {
            w.owner == launch.pid && w.commit_id != 0 && entries.iter().any(|e|
                e.id == w.id && e.commit_id != 0 && e.pixel_gen >= w.pixel_gen)
        }));
    }

    fn expire(&mut self) {
        let now = libbreenix::ui_clock::now_ns();
        while let Some(index) = self.pending.iter().position(|p| now >= p.deadline) {
            let launch = self.pending.remove(index);
            self.show_error("Launch error: no window", &launch.path,
                "Program did not present a window within 5000 ms");
        }
    }
}

/// Called by the loop only; signal delivery itself sets an atomic flag.
impl State {
    pub(super) fn lifecycle(&mut self) -> bool {
        if TERMINATE.load(Ordering::Relaxed) {
            LIFECYCLE.with(|state| state.borrow_mut().error = None);
            let release = window::release_compositor();
            let restore = libbreenix::graphics::give_back_display();
            if let Err(error) = release.and(restore) {
                eprintln!("[bwm] shutdown failed: {error}");
                process::exit(1);
            }
            return false;
        }
        loop {
            match libbreenix::process::waitpid(-1, core::ptr::null_mut(), libbreenix::process::WNOHANG) {
                Ok(pid) if pid.raw() != 0 => continue,
                Err(Error::Os(Errno::EINTR)) => continue,
                _ => break,
            }
        }
        LIFECYCLE.with(|state| {
            let mut state = state.borrow_mut();
            state.poll_error();
            state.escalate();
            state.acknowledge(self.gpu.last_serial);
            state.expire();
        });
        true
    }
}

/// W28/W30: first request is cooperative, second sends TERM, then UI-clock KILL.
pub(super) fn request_close(window_id: u64, owner_pid: u32) {
    if window_id != 0 {
        LIFECYCLE.with(|state| state.borrow_mut().close(window_id, owner_pid));
    }
}

pub(super) fn execute_hotkey_action(
    action: &HotkeyAction,
    windows: &mut Vec<Window>,
    focused_win: usize,
) {
    match action {
        HotkeyAction::Exec(path) => {
            // For blauncher, toggle: if already open, close it; otherwise spawn
            if path == b"/usr/bin/blauncher" {
                let launcher_idx = windows
                    .iter()
                    .position(|w| w.chromeless && w.title_len > 1 && w.title[0] == 1);
                if let Some(idx) = launcher_idx {
                    request_close(windows[idx].window_id, windows[idx].owner_pid);
                    return;
                }
            }
            LIFECYCLE.with(|state| state.borrow_mut().launch(path));
        }
        HotkeyAction::Close => {
            if !windows.is_empty() && focused_win < windows.len() {
                request_close(windows[focused_win].window_id, windows[focused_win].owner_pid);
            }
        }
        HotkeyAction::FocusNext | HotkeyAction::FocusPrev => {
            // Focus cycling is handled in the main loop since it needs
            // mutable access to focused_win and redraw state.
            // The main loop checks for these after calling update().
        }
    }
}
