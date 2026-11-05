use super::*;
use std::cell::RefCell;
use libbreenix::window::{self, abi, abi::Plain};
thread_local! { static ERROR: RefCell<Option<breengel::Window>> = const { RefCell::new(None) }; }

fn show_error(path: &[u8], kind: children::LaunchError) {
    let (title, reason) = match kind {
        children::LaunchError::Missing => ("Launch error: missing program", "Executable does not exist"),
        children::LaunchError::Fork => ("Launch error: fork failed", "Cannot create the program's process"),
        children::LaunchError::Other => ("Launch error", "Cannot start the executable"),
        children::LaunchError::NoWindow => ("Launch error: no window", "Program did not present a window within 5000 ms"),
    };
    ERROR.with(|slot| {
        let mut slot = slot.borrow_mut();
        let result = (|| -> Result<(), libbreenix::error::Error> {
            if let Some(error) = slot.as_mut() { window::set_title(error.id(), title.as_bytes())?; }
            else { *slot = Some(breengel::Window::new_without_fonts(title.as_bytes(), 640, 140)?); }
            let error = slot.as_mut().unwrap();
            error.acquire(0)?;
            let surface = error.surface()?;
            fill_rect(surface, 0, 0, 640, 140, Color::rgb(45, 24, 30));
            draw_text_at(surface, reason.as_bytes(), 16, 16, Color::rgb(255, 225, 225), None);
            draw_text_at(surface, path, 16, 64, Color::rgb(255, 255, 255), None);
            error.present()
        })();
        if let Err(error) = result { eprintln!("[bwm] cannot show launch error: {error}"); }
    });
}

pub(super) fn poll_errors(children: &mut children::Children) {
    ERROR.with(|slot| {
        let mut slot = slot.borrow_mut();
        if let Some(error) = slot.as_ref() {
            let mut events = [abi::WindowEvent::ZERO; abi::limits::EVENT_RING];
            if let Ok(count) = window::read_events(error.id(), &mut events, false, 0) {
                if events[..count].iter().any(|e| e.kind == abi::event_kind::CLOSE_REQUESTED) { *slot = None; }
            }
        }
    });
    for (path, error) in children.expire(libbreenix::ui_clock::now_ns()) { show_error(&path, error); }
}

// ─── Hotkey Action Execution ─────────────────────────────────────────────────

pub(super) fn execute_hotkey_action(action: &HotkeyAction, windows: &[Window], stack: &Stack, children: &mut children::Children) {
    match action {
        HotkeyAction::Exec(path) => {
            // For blauncher, toggle: if already open, close it; otherwise spawn
            if path == b"/usr/bin/blauncher" {
                let launcher_idx = windows.iter().position(|w|
                    w.chromeless && w.title_len > 1 && w.title[0] == 1
                );
                if let Some(idx) = launcher_idx {
                    send_to(windows, idx, Input::Close);
                    return;
                }
            }
            if path.is_empty() || path.contains(&0) {
                show_error(path, children::LaunchError::Other);
                return;
            }
            let mut executable = path.to_vec();
            executable.push(0);
            match libbreenix::process::spawn(&executable) {
                Ok(pid) => children.track_path(pid.raw() as u32, libbreenix::ui_clock::now_ns(), path),
                Err(libbreenix::error::Error::Os(errno)) => show_error(path, children::Children::spawn_error(errno as i32)),
            }
        }
        HotkeyAction::Close => {
            send_to(windows, focus_index(windows, stack), Input::Close);
        }
        HotkeyAction::FocusNext | HotkeyAction::FocusPrev => {
            // Focus cycling is handled in the main loop since it needs
            // mutable access to the stack and redraw state.
            // The main loop checks for these after calling update().
        }
    }
}

/// Drain exited children without blocking the compositor.
pub(super) fn reap(children: &mut children::Children) {
    children.reap(|| match libbreenix::process::waitpid(-1, core::ptr::null_mut(), libbreenix::process::WNOHANG) {
        Ok(pid) if pid.raw() != 0 => children::Wait::Exited(pid.raw() as u32),
        Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::EINTR)) => children::Wait::Interrupted,
        _ => children::Wait::Empty,
    });
}

/// Match the completed frame to the current LIST, independent of newer commits.
pub(super) fn acknowledge(children: &mut children::Children, serial: u64) {
    if children.is_empty() || serial == 0 { return; }
    #[repr(C)]
    struct Record { header: abi::CompositeRecordHeader, entries: [abi::CompositeEntry; abi::limits::RECORD_WINDOWS] }
    let mut record = Record { header: abi::CompositeRecordHeader::default(), entries: [abi::CompositeEntry::default(); abi::limits::RECORD_WINDOWS] };
    let mut query = abi::RecordQuery::ZERO;
    query.serial = serial;
    query.ptr = (&mut record as *mut Record) as u64;
    query.len = core::mem::size_of::<Record>() as u32;
    if window::call(abi::op::COMPOSITE_RECORD, &mut query).is_err() || record.header.flags != 0 { return; }
    let mut infos = vec![abi::WinInfo::ZERO; 32];
    loop {
        let Ok(count) = window::list(&mut infos) else { return; };
        if count <= infos.len() { infos.truncate(count); break; }
        infos.resize(count + 8, abi::WinInfo::ZERO);
    }
    let listed: Vec<_> = infos.iter().map(|w| children::Listed { id: w.id, owner: w.owner, commit: w.commit_id }).collect();
    let frame: Vec<_> = record.entries[..(record.header.count as usize).min(record.entries.len())].iter()
        .map(|e| children::Composed { id: e.id, commit: e.commit_id, pixels: e.pixel_gen }).collect();
    for pid in children.acknowledge(&listed, &frame) { record_ack(pid); }
}

/// Retain launch receipts when the suite redirects the readiness pipe.
pub(super) fn record_ack(pid: u32) {
    let record = format!("[bwm] launch ack pid={pid}\n");
    print!("{record}");
    if let Ok(mut evidence) = std::fs::OpenOptions::new().write(true).open("/dev/screen-evidence") {
        use std::io::Write;
        let _ = evidence.write_all(record.as_bytes());
    }
}
