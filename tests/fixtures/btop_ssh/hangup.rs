use std::sync::Mutex;

type ProcessId = u32;
const LEADER: ProcessId = 34;
const JOB: ProcessId = 35;

static SESSION: Mutex<Option<ProcessId>> = Mutex::new(Some(LEADER));
static SIGNALS: Mutex<Vec<(&str, ProcessId, u32)>> = Mutex::new(Vec::new());

enum TtyRef { Pty(Pair), Vt(()) }
struct Pair { ctl: ControlLock }
impl Pair {
    fn master_closed(&self) -> bool { true }
}
struct ControlLock(Mutex<Control>);
impl ControlLock {
    fn lock(&self) -> std::sync::MutexGuard<'_, Control> { self.0.lock().unwrap() }
}
struct Control { fg: Option<ProcessId>, last_hangup_fg: Option<ProcessId> }
impl Control {
    fn detach(&mut self) -> Option<ProcessId> { self.fg.take() }
}
impl TtyRef {
    fn detach(&self) -> Option<ProcessId> {
        match self {
            Self::Pty(pair) => pair.ctl.lock().detach(),
            Self::Vt(_) => None,
        }
    }
    fn session(&self) -> Option<ProcessId> { *SESSION.lock().unwrap() }
    fn id(&self) -> TtyId {
        match self {
            Self::Pty(_) => TtyId::Pty,
            Self::Vt(_) => TtyId::Vt(()),
        }
    }
}
enum TtyId { Pty, Vt(()) }
struct SessionExit { tty: TtyRef, sid: ProcessId, fg: Option<ProcessId> }
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
fn clear_session(_: &mut (), sid: ProcessId, _: TtyId) {
    assert_eq!(SESSION.lock().unwrap().take(), Some(sid));
}
// @HANGUP@
// @LEADER_EXIT@

fn main() {
    let ctl = Control { fg: Some(JOB), last_hangup_fg: None };
    assert!(ctl.last_hangup_fg.is_none());
    let tty = TtyRef::Pty(Pair { ctl: ControlLock(Mutex::new(ctl)) });
    hangup_ref(&tty);
    hangup_ref(&tty);
    assert_signals();
    finish_session_exit(SessionExit { tty, sid: LEADER, fg: Some(JOB) });
    assert_signals();
    let _ = TtyRef::Vt(());
}

fn assert_signals() {
    use signal::constants::{SIGCONT, SIGHUP};
    let signals = SIGNALS.lock().unwrap();
    assert_eq!(signals.len(), 4, "hangup must alert both recipients exactly once: {signals:?}");
    for (kind, id) in [("process", LEADER), ("group", JOB)] {
        let received: Vec<_> = signals.iter().filter(|entry| entry.0 == kind && entry.1 == id)
            .map(|entry| entry.2).collect();
        assert_eq!(received, [SIGHUP, SIGCONT]);
    }
}
