"""The supervisor that stamps injected events must use its session clock."""
from pathlib import Path
import os
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class SupervisorClock(unittest.TestCase):
    def test_configure_installs_the_prepared_clock_before_serving_input(self):
        source = (ROOT / 'libs/libuitest/src/supervisor_runtime.rs').read_text()
        configure = source.split('let root = Path::new(environment.as_ref().unwrap().config_root());', 1)[1]
        configure = configure.split('channel.queue(&Message::Configured)?;', 1)[0]
        # Execute the production clock-configuration statements from this branch.
        setup = '\n'.join(line for line in configure.splitlines()
                          if 'std::env::set_var("BREENIX_UI_CLOCK"' in line)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            harness = ('#[path = ' + repr(str(ROOT / 'libs/libbreenix/src/ui_clock.rs')).replace("'", '"') + ']\n'
                       'mod ui_clock;\nfn main() {\n'
                       'let root = std::path::Path::new(std::env::args_os().nth(1).as_ref().unwrap()).to_path_buf();\n'
                       'ui_clock::ClockPage::create(&root.join("ui-clock"), 1000).unwrap();\n'
                       'let page = ui_clock::ClockPage::open(&root.join("ui-clock")).unwrap();\n'
                       + setup + '\n'
                       'assert_eq!(ui_clock::now_ns(), 0, "supervisor injected real time");\n'
                       'page.advance(600).unwrap();\n'
                       'assert_eq!(ui_clock::now_ns(), 600_000_000);\n'
                       'assert_eq!(ui_clock::real_clock_reads(), 0);\n}\n')
            (path / 'clock.rs').write_text(harness)
            built = subprocess.run(['taskpolicy', '-b', 'rustc', '--edition=2021', '-Adead_code', str(path / 'clock.rs'), '-o', str(path / 'clock')], capture_output=True, text=True)
            self.assertEqual(built.returncode, 0, built.stdout + built.stderr)
            env = {key: value for key, value in os.environ.items() if key != 'BREENIX_UI_CLOCK'}
            result = subprocess.run([str(path / 'clock'), str(path)], env=env, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == '__main__':
    unittest.main()
