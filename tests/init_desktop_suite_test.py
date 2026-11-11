"""Gus Kit selections enter PID-1 init before the GUI runner is spawned."""
import subprocess
import tempfile
import unittest
from pathlib import Path

from gt710_gr_live_unwind_test import item
import metal_smb_test

ROOT = Path(__file__).resolve().parents[1]


class DesktopSuiteLaunch(unittest.TestCase):
    def test_staging_selects_init_desktop_only_for_guskit(self):
        fixture = metal_smb_test.SmbStagingTest()
        for case in (None, 'guskit', 'guskit/smb-save-open', 'bterm'):
            with self.subTest(case=case), fixture.gui_root() as (image, read, config):
                import metal_root
                metal_root.stage_root(image, 'suite', 'gui-apps', {}, smb_config=config, case=case)
                content = read('cat /etc/breenix/desktop-suite').stdout
                self.assertEqual(content, b'gui-apps\n' if case != 'bterm' else b'')
                if case != 'bterm':
                    metal_root.stage_root(image, 'suite', 'gui-apps', {}, smb_config=config, case='bterm')
                    self.assertEqual(read('cat /etc/breenix/desktop-suite').stdout, b'')
        kernel = (ROOT / 'kernel/src/main.rs').read_text()
        self.assertTrue('name == "suite-users" || name == "suite-gui-apps"' in kernel)
        self.assertTrue('id == "users" || id == "gui-apps"' in kernel)

    def test_launch_waits_for_shares_and_excludes_nested_init(self):
        source = (ROOT / 'userspace/programs/src/init.rs').read_text()
        decision = item(source, 'fn desktop_suite_ready(')
        harness = r'''
// DECISION
#[test]
fn launch_decision() {
    for state in [None, Some("waiting"), Some("running"), Some("backoff"), Some("stopping")] {
        assert!(!desktop_suite_ready(1, true, state));
    }
    for state in ["done", "failed", "stopped", "crash-loop"] {
        assert!(desktop_suite_ready(1, true, Some(state)));
        assert!(!desktop_suite_ready(2, true, Some(state)));
        assert!(!desktop_suite_ready(1, false, Some(state)));
    }
}
'''.replace('// DECISION', decision)
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / 'launch.rs').write_text(harness)
            subprocess.run(['taskpolicy', '-b', 'rustc', '--edition=2021', '-Dwarnings', '--test',
                            str(path / 'launch.rs'), '-o', str(path / 'launch')], check=True)
            subprocess.run([str(path / 'launch')], check=True)


if __name__ == '__main__':
    unittest.main()
