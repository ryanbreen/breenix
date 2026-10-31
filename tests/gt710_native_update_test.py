"""Run native UPDATE construction with retained active and armed display state."""
from pathlib import Path
import subprocess,sys,tempfile,unittest
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'tests'))
from gt710_gr_live_unwind_test import item
class Update(unittest.TestCase):
    def test_same_surface_keeps_fresh_update_without_dirtying_head_state(self):
        code=item((ROOT/'kernel/src/gpu/gk208/native.rs').read_text(),'fn submit(&mut self, methods:&[(u32,u32)])')
        fixture=r'''
use std::collections::BTreeMap;
struct Registers{regs:BTreeMap<usize,u32>,sent:Vec<(u32,u32)>}
impl Registers{fn read(&mut self,r:usize)->u32{*self.regs.get(&r).unwrap_or(&0)}}
struct Evo{head:u32,put:u32,push:u64,total:u64}
mod modeset{
 #[derive(Clone,Copy)]pub struct Viewport{pub good:bool}
 impl Viewport{
  pub fn methods(&self,_:u32)->Result<Vec<(u32,u32)>,&'static str>{Ok(vec![(0x4b8,0x06400a00)])}
  pub fn verify(&self,_:&mut crate::Registers,_:u32)->Result<(),&'static str>{if self.good{Ok(())}else{Err("viewport differs")}}
 }
 pub fn publish(_:&mut crate::Registers,_:u32)->Result<(),&'static str>{Ok(())}
 pub fn wait(_:&mut crate::Registers,_:usize,_:u32,_:u32)->Result<(),&'static str>{Ok(())}
}
fn write_words(_:&mut Registers,_:u64,_:u64,_:&[u32])->Result<(),&'static str>{Ok(())}
fn submit(r:&mut Registers,_:u64,_:u64,put:&mut u32,m:&[(u32,u32)])->Result<(),&'static str>{r.sent=m.to_vec();*put+=m.len() as u32*8;Ok(())}
struct Update<'a>{evo:&'a mut Evo,regs:&'a mut Registers,view:modeset::Viewport}
impl Update<'_>{CODE}
fn methods()->Vec<(u32,u32)>{vec![(0x460,0x123400),(0x468,0x06400a00),(0x46c,0xa004),(0x470,0xcf00),(0x474,3),(0x84,0x80000000),(0x80,0)]}
fn run(same:bool,viewport:bool)->Vec<(u32,u32)>{
 let m=methods();let mut regs=Registers{regs:BTreeMap::new(),sent:vec![]};
 for &(r,v) in &m[..5]{regs.regs.insert(0x640000+r as usize,v);regs.regs.insert(0x660000+r as usize,v);}
 assert_eq!(regs.read(0x640460),0x123400);
 if !same{regs.regs.insert(0x640460,0x567800);}
 let mut evo=Evo{head:0,put:0,push:0,total:0};let view=modeset::Viewport{good:viewport};
 let _=view.verify(&mut regs,0);
 Update{evo:&mut evo,regs:&mut regs,view}.submit(&m).unwrap();regs.sent
}
#[test]fn no_op_head_still_receives_a_fresh_notifier_and_update(){assert_eq!(run(true,true),methods()[5..]);}
#[test]fn changed_surface_or_viewport_is_programmed_before_update(){
 assert_eq!(run(false,true),methods());
 assert_eq!(run(true,false),vec![(0x4b8,0x06400a00),(0x84,0x80000000),(0x80,0)]);
}
'''.replace('CODE',code)
        priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp);(p/'update.rs').write_text(fixture)
            subprocess.run(priority+['rustc','--edition=2021','--test','-Dwarnings',str(p/'update.rs'),'-o',str(p/'update')],check=True)
            subprocess.run([str(p/'update'),'--nocapture'],check=True)
if __name__=='__main__':unittest.main()
