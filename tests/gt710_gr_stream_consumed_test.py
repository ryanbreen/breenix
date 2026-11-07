"""Exercise the production GR consumption predicate and cold wait on host IO."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]
EVIDENCE = Path.home() / 'fun/code/breenix.worktrees/fx/ledgers/evidence/gr-startup-2669'
EVIDENCE.mkdir(parents=True, exist_ok=True)


class StreamConsumed(unittest.TestCase):
    def test_consumption_and_deadline(self):
        source = (ROOT / 'kernel/src/gpu/gk208/gr/channel.rs').read_text()
        queue = (ROOT / 'kernel/src/gpu/gk208/queue.rs').read_text()
        poll = item(source, 'pub trait Poll')
        if 'fn stream_consumed(' in source:
            predicate = item(source, 'struct StreamSample') + '\n' + item(source, 'fn stream_consumed(')
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
        for key, value in [('POLL', poll), ('QUEUE', item(queue, 'pub fn consumed(')),
                           ('PREDICATE', predicate), ('AWAIT', awaiter)]:
            fixture = fixture.replace('// ' + key, value)
        with tempfile.TemporaryDirectory(dir=EVIDENCE) as directory:
            path = Path(directory)
            (path / 'stream.rs').write_text(fixture)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                                      str(path / 'stream.rs'), '-o', str(path / 'stream')], check=True, cwd=ROOT)
            subprocess.run([str(path / 'stream')], check=True)


if __name__ == '__main__':
    unittest.main()
