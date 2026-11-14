"""Shared mode checks accept different monitors and refuse incorrect scales."""
from pathlib import Path
import os
import subprocess
import sys
import tempfile
import unittest
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'tests'))
from gt710_gr_live_unwind_test import item

class SuiteModesTest(unittest.TestCase):
    def test_monitor_independent_catalogue_and_scale_controls(self):
        source=(ROOT/'userspace/programs/src/suite_gpu/present.rs').read_text()
        if 'fn valid_catalogue(' in source:
            helper=item(source,'fn expected_scale(')+'\n'+item(source,'fn valid_catalogue(')
            self.assertIn('expected_scale(',item(source,'pub fn scale_reported('))
        else:
            helper=item(source,'fn expected_modes(')+"\nfn valid_catalogue(native:bool,modes:&[(u32,u32,u32)])->bool{expected_modes(native)==modes}"
        code=r'''#![allow(dead_code)]
extern crate alloc;
#[path="ROOT/kernel/src/gpu/gk208/edid.rs"] mod edid;
#[path="ROOT/kernel/src/gpu/gk208/modeset.rs"] mod modeset;
HELPER
fn main(){
 let hex=include_str!("ROOT/tests/fixtures/gt710-edid/lcd17-19.hex").trim();
 let bytes:Vec<u8>=(0..hex.len()).step_by(2).map(|i|u8::from_str_radix(&hex[i..i+2],16).unwrap()).collect();
 assert!(valid_catalogue(true,&modeset::display_modes(&edid::parse(&bytes).unwrap()).unwrap()));
 assert!(valid_catalogue(false,&[(1280,800,1),(2560,1600,2)]));
 for modes in [vec![(2048,1536,2),(1024,768,1)],
  vec![(1920,1080,1),(1366,768,1),(2732,1536,2)],
  vec![(1920,1080,1),(960,540,1),(1920,1080,2)]] {
  assert!(valid_catalogue(true,&modes),"shared suite pins one monitor: {modes:?}");
  for index in 0..modes.len(){let mut wrong=modes.clone();wrong[index].2=3-wrong[index].2;
   assert!(!valid_catalogue(true,&wrong),"wrong published scale accepted: {wrong:?}");}
  let mut wrong=modes.clone();let i=wrong.iter().position(|m|m.2==2).unwrap();wrong[i].0+=1;
  assert!(!valid_catalogue(true,&wrong));
 }
 assert!(!valid_catalogue(true,&[]));assert!(!valid_catalogue(true,&vec![(1,1,1);11]));
 assert!(!valid_catalogue(false,&[(1280,800,2),(2560,1600,1)]));
}
'''.replace('ROOT',str(ROOT)).replace('HELPER',helper)
        priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp);(p/'check.rs').write_text(code)
            subprocess.run(priority+['rustc','--edition=2021','-Dwarnings',str(p/'check.rs'),'-o',str(p/'check')],check=True)
            subprocess.run([str(p/'check')],check=True)
    def test_scale_case_refuses_a_catalogue_that_agrees_with_itself(self):
        source=Path(os.environ.get('GT710_PRESENT_SOURCE',ROOT/'userspace/programs/src/suite_gpu/present.rs')).read_text()
        body=item(source,'pub fn scale_reported(')
        helpers=''
        if 'fn expected_scale(' in source:
            helpers=item(source,'fn expected_scale(')+'\n'+item(source,'fn valid_catalogue(')
        code=r'''use std::sync::atomic::{AtomicU32,Ordering};
type CaseResult=Result<(),String>;
static SELECTED:AtomicU32=AtomicU32::new(0);
mod abi {pub mod backend {pub const GK208:u32=6;}}
struct Caps{backend:u32}
fn caps()->Result<Caps,String>{Ok(Caps{backend:6})}
fn begin()->Result<(),String>{Ok(())}fn take()->Result<(),String>{Ok(())}
fn fail_msg(s:String)->String{s}
fn ensure(test:bool,message:impl FnOnce()->String)->CaseResult{if test{Ok(())}else{Err(message())}}
fn mode(index:u32)->CaseResult{SELECTED.store(index,Ordering::Relaxed);Ok(())}
fn target_modes()->Result<Vec<(u32,u32,u32)>,String>{Ok(vec![(1024,768,1),(1280,1024,2),(2560,2048,1)])}
fn u32_at(b:&[u8],at:usize)->u32{u32::from_le_bytes(b[at..at+4].try_into().unwrap())}
fn modes()->Result<[u8;256],String>{let mut b=[0;256];b[12..16].copy_from_slice(&SELECTED.load(Ordering::Relaxed).to_le_bytes());
 for(i,(_,_,scale))in target_modes()?.into_iter().enumerate(){let at=if i<2{32+i*16+8}else{128+(i-2)*16+8};b[at..at+4].copy_from_slice(&scale.to_le_bytes());}Ok(b)}
HELPERS
BODY
fn main(){assert!(scale_reported().is_err(),"scale-reported compared the published scale against itself");}
'''.replace('HELPERS',helpers).replace('BODY',body)
        if not helpers:
            code='\n'.join(line for line in code.splitlines() if not line.startswith(('mod abi ', 'struct Caps', 'fn caps()', 'fn fail_msg(')))
        priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp);(p/'check.rs').write_text(code)
            subprocess.run(priority+['rustc','--edition=2021','-Dwarnings',str(p/'check.rs'),'-o',str(p/'check')],check=True)
            subprocess.run([str(p/'check')],check=True)
if __name__=='__main__':unittest.main()
