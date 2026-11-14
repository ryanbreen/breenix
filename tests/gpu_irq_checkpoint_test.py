"""The IRQ workload keeps exact scanout checks and its end-only checkpoint (#2970)."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class IrqCheckpoint(unittest.TestCase):
    def test_irq_view_does_not_request_an_undeclared_intent(self):
        common = (ROOT / 'userspace/programs/src/suite_gpu/common.rs').read_text()
        irq = item((ROOT / 'userspace/programs/src/suite_gpu/isolation.rs').read_text(), 'pub fn irq_masked_bound(')
        presentation = next(line.strip() for line in irq.splitlines() if 'present_view(ctx, t)?' in line)
        alias = '\n'.join(line for line in irq.splitlines() if 'use super::common::' in line)
        alias = alias.replace('super::common::', 'crate::')
        intent = (ROOT / 'libs/libbreenix/src/suite_intent.rs').read_text()
        intent = intent[intent.index('#[derive'):intent.index('pub(super) fn configured')]
        intent = intent.replace('include_str!("../../../scripts/metal/intent-pictures.tsv")',
                                'include_str!("' + str(ROOT / 'scripts/metal/intent-pictures.tsv') + '")')
        code = (ROOT / 'tests/fixtures/gpu_irq_checkpoint.rs').read_text()
        code = code.replace('// INTENT', intent).replace('// ALIAS', alias).replace('// IRQ', presentation)
        code = code.replace('// VIEW', common[common.index('pub fn present_view('):common.index('/// A frame for target')])
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            (p / 'check.rs').write_text(code)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', '--crate-name', 'gpu_abi', '--crate-type', 'rlib',
                                      str(ROOT / 'libs/gpu-abi/src/lib.rs'), '-o', str(p / 'libgpu_abi.rlib')], check=True)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', str(p / 'check.rs'),
                                      '--extern', f'gpu_abi={p}/libgpu_abi.rlib', '-o', str(p / 'check')], check=True)
            subprocess.run([str(p / 'check')], check=True)


if __name__ == '__main__':
    unittest.main()
