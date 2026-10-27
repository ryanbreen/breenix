"""Compile the real failure producer with the suite's real VALUE validator."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


def function(source, name):
    start = source.index('fn ' + name + '(')
    if source[max(0, start - 4):start] == 'pub ':
        start -= 4
    brace = source.index('{', start)
    depth = 1
    end = brace + 1
    while depth:
        depth += (source[end] == '{') - (source[end] == '}')
        end += 1
    return source[start:end]


class InputDiagnostics(unittest.TestCase):
    def test_failed_case_keeps_its_verdict_and_emits_valid_numeric_records(self):
        path = Path(os.environ.get('CONSOLE_INPUT_SOURCE', ROOT / 'userspace/programs/src/suite_console.rs'))
        suite = (ROOT / 'libs/libbreenix/src/suite.rs').read_text()
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            counters = directory / 'counters'
            counters.write_text('XHCI_IRQ_ENTRY_TOTAL=99\nKBD_NONZERO_TOTAL=1\n'
                                'HOSTKEY_QUEUED=2 HOSTKEY_REPLAYED=2 HOSTKEY_DISPATCHED=2 '
                                'HOSTKEY_CHANGED=1 HOSTKEY_REPLAY_MAX_NS=1000000\n')
            measured = function(path.read_text(), 'measured').replace('/proc/xhci/counters', str(counters))
            # Execute this x86-only producer on the ARM64 development host.
            measured = measured.replace('#[cfg(target_arch = "x86_64")]', '')
            harness = '''
type Verdict<T = ()> = Result<T, String>;
type CaseResult = Result<(), breenix_suite::suite::CaseError>;
mod breenix_suite { pub mod suite {
use std::fmt::Write;
pub static RECORDS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
#[derive(Debug, PartialEq)] pub enum CaseError { Fail(String) }
fn record(kind: &str, rest: &str) { RECORDS.lock().unwrap().push(format!("{kind} {rest}")); }
IS_WORD
VALUE
}}
MEASURED
fn main() {
    breenix_suite::suite::value("INVALID", 1, "count", None);
    assert!(breenix_suite::suite::RECORDS.lock().unwrap().is_empty());
    assert_eq!(measured(|| Err("missing key".into())), Err(breenix_suite::suite::CaseError::Fail("missing key".into())));
    let records = breenix_suite::suite::RECORDS.lock().unwrap().clone();
    assert_eq!(records, ["VALUE kbd-nonzero-total=1count", "VALUE hostkey-queued=2count",
        "VALUE hostkey-replayed=2count", "VALUE hostkey-dispatched=2count",
        "VALUE hostkey-changed=1count", "VALUE hostkey-replay-max-ns=1000000ns"]);
    breenix_suite::suite::RECORDS.lock().unwrap().clear();
    assert_eq!(measured(|| Ok(())), Ok(()));
    assert!(breenix_suite::suite::RECORDS.lock().unwrap().is_empty());
}
'''.replace('IS_WORD', function(suite, 'is_word')).replace('VALUE\n', function(suite, 'value') + '\n').replace('MEASURED', measured)
            source = directory / 'producer.rs'
            source.write_text(harness)
            built = subprocess.run(['rustc', '--edition=2021', str(source), '-o', str(directory / 'producer')],
                                   capture_output=True, text=True)
            self.assertEqual(built.returncode, 0, built.stdout + built.stderr)
            self.assertNotIn('warning:', built.stderr)
            result = subprocess.run([str(directory / 'producer')], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == '__main__':
    unittest.main()
