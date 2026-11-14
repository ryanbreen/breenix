"""Run reset, native restoration and stage takeover with independent mode state."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from gt710_display_restore_test import fixture_source, ROOT
from gt710_gr_live_unwind_test import item


class ResetMode(unittest.TestCase):
    def test_reset_restore_take_display_keeps_the_requested_mode(self):
        display = (ROOT / 'kernel/src/gpu/display.rs').read_text()
        stage = (ROOT / 'libs/libbreenix/src/suite_stage.rs').read_text()
        fixture = fixture_source()
        methods = item(display, 'pub(super) fn display_reset(').replace('pub(super)', 'pub(crate)')
        fixture = fixture.replace('impl Core {', 'impl Core {\n' + methods)
        take = item(display, 'pub(super) fn take(')
        fixture = fixture.replace('mod display {\n use super::*;', 'mod display {\n use super::*;\n' + take)
        fixture = fixture.replace('const EIO:i64=5;', 'const EBUSY:i64=16;const ENOMEM:i64=12;const EINVAL:i64=22;const EIO:i64=5;')
        fixture = fixture.replace('fn acquire(_:Option<u64>)->Result<Self,i64>{Ok(Self)}',
                                  'fn acquire(_:Option<u64>)->Result<Self,i64>{Ok(Self)} fn for_mutation(_:u64)->Result<Self,i64>{Ok(Self)}')
        fixture = fixture.replace('pub fn give_back_display(){crate::event("vt-back",0);}', '''
          pub fn give_back_display(){crate::event("vt-back",0);}
          pub fn take_display(_:u64,_:usize)->Result<(),i32>{Ok(())}
          pub fn take_display_nested(_:u64,_:usize)->Result<(),i32>{Ok(())}
        ''')
        # Run the real SET_MODE syscall path if stage takeover needs it.
        mode = item(display, 'pub(super) fn set_mode(')
        fixture = fixture.replace('mod display {\n use super::*;', 'mod display {\n use super::*;\n' + mode)
        fixture += r'''
fn read_arg<T:Copy>(arg:u64,len:u64)->Result<T,i64>{
 assert_eq!(len as usize,std::mem::size_of::<T>());Ok(unsafe{*(arg as *const T)})
}
mod graphics {
 pub fn take_over_display()->Result<(),i64>{crate::display::take(99,99,1)}
}
mod stage_gpu {
 use crate::abi::Plain;
 pub fn modes()->(i64,crate::abi::Modes){
  let mut modes=crate::abi::Modes::ZERO;
  crate::with_core(|c|{modes.current=c.display.mode;modes.count=c.display.modes.len() as u32;});
  (0,modes)
 }
 pub fn set_mode(index:u32)->i64{
  crate::event("stage-set-mode",index);
  let a=crate::abi::SetMode{index,reserved:[0;3]};
  crate::display::set_mode(&a as *const _ as u64,std::mem::size_of_val(&a) as u64).map_or_else(|e|-e,|v|v as i64)
 }
}
mod stage {
 use crate::{graphics,stage_gpu as gpu};
 static DISPLAY_MODE:std::sync::atomic::AtomicU32=std::sync::atomic::AtomicU32::new(2);
 TAKE_DISPLAY
}
fn catalogue(c:&mut Core){
 c.display.modes=vec![abi::Mode{width:1024,height:768,scale:1,reserved:0},
  abi::Mode{width:1280,height:1024,scale:1,reserved:0},abi::Mode{width:2560,height:2048,scale:2,reserved:0}];
 let r=c.resources.get_mut(&50).unwrap();r.width=1280;r.height=1024;
}
#[test] fn reset_then_restore_then_stage_takeover(){
 reset();with_core(|c|{
  catalogue(c);c.display.stack=vec![display::Owner{group:99,pid:99,mode:2,
   last:Some(display::Binding{id:22,fence:42,answer:0x60000000})}];
  c.display.mode=2;
 });
 syscall::display::DISPLAY_OWNER_GROUP.store(99,Ordering::Release);
 HOST.with(|h|h.borrow_mut().mode=2);
 with_core(|c|c.display_reset());
 assert!(with_core(|c|c.display.stack[0].last.is_none()).unwrap());
 display::restore().unwrap();
 let wanted=with_core(|c|c.display.stack.last().unwrap().mode).unwrap();
 let restored_native=HOST.with(|h|h.borrow().mode);
 stage::take_display().unwrap();
 assert_eq!(restored_native,wanted,"native mode diverged from owner after reset restore");
 assert_eq!(with_core(|c|c.display.mode),Some(wanted),"published mode diverged after reset restore");
 assert_eq!(HOST.with(|h|h.borrow().mode),2);
 assert_eq!(with_core(|c|c.display.mode),Some(2));
 assert!(!events().iter().any(|(name,_)|*name=="stage-set-mode"),"takeover required a repair retry");
 // With no owner the same recovery returns to, and publishes, the kernel mode.
 with_core(|c|{c.display.stack.clear();c.display_reset();});
 display::restore().unwrap();
 assert_eq!(HOST.with(|h|h.borrow().mode),display::KERNEL_MODE as u32);
 assert_eq!(with_core(|c|c.display.mode),Some(display::KERNEL_MODE as u32));
}
'''.replace('TAKE_DISPLAY', item(stage, 'pub(super) fn take_display()'))
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'reset.rs').write_text(fixture)
            subprocess.run(priority + ['rustc', '--edition=2021', '--crate-name', 'gpu_abi',
                '--crate-type', 'rlib', str(ROOT / 'libs/gpu-abi/src/lib.rs'),
                '-o', str(path / 'libgpu_abi.rlib')], check=True)
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                '--extern', f'gpu_abi={path / "libgpu_abi.rlib"}', str(path / 'reset.rs'),
                '-o', str(path / 'reset')], check=True)
            subprocess.run([str(path / 'reset'), '--exact', 'reset_then_restore_then_stage_takeover',
                            '--nocapture', '--test-threads=1'], check=True)


if __name__ == '__main__':
    unittest.main()
