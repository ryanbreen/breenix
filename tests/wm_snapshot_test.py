"""Force a publication between the real WM checkpoint's client and screen reads."""
from pathlib import Path
import subprocess
import tempfile
import unittest

from ui_idle_capture_test import function

ROOT = Path(__file__).resolve().parents[1]


class WmSnapshot(unittest.TestCase):
    def test_superseded_snapshot_is_reacquired_and_stale_composite_refused(self):
        checkpoint = function('userspace/programs/src/uitest_suite/proof.rs', 'fn wm_checkpoint(')
        checkpoint = checkpoint.replace('std::time::Instant', 'Clock')
        binding = function('userspace/programs/src/uitest_suite/proof.rs', 'fn wm_front_matches(')
        front = function('userspace/programs/src/uitest_suite/proof_wm_checks.rs', 'pub fn front_matches(')
        rust = (ROOT / 'tests/fixtures/wm_snapshot.rs').read_text()
        rust = rust.replace('mod libuitest {', 'mod libuitest { pub use crate::phases;')
        rust += '\npub mod phases {\n' + (ROOT / 'libs/libuitest/src/phases.rs').read_text().replace('//!', '//') + '\n}\n'
        rust = rust.replace('CHECKPOINT', checkpoint).replace('BINDING', binding).replace('FRONT', front)
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / 'check.rs').write_text(rust)
            subprocess.run(['taskpolicy', '-b', 'rustc', '--edition=2021', '-Dwarnings',
                            str(path / 'check.rs'), '-o', str(path / 'check')], check=True)
            result = subprocess.run([str(path / 'check')], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == '__main__':
    unittest.main()
