"""Private QEMU window-copy Sync streams must drain their written destination."""
from pathlib import Path
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust
ROOT=Path(__file__).resolve().parents[1]
class CopyCompletion(unittest.TestCase):
 def test_private_copy_has_its_own_qemu_completion_read(self):
  source=(ROOT/'kernel/src/gpu/window_targets.rs').read_text();body=item(source,'    fn execute_taken(')
  start=body.index('let mut enc = encode::Enc::new();');end=body.index('Chunks::Virgl(enc.finish().ok_or(ENOMEM)?)',start)+len('Chunks::Virgl(enc.finish().ok_or(ENOMEM)?)')
  run_rust(self,r'''
use std::sync::atomic::{AtomicBool,Ordering};
static QEMU:AtomicBool=AtomicBool::new(true);
mod platform_config{pub fn is_qemu()->bool{crate::QEMU.load(crate::Ordering::Relaxed)}}
const FLIP_Y:bool=false;const ENOMEM:i64=12;
mod encode{
 pub struct Enc{pub rendered:bool,pub drained:Option<(u32,u32)>}
 impl Enc{pub fn new()->Self{Self{rendered:false,drained:None}}pub fn completion_read(&mut self,id:u32,pitch:u32){self.drained=Some((id,pitch));}pub fn finish(self)->Option<Self>{Some(self)}}
 pub struct Extras{pub vertex_buffer:u32,pub scratch:[u32;2],pub data:u32,pub data_width:u32,pub flip_y:bool,pub corrupt:Option<()>,pub suppress:Option<()>,pub swap:Option<()>}
 pub fn translate(enc:&mut Enc,_:&mut(),_:&[u32],_:&(),e:Extras){assert_eq!((e.vertex_buffer,e.scratch,e.data,e.data_width,e.flip_y),(1,[2,3],0,0,false));assert!((e.corrupt,e.suppress,e.swap)==(None,None,None));enc.rendered=true;}
}
enum Chunks{Virgl(encode::Enc)}
struct Target{id:u32,width:u32}
impl Target{fn stream(&self)->Result<Chunks,i64>{let mut objects=();let words=&[];let bindings=();let(vb,scratch,scratch_texture)=(1,2,3);Ok({@ENCODE@})}}
#[test]fn private_stream_completes_without_a_foreground_submit(){
 assert!(platform_config::is_qemu());let target=Target{id:17,width:240};
 let mut expected=encode::Enc::new();expected.completion_read(target.id,(target.width*4).next_multiple_of(256));
 let Chunks::Virgl(got)=target.stream().unwrap();assert!(got.rendered);assert_eq!(got.drained,expected.drained,"private Sync copy has no foreground Submit to enroll its completion drain");
 QEMU.store(false,Ordering::Relaxed);let Chunks::Virgl(got)=target.stream().unwrap();assert!(got.rendered);assert!(got.drained.is_none());
}
'''.replace('@ENCODE@',body[start:end]))
if __name__=='__main__':unittest.main()
