"""Run shared ownership and console restoration with injected device receipts.

The production stack, binding publication, screen upload/bind/flush and native
adapter dispatch run on the host. Hardware latch/pixels stay with metal acceptance.
"""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class DisplayRestore(unittest.TestCase):
    def test_ownership_and_console_device_flow(self):
        display = (ROOT / 'kernel/src/gpu/display.rs').read_text()
        screen = (ROOT / 'kernel/src/gpu/screen.rs').read_text()
        device = (ROOT / 'kernel/src/gpu/dev.rs').read_text()
        source = (ROOT / 'kernel/src/gpu/mod.rs').read_text()
        linear = (ROOT / 'kernel/src/console/display/linear.rs').read_text()
        native = (ROOT / 'kernel/src/gpu/gk208/ce/readback.rs').read_text()
        fixture = (ROOT / 'tests/fixtures/gt710_display_restore.rs').read_text()
        definitions = display[display.index('pub(super) const MODES:'):display.index('#[derive(Clone, Copy)]')]
        definitions += '\n#[derive(Clone, Copy)]\n' + item(display, 'pub(super) struct Binding')
        definitions += '\n' + item(display, 'pub(super) struct Owner')
        definitions += '\n' + item(display, 'pub(super) struct State')
        definitions += '\n' + item(display, 'impl State')
        methods = '\n'.join(item(display, marker) for marker in [
            'pub(super) fn native_console_restored(',
        ])
        functions = '\n'.join(item(display, marker) for marker in [
            'pub(super) fn owner(', 'fn publish_owner(', 'pub(super) fn back(',
            'pub(super) fn ended(', 'pub(super) fn restore()', 'pub(super) fn restored(',
            'pub(super) fn readable(', 'pub(super) fn binding(',
        ])
        screen_methods = '\n'.join(item(screen, marker) for marker in [
            'fn fenced(', 'fn upload(', 'fn flush(', 'fn bind(', 'fn show(',
        ])
        screen_functions = '\n'.join(item(screen, marker) for marker in [
            'pub(super) fn show()', 'pub(super) fn restore()',
            'pub(super) fn bind_saved(', 'pub(super) fn describe()',
        ])
        # Execute actual permission preflights, stopping before any user copy,
        # resource lookup, transition acquisition or device request.
        present = item(display, 'pub(super) fn present_as(')
        present = present[present.index('    if !owner(tgid)'):present.index('    // Wait for')]
        mode = item(display, 'pub(super) fn set_mode(')
        mode = mode[mode.index('    let tgid = caller()?;'):mode.index('    let a: abi::SetMode')]
        readback = item(source, 'fn read_back(')
        readback = readback[readback.index('    let window ='):readback.index('    let _scanout =')]
        replacements = {
            '// DEFINITIONS': definitions, '// CORE_METHODS': methods.replace('pub(super)', 'pub(crate)'),
            '// DISPLAY_FUNCTIONS': functions, '// SCREEN_METHODS': screen_methods,
            '// SCREEN_FUNCTIONS': screen_functions,
            '// FLUSH_RECT': item(device, 'pub fn flush_rect('),
            '// NATIVE_FLUSH_RECT': item(native, 'pub(crate) fn flush_rect(')
                .replace('super::super::adapter::Request', 'crate::Command')
                .replace('super::with_resources', 'with_resources')
                .replace('crate::gpu::dev::Tag', 'crate::Tag'),
            '// LINEAR_METHODS': '\n'.join([item(linear, marker) for marker in [
                'pub fn layout(', 'fn buffer(', 'pub fn available(', 'pub fn write(',
            ]] + ([item(linear, 'pub fn uses_gpu('), item(linear, 'fn gpu(')] if 'fn gpu(' in linear else [])),
            '// PRESENT_PERMISSION': present, '// MODE_PERMISSION': mode,
            '// READBACK_PERMISSION': readback,
        }
        for marker, text in replacements.items():
            fixture = fixture.replace(marker, text)
        fixture = fixture.replace('#[cfg(target_arch = "x86_64")]', '')
        fixture = fixture.replace('#[cfg(target_arch = "aarch64")]\n        { true }', '')
        fixture = fixture.replace('{ !self.disabled.load(Ordering::Acquire) }', '!self.disabled.load(Ordering::Acquire)')
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'restore.rs').write_text(fixture)
            subprocess.run(priority + ['rustc', '--edition=2021', '--crate-name', 'gpu_abi',
                '--crate-type', 'rlib', str(ROOT / 'libs/gpu-abi/src/lib.rs'),
                '-o', str(path / 'libgpu_abi.rlib')], check=True)
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                '--extern', f'gpu_abi={path / "libgpu_abi.rlib"}', str(path / 'restore.rs'),
                '-o', str(path / 'restore')], check=True)
            subprocess.run([str(path / 'restore'), '--nocapture', '--test-threads=1'], check=True)

    def test_x86_entries_reach_shared_ownership_and_console(self):
        handlers = (ROOT / 'kernel/src/syscall/handlers.rs').read_text()
        for marker, call in [('pub fn sys_take_over_display(', 'if crate::gpu::display_active()'),
                             ('pub fn sys_give_back_display(', 'if crate::gpu::display_active()'),
                             ('pub(crate) fn console_owner_exited(', 'if crate::gpu::display_active()')]:
            function = item(handlers, marker)
            before = function[:function.index(call)].rstrip()
            self.assertFalse(before.endswith('#[cfg(target_arch = "aarch64")]'), marker)
        core = (ROOT / 'kernel/src/gpu/mod.rs').read_text()
        self.assertNotIn('#[cfg(target_arch = "aarch64")]\nmod compose;', core)
        display = (ROOT / 'kernel/src/gpu/display.rs').read_text()
        self.assertNotIn('#[cfg(target_arch = "aarch64")]\npub(super) fn give(', display)
        # Once native admission is active, normal console rows must go to the
        # shared backing, including the initially installed linear wrapper.
        linear = (ROOT / 'kernel/src/console/display/linear.rs').read_text()
        write = item(linear, 'pub fn write(')
        self.assertIn('gpu.write(', write)
        self.assertLess(write.index('gpu.write('), write.index('write_bands('))
        self.assertIn('gpu.flush(', linear)
        self.assertIn('gpu.settle(', item(linear, 'pub fn settle('))
        self.assertIn('gpu.read_line(', item(linear, 'pub fn read_line('))


if __name__ == '__main__':
    unittest.main()
