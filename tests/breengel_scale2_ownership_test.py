"""Replay BreenGL stage preparation and case custody through production display transitions."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from gt710_display_restore_test import fixture_source
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class Scale2Ownership(unittest.TestCase):
    def test_runner_sequence(self):
        stage = (ROOT / 'libs/libbreenix/src/suite_stage.rs').read_text()
        display = (ROOT / 'kernel/src/gpu/display.rs').read_text()
        runner = (ROOT / 'libs/libbreenix/src/suite.rs').read_text()
        core = (ROOT / 'kernel/src/gpu/mod.rs').read_text()
        intent = (ROOT / 'libs/libbreenix/src/suite_intent.rs').read_text()
        fixture = fixture_source()
        fixture = fixture.replace('fn caller()->Result<u64,i64>{Ok(99)}',
                                  'fn caller()->Result<u64,i64>{Ok(GROUP.with(|g|g.get()))}')
        fixture = fixture.replace('impl Transition {', 'impl Transition {fn for_mutation(_:u64)->Result<Self,i64>{Ok(Self)}')
        fixture = fixture.replace('pub(super) struct Transition;', 'pub(super) struct Transition;impl Drop for Transition{fn drop(&mut self){with_core(|c|c.display.changing=false);}}')
        fixture = fixture.replace('pub struct Presentation;',
            'pub fn take_display(_:u64,_:usize)->Result<(),i64>{Ok(())}\n'
            'pub fn take_display_nested(_:u64,_:usize)->Result<(),i64>{Ok(())}\n'
            'pub struct Presentation;')
        transitions = '\n'.join(item(display, name) for name in ['pub(super) fn take(', 'pub(super) fn set_mode('])
        # Hardware acceleration is the injected boundary. The ARM SET_MODE path
        # does not call the x86-only native_console_restored method.
        transitions = transitions.replace('#[cfg(target_arch = "x86_64")]', '')
        transitions = transitions.replace('c.native_console_restored();',
            'if NATIVE.with(|n|n.get()) { c.native_console_restored(); }')
        # fixture_source has already filled its markers.
        at = fixture.index('pub(super) fn owner(')
        fixture = fixture[:at] + transitions + '\n' + fixture[at:]
        prefix = item(stage, 'pub(super) fn init(').split('    let mut entries')[0]
        prefix = prefix[prefix.index('{') + 1:]
        take = item(stage, 'pub(super) fn take_display(') if 'pub(super) fn take_display(' in stage else ''
        ready = next(line.strip() for line in runner.splitlines() if 'let ready = if screen.gui &&' in line)
        ready = ready.replace('screen.gui', 'true').replace('crate::suite_stage::', 'stage::')
        prepare = item(stage, 'pub(super) fn prepare(').replace('pub(super)', 'pub(crate)')
        check_start = core.index('    if a.target == 0\n        && !with_core(')
        check_end = core.index('    let (id, fmt,', check_start)
        binding_check = core[check_start:check_end].replace('Refused::', 'StageRefused::')
        code = (ROOT / 'tests/fixtures/breengel_scale2_ownership.rs').read_text()
        present = item(intent, 'fn present(')
        begin = present.index('        if self.stage {') if 'if self.stage {' in present else present.index('        graphics::take_over_display()')
        custody = present[begin:present.index('        let (r, presented)', begin)]
        frame = item(intent, 'struct Frame {') + '\n' + item(intent, 'impl Drop for Frame')
        code = code.replace('// INTENT_FRAME', frame)
        code = code.replace('// INTENT_CUSTODY', custody + '\n let expected=if NATIVE.with(|n|n.get()){2}else{1};assert_eq!(with_core(|c|c.display.mode),Some(expected),\"intent page lost scale2\");')
        code = code.replace('// INTENT_FIELDS', ', stage:true' if 'stage: bool' in frame else '')
        code = code.replace('// INTENT_TAKE', 'take_display()' if take else 'graphics::take_over_display().map_err(|e|e.to_string())')
        code = code.replace('// PREPARE', prepare).replace('// TAKE', take)
        code = code.replace('// REMEMBER', prefix).replace('// CHILD_READY', ready)
        code = code.replace('// BINDING_CHECK', binding_check)
        fixture += '\n' + code
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'case.rs').write_text(fixture)
            subprocess.run(priority + ['rustc', '--edition=2021', '--crate-name', 'gpu_abi',
                '--crate-type', 'rlib', str(ROOT / 'libs/gpu-abi/src/lib.rs'),
                '-o', str(path / 'libgpu_abi.rlib')], check=True)
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                '--extern', f'gpu_abi={path / "libgpu_abi.rlib"}', str(path / 'case.rs'),
                '-o', str(path / 'case')], check=True)
            subprocess.run([str(path / 'case'), 'breengel_', '--nocapture', '--test-threads=1'], check=True)


if __name__ == '__main__':
    unittest.main()
