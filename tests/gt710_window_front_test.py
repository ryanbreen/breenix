"""Run window publication, replay and capture against a controlled native queue.

Production window operations and target methods run with modeled user memory,
waits and device completion. The native translator is unchanged; preparation and
execution are device boundaries, not evidence of hardware pixels.
"""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class WindowFronts(unittest.TestCase):
    def test_backend_six_window_protocol(self):
        target = (ROOT / 'kernel/src/gpu/window_targets.rs').read_text()
        ops = (ROOT / 'kernel/src/window/ops.rs').read_text()
        window = (ROOT / 'kernel/src/window/mod.rs').read_text()
        present = (ROOT / 'kernel/src/window/present.rs').read_text()
        fixture = (ROOT / 'tests/fixtures/gt710_window_front.rs').read_text()
        core = (ROOT / 'kernel/src/gpu/mod.rs').read_text()
        fixture = fixture.replace('// CORE_READ_LIFETIME', '\n'.join(item(core, m) for m in (
            '    fn unhold(', '    fn end_reading(', '    fn sync_ended(')))
        releases = item(core, 'fn release_other(')
        if 'fn release_cpu_read(' in core:
            releases += '\n' + item(core, 'fn release_cpu_read(')
        fixture = fixture.replace('// READ_RELEASES', releases)
        fixture = fixture.replace('// WRITER', '\n'.join(item(target, m) for m in (
            'pub(super) struct Writer {', 'impl Writer {')) + '\n#[derive(Clone)]\n' +
            item(target, 'pub struct Seal {') + '\n' + item(target, 'impl Seal {'))
        methods = '\n'.join(item(target, m) for m in (
            '    pub fn seal_writes(', '    pub fn seal(', '    pub fn available(',
            '    pub fn idle(', '    pub fn identity(', '    pub fn ready(',
            '    pub fn clear(', '    pub fn copy_from(', '    fn execute(',
            '    fn execute_taken(', '    pub fn read_to(', '    fn read_to_inner('))
        fixture = fixture.replace('// TARGET_METHODS', methods)
        bindings = item(target, "struct Bindings<'a>") + '\n' + item(target, "impl Bindings<'_>")
        bindings += '\n' + item(target, "impl encode::Bindings for Bindings<'_>")
        for marker in ("impl Resolver for Bindings<'_>", "impl crate::gk208::encode::Bindings for Bindings<'_>"):
            if marker in target:
                bindings += '\n' + item(target, marker)
        fixture = fixture.replace('// BINDINGS', bindings)
        definitions = ''
        for marker in ('pub enum BufState {', 'pub struct Buffer {', 'pub enum Damage {',
                       'impl Damage {', 'pub fn bounds(', 'pub struct Waits {', 'pub struct Pending {'):
            prefix = '#[derive(Clone, Debug, PartialEq, Eq)]\n' if marker.startswith('pub enum') else ''
            if marker.startswith('pub struct Pending'):
                prefix = '#[derive(Clone)]\n'
            definitions += prefix + item(window, marker) + '\n'
        fixture = fixture.replace('// WINDOW_RECORDS', definitions)
        fixture = fixture.replace('// PIN', '\n'.join(item(present, m) for m in (
            'pub struct Pin {', 'impl Pin {', 'impl Drop for Pin {')))
        fixture = fixture.replace('// OPS', '\n'.join(item(ops, m) for m in (
            'fn deadline(', 'fn wait(', 'pub fn unpin(', 'fn buffer_bytes(',
            'fn pick_free(', 'fn since(', 'fn replay(', 'pub fn acquire(',
            'struct Gate {', 'impl Gate {', 'impl Drop for Gate {',
            'fn overlaps(', 'fn rect_in(', 'pub fn commit(', 'pub fn publish_ready(',
            'fn pending_decision(', 'fn pixels_out(', 'fn bytes_out(',
            'struct TreePin {', 'impl Drop for TreePin {', 'pub fn capture(')))
        # Exercise backend 6 on the ARM host, without selecting a guest target.
        fixture = fixture.replace('#[cfg(target_arch = "x86_64")]', '').replace('pub(super)', 'pub(crate)')
        fixture = fixture.replace('#[cfg(not(target_arch = "x86_64"))]', '#[cfg(any())]')
        with tempfile.TemporaryDirectory(prefix='gt710-window-front-') as tmp:
            p = Path(tmp)
            native = (ROOT / 'kernel/src/gpu/gk208/encode.rs').read_text()
            native = native.replace('include_bytes!("caps/integer.inputs")',
                'include_bytes!("' + str(ROOT / 'kernel/src/gpu/gk208/caps/integer.inputs') + '")')
            native += '''
pub(crate) fn target(id:u32, format:u32)->Option<Target> {
    Some(Target {id,address:u64::from(id)*0x100000,width:8,height:8,format,tile_mode:0x30})
}
pub(crate) fn prepare(frame:Frame, backing:alloc::sync::Arc<crate::Pages>)
    ->Result<Vec<Chunk>, &'static str> {
    assert!(!backing.0.borrow().is_empty());
    Ok(frame.chunks)
}
'''
            (p / 'encode.rs').write_text(native)
            fixture = fixture.replace('NATIVE', str(p / 'encode.rs')).replace('ROOT', str(ROOT))
            (p / 'front.rs').write_text(fixture)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            for crate in ('gpu-abi', 'breengel-abi'):
                name = crate.replace('-', '_')
                subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', '--crate-name', name,
                    '--crate-type', 'rlib', '--cfg', 'feature="alloc"', str(ROOT / f'libs/{crate}/src/lib.rs'),
                    '-o', str(p / f'lib{name}.rlib')], check=True)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', '--test',
                '--extern', f'gpu_abi={p / "libgpu_abi.rlib"}',
                '--extern', f'breengel_abi={p / "libbreengel_abi.rlib"}',
                str(p / 'front.rs'), '-o', str(p / 'front')], check=True)
            subprocess.run([str(p / 'front'), '--test-threads=1'], check=True)


if __name__ == '__main__':
    unittest.main()
