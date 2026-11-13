"""Drive ARM's terminal record policy and UART owner against an exact wire."""
from pathlib import Path
import os
import subprocess
import tempfile
import unittest

from serial_records_test import function

ROOT = Path(__file__).resolve().parents[1]


class TtyContinuation(unittest.TestCase):
    def test_prompt_enter_and_flushed_fragment_continue_the_tty_line(self):
        policy = function((ROOT / 'kernel/src/serial_line.rs').read_text(),
                          'fn terminal_kind(')
        harness = (ROOT / 'tests/uart_owner.rs').read_text()
        for name in ['stage_lines', 'uart_owner']:
            harness = harness.replace('../kernel/src/' + name + '.rs',
                                      str(ROOT / 'kernel/src' / (name + '.rs')))
        harness += '\n' + policy + r'''
fn tty(bytes: &[u8]) { record(2, bytes, terminal_kind(bytes)); }

#[test]
fn prompt_then_enter_has_no_extra_blank_line() {
    let _one = SERIAL.lock().unwrap();
    tty(b"prompt> ");
    tty(b"\n");
    assert_eq!(&*WIRE.lock().unwrap(), b"prompt> \n");
}

#[test]
fn flushed_a_then_b_is_one_host_line() {
    let _one = SERIAL.lock().unwrap();
    tty(b"a"); // A fragment already flushed to the UART.
    tty(b"b\n");
    assert_eq!(&*WIRE.lock().unwrap(), b"ab\n");
}
'''
        with tempfile.TemporaryDirectory(prefix='tty-continuation-') as directory:
            path = Path(directory)
            (path / 'check.rs').write_text(harness)
            subprocess.run([
                'taskpolicy', '-b', os.environ.get('RUSTC', 'rustc'),
                '--edition=2021', '--test', '-Dwarnings', str(path / 'check.rs'),
                '-o', str(path / 'check'),
            ], check=True)
            # Fresh process state for each exact-wire case; collect both failures.
            failures = []
            for case in ['prompt_then_enter_has_no_extra_blank_line',
                         'flushed_a_then_b_is_one_host_line']:
                result = subprocess.run([str(path / 'check'), '--exact', case],
                                        timeout=10)
                if result.returncode:
                    failures.append(case)
            self.assertEqual(failures, [])


if __name__ == '__main__':
    unittest.main()
