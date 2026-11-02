"""Native image creation completes a zero-backing CE copy before publication."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class ImageInitialization(unittest.TestCase):
    def test_native_creation_and_completion(self):
        source = (ROOT / 'kernel/src/gpu/mod.rs').read_text()
        create = item(source, 'fn op_resource_create(')
        self.assertIn('initialize_native_image(', create,
                      'native creation publishes uninitialized VRAM')
        self.assertLess(create.index('dev::attach_backing'), create.index('initialize_native_image('))
        self.assertLess(create.index('initialize_native_image('), create.index('let handle ='))
        admission = create[create.index('dev::attach_backing'):create.index('let ctxs:')]
        self.assertIn('dev::backend() == abi::backend::GK208', admission)
        self.assertIn('a.kind != kind::BUFFER', admission)
        helper = item(source, 'fn initialize_native_image(')
        code = r'''
extern crate alloc;
use alloc::sync::Arc;
use std::cell::{Cell,RefCell};
const ENODEV:i64=19;const ENOMEM:i64=12;const EIO:i64=5;
struct Pages(Vec<u8>);
impl Pages{fn bytes(&self)->usize{self.0.len()}fn maintain(&self,o:usize,n:usize,read:bool){assert_eq!((o,n,read),(0,self.0.len(),false));}}
struct Core;impl Core{fn serial(&mut self)->u64{7}fn device_fence(&mut self)->u64{107}}
fn with_core<T>(f:impl FnOnce(&mut Core)->T)->Option<T>{Some(f(&mut Core))}
#[derive(Clone,Copy)]enum Tag{Sync(u64)}
struct Request{backing:Arc<Pages>,id:u32,rect:[u32;4],stride:u32,fence:u64}
struct Done{answer:u32,fence:u64}impl Done{fn matches_fence(&self,f:u64)->bool{self.fence==f}}
thread_local!{static VRAM:RefCell<Vec<u8>>=RefCell::new(vec![0x55;64]);static RESPONSE:Cell<(u32,u64)>=const{Cell::new((0x60000000,107))};}
mod dev{use super::*;
pub fn ok(a:u32)->bool{a==0x60000000}
pub fn transfer_to_host(tag:Tag,ctx:u32,fence:u64,id:u32,backing:Arc<Pages>,x:u32,y:u32,w:u32,h:u32,offset:u64,stride:u32)->Option<Request>{
let Tag::Sync(serial)=tag;assert_eq!((serial,ctx,fence,offset),(7,0,107,0));
Some(Request{backing,id,rect:[x,y,w,h],stride,fence})}}
fn exec_req(serial:u64,r:Request,ctx:u32,holds:&[u32],reading:&[u32])->Result<Done,i64>{
assert_eq!((serial,ctx,r.id,r.fence),(7,0,42,107));assert_eq!(holds,&[42]);assert!(reading.is_empty());assert!(Arc::strong_count(&r.backing)>=2);
let [x,y,w,h]=r.rect;assert_eq!((x,y,w,h,r.stride),(0,0,3,4,64));
VRAM.with(|v|for row in 0..h as usize{v.borrow_mut()[row*16..row*16+w as usize*4].copy_from_slice(&r.backing.0[row*64..row*64+w as usize*4]);});
let (answer,fence)=RESPONSE.with(Cell::get);Ok(Done{answer,fence})}
HELPER
fn main(){let backing=Arc::new(Pages(vec![0;256]));
assert_eq!(initialize_native_image(42,backing.clone(),3,4,64),Ok(()));
VRAM.with(|v|for row in v.borrow().chunks_exact(16){assert_eq!(&row[..12],&[0;12]);assert_eq!(&row[12..],&[0x55;4]);});
for response in [(0x60000000,106),(0x60010000,107)]{RESPONSE.with(|r|r.set(response));assert_eq!(initialize_native_image(42,backing.clone(),3,4,64),Err(EIO));}}
'''.replace('HELPER', helper)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory(prefix='gt710-image-init-') as tmp:
            p = Path(tmp)
            (p / 'init.rs').write_text(code)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', str(p / 'init.rs'), '-o', str(p / 'init')], check=True)
            subprocess.run([str(p / 'init')], check=True)


if __name__ == '__main__':
    unittest.main()
