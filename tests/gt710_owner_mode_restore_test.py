"""Restore a parent's selected mode independently of its retained frame extent."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from gt710_display_restore_test import ROOT, fixture_source


class OwnerModeRestore(unittest.TestCase):
    def test_giveback_restores_owner_mode_and_original_resource(self):
        fixture = fixture_source() + r'''
#[test] fn parent_mode_and_saved_frame_are_independent() {
 reset();
 with_core(|c| {
  c.display.modes=vec![abi::Mode{width:1024,height:768,scale:1,reserved:0},
   abi::Mode{width:1280,height:1024,scale:1,reserved:0},
   abi::Mode{width:2560,height:2048,scale:2,reserved:0}];
  c.display.stack[0].mode=0;c.display.stack[1].mode=2;
  let r=c.resources.get_mut(&11).unwrap();r.width=1280;r.height=1024;
 });
 HOST.with(|h|h.borrow_mut().mode=2);
 display::back(2).unwrap();
 assert_eq!(HOST.with(|h|h.borrow().mode),0,"saved frame selected the native mode");
 with_core(|c| {
  assert_eq!(c.display.mode,0,"published mode differs from parent request");
  assert_eq!(c.display.stack[0].mode,0);
  assert_eq!(c.display.binding.unwrap().id,11,"lost original saved target");
  assert_eq!(c.display.binding.unwrap().fence,41);
  assert!(c.display.known);
  assert_eq!((c.resources[&11].width,c.resources[&11].height),(1280,1024));
 });
 assert!(events().contains(&("flip",11)));
}
'''
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as directory:
            p = Path(directory)
            (p / 'restore.rs').write_text(fixture)
            subprocess.run(priority + ['rustc', '--edition=2021', '--crate-name', 'gpu_abi',
                '--crate-type', 'rlib', '-Dwarnings', str(ROOT / 'libs/gpu-abi/src/lib.rs'),
                '-o', str(p / 'libgpu_abi.rlib')], check=True)
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                '--extern', f'gpu_abi={p / "libgpu_abi.rlib"}', str(p / 'restore.rs'),
                '-o', str(p / 'restore')], check=True)
            subprocess.run([str(p / 'restore'), '--exact', 'parent_mode_and_saved_frame_are_independent',
                            '--nocapture', '--test-threads=1'], check=True)


if __name__ == '__main__':
    unittest.main()
