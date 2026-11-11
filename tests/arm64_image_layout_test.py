#!/usr/bin/env python3
"""Host checks for the ARM64 image layout guard and LLD map report."""
import importlib.util
import io
from contextlib import redirect_stdout
from pathlib import Path
import shutil
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('arm64_layout', ROOT / 'scripts/check-arm64-image-layout.py')
check = importlib.util.module_from_spec(spec)
spec.loader.exec_module(check)


class LayoutTest(unittest.TestCase):
    def test_current_tree_has_shared_layout_contract(self):
        check.layout(ROOT)

    def test_static_geometry_and_contract(self):
        paths = ['kernel/src/arch_impl/aarch64/linker.ld', 'parallels-loader/src/page_tables.rs',
                 'libs/arm64-boot-contract/src/lib.rs', 'kernel/src/arch_impl/aarch64/constants.rs',
                 'kernel/src/platform_config.rs']
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for path in paths:
                (root / path).parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(ROOT / path, root / path)
            linker = root / paths[0]
            loader = root / paths[1]
            contract = root / paths[2]
            check.layout(root)
            original = linker.read_text()
            linker.write_text(original.replace('KERNEL_IMAGE_LIMIT_PHYS = 0x42E00000', 'KERNEL_IMAGE_LIMIT_PHYS = 0x42F00000'))
            with self.assertRaisesRegex(ValueError, 'differs'):
                check.layout(root)
            linker.write_text(original)
            original = contract.read_text()
            contract.write_text(original.replace('LOADER_BOOT_STACK_TOP_PHYS: u64 = 0x4300_0000', 'LOADER_BOOT_STACK_TOP_PHYS: u64 = 0x4400_0000'))
            with self.assertRaisesRegex(ValueError, 'overlaps per-CPU stacks'):
                check.layout(root)
            contract.write_text(original)
            platform = root / paths[4]
            platform.write_text(platform.read_text().replace('0x5A20_0000', '0x5010_0000'))
            with self.assertRaisesRegex(ValueError, 'DMA block.*leaves'):
                check.layout(root)

    def test_image_literal_boundaries(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / 'kernel/src/arch_impl/aarch64/example.rs'
            source.parent.mkdir(parents=True)
            source.write_text('const A: u64 = 0x4020_0000;\nconst B: u64 = 0x42df_ffff;\n')
            with self.assertRaisesRegex(ValueError, r'example.rs:1: 0x4020_0000[\s\S]*example.rs:2: 0x42df_ffff'):
                check.check_image_literals(root, 0x40200000, 0x42e00000)
            source.write_text('const A: u64 = 0x401f_ffff;\nconst B: u64 = 0x42e0_0000;\n')
            check.check_image_literals(root, 0x40200000, 0x42e00000)

    def test_real_maps_cover_production_and_testing_profiles(self):
        fixtures = ROOT / 'tests/fixtures/arm64-image-layout'
        # These historical maps always use the old ceiling, even after the
        # live layout moves: the overflowing testing image must stay refused.
        cases = [
            ('1055dfd2-testing', '+0x156b00', 'text=0x11b24 rodata=0x12e630', False),
            ('2b967324-production', '+0x186970', 'text=0x1085c rodata=0x1c4328', False),
            ('2b967324-testing', '-0xa8d20', 'text=0x1d8e28 rodata=0xb9bb0', True),
        ]
        for name, margin, padding, overflow in cases:
            with self.subTest(profile=name):
                output = io.StringIO()
                with redirect_stdout(output):
                    if overflow:
                        with self.assertRaisesRegex(ValueError, 'exceeds image ceiling'):
                            check.map_report((fixtures / f'{name}.map').read_text(), 0x41e00000)
                    else:
                        check.map_report((fixtures / f'{name}.map').read_text(), 0x41e00000)
                self.assertIn(f'margin: {margin} bytes', output.getvalue())
                self.assertIn(f'2 MiB W^X padding: {padding}', output.getvalue())



if __name__ == '__main__':
    unittest.main()
