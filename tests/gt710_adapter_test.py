"""Run native adapter lifecycle checks without a device or a kernel boot."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]

class NativeAdapter(unittest.TestCase):
    def test_native_device_error_dispatch(self):
        source = (ROOT / 'kernel/src/gpu/dev.rs').read_text()
        function = source.split('pub fn undefined_command(', 1)[1].split('\npub fn ', 1)[0]
        self.assertIn('backend::GK208 =>', function)

    def test_control_checks_its_own_directory_before_publication(self):
        source = (ROOT / 'kernel/src/gpu/gk208/gr/channel.rs').read_text()
        arm = source.split('Operation::Undefined =>', 1)[1].split('Operation::Draw', 1)[0]
        self.assertIn('error::address(sequence)?', arm)
        self.assertIn('s.context.space().directory.range.start', arm)
        self.assertIn('address / (1 << 27) * 8', arm)
        self.assertIn('read(&mut regs, self.total, pde)? != 0', arm)
        self.assertIn('read(&mut regs, self.total, pde + 4)? != 0', arm)
        self.assertIn('return Err("device-error control PDE is mapped")', arm)

    def test_native_completion_and_room(self):
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as directory:
            out = Path(directory)
            subprocess.run(priority + ['rustc', '--edition=2021', '--crate-name', 'gpu_abi',
                '--crate-type', 'rlib', str(ROOT / 'libs/gpu-abi/src/lib.rs'),
                '-o', str(out / 'libgpu_abi.rlib')], check=True, cwd=ROOT)
            source = (ROOT / 'tests/gt710_adapter_test.rs').read_text()
            source = source.replace('../../kernel/', str(ROOT / 'kernel') + '/')
            source = source.replace('../kernel/', str(ROOT / 'kernel') + '/')
            (out / 'adapter.rs').write_text(source)
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                '--extern', f'gpu_abi={out / "libgpu_abi.rlib"}',
                str(out / 'adapter.rs'), '-o', str(out / 'adapter')], check=True, cwd=ROOT)
            subprocess.run([str(out / 'adapter')], check=True)

if __name__ == '__main__':
    unittest.main()
