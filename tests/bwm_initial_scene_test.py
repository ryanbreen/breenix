"""A desktop with no clients must publish its first composed scene before waiting."""
from pathlib import Path
import os
import re
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]

class InitialScene(unittest.TestCase):
    def test_empty_desktop_does_not_wait_for_a_client_or_clock(self):
        main = (ROOT / 'userspace/programs/src/bwm.rs').read_text()
        initial = re.search(r'let mut full_redraw = [^;]+;', main).group()
        wake = ROOT / 'userspace/programs/src/bwm/wake.rs'
        source = f'''#[path = "{wake}"] mod wake;
#[test] fn empty_desktop() {{
    {initial}
    // Initial discovery found no clients. No input, wall-clock tick or UI
    // timer will wake this frozen desktop; the scene itself must be pending.
    let initial_windows = false;
    if initial_windows {{ full_redraw = true; }}
    assert_eq!(wake::timeout(full_redraw, false, 0, 0,
        wake::Deadlines::default()), 0, "first scene slept before publishing a receipt");
    assert_eq!(wake::wait_ns(0, 0, None), Some(0));
}}'''
        with tempfile.TemporaryDirectory() as tmp:
            src = Path(tmp) / 'scene.rs'
            out = Path(tmp) / 'scene'
            src.write_text(source)
            command = ['rustc', '--edition=2021', '-Dwarnings', '--test', str(src), '-o', str(out)]
            if os.uname().sysname == 'Darwin': command = ['taskpolicy', '-b'] + command
            subprocess.run(command, check=True)
            subprocess.run([str(out)], check=True)

if __name__ == '__main__': unittest.main()
