"""Observe the kernel screen after the production root-owner giveback."""
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest
from gt710_display_restore_test import fixture_source
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class ConsoleObservation(unittest.TestCase):
    def test_kernel_cases_observe_the_restored_mode(self):
        composition = (ROOT / 'userspace/programs/src/suite_breengel/window_cases/composition.rs').read_text()
        panel = (ROOT / 'userspace/programs/src/suite_breengel/window_cases/panel.rs').read_text()
        show = (ROOT / 'userspace/programs/src/suite_breengel/show.rs').read_text()
        # Execute the readback route each real case calls after giveback.
        ribbon = item(show, 'fn panel_boot_ribbon(').split('ensure(give_back()>=0')[1]
        kernel = item(panel, 'fn panel_kernel_screens(').split('let gave = give_back();')[1]
        ribbon_call = re.search(r'let result\s*=\s*(\w+)\(\)', ribbon).group(1)
        kernel_call = re.search(r'let shown\s*=\s*(\w+)\(\)', kernel).group(1)
        functions = '\n'.join(item(composition, marker) for marker in
            ['pub(crate) fn screen_size(', 'pub(crate) fn screen('] +
            (['fn screen_at('] if 'fn screen_at(' in composition else []) +
            (['pub(crate) fn console_screen('] if 'pub(crate) fn console_screen(' in composition else []))
        functions = functions.replace('breenix_suite::suite::CaseError::Fail(', 'String::from(')
        code = r'''
mod handoff {pub fn word(rec:&[u8],at:usize)->u32{u32::from_le_bytes(rec[at..at+4].try_into().unwrap())}}
mod console_probe {
 use super::*;
 mod breenix_suite {pub mod suite {pub type CaseError=String;}}
 trait Cite<T>{fn cite(self,_:&str)->Result<T,String>;}
 impl<T,E:std::fmt::Display>Cite<T> for Result<T,E>{fn cite(self,_:&str)->Result<T,String>{self.map_err(|e|e.to_string())}}
 fn ensure(ok:bool,_:&str,msg:&str)->Result<(),String>{if ok{Ok(())}else{Err(msg.into())}}
 const CONTRACT:&str="breengel";
 fn outcome(r:i64)->String{r.to_string()}
 mod bgl {
  use gpu_abi::Plain;
  pub fn modes()->Result<gpu_abi::Modes,i64>{Ok(super::with_core(|c|{
   let mut m=gpu_abi::Modes::ZERO;m.current=c.display.mode;m.count=c.display.modes.len() as u32;
   m.owner_depth=c.display.stack.len() as u32;
   for(i,v)in c.display.modes.iter().enumerate(){if i<2{m.modes[i]=*v;}else{m.extra_modes[i-2]=*v;}}m
  }).unwrap())}
 }
 mod libbreenix {
  pub mod gpu {pub use gpu_abi as abi;}
  pub mod error {pub enum Error{Os(u32)}}
  pub mod window {
   pub mod abi {pub struct Readback{pub dst:u64,pub dst_len:u64,pub record:u64,pub record_len:u32,pub reason:u32}
    impl Readback{pub const ZERO:Self=Self{dst:0,dst_len:0,record:0,record_len:0,reason:0};}}
   pub fn readback(rb:&mut abi::Readback)->Result<(),super::error::Error>{
    let m=super::super::bgl::modes().map_err(|_|super::error::Error::Os(5))?.current_mode().unwrap();
    assert_eq!(rb.dst_len,u64::from(m.width)*u64::from(m.height)*4);
    let bytes=unsafe{std::slice::from_raw_parts_mut(rb.record as *mut u8,rb.record_len as usize)};
    for(at,v)in [(0,2u32),(4,1024),(gpu_abi::record::WIDTH,m.width),(gpu_abi::record::HEIGHT,m.height)]{bytes[at..at+4].copy_from_slice(&v.to_le_bytes());}
    Ok(())
   }
  }
 }
 FUNCTIONS
 pub fn check(native:bool){
  assert_eq!(libbreenix::gpu::abi::record::VERSION,2);
  reset();with_core(|c|{
   if native{c.display.modes=vec![abi::Mode{width:1024,height:768,scale:1,reserved:0},abi::Mode{width:1280,height:1024,scale:1,reserved:0},abi::Mode{width:2560,height:2048,scale:2,reserved:0}];}
   c.display.mode=if native{2}else{1};
   c.display.stack=vec![display::Owner{pid:99,group:99,mode:c.display.mode,last:None}];
  });
  syscall::display::DISPLAY_OWNER_GROUP.store(99,Ordering::Release);
  display::back(99).unwrap();
  assert_eq!(with_core(|c|c.display.mode),Some(1));
  let expected=if native{(1280,1024)}else{(2560,1600)};
  let result=RIBBON_CALL();
  assert!(result.is_ok(),"native={native}: boot-ribbon after giveback: {:?}",result.as_ref().err());
  let (pixels,record)=result.unwrap();assert_eq!(pixels.len(),expected.0*expected.1*4);
  assert_eq!(u32::from_le_bytes(record[gpu_abi::record::WIDTH..gpu_abi::record::WIDTH+4].try_into().unwrap()) as usize,expected.0);
  let result=KERNEL_CALL();assert!(result.is_ok(),"kernel-screens after giveback: {:?}",result.as_ref().err());
  assert_eq!(result.unwrap().0.len(),pixels.len());
  if native{assert!(screen_size().is_err(),"the composed-stage helper must still require scale2");}
 }
}
#[test]fn kernel_screen_readback_uses_its_unowned_native_mode(){console_probe::check(true);console_probe::check(false);}
'''
        code = code.replace('FUNCTIONS', functions).replace('RIBBON_CALL', ribbon_call).replace('KERNEL_CALL', kernel_call)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'case.rs').write_text(fixture_source() + code)
            subprocess.run(priority + ['rustc', '--edition=2021', '--crate-name', 'gpu_abi',
                '--crate-type', 'rlib', str(ROOT / 'libs/gpu-abi/src/lib.rs'),
                '-o', str(path / 'libgpu_abi.rlib')], check=True)
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                '--extern', f'gpu_abi={path / "libgpu_abi.rlib"}', str(path / 'case.rs'),
                '-o', str(path / 'case')], check=True)
            subprocess.run([str(path / 'case'), 'kernel_screen_readback_', '--nocapture'], check=True)


if __name__ == '__main__':
    unittest.main()
