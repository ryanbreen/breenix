"""The logical and firmware consoles may have identical native geometry."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class ConsoleRole(unittest.TestCase):
    def test_bar_offset_follows_console_role(self):
        native = (ROOT / 'kernel/src/gpu/gk208/native.rs').read_text()
        function = item(native, 'pub(in crate::gk208) fn console_bar_offset(')
        function = function.replace('pub(in crate::gk208) ', '')
        fixture = r'''
struct ConsoleSurface { width: usize, height: usize, logical: bool }
// OFFSET
fn main() {
    let firmware = ConsoleSurface { width: 1280, height: 1024, logical: false };
    let logical = ConsoleSurface { logical: true, ..firmware };
    assert_eq!((firmware.width, firmware.height), (logical.width, logical.height));
    assert!(!firmware.logical && logical.logical);
    assert_eq!(console_bar_offset(&firmware, 0x10000), 0x10000);
    assert_eq!(console_bar_offset(&logical, 0x10000), 16 << 20);
}
'''.replace('// OFFSET', function)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / 'role.rs').write_text(fixture)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings',
                                      str(path / 'role.rs'), '-o', str(path / 'role')], check=True)
            subprocess.run([str(path / 'role')], check=True)


if __name__ == '__main__':
    unittest.main()
