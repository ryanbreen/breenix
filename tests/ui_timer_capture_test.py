"""U23: a metadata acknowledgment may be newer than the captured pixel front."""
from pathlib import Path
import subprocess
import tempfile
import unittest
ROOT = Path(__file__).resolve().parents[1]
class TimerCapture(unittest.TestCase):
    def test_no_change_history_and_stale_front_refusals(self):
        source = (ROOT / 'userspace/programs/src/uitest_suite/timers.rs').read_text()
        signature = 'fn capture_current('
        self.assertIn(signature, source)
        rest = source[source.index(signature):]
        start = rest.index('{')
        depth = 1
        end = start + 1
        while depth:
            depth += (rest[end] == '{') - (rest[end] == '}')
            end += 1
        function = rest[:end]
        harness = function + r'''
fn main() {
    let (captured, latest) = (5, 6);
    assert!(captured != latest, "old equality rejects the recorded metadata-only history");
    assert!(capture_current(captured, latest, 5, 5, Some((5, 5))));
    assert!(!capture_current(4, latest, 5, 5, Some((5, 5))), "stale capture");
    assert!(!capture_current(5, latest, 5, 6, Some((5, 5))), "front moved");
    assert!(!capture_current(5, latest, 6, 6, Some((5, 5))), "a newer pixel commit is not metadata-only");
    assert!(!capture_current(7, latest, 5, 5, Some((5, 5))), "future capture");
    assert!(capture_current(7, 7, 6, 6, None));
    assert!(!capture_current(6, 7, 6, 6, None), "stale redraw capture");
}
'''
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'test.rs').write_text(harness)
            result = subprocess.run(['taskpolicy', '-b', 'rustc', '--edition=2021', '-Dwarnings', str(path / 'test.rs'), '-o', str(path / 'test')], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            result = subprocess.run([str(path / 'test')], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
if __name__ == '__main__': unittest.main()
