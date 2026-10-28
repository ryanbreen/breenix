"""Exercise production GR construction, pair publication and teardown on host IO.

This checks ownership and register/table bytes, without claiming GPU execution.
"""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class TwoChannels(unittest.TestCase):
    def test_private_contexts_and_pair_lifetime(self):
        source = (ROOT / 'kernel/src/gpu/gk208/gr/channel.rs').read_text()
        native = source[source.index('mod native {'):]
        methods = '\n'.join(item(native, marker) for marker in [
            'fn new(', 'fn new_channel(', 'fn open_second(', 'fn activate(',
            'fn activate_channel(', 'fn quiesce(', 'fn quiesce_channel(',
            'fn finish(', 'fn finish_channel(',
        ])
        state = item(native, 'pub(in crate::gk208::gr) struct Session')
        methods = methods.replace('super::super::ctxctl', 'ctxctl').replace('super::super::hardware_error', 'hardware_error')
        state = state.replace('pub(in crate::gk208::gr)', 'pub(crate)')
        constants = native[native.index('    const COMMAND:'):native.index('    pub(in crate::gk208::gr) struct Session')]
        fixture = (ROOT / 'tests/fixtures/gt710_gr_two_channel.rs').read_text()
        fixture = fixture.replace('ROOT', str(ROOT)).replace('// STATE', constants + state)
        fixture = fixture.replace('// METHODS', methods)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'pair.rs').write_text(fixture)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                                      str(path / 'pair.rs'), '-o', str(path / 'pair')], check=True, cwd=ROOT)
            subprocess.run([str(path / 'pair'), '--nocapture'], check=True)


if __name__ == '__main__':
    unittest.main()
