"""Preserve draw-time state and action order while omitting unchanged bindings."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class StateCompaction(unittest.TestCase):
    def test_binding_state_at_draws_and_action_packets(self):
        encoder = (ROOT / 'kernel/src/gpu/gk208/encode.rs').read_text().replace(
            'include_bytes!("caps/integer.inputs")',
            'include_bytes!("' + str(ROOT / 'kernel/src/gpu/gk208/caps/integer.inputs') + '")')
        fixture = r'''
pub fn check() {
 use std::collections::BTreeMap;
 fn packet(out:&mut Vec<u32>,m:u32,data:&[u32]){out.push(0x20000000|((data.len()as u32)<<16)|m/4);out.extend(data);}
 fn observations(words:&[u32])->(Vec<BTreeMap<u32,u32>>,Vec<Vec<u32>>) {
  let mut state=BTreeMap::new();let mut draws=Vec::new();let mut actions=Vec::new();let mut at=0;
  while at<words.len(){let n=((words[at]>>16)&0x1fff)as usize;let m=(words[at]&0x1fff)*4;
   if m==0{state.clear();}
   if matches!(m,0|0xd74|0x1618|0x1614|0x110|0x21c|0xda4|0x2490|0x1b00|0x10f4){actions.push(words[at..at+n+1].to_vec());}
   else{for (i,v)in words[at+1..at+n+1].iter().enumerate(){state.insert(m+i as u32*4,*v);}}
   if m==0xd74{draws.push(state.clone());}at+=n+1;
  }(draws,actions)
 }
 let mut words=Vec::new();packet(&mut words,0,&[0xa197]);
 for i in 0..8 {
  if i==4 {packet(&mut words,0x804,&[99]);}
  if i==6 {packet(&mut words,0,&[0xa197]);}
  packet(&mut words,0x800,&[1,2,64,64,0xcf,0x30,1,0,0]);
  packet(&mut words,0x2040,&[0x11,0x30]);packet(&mut words,0x204c,&[4]);
  packet(&mut words,0x2140,&[0x51,if i<3{0x1b0}else{0x2b0}]);
  packet(&mut words,0x214c,&[4]);packet(&mut words,0xda4,&[0x1011]);
  packet(&mut words,0x2490,&[1]);packet(&mut words,0x1618,&[4]);
  packet(&mut words,0xd74,&[0,6]);packet(&mut words,0x1614,&[0]);
 }
 packet(&mut words,0x110,&[0]);packet(&mut words,0x10f4,&[0x100]);
 packet(&mut words,0x1b00,&[0,0x2000000,7,0x1000f010]);
 let expected=observations(&words);let old=words.len();
 compact_state(&mut words).unwrap();
 assert!(words.len()<old-40,"unchanged bindings still reprogram the GPU");
 assert_eq!(observations(&words),expected,"draw-time state and ordered actions must match");
 let once=words.clone();compact_state(&mut words).unwrap();assert_eq!(words,once);
 assert!(compact_state(&mut vec![0x20090000,1]).is_err());
}
'''
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory(prefix='gt710-state-') as directory:
            p = Path(directory)
            (p / 'encode.rs').write_text(encoder + fixture)
            (p / 'check.rs').write_text('''extern crate alloc;
#[allow(dead_code)] // Import the complete encoder dependencies for this packet check.
#[path="SHADERS"]pub mod shaders;
#[allow(dead_code)]
#[path="CAPS"]pub mod caps;
#[allow(dead_code)]
mod encode;
fn main(){encode::check();}
'''.replace('SHADERS',str(ROOT/'kernel/src/gpu/gk208/gr/shaders.rs')).replace('CAPS',str(ROOT/'kernel/src/gpu/gk208/caps.rs')))
            subprocess.run(priority+['rustc','--edition=2021','--crate-name','gpu_abi','--crate-type','rlib','--cfg','feature="alloc"','-Dwarnings',str(ROOT/'libs/gpu-abi/src/lib.rs'),'-o',str(p/'libgpu_abi.rlib')],check=True)
            subprocess.run(priority+['rustc','--edition=2021','-Dwarnings','--extern',f'gpu_abi={p/"libgpu_abi.rlib"}',str(p/'check.rs'),'-o',str(p/'check')],check=True)
            subprocess.run([str(p/'check')],check=True)


if __name__ == '__main__':
    unittest.main()
