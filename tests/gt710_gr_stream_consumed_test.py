"""Exercise the production GR consumption predicate and cold wait on host IO."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class StreamConsumed(unittest.TestCase):
    def test_consumption_and_deadline(self):
        source = (ROOT / 'kernel/src/gpu/gk208/gr/channel.rs').read_text()
        queue = (ROOT / 'kernel/src/gpu/gk208/queue.rs').read_text()
        poll = item(source, 'pub trait Poll')
        if 'fn stream_consumed(' in source:
            predicate = '#[derive(Clone, Copy)]\n' + item(source, 'struct StreamSample') + '\n' + item(source, 'fn stream_get_progress(') + '\n' + item(source, 'fn stream_consumed(')
            awaiter = item(source, 'fn await_consumed(')
        else:
            snapshot = item(source, '        fn snapshot(&mut self)')
            body = snapshot[snapshot.index('            let b ='):snapshot.index('            Ok((value')]
            predicate = '''fn stream_consumed(sample: StreamSample, id: Identity, sequence: u32, queued_put: Option<u32>) -> bool {
                let session = Session { pbdma: 0, userd: 0, report: Report { identity: id } };
                let s = &session;
                let mut regs = Registers(sample);
                let this = Monitor { session: s, regs: &mut regs, total: 0, sequence, queued_put };
                let self_ = this;
            ''' + body.replace('self.', 'self_.').replace('s.userd + 0x88)?', 's.userd + 0x88).unwrap()') + 'consumed\n}'
            predicate = 'struct StreamSample { channel:u32, get:u32, reference:u32, userd_get:u32, bind:u32 }\n' + predicate
            issue = item(source, '        fn issue_words(')
            block = item(issue, '            if budget < BOUND_US')
            loop = block[block.index('                let start'):block.rindex('            }')]
            awaiter = "fn await_consumed(monitor: &mut impl Poll) -> Result<(), &'static str> {\n" + loop + '\nOk(())\n}'
        fixture = (ROOT / 'tests/fixtures/gt710_gr_stream_consumed.rs').read_text()
        snapshot = item(source, '        fn snapshot(&mut self)')
        sampling = snapshot[snapshot.index('            let b ='):snapshot.index('            Ok((value')]
        sampling = sampling.replace('self.', 'monitor.')
        snapshot_check = '''fn monitor_consumed(sample: StreamSample, saved_reference: u32,
            id: Identity, sequence: u32, queued_put: Option<u32>) -> bool {
            let session = Session { pbdma: 0, userd: 0, report: Report { identity: id } };
            let mut regs = Registers(sample, saved_reference);
            let mut monitor = Monitor { session: &session, regs: &mut regs, total: 0, sequence, queued_put, resident_polls: 0, nonresident_polls: 0 };
            let s = monitor.session;
            (|| -> Result<bool, &\'static str> {
        ''' + sampling + '''
            let resident = sample.channel & 0x2fff == 0x2000 | id.id;
            assert_eq!(monitor.resident_polls, u64::from(resident));
            assert_eq!(monitor.nonresident_polls, u64::from(!resident));
            Ok(consumed)
        })().unwrap()
    }'''
        for key, value in [('POLL', poll), ('QUEUE', item(queue, 'pub fn consumed(')),
                           ('PREDICATE', predicate), ('AWAIT', awaiter), ('SNAPSHOT', snapshot_check)]:
            fixture = fixture.replace('// ' + key, value)
        fixture += r'''
struct Scheduler {
    now: u64, polls: u32, switch_at: u32,
    saved_get: u32, saved_reference: u32, bind: u32, resident: bool,
}
impl Poll for Scheduler {
    fn snapshot(&mut self) -> Result<(u32, u64, bool), &'static str> {
        self.polls += 1;
        let switched = !self.resident && self.polls >= self.switch_at;
        // Channel 0 drains before channel 1 takes the PBDMA. Live GET and
        // reference then belong to the idle peer, not the saved channel.
        let sample = if switched {
            StreamSample { channel: 0x2001, get: 0, reference: 0,
                userd_get: self.saved_get, bind: self.bind }
        } else if self.resident {
            StreamSample { channel: 0x2000, get: self.saved_get,
                reference: self.saved_reference, userd_get: 0, bind: self.bind }
        } else {
            StreamSample { channel: 0x2000, get: 0, reference: 0,
                userd_get: 0, bind: self.bind }
        };
        let id = Identity { id: 0, instance: 0x123000 };
        Ok((0, 0, monitor_consumed(sample, self.saved_reference, id, 1, None)))
    }
    fn now_us(&mut self) -> u64 { self.now }
    fn relax(&mut self) { self.now += 10; }
}
fn scheduler(saved_get: u32, saved_reference: u32, bind: u32, resident: bool) -> Scheduler {
    Scheduler { now: 0, polls: 0, switch_at: 4, saved_get, saved_reference, bind, resident }
}
#[test]
fn descheduled_consumed_stream_completes() {
    let mut model = scheduler(1, 1, 0x80000123, false);
    assert_eq!(await_consumed(&mut model), Ok(()));
    assert!(model.now < BOUND_US / 10);
}
#[test]
fn scheduler_negative_controls() {
    for resident in [false, true] {
        for (get, reference, bind) in [(0, 1, 0x80000123), (1, 0, 0x80000123), (1, 1, 0)] {
            let mut model = scheduler(get, reference, bind, resident);
            assert_eq!(await_consumed(&mut model), Err("GR control stream not consumed; backing retained"));
            assert_eq!(model.now, BOUND_US);
        }
    }
    let mut model = scheduler(1, 1, 0x80000123, true);
    assert_eq!(await_consumed(&mut model), Ok(()));
    assert_eq!(model.now, 0);
}
#[test]
fn saved_channel_progress_and_reference() {
    let id = Identity { id: 0, instance: 0x123000 };
    let sample = StreamSample { channel: 0x2001, get: 99, reference: 99,
        userd_get: 5, bind: 0x80000123 };
    assert!(monitor_consumed(sample, 5, id, 5, None));
    assert!(monitor_consumed(sample, 5, id, 5, Some(5)));
    assert!(!monitor_consumed(sample, 0, id, 5, None));
    assert!(!monitor_consumed(sample, 4, id, 5, Some(5)));
    assert!(!monitor_consumed(sample, 6, id, 5, None));
    assert!(monitor_consumed(sample, 6, id, 5, Some(5)));
    let mut partial = sample; partial.userd_get = 4;
    assert!(!monitor_consumed(partial, 5, id, 5, None));
    let mut unbound = sample; unbound.bind = 0;
    assert!(!monitor_consumed(unbound, 5, id, 5, None));
    let mut peer_instance = sample; peer_instance.bind = 0x80000456;
    assert!(!monitor_consumed(peer_instance, 5, id, 5, None));
    let resident = StreamSample { channel: 0x2000, get: 5, reference: 5,
        userd_get: 0, bind: 0x80000123 };
    assert!(monitor_consumed(resident, 0, id, 5, None));
    let mut stale_live = resident; stale_live.reference = 4;
    assert!(!monitor_consumed(stale_live, 5, id, 5, None));
}
'''
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'stream.rs').write_text(fixture)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                                      str(path / 'stream.rs'), '-o', str(path / 'stream')], check=True, cwd=ROOT)
            subprocess.run([str(path / 'stream')], check=True)


if __name__ == '__main__':
    unittest.main()
