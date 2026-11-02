"""Execute core alias resolution, native translation and submission retirement.

Device storage and window pins are modeled; the resolver, validator, translator,
scene check, handle lookup and retirement logic come from production sources.
"""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class SourceAliases(unittest.TestCase):
    def test_native_alias_authority_and_pool_holds(self):
        core = (ROOT / 'kernel/src/gpu/mod.rs').read_text()
        compose = (ROOT / 'kernel/src/gpu/compose.rs').read_text()
        fixture = (ROOT / 'tests/fixtures/gt710_source_alias.rs').read_text()
        definitions = '\n'.join(item(core, marker) for marker in (
            'struct Entry {', 'struct View {', 'impl View {', 'struct Alias {',
            "impl Resolver for Resolved<'_> {", "impl crate::gk208::encode::Bindings for Resolved<'_> {",
            'fn named('))
        definitions += '\n' + '\n'.join(item(compose, marker) for marker in (
            'struct Scene {', 'fn handle_at(', 'fn scene_of('))
        fixture = fixture.replace('// RESOLUTION', definitions)
        fixture = fixture.replace('// RES_OF', item(core, '    fn res_of('))
        fixture = fixture.replace('// SUB', '#[derive(Default)]\n' + item(core, 'struct Sub {'))
        fixture = fixture.replace('// RETIRE', item(core, '    fn retire_if_done('))
        fixture = fixture.replace('// FAILED_RESET', item(core, '    fn reset_failed('))
        submit = item(core, 'fn submit_stream(')
        if '    let sampled_pins = {' in submit:
            start = submit.index('    let sampled_pins = {')
            end = submit.index('    // Take the context', start)
            sampling = submit[start:end] + '\n    Ok(sampled_pins)'
        else:
            sampling = 'let _ = (aliases, named_ids); Ok(Vec::new())'
        fixture = fixture.replace('// SAMPLE_PREPARE', sampling)
        # A missing retained field remains executable: dropping call-local pins
        # then exposes the early pool release, rather than just failing to compile.
        fixture = fixture.replace('/* SAMPLE_INIT */',
            'sampled: pins,' if '    sampled: Vec<Arc<crate::window::present::Pin>>' in core else '')
        fixture = fixture.replace('/* ALIAS_PIN */',
            'pin: Arc::new(crate::window::present::Pin),' if '    pin: Arc<crate::window::present::Pin>' in core else '')
        fixture = fixture.replace('// RELEASE_LOCAL', '' if '    sampled: Vec<Arc<crate::window::present::Pin>>' in core else 'drop(pins);')
        # Execute the real scene snapshot comparison before pins are constructed.
        scene = (ROOT / 'kernel/src/window/scene.rs').read_text()
        start = scene.index('        let current = keys(&reg)?;')
        end = scene.index('        reg.present.records.try_reserve', start)
        fixture = fixture.replace('// STALE', scene[start:end])
        with tempfile.TemporaryDirectory(prefix='gt710-alias-') as tmp:
            p = Path(tmp)
            native = (ROOT / 'kernel/src/gpu/gk208/encode.rs').read_text()
            native = native.replace('include_bytes!("caps/integer.inputs")', 'include_bytes!("' + str(ROOT / 'kernel/src/gpu/gk208/caps/integer.inputs') + '")')
            native += '''\npub(crate) fn target(id:u32, format:u32)->Option<Target> {
                Some(Target {id,address:(id as u64)*0x100000,width:64,height:64,format,tile_mode:0x30})
            }\n'''
            (p / 'encode.rs').write_text(native)
            # Exercise backend 6 on an ARM host without changing the native encoder.
            fixture = fixture.replace('#[cfg(target_arch = "x86_64")]', '')
            fixture = fixture.replace('NATIVE', str(p / 'encode.rs')).replace('ROOT', str(ROOT))
            (p / 'alias.rs').write_text(fixture)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', '--crate-name', 'gpu_abi',
                '--crate-type', 'rlib', '--cfg', 'feature="alloc"', str(ROOT / 'libs/gpu-abi/src/lib.rs'),
                '-o', str(p / 'libgpu_abi.rlib')], check=True)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', '--test',
                '--extern', f'gpu_abi={p / "libgpu_abi.rlib"}', str(p / 'alias.rs'), '-o', str(p / 'alias')], check=True)
            subprocess.run([str(p / 'alias'), '--test-threads=1'], check=True)

    def test_composition_moves_sampled_pins_into_the_queued_submission(self):
        core = (ROOT / 'kernel/src/gpu/mod.rs').read_text()
        compose = (ROOT / 'kernel/src/gpu/compose.rs').read_text()
        self.assertIn('pin: pin.clone()', compose)
        native_handoff = compose[compose.index('    submitted?;'):compose.index('    if let Some(point) = forced')]
        self.assertIn('drop(aliases)', native_handoff)
        self.assertIn('source.pin.take()', native_handoff)
        submit = item(core, 'fn submit_stream(')
        self.assertIn('sampled: sampled_pins', submit)
        self.assertIn('named_ids.contains(&x.id)', submit)
        self.assertIn('x.pin.clone()', submit)
        # Pool-pin destruction may drop Target and re-enter the core. It must
        # happen after the lock scope, outside retire_if_done's locked caller.
        locking = item(core, 'fn with_core<R>')
        self.assertIn('core.as_mut().map(|c| core::mem::take(&mut c.retired_pins))', locking)
        self.assertLess(locking.index('crate::per_cpu::preempt_enable();'), locking.index('drop(retired_pins)'))


if __name__ == '__main__':
    unittest.main()
