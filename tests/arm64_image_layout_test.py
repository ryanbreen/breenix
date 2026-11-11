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
            linker.write_text(linker.read_text().replace('KERNEL_PHYS_BASE =', 'IMAGE_CEILING = 0x41E00000;\nKERNEL_PHYS_BASE =').replace('__kernel_writable_phys_end <= 0x41E00000', '__kernel_writable_phys_end <= IMAGE_CEILING'))
            loader.write_text(loader.read_text().replace('BOOT_STACK_BOTTOM_PHYS: u64 = 0x41E0_0000', 'BOOT_STACK_BOTTOM_PHYS: u64 = arm64_boot_contract::IMAGE_CEILING'))
            contract.write_text(contract.read_text() + '\npub const IMAGE_CEILING: u64 = 0x41E0_0000;\n')
            check.layout(root)
            original = linker.read_text()
            linker.write_text(original.replace('IMAGE_CEILING = 0x41E00000', 'IMAGE_CEILING = 0x41F00000'))
            with self.assertRaisesRegex(ValueError, 'differs'):
                check.layout(root)
            linker.write_text(original)
            original = loader.read_text()
            loader.write_text(original.replace('BOOT_STACK_TOP_PHYS: u64 = 0x4200_0000', 'BOOT_STACK_TOP_PHYS: u64 = 0x4400_0000'))
            with self.assertRaisesRegex(ValueError, 'overlaps per-CPU stacks'):
                check.layout(root)
            loader.write_text(original)
            platform = root / paths[4]
            platform.write_text(platform.read_text().replace('0x5A20_0000', '0x5010_0000'))
            with self.assertRaisesRegex(ValueError, 'DMA block.*leaves'):
                check.layout(root)

    def test_map_figures_and_padding(self):
        def lld_map(end):
            return f'''             VMA              LMA     Size Align Out     In      Symbol
ffff000040600000         40600000     1000  2048 .text.vectors
ffff000040601000         40601000   2ef000  4096 .text
ffff000040a00000         40a00000   180000  4096 .rodata
ffff000040c00000         40c00000    10000  4096 .data
ffff000040c10000         40c10000   d00000  4096 .bss
{0xffff000000000000 + end - 0x10000:x}         {end - 0x10000:x}    10000    16 .bss.stack
'''
        output = io.StringIO()
        with redirect_stdout(output):
            check.map_report(lld_map(0x41a7a6a0), 0x41e00000)
        self.assertIn('text=0x110000 rodata=0x80000', output.getvalue())
        self.assertIn('margin: +0x385960', output.getvalue())
        with redirect_stdout(io.StringIO()), self.assertRaisesRegex(ValueError, 'exceeds image ceiling'):
            check.map_report(lld_map(0x41e79a60), 0x41e00000)


if __name__ == '__main__':
    unittest.main()
