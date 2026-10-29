"""Execute native restart against dirty saved RAMFC and USERD state, without a boot."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class RestartState(unittest.TestCase):
    def test_restart_rebuilds_constructor_fifo_state_and_preserves_health_error(self):
        source = (ROOT / 'kernel/src/gpu/gk208/gr/channel.rs').read_text()
        restart = item(source, '        pub(crate) fn restart(&mut self)')
        health = item(source, '        pub(crate) fn healthy(&self)')
        for old, new in [('super::super::topology', 'topology'),
                         ('super::super::ctxctl', 'ctxctl'),
                         ('super::super::hardware_error', 'hardware_error')]:
            restart = restart.replace(old, new)
        # Run the constructor's RAMFC writer as well as restart. Allocation and
        # firmware startup are substituted; saved words and reset writes are real.
        if '    fn init_fifo(' in source:
            initializer = item(source, '    fn init_fifo(')
            construct = 'init_fifo(&mut regs, c.total, instance, userd, commands, id)?;'
        else:
            initializer = ''
            start = source.index('                for (offset, value) in [', source.index('let instance = context.space().instance.range.start;'))
            construct = item(source[start:], '                for (offset, value) in [')
            construct = construct.replace('h.regs, h.total', '&mut regs, c.total')
        harness = (ROOT / 'tests/fixtures/gt710_restart_state.rs').read_text()
        for key, value in [('INITIALIZER', initializer), ('CONSTRUCT', construct),
                           ('RESTART', restart), ('HEALTH', health)]:
            harness = harness.replace('// ' + key, value)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / 'restart.rs').write_text(harness)
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                           str(path / 'restart.rs'), '-o', str(path / 'restart')], check=True)
            subprocess.run([str(path / 'restart')], check=True)


if __name__ == '__main__':
    unittest.main()
