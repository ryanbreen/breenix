"""Execute production point dispatch and output restoration against latched banks."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


def compile_run(source, arch='x86_64', success=True):
    for target in ['x86_64', 'aarch64']:
        source = source.replace(f'target_arch = "{target}"',
                                'all()' if target == arch else 'any()')
    with tempfile.TemporaryDirectory(prefix='output-perturb-') as directory:
        path = Path(directory)
        (path / 'check.rs').write_text(source)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings',
                                  str(path / 'check.rs'), '-o', str(path / 'check')], check=True)
        result = subprocess.run([str(path / 'check')], capture_output=True, text=True)
        if (result.returncode == 0) != success:
            raise AssertionError(result.stdout + result.stderr)


def tables():
    return '\n'.join(item((ROOT / f'libs/{abi}/src/lib.rs').read_text(), f'pub mod {name}')
                     for abi, name in [('gpu-abi', 'test_point'), ('breengel-abi', 'tc_point')])


class OutputPerturb(unittest.TestCase):
    def test_point_ownership_has_no_collision_and_rejects_32(self):
        # IRQ_HOLD and EMIT_DIAGNOSTIC in gpu-abi are documented aliases of
        # BreenGL points, not independently allocated GPU point numbers.
        check = r'''
fn main() {
    let overlap:Vec<_>=(0..=tc_point::LAST.max(test_point::CRED_ORPHAN))
        .filter(|&p| tc_point::defined(p) && test_point::defined(p)).collect();
    assert_eq!(overlap, [test_point::IRQ_HOLD, test_point::EMIT_DIAGNOSTIC]);
    assert_eq!(tc_point::IRQ_HOLD, test_point::IRQ_HOLD);
    assert_eq!(tc_point::EMIT_DIAGNOSTIC, test_point::EMIT_DIAGNOSTIC);
}
'''
        source = tables()
        compile_run(source + check)
        # Mutate the real allocation and its defined predicate together; putting
        # display perturbation on CRED_ORPHAN must trip the ownership assertion.
        window = item(source, 'pub mod tc_point')
        if 'DISPLAY_OUTPUT_PERTURB' in window:
            bad = window.replace('DISPLAY_OUTPUT_PERTURB: u32 = 35',
                                 'DISPLAY_OUTPUT_PERTURB: u32 = 32')
        else:
            bad = window.replace('pub const fn defined(point: u32) -> bool {',
                                 'pub const fn defined(point: u32) -> bool {\n'
                                 '        if point == 32 { return true; }')
        compile_run(source.replace(window, bad) + check, success=False)

    def test_one_shot_restore_counter_and_backend_refusals(self):
        crc = (ROOT / 'kernel/src/gpu/gk208/display_crc.rs').read_text()
        self.assertIn('pub struct Perturbation', crc, 'no output perturbation producer')
        control = (ROOT / 'kernel/src/test_control.rs').read_text()
        dev = (ROOT / 'kernel/src/gpu/dev.rs').read_text()
        fixture = (ROOT / 'tests/fixtures/gt710_output_perturb.rs').read_text()
        fixture = fixture.replace('TABLES', tables())
        fixture = fixture.replace('DISPATCH', item(control, 'pub fn sys_test_control('))
        fixture = fixture.replace('DEFINED', item(control, 'fn defined('))
        fixture = fixture.replace('SUPPORTED', item(dev, 'pub fn output_perturb_supported('))
        fixture = fixture.replace('ARM_RESTORE', item(dev, 'pub fn restore_output_perturbation('))
        fixture = fixture.replace('DRIVER', str(ROOT / 'kernel/src/gpu/gk208'))
        fixture = fixture.replace('POINT"', str(ROOT / 'kernel/src/test_control/point.rs') + '"')
        fixture = fixture.replace('CHECKPOINT', item(crc, 'pub fn perturb_checkpoint('))
        fixture = fixture.replace('ABI/', str(ROOT / 'libs/gpu-abi/src') + '/')
        for arch in ['x86_64', 'aarch64']:
            with self.subTest(arch=arch):
                compile_run(fixture, arch)


if __name__ == '__main__':
    unittest.main()
