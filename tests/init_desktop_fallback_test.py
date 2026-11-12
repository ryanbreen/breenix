"""Exercise production desktop recovery after admission refusal and compositor give-up."""
from pathlib import Path
import subprocess
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class DesktopFallback(unittest.TestCase):
    def test_stops_compositor_then_adds_logins_without_restarting_daemons(self):
        source = (ROOT / 'userspace/programs/src/init_services.rs').read_text()
        recovery = item(source, '    fn recover_desktop(')
        request = item(source, '    pub fn fallback_to_console(')
        harness = r'''
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
static TEXT: AtomicUsize = AtomicUsize::new(0);
static DETACH: AtomicUsize = AtomicUsize::new(0);
fn console_text_mode() { TEXT.fetch_add(1, Ordering::Relaxed); }
fn detach_console() { DETACH.fetch_add(1, Ordering::Relaxed); }
#[derive(PartialEq, Eq)]
enum Scope { Desktop, All, Logins }
#[derive(PartialEq, Eq)]
enum State { Waiting, Running, Stopping, Stopped, Failed }
impl State { fn is_live(&self) -> bool { matches!(self, Self::Running | Self::Stopping) } }
struct Service { name: String }
struct Entry { def: Service, state: State }
impl Entry {
    fn new(def: Service, _: u64) -> Self { Self { def, state: State::Waiting } }
    fn set(&mut self, state: State, _: u64) { self.state = state; }
}
fn login_terminal(service: &Service) -> Option<()> { service.name.starts_with("getty-").then_some(()) }
mod services {
    pub fn load(_: &std::path::Path) -> (Vec<crate::Service>, Vec<String>) {
        (["bwm", "blogd", "getty-console", "getty-tty2"].iter()
            .map(|name| crate::Service { name: name.to_string() }).collect(), vec![])
    }
}
struct Notices;
impl Notices { fn alert(&mut self, _: &str) {} }
enum AfterStop { Stay }
struct Supervisor {
    scope: Scope, desktop_fallback: bool, entries: Vec<Entry>, publish: bool,
    notices: Notices, changed: bool,
}
impl Supervisor {
    fn stop(entry: &mut Entry, _: AfterStop, _: u64) { entry.state = State::Stopping; }
    // REQUEST
    // RECOVERY
}
fn supervisor(state: State) -> Supervisor {
    Supervisor {
        scope: Scope::Desktop, desktop_fallback: false, publish: true,
        entries: vec![Entry { def: Service { name: "bwm".into() }, state },
            Entry { def: Service { name: "blogd".into() }, state: State::Running }],
        notices: Notices, changed: false,
    }
}
#[test]
fn admission_failure_waits_for_stop_before_terminal_handoff() {
    let mut s = supervisor(State::Running);
    s.recover_desktop(0);
    assert!(s.scope == Scope::Desktop);
    s.fallback_to_console();
    s.recover_desktop(1);
    assert!(s.entries[0].state == State::Stopping);
    assert_eq!(TEXT.load(Ordering::Relaxed), 0);
    s.recover_desktop(2);
    assert_eq!(s.entries.len(), 2);
    s.entries[0].state = State::Stopped;
    s.recover_desktop(3);
    assert!(s.scope == Scope::All);
    assert_eq!(TEXT.load(Ordering::Relaxed), 1);
    assert_eq!(DETACH.load(Ordering::Relaxed), 1);
    assert!(s.entries[1].state == State::Running);
    assert!(s.entries.iter().skip(2).all(|e| e.state == State::Waiting));
    assert_eq!(s.entries.len(), 4);
    s.recover_desktop(4);
    assert_eq!(s.entries.len(), 4);
}
#[test]
fn failed_compositor_opens_login_scope() {
    let mut s = supervisor(State::Failed);
    s.recover_desktop(0);
    assert!(s.scope == Scope::All);
    assert!(s.entries[0].state == State::Failed);
    assert!(s.changed);
    assert_eq!(s.entries.iter().filter(|e| login_terminal(&e.def).is_some()).count(), 2);
}
'''.replace('// REQUEST', request).replace('// RECOVERY', recovery)
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / 'fallback.rs').write_text(harness)
            subprocess.run(['taskpolicy', '-b', 'rustc', '--edition=2021', '-Dwarnings',
                            '--test', str(path / 'fallback.rs'), '-o', str(path / 'fallback')], check=True)
            subprocess.run([str(path / 'fallback'), '--test-threads=1'], check=True)
            child_harness = harness.split('#[test]')[0].replace(
                'fn console_text_mode() { TEXT.fetch_add(1, Ordering::Relaxed); }',
                'fn console_text_mode() { println!("TEXT-MODE"); TEXT.fetch_add(1, Ordering::Relaxed); }'
            ).replace(
                'fn detach_console() { DETACH.fetch_add(1, Ordering::Relaxed); }',
                'fn detach_console() { println!("DETACH"); DETACH.fetch_add(1, Ordering::Relaxed); }'
            ).replace(
                'impl Notices { fn alert(&mut self, _: &str) {} }',
                'impl Notices { fn alert(&mut self, message: &str) { println!("{message}"); } }'
            ).replace(
                'pub fn load(_: &std::path::Path) -> (Vec<crate::Service>, Vec<String>) {',
                'pub fn load(_: &std::path::Path) -> (Vec<crate::Service>, Vec<String>) { println!("LOAD-LOGINS");'
            ) + r'''
fn main() {
    let mut s = supervisor(State::Stopping);
    s.publish = false;
    s.fallback_to_console();
    s.recover_desktop(0);
    assert!(s.scope == Scope::Desktop);
    assert!(s.entries[0].state == State::Stopping);
    assert_eq!(s.entries.len(), 2);
    assert_eq!(TEXT.load(Ordering::Relaxed), 0);
    assert_eq!(DETACH.load(Ordering::Relaxed), 0);
    println!("STOPPING-WAITS");
    s.entries[0].state = State::Failed;
    s.recover_desktop(1);
    println!("RETURNED scope={} logins={}", s.scope == Scope::Logins,
             s.entries.iter().filter(|e| login_terminal(&e.def).is_some()).count());
}
'''
            (path / 'child.rs').write_text(child_harness)
            subprocess.run(['taskpolicy', '-b', 'rustc', '--edition=2021', '-Dwarnings',
                            str(path / 'child.rs'), '-o', str(path / 'child')], check=True)
            child = subprocess.run([str(path / 'child')], capture_output=True, text=True)
            self.assertEqual(child.returncode, 1, child.stdout)
            self.assertIn('STOPPING-WAITS', child.stdout)
            self.assertIn('[init] desktop unavailable', child.stdout)
            self.assertNotIn('TEXT-MODE', child.stdout)
            self.assertNotIn('DETACH', child.stdout)
            self.assertNotIn('LOAD-LOGINS', child.stdout)
            self.assertNotIn('RETURNED', child.stdout)


if __name__ == '__main__':
    unittest.main()
