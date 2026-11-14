"""Force runner scanout observation immediately after owner restoration."""
from pathlib import Path
import os
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
from gt710_display_restore_test import ROOT, fixture_source
from gt710_gr_live_unwind_test import item


class RunnerRestore(unittest.TestCase):
    def test_empty_owner_has_a_registered_scanout_before_runner_begin(self):
        original = Path.read_text
        revision = os.environ.get('BREENIX_RESTORE_SOURCE_REV')
        def read(path, *args, **kwargs):
            if revision and str(path).startswith(str(ROOT / 'kernel')):
                return subprocess.check_output(['git', 'show', f'{revision}:{path.relative_to(ROOT)}'], cwd=ROOT, text=True)
            return original(path, *args, **kwargs)
        with patch.object(Path, 'read_text', read):
            # The source revision may predate show_saved; include precisely the
            # helpers its restore body calls, without changing the assertions.
            from gt710_display_restore_test import fixture_source as make
            if revision:
                helper = read(ROOT / 'kernel/src/gpu/screen.rs')
                if 'pub(super) fn show_saved()' not in helper:
                    with patch('gt710_display_restore_test.item', side_effect=lambda s,m: '' if m == 'pub(super) fn show_saved()' else item(s,m)):
                        fixture = make()
                else:
                    fixture = make()
            else:
                fixture = fixture_source()
        screen = (ROOT / 'libs/libbreenix/src/suite_screen.rs').read_text()
        begin = item(screen, 'pub(super) fn begin(')
        begin = begin[begin.index('    let mut pixel'):begin.index('    LINES.lock()')]
        snapshot = item(screen, 'fn snapshot(')
        snapshot = snapshot[snapshot.index('    let (status, modes)'):snapshot.index('    let (status, caps)')]
        fixture += r'''
static RUNNER:AtomicU64=AtomicU64::new(0);
static RUNNER_RESOURCE:AtomicU64=AtomicU64::new(0);
mod r {pub const SIZE:usize=32;pub const CONTEXT:usize=0;pub const RESOURCE:usize=8;}
fn quad(r:&[u8],at:usize)->u64{u64::from_le_bytes(r[at..at+8].try_into().unwrap())}
fn runner_begin(){ BEGIN }
fn native_crc_enabled()->bool{true}
fn reuse_snapshot()->Option<Result<(),String>>{None}
fn non_case_binding(known:bool,id:u64,console:u64,runner:u64)->bool{known&&(id==0||id==console||id==runner)}
fn snapshot_preflight()->Result<(),String>{ SNAPSHOT Ok(()) }
'''.replace('BEGIN',begin).replace('SNAPSHOT',snapshot)
        fixture = fixture.replace('mod gpu {pub fn display_active()', r'''mod gpu {
 pub fn readback(_:u64,_:[u32;4],_:&mut [u8],_:u32,rec:&mut [u8])->i64{
  let binding=crate::with_core(|c|c.display.binding.filter(|_|c.display.known)).flatten();
  let Some(binding)=binding else{return -5};
  rec[..8].copy_from_slice(&7u64.to_le_bytes());
  rec[8..16].copy_from_slice(&u64::from(binding.id).to_le_bytes());0
 }
 pub struct Modes{pub binding_known:u32,pub scanout_resource:u64,pub console_resource:u64}
 pub fn modes()->(i64,Modes){crate::with_core(|c|(0,Modes{binding_known:c.display.known as u32,
  scanout_resource:c.display.binding.map_or(0,|b|u64::from(b.id)),console_resource:50})).unwrap()}
 pub fn display_active()''')
        fixture += r'''
#[test] fn runner_can_observe_restoration_before_the_next_case() {
 for mode in [0,1] {
  reset();with_core(|c| {
   c.display.stack.truncate(1);c.display.stack[0].mode=mode;c.display.stack[0].last=None;
   c.display.binding=None;c.display.known=true;
  });
  display::restore().unwrap();
  // suite_screen::begin reads scanout 0 at this point, before the child presents.
  runner_begin();
  // The next case publishes its own target; the snapshot must still know the runner.
  let restored=with_core(|c|c.display.binding).flatten();
  with_core(|c|c.display.binding=Some(display::Binding{id:22,fence:42,answer:0x60000000}));
  assert!(snapshot_preflight().is_ok(),"case checkpoint failed after restoration");
  with_core(|c|c.display.binding=restored);
  assert_eq!(RUNNER.load(Ordering::Relaxed),7);
  let runner=with_core(|c|c.display.binding.filter(|_|c.display.known)).flatten();
  assert!(runner.is_some(),"restoration left only the unregistered native allocation");
  let runner=runner.unwrap();
  assert_eq!(runner.id,50);assert_ne!(runner.fence,0);
  assert_eq!(with_core(|c|c.display.mode),Some(mode));
  assert_eq!(HOST.with(|h|h.borrow().mode),mode);
 }
}
'''
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as directory:
            p=Path(directory);(p/'runner.rs').write_text(fixture)
            subprocess.run(priority+['rustc','--edition=2021','--crate-name','gpu_abi','--crate-type','rlib','-Dwarnings',str(ROOT/'libs/gpu-abi/src/lib.rs'),'-o',str(p/'libgpu_abi.rlib')],check=True)
            subprocess.run(priority+['rustc','--edition=2021','--test','-Dwarnings','--extern',f'gpu_abi={p/"libgpu_abi.rlib"}',str(p/'runner.rs'),'-o',str(p/'runner')],check=True)
            subprocess.run([str(p/'runner'),'--exact','runner_can_observe_restoration_before_the_next_case','--nocapture','--test-threads=1'],check=True)


if __name__ == '__main__':
    unittest.main()
