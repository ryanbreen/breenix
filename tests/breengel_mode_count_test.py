"""Reject a truncated or padded MODES count in BreenGL's geometry case."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class ModeCount(unittest.TestCase):
    def test_count_matches_populated_slots_and_display_contract(self):
        source = (ROOT / 'userspace/programs/src/suite_breengel/show.rs').read_text()
        if 'fn complete_catalogue(' in source:
            check = item(source, 'fn complete_catalogue(')
        else:
            check = ('fn complete_catalogue(modes: &Modes, _: bool) -> bool {'
                     'modes.current<modes.count && modes.count as usize==modes.mode_list().count()}')
        code = r'''extern crate gpu_abi;
use gpu_abi::{Mode, Modes, Plain};
CHECK
fn catalogue(rows:&[(u32,u32,u32)])->Modes {
 let mut m=Modes::ZERO;m.count=rows.len() as u32;
 for (slot,&(width,height,scale)) in m.modes.iter_mut().chain(m.extra_modes.iter_mut()).zip(rows) {
  *slot=Mode{width,height,scale,reserved:0};
 } m
}
fn main(){
 for (native,rows) in [
  (true,vec![(1024,768,1),(1280,1024,1),(2560,2048,2)]),
  (false,vec![(1280,800,1),(2560,1600,2)]),
  (true,vec![(2560,2048,2),(1280,1024,1)])] {
  let m=catalogue(&rows);assert!(complete_catalogue(&m,native));
  for count in 1..=10 {if count==m.count {continue;}
   let mut bad=m;bad.count=count;
   assert!(!complete_catalogue(&bad,native),"accepted count {count} for {} populated slots",m.count);
  }
  let mut bad=m;bad.current=m.count;assert!(!complete_catalogue(&bad,native));
  let mut bad=m;bad.modes[1].scale=3;assert!(!complete_catalogue(&bad,native));
 }
 let m=catalogue(&[(1024,768,1),(1280,1024,1),(2560,2048,2)]);
 assert!(!complete_catalogue(&m,false),"panel accepted a third mode");
}
'''.replace('CHECK', check.replace('libbreenix::gpu::abi::Modes', 'Modes'))
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'check.rs').write_text(code)
            lib = path / 'libgpu_abi.rlib'
            subprocess.run(priority + ['rustc', '--edition=2021', '--crate-name', 'gpu_abi',
                '--crate-type', 'rlib', str(ROOT / 'libs/gpu-abi/src/lib.rs'), '-o', str(lib)], check=True)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', str(path / 'check.rs'),
                '--extern', f'gpu_abi={lib}', '-o', str(path / 'check')], check=True)
            subprocess.run([str(path / 'check')], check=True)


if __name__ == '__main__':
    unittest.main()
