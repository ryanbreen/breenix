#[path = "../userspace/programs/src/bwm/shutdown.rs"]
mod shutdown;

use shutdown::{boundary, Runtime};

#[derive(Default)]
struct Fake {
    term: bool,
    pending: bool,
    blocked: bool,
    events: Vec<&'static str>,
}

impl Runtime for Fake {
    type Ready = ();
    fn unblock(&mut self) { self.events.push("unblock"); self.blocked = false; if self.pending { self.term = true; } }
    fn wait(&mut self) { self.events.push("wait"); }
    fn block(&mut self) { self.events.push("block"); self.blocked = true; }
    fn terminated(&self) -> bool { self.term }
    fn release(&mut self) { self.events.push("release"); }
    fn restore(&mut self) { self.events.push("restore"); }
    fn exit(&mut self) { self.events.push("exit"); }
}

#[test]
fn term_during_gpu_work_waits_for_boundary_and_returns_custody() {
    let mut fake = Fake { blocked: true, ..Fake::default() };
    fake.events.push("gpu-start");
    fake.pending = true;
    assert!(fake.blocked);
    fake.events.push("gpu-end");
    assert!(!fake.term);
    assert!(boundary(&mut fake).is_none());
    assert_eq!(fake.events, ["gpu-start", "gpu-end", "unblock", "block", "release", "restore", "exit"]);
}

#[test]
fn direct_spawn_starts_with_fresh_signal_state() {
    // The shared direct-spawn implementation creates a new Process, whose
    // default SignalState supplies both an empty mask and default handlers.
    let manager = include_str!("../kernel/src/process/manager.rs");
    let process = include_str!("../kernel/src/process/process.rs");
    let signals = include_str!("../kernel/src/signal/types.rs");
    assert!(manager.contains("let mut process = Process::new(pid, name.clone(), loaded_elf.entry_point)"));
    assert!(manager.contains("let mut process = Process::new(pid, name.clone(), entry_point)"));
    assert!(process.contains("signals: SignalState::default()"));
    assert!(signals.contains("thread: Arc::new(ThreadSignals::with_mask(0))"));
    assert!(signals.contains("SignalTable::new()"));
}
