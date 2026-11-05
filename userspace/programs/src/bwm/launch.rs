use super::*;

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
            // Spawn the program — build null-terminated path on stack
            let mut spawn_buf = [0u8; 256];
            let len = path.len().min(254);
            spawn_buf[..len].copy_from_slice(&path[..len]);
            spawn_buf[len] = 0;
            if let Ok(pid) = libbreenix::process::spawn(&spawn_buf[..len + 1]) {
                children.track(pid.raw() as u32, libbreenix::ui_clock::now_ns());
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
    use libbreenix::window::{self, abi, abi::Plain};
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
    for pid in children.acknowledge(&listed, &frame) { print!("[bwm] launch ack pid={}\n", pid); }
}
