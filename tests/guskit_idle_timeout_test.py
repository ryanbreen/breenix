"""Exercise Gus Kit's dispatch result handling on an ordinary idle timeout."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class GusKitIdleTimeout(unittest.TestCase):
    def test_idle_timeout_keeps_window_alive_and_other_errors_remain_fatal(self):
        source = (ROOT / 'userspace/programs/src/guskit.rs').read_text()
        start = source.index('        let count=result.')
        end = source.index('\n', start)
        expression = source[start:end]
        program = r'''
#[derive(Debug)] enum Errno { ETIMEDOUT, EIO }
#[derive(Debug)] enum Error { Os(Errno) }
impl std::fmt::Display for Error {
 fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { match self { Self::Os(errno) => write!(f, "{errno:?}") } }
}
fn main() {
 let error = if std::env::args().nth(1).as_deref() == Some("timeout") { Errno::ETIMEDOUT } else { Errno::EIO };
 let result: Result<usize, Error> = Err(Error::Os(error));
 // EXPRESSION
 assert_eq!(count, 0);
}
'''.replace('// EXPRESSION', expression)
        with tempfile.TemporaryDirectory(prefix='guskit-idle-') as directory:
            path = Path(directory)
            (path / 'main.rs').write_text(program)
            compiler = (['taskpolicy', '-b'] if sys.platform == 'darwin' else []) + ['rustc', '--edition=2021', '-Dwarnings']
            subprocess.run(compiler + [str(path / 'main.rs'), '-o', str(path / 'check')], check=True)
            timeout = subprocess.run([str(path / 'check'), 'timeout'], capture_output=True, text=True)
            self.assertEqual(timeout.returncode, 0, timeout.stderr)
            other = subprocess.run([str(path / 'check'), 'other'], capture_output=True, text=True)
            self.assertNotEqual(other.returncode, 0)
            self.assertIn('EIO', other.stderr)


if __name__ == '__main__':
    unittest.main()
