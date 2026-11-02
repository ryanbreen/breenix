"""Execute the native publisher with fragmented coherent command pages."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class PushDma(unittest.TestCase):
    def test_primary_upload_uses_coherent_dma_and_keeps_terminal_vram(self):
        channel = (ROOT / 'kernel/src/gpu/gk208/gr/channel.rs').read_text()
        publisher = item(channel, '        fn publish_words(')
        dma = (ROOT / 'kernel/src/gpu/gk208/system_dma.rs').read_text()
        write = item(dma, '    pub fn write(') if '    pub fn write(' in dma else ''
        source = r'''
use std::cell::Cell;
thread_local! { static UC:Cell<usize>=const{Cell::new(0)}; static FLUSH:Cell<usize>=const{Cell::new(0)}; static FENCES:Cell<usize>=const{Cell::new(0)}; }
mod gpu_cache {pub fn drain_wc(){crate::FENCES.with(|n|n.set(n.get()+1));}}
#[allow(dead_code)] // Exact publisher signature includes optional hardware tracing.
mod gk208 {pub mod queue {pub struct Queue;impl Queue {pub fn entry(s:u32)->u32{(s-1)%512}}}
 pub mod timing {pub mod submit_cost {pub enum Phase{CommandUpload,DescriptorUpload,BarFlush,GpPut}pub enum Bytes{Command,Descriptor,GpPut}}
 pub struct Scope;impl Drop for Scope{fn drop(&mut self){}}
 pub struct Submit;impl Submit {pub fn scope(&self,_:submit_cost::Phase)->Scope{Scope}pub fn bytes(&self,_:submit_cost::Bytes,_:u64){}}
}}
struct Registers;
impl Registers {fn read(&mut self,_:usize)->u32{0}}
mod modeset {pub fn bar_flush(_: &mut super::Registers)->Result<(),&'static str>{crate::FLUSH.with(|n|n.set(n.get()+1));Ok(())}}
fn words(_: &mut Registers,_:u64,_:u64,_:&[u32])->Result<(),&'static str>{UC.with(|n|n.set(n.get()+1));Ok(())}
const COMMAND:u64=0x1000000;const SLOT:u64=0;const COMMAND_PAGES:usize=16;const REPORT_PAGES:usize=17;
struct Pages{frames:Vec<Box<[u8;4096]>>}
impl Pages {
 fn new()->Self{Self{frames:(0..REPORT_PAGES+COMMAND_PAGES).map(|_|Box::new([0;4096])).collect()}}
 fn va(&self,i:usize)->u64{self.frames[i].as_ptr() as u64}
 fn publish(&self){FENCES.with(|n|n.set(n.get()+1));}
 // WRITE
 fn get(&self,at:usize)->u32 {let at=at+REPORT_PAGES*4096;unsafe{((self.va(at/4096)+at as u64%4096) as *const u32).read_unaligned()}}
}
struct Range{start:u64}struct Allocation{range:Range}struct Identity{id:u32}struct Report{identity:Identity}
struct Session {bound:bool,quiesced:bool,commands:Allocation,pages:Pages,report:Report,bar:usize}
impl Session {// PUBLISH
}
fn main(){
 let mut userd=vec![0u32;1024];
 let mut s=Session{bound:true,quiesced:false,commands:Allocation{range:Range{start:0x400000}},pages:Pages::new(),report:Report{identity:Identity{id:0}},bar:userd.as_mut_ptr() as usize};
 s.pages.publish();FENCES.with(|n|n.set(0));
 // Each command crosses a physical page; four live slots must stay disjoint.
 let command:Vec<_>=(0..1500).map(|i|0xa0000000+i).collect();
 for sequence in [1,2,3,4,513] {
  let offset=16384+u64::from((sequence-1)%4)*8192;
  s.publish_words(&mut Registers,1<<30,sequence,&command,offset,8192,None).unwrap();
  for (i,&v) in command.iter().enumerate(){assert_eq!(s.pages.get(offset as usize+i*4),v);}
  assert_eq!(s.pages.get(((sequence-1)%512) as usize*8),(COMMAND+offset) as u32);
  assert_eq!(s.pages.get(((sequence-1)%512) as usize*8+4),((command.len() as u32)<<10)|0x100);
  assert_eq!(userd[0x8c/4],sequence%512);
 }
 assert_eq!(UC.with(Cell::get),0,"primary command uploads must not traverse uncached PRAMIN");
 assert_eq!(FLUSH.with(Cell::get),0,"coherent command memory requires ordering, not BAR flush polling");
 assert!(FENCES.with(Cell::get)>=10,"publish stores before GP_PUT");
 assert!(s.publish_words(&mut Registers,1<<30,6,&command,65536,8192,None).is_err());
 s.report.identity.id=1;
 s.publish_words(&mut Registers,1<<30,6,&command,16384,8192,None).unwrap();
 assert_eq!(UC.with(Cell::get),2,"terminal transport stays in VRAM");assert_eq!(FLUSH.with(Cell::get),1);
}
'''.replace('// PUBLISH', publisher).replace('// WRITE', write)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as d:
            p = Path(d)
            (p / 'check.rs').write_text(source)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', str(p / 'check.rs'), '-o', str(p / 'check')], check=True)
            subprocess.run([str(p / 'check')], check=True)
        constructor = item(channel, '        fn new_channel(')
        self.assertIn('COMMAND_PAGES', constructor)
        self.assertIn('COMMAND,', constructor[constructor.index('let dma ='):])


if __name__ == '__main__':
    unittest.main()
