"""W14: the published ACK time, rather than poll latency, anchors delivery."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class AckDeadline(unittest.TestCase):
    def test_late_observation_does_not_extend_the_2000_ms_bound(self):
        suite = (ROOT / 'libs/libbreenix/src/suite.rs').read_text()
        desktop = (ROOT / 'userspace/programs/src/wm_suite/desktop.rs').read_text()
        program = r'''
use std::sync::atomic::{AtomicU64, Ordering};
static NOW: AtomicU64 = AtomicU64::new(0);
fn now() -> u64 { NOW.load(Ordering::Relaxed) }
fn pause() { NOW.fetch_add(5_000_000, Ordering::Relaxed); }
fn case_ms_left() -> u64 { 10_000 }
PARSER
DEADLINE
#[test] fn published_ack_deadline() {
    let record = "HOSTINPUT_ACK BXKEY_ACK 000000000000000b ticket=55 ns=2150000000\n";
    let ack = host_ack_time(record, "BXKEY_ACK", 55).unwrap();
    assert_eq!(host_ack_time(record.trim_end(), "BXKEY_ACK", 55), None, "partial log line");
    assert_eq!(host_ack_time(record, "BXPTR_ACK", 55), None);
    assert_eq!(host_ack_time(record, "BXKEY_ACK", 54), None);
    for delay in [0, 100_000_000, 500_000_000] {
        NOW.store(ack + delay, Ordering::Relaxed);
        let result = until_from(ack, 2000, "typed text", || Ok(now() >= ack + 2_005_000_000));
        assert_eq!(result, Err("typed text: no acknowledgment within 2000 ms".into()));
        assert_eq!(now(), ack + 2_000_000_000);
    }
}
'''
        program = program.replace('PARSER', item(suite, 'fn host_ack_time('))
        program = program.replace('DEADLINE', item(desktop, 'pub fn until_from('))
        with tempfile.TemporaryDirectory(prefix='wm-ack-deadline-') as directory:
            temp = Path(directory)
            (temp / 'check.rs').write_text(program)
            compiler = (['taskpolicy', '-b'] if sys.platform == 'darwin' else []) + ['rustc', '--edition=2021', '-Dwarnings']
            subprocess.run(compiler + ['--test', str(temp / 'check.rs'), '-o', str(temp / 'check')], check=True)
            subprocess.run([str(temp / 'check')], check=True)


if __name__ == '__main__':
    unittest.main()
