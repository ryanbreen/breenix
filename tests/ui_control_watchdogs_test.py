"""U24 inventory: peer watchdogs are measured; case-subject bounds stay fixed."""
from collections import Counter
from pathlib import Path
import re
import unittest
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]
PEERS = {
    'libs/libuitest/src/session.rs': [
        'CompositorRegistration', 'SupervisorReady', 'Configure', 'HoldWindow',
        'InputAck', 'InputAck', 'InputAck', 'Stderr', 'Launch', 'AppOutput',
        'AppExit', 'Stopped'],
    'libs/libuitest/src/channel.rs': ['ChannelSend'],
    'libs/libuitest/src/supervisor_runtime.rs': ['SupervisorTeardown', 'InputGrab'],
    'libs/libbreenix/src/suite_session.rs': ['CollectorReply', 'DiscardReply'],
    'libs/libuitest/src/executor.rs': ['ControllerReply'],
    'libs/libuitest/src/retention.rs': ['DiagnosticUpload'],
}


class ControlWatchdogs(unittest.TestCase):
    def test_peer_inventory_and_ten_second_watchdogs(self):
        for relative, names in PEERS.items():
            with self.subTest(path=relative):
                text = (ROOT / relative).read_text()
                actual = re.findall(r'Watchdog::new\(Wait::(\w+)\)', text)
                self.assertEqual(Counter(actual), Counter(names), 'unclassified or missing control wait')
                self.assertNotRegex(text, r'now\(\)\?\s*\+\s*[0-9]', 'ad hoc peer timeout')
        path = ROOT / 'libs/libbreenix/src/suite_control.rs'
        self.assertTrue(path.is_file(), 'shared control watchdog is missing')
        text = path.read_text()
        self.assertRegex(text, r'pub const HANG_MS: u64 = 10_000;')
        variants = set(re.findall(r'Wait::(\w+)', text))
        self.assertEqual(variants, set(sum(PEERS.values(), [])))
        runner = (ROOT / 'libs/libbreenix/src/suite.rs').read_text()
        self.assertIn('session.finish(control::HANG_MS)', runner)
        self.assertIn('i128::from(control::HANG_MS) * 1_000_000', runner)
        executor = (ROOT / 'libs/libuitest/src/executor.rs').read_text()
        self.assertIn('session.finish(control::HANG_MS)', executor)
        self.assertIn('spec.limit_ms + control::HANG_MS', executor)
        budget = item((ROOT / 'libs/libuitest/src/artifacts.rs').read_text(), 'pub fn upload_budget_ms(')
        self.assertIn('saturating_sub(1750)', budget)
        self.assertIn('min(breenix_suite::suite::control::HANG_MS)', budget)

    def test_watchdog_durations_survive_forks(self):
        from ui_idle_capture_test import run_rust
        source = r'''extern crate self as libbreenix;
use std::sync::atomic::{AtomicU64,Ordering};
static NOW:AtomicU64=AtomicU64::new(100);
pub mod time {
 pub struct Stamp(u64);impl Stamp{pub fn as_nanos(&self)->i128{i128::from(self.0)*1_000_000}}
 pub fn now_monotonic()->Result<Stamp,String>{Ok(Stamp(super::NOW.load(super::Ordering::Relaxed)))}
}
#[path="MODULE"]mod control;
extern "C" {fn fork()->i32;fn waitpid(pid:i32,status:*mut i32,flags:i32)->i32;fn _exit(status:i32)->!;}
fn main(){
 let scope=control::Scope::new().unwrap();
 let watchdog=control::Watchdog::new(control::Wait::Configure).unwrap();
 assert_eq!(watchdog.deadline_ms(),10100);assert_eq!(watchdog.deadline_ns(),10_100_000_000);
 NOW.store(125,Ordering::Relaxed);drop(watchdog);
 let pid=unsafe{fork()};assert!(pid>=0);
 if pid==0{
  let watchdog=control::Watchdog::new(control::Wait::Configure).unwrap();
  NOW.store(205,Ordering::Relaxed);drop(watchdog);
  let _unfinished=control::Watchdog::new(control::Wait::Stopped).unwrap();
  unsafe{_exit(0)}
 }
 let mut status=0;assert_eq!(unsafe{waitpid(pid,&mut status,0)},pid);assert_eq!(status,0);
 let report=scope.report("ui-testing","test");
 assert!(report.iter().any(|s|s.contains("wait=Configure started=2 completed=2 total_ms=105 max_ms=80")),"{report:?}");
 assert!(report.iter().any(|s|s.contains("wait=Stopped started=1 completed=0")),"{report:?}");
 assert!(report.iter().all(|s|s.contains("clock_errors=0")));
 drop(scope);
 let scope=control::Scope::new().unwrap();assert!(scope.report("next","case").is_empty());
}
'''.replace('MODULE', str(ROOT / 'libs/libbreenix/src/suite_control.rs'))
        result = run_rust(source)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_named_subjects_keep_their_original_bounds(self):
        for relative, text in [
            ('libs/libuitest/src/clock.rs', 'pub const STEP_BOUND_NS: u64 = 2_000_000_000;'),
            ('libs/libuitest/src/settle.rs', 'pub const BOUND_NS: u64 = 2_000_000_000;'),
            ('libs/libuitest/src/startup.rs', 'composited && elapsed_ns <= 2_000_000_000'),
        ]:
            self.assertIn(text, (ROOT / relative).read_text())
        session = (ROOT / 'libs/libuitest/src/session.rs').read_text()
        self.assertIn('saturating_add(2_000_000_000)', item(session, 'pub fn wait_window('))
        self.assertIn('if at > deadline_ns', item(session, 'pub fn wait_window_until('))
        pointer = (ROOT / 'libs/libuitest/src/pointer_live.rs').read_text()
        subjects = ['fn wait_delivery(', 'pub fn dispositions(', 'pub fn wait_pointer_idle(',
                    'pub fn desktop_tree(', 'fn activate(', 'pub fn watch(', 'fn source_event(', 'fn resolve(']
        self.assertEqual(pointer.count('Duration::from_secs(2)'), len(subjects))
        for signature in subjects:
            self.assertIn('Duration::from_secs(2)', item(pointer, signature))
        executor = (ROOT / 'libs/libuitest/src/executor.rs').read_text()
        self.assertIn('spec.limit_ms, WriterRole::Logic', executor)
        runner_case = (ROOT / 'userspace/programs/src/uitest_suite/runner.rs').read_text()
        self.assertIn('timed out after 1500 ms', runner_case)
        frames = (ROOT / 'userspace/programs/src/uitest_suite/comparison_cases.rs').read_text()
        self.assertIn('500_000_000', item(frames, 'pub fn compare_cost('))
        runner = (ROOT / 'libs/libbreenix/src/suite.rs').read_text()
        performance = re.findall(r'\("window-manager", "performance/([a-z-]+)", ([0-9_]+)\)', runner)
        self.assertEqual(dict(performance), {name: '20_000' for name in [
            'typing-latency', 'pointer-latency', 'latency-under-animation', 'present-latency',
            'frame-floor', 'idle-cpu', 'clipboard-latency']})


if __name__ == '__main__':
    unittest.main()
