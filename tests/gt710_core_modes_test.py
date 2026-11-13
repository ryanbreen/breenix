"""The GPU core publishes the mode table installed for its device."""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[1]


class CoreModes(unittest.TestCase):
    def test_device_mode_table(self):
        gpu = ROOT / 'kernel/src/gpu'
        display = (gpu / 'display.rs').read_text()
        core = (gpu / 'mod.rs').read_text()
        targets = (gpu / 'window_targets.rs').read_text()
        native = (gpu / 'gk208/mod.rs').read_text()
        process = (gpu / 'gk208/ce/process.rs').read_text()
        for source in (display, core, targets):
            self.assertNotRegex(source, r'\bMODES\s*(?:\[|\.iter\s*\()')
        self.assertRegex(display, r'pub\s+modes:\s*\[abi::Mode;\s*2\]')
        self.assertIn('a.modes = c.display.modes;', display)
        self.assertIn('c.display.modes[c.display.mode as usize]', display)
        self.assertIn('c.display.modes.iter()', display)
        self.assertIn('c.display.modes[c.display.mode as usize]', core)
        self.assertIn('c.display.modes[c.display.mode as usize]', targets)
        self.assertIn('modeset::display_modes(&data)?', native)
        self.assertIn('install_native(native,', process)
        self.assertRegex(core, r'core\.display\.modes\s*=\s*modes')
        panel = re.search(r'const PANEL_MODES:\s*\[abi::Mode;\s*2\]\s*=\s*\[(.*?)\];', display, re.S)
        self.assertIsNotNone(panel)
        values = re.findall(r'abi::Mode\s*\{\s*width:\s*(\d+),\s*height:\s*(\d+),\s*scale:\s*(\d+),\s*reserved:\s*(\d+)\s*\}', panel.group(1))
        self.assertEqual(values, [('1280', '800', '1', '0'), ('2560', '1600', '2', '0')])
        self.assertRegex(display, r'const KERNEL_MODE:\s*usize\s*=\s*1;')
        self.assertIn('modes: PANEL_MODES', display)


if __name__ == '__main__':
    unittest.main()
