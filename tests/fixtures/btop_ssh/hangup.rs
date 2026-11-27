use std::sync::Mutex;

type ProcessId = u32;
const LEADER: ProcessId = 34;
const JOB: ProcessId = 35;

static SESSION: Mutex<Option<ProcessId>> = Mutex::new(Some(LEADER));
static SIGNALS: Mutex<Vec<(&str, ProcessId, u32)>> = Mutex::new(Vec::new());

enum TtyRef { Pty(Pair), Vt(()) }
struct Pair { ctl: ControlLock }
struct ControlLock(Mutex<Control>);
impl ControlLock {
    fn lock(&self) -> std::sync::MutexGuard<'_, Control> { self.0.lock().unwrap() }
}
struct Control { fg: Option<ProcessId>, last_hangup_fg: Option<ProcessId> }
impl Control {
    fn detach(&mut self) -> Option<ProcessId> { self.fg.take() }
}
impl TtyRef {
    fn detach(&self) -> Option<ProcessId> { None }
    fn session(&self) -> Option<ProcessId> { *SESSION.lock().unwrap() }
    fn id(&self) -> u32 { 0 }
}
mod process {
    pub fn manager() -> std::sync::MutexGuard<'static, Option<()>> {
        static PM: std::sync::Mutex<Option<()>> = std::sync::Mutex::new(Some(()));
        PM.lock().unwrap()
    }
}
mod signal {
    pub mod constants {
        pub const SIGHUP: u32 = 1;
        pub const SIGCONT: u32 = 18;
    }
}
pub mod syscall {
    pub mod signal {
        use crate::{ProcessId, SESSION, SIGNALS};
        pub fn signal_process_from_kernel(pid: ProcessId, signal: u32) {
            assert_eq!(*SESSION.lock().unwrap(), None);
            SIGNALS.lock().unwrap().push(("process", pid, signal));
        }
        pub fn signal_pgrp_from_kernel(group: ProcessId, signal: u32) {
            assert_eq!(*SESSION.lock().unwrap(), None);
            SIGNALS.lock().unwrap().push(("group", group, signal));
        }
    }
}
fn clear_session(_: &mut (), sid: ProcessId, _: u32) {
    assert_eq!(SESSION.lock().unwrap().take(), Some(sid));
}
// @HANGUP@

fn main() {
    let tty = TtyRef::Pty(Pair { ctl: ControlLock(Mutex::new(Control { fg: Some(JOB), last_hangup_fg: None })) });
    hangup_ref(&tty);
    hangup_ref(&tty);
    let TtyRef::Pty(pair) = tty else { unreachable!() };
    assert_eq!(pair.ctl.lock().last_hangup_fg, Some(JOB));
    use signal::constants::{SIGCONT, SIGHUP};
    let signals = SIGNALS.lock().unwrap();
    assert_eq!(signals.len(), 4, "hangup must alert both recipients exactly once: {signals:?}");
    for (kind, id) in [("process", LEADER), ("group", JOB)] {
        let received: Vec<_> = signals.iter().filter(|entry| entry.0 == kind && entry.1 == id)
            .map(|entry| entry.2).collect();
        assert_eq!(received, [SIGHUP, SIGCONT]);
    }
    let _ = TtyRef::Vt(());
}
