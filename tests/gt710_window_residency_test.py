"""Host execution of window backing, quota guards and native resource retirement.

Physical frames and process mappings are modeled. Production sizing, charge guards,
native allocation/VM and resource entry points run without a GPU or a guest boot.
"""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class WindowResidency(unittest.TestCase):
    def test_backend_six_backing_and_retirement(self):
        core = (ROOT / 'kernel/src/gpu/mod.rs').read_text()
        window = (ROOT / 'kernel/src/gpu/window_targets.rs').read_text()
        console = (ROOT / 'kernel/src/gpu/console.rs').read_text()
        native = (ROOT / 'kernel/src/gpu/gk208/resource.rs').read_text()
        # Run the allocation/adoption portion of Target::make, with the device
        # boundary below connected to the real native resource entry points.
        make = item(window, '    fn make(')
        allocation = make[make.index('        let bytes ='):make.index('        let published =')]
        fixture = (ROOT / 'tests/fixtures/gt710_window_residency.rs').read_text()
        fixture = fixture.replace('ROOT', str(ROOT))
        fixture = fixture.replace('// BACKING_LEN', item(window, 'pub fn backing_len('))
        fixture = fixture.replace('// DEVICE_FRAMES', item(core, 'fn device_frames('))
        fixture = fixture.replace('// WINDOW_ALLOCATION', allocation)
        fixture = fixture.replace('// PITCH', 'pub(super) ' + item(console, 'fn pitch_of('))
        fixture = fixture.replace('// CHARGE_COPY', item(
            (ROOT / 'kernel/src/gpu/dma.rs').read_text(), '    pub(super) fn charge_copy(').replace('pub(super)', 'pub(crate)'))
        # The adapter's storage receipt retains backing until its acknowledgement
        # is reaped. Replace only that transport, not native storage retirement.
        entry_points = '\n'.join(item(native, marker) for marker in (
            '    pub fn resource_create(', '    pub fn attach_backing(',
            '    pub fn resource_unref(', '    pub(crate) fn ownership_census('))
        with tempfile.TemporaryDirectory(prefix='gt710-window-') as tmp:
            p = Path(tmp)
            resource = native.replace('#[path = "present.rs"]',
                f'#[path = "{ROOT / "kernel/src/gpu/gk208/present.rs"}"]')
            resource += '\nmod host_entry_points {\nuse super::*;\nuse crate::gpu::{dev::Tag,dma::Pages};\nuse crate::gk208::{ce,adapter::Request};\n' + entry_points + '\n}\npub(crate) use host_entry_points::*;\n'
            (p / 'resource.rs').write_text(resource)
            fixture = fixture.replace('// RESOURCE_MODULE',
                f'#[allow(dead_code)] // Full storage module; tests exercise allocation and retirement.\n#[path = "{p / "resource.rs"}"] pub mod resource;')
            (p / 'window.rs').write_text(fixture)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', '--test',
                str(p / 'window.rs'), '-o', str(p / 'window')], check=True)
            subprocess.run([str(p / 'window'), '--test-threads=1'], check=True)


if __name__ == '__main__':
    unittest.main()
