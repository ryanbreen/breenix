"""Exercise init's production desktop serial receipt filter."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class DesktopLogForward(unittest.TestCase):
    def test_prompt_and_client_failure_receipts_reach_serial(self):
        source = (ROOT / 'userspace/programs/src/init_services.rs').read_text()
        if 'fn desktop_log_receipt(' in source:
            function = item(source, 'fn desktop_log_receipt(')
            self.assertIn('desktop_log_receipt(line)', item(source, 'fn drain_logs('))
        else:
            # Execute the original inline production expression before its extraction.
            expression = source.split('let desktop = entry.def.name == "bwm"\n', 1)[1].split(';', 1)[0]
            function = 'fn desktop_log_receipt(line: &[u8]) -> bool { true ' + expression + ' }'
        harness = function + r'''
fn main() {
    for line in [
        &b"[bwm] terminal prompt read back: commit=1"[..],
        &b"[bwm] terminal prompt check FAILED: empty"[..],
        &b"[bwm] client frame check FAILED: mismatch"[..],
    ] {
        println!("{}", desktop_log_receipt(line));
    }
    println!("{}", desktop_log_receipt(b"ordinary daemon output"));
}
'''
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / 'check.rs').write_text(harness)
            command = ['rustc', '--edition=2021', '-Dwarnings',
                       str(path / 'check.rs'), '-o', str(path / 'check')]
            if sys.platform == 'darwin':
                command = ['taskpolicy', '-b', *command]
            subprocess.run(command, check=True)
            result = subprocess.run([str(path / 'check')], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout.splitlines(), ["true", "true", "true", "false"])


if __name__ == '__main__':
    unittest.main()
