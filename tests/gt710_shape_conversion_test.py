"""Keep shape bytes exact while avoiding repeated general software division."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ShapeConversion(unittest.TestCase):
    def test_repeated_shape_colour_and_power_two_coordinates(self):
        encoder = (ROOT / 'kernel/src/gpu/gk208/encode.rs').read_text().replace(
            'include_bytes!("caps/integer.inputs")',
            'include_bytes!("' + str(ROOT / 'kernel/src/gpu/gk208/caps/integer.inputs') + '")')
        encoder = encoder.replace('gpu_abi::float::ratio_bits(', 'crate::counted_ratio(')
        source = r'''
extern crate alloc;
use std::sync::atomic::{AtomicUsize,Ordering};
static RATIOS:AtomicUsize=AtomicUsize::new(0);
fn counted_ratio(n:u32,d:u32)->u32{RATIOS.fetch_add(1,Ordering::Relaxed);gpu_abi::float::ratio_bits(n,d)}
#[allow(dead_code)]#[path="SHADERS"]mod shaders;
#[allow(dead_code)]#[path="CAPS"]mod caps;
#[allow(dead_code)]mod encode;
use gpu_abi::{draw,format,pipeline,stream::{self,Resolver,SourceInfo,TargetInfo}};
struct View(u32,u32);
impl Resolver for View{
 fn target(&self,_:u64)->Option<TargetInfo>{Some(TargetInfo{width:self.0,height:self.1,format:format::B8G8R8A8,invalid:false})}
 fn source(&self,_:u64)->Option<SourceInfo>{None}
 fn buffer_size(&self,_:u64)->Option<u64>{None}
 fn buffer_rect(&self,_:u64,_:u64)->[i32;4]{panic!("no buffer")}
}
impl encode::Bindings for View{
 fn render_target(&self,_:u64)->Option<encode::Target>{Some(encode::Target{id:1,address:0x200000000,width:self.0,height:self.1,format:format::B8G8R8A8,tile_mode:0x30})}
 fn sampled_source(&self,_:u64)->Option<encode::Target>{None}
}
fn main(){
 for (width,height)in[(256,256),(64,128),(257,255),(2560,1600)]{
  let instances:Vec<_>=(0..4).map(|n|{let mut i=draw::Instance{rect:[0,0,64,64],params:[0;16]};i.params[..4].copy_from_slice(&[4+n,6+n,45+n,48+n]);i}).collect();
  let mut words=Vec::new();stream::encode::begin(&mut words,1);draw::encode(&mut words,pipeline::LINE,0x8142a5e7,0,0,0,&instances);
  RATIOS.store(0,Ordering::Relaxed);
  let f=encode::translate_resident(words.as_slice(),&View(width,height)).unwrap();
  let calls=RATIOS.load(Ordering::Relaxed);
  assert_eq!(f.chunks.len(),4);
  for (c,i)in f.chunks.iter().zip(&instances){
   let mut at=0;let(mut cb,mut vb)=(None,None);
   while at<c.words.len(){let n=((c.words[at]>>16)&0x1fff)as usize;match(c.words[at]&0x1fff)*4{0x2380=>cb=Some(c.words[at+3]as usize),0x1c00=>vb=Some(c.words[at+3]as usize),_=>()};at+=n+1;}
   let cb=cb.unwrap();let vb=vb.unwrap();
   let colour:[u32;4]=[16,8,0,24].map(|s|gpu_abi::float::ratio_bits(0x8142a5e7>>s&255,255));
   for (k,want)in colour.into_iter().enumerate(){assert_eq!(u32::from_le_bytes(f.data[cb+80+k*4..cb+84+k*4].try_into().unwrap()),want);}
   let [x0,y0,x1,y1]=draw::clip(pipeline::LINE,*i,width,height,None).unwrap();
   for (n,[x,y])in [[x0,y0],[x1,y0],[x1,y1],[x0,y0],[x1,y1],[x0,y1]].into_iter().enumerate(){
    for (k,(p,size))in[(x,width),(y,height)].into_iter().enumerate(){let v=i64::from(p)*2-i64::from(size);let want=gpu_abi::float::ratio_bits(v.unsigned_abs()as u32,size)|if v<0{1<<31}else{0};assert_eq!(u32::from_le_bytes(f.data[vb+n*32+k*4..vb+n*32+k*4+4].try_into().unwrap()),want);}
   }
  }
  let expected=4+if width.is_power_of_two()&&height.is_power_of_two(){0}else{16};
  assert_eq!(calls,expected,"{width}x{height}: convert colour once per command; use exact scaling for power-two coordinates");
 }
}
'''.replace('SHADERS', str(ROOT / 'kernel/src/gpu/gk208/gr/shaders.rs')).replace('CAPS', str(ROOT / 'kernel/src/gpu/gk208/caps.rs'))
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory(prefix='gt710-shape-convert-') as directory:
            p = Path(directory)
            (p / 'encode.rs').write_text(encoder)
            (p / 'check.rs').write_text(source)
            subprocess.run(priority + ['rustc', '--edition=2021', '--crate-name', 'gpu_abi', '--crate-type', 'rlib', '--cfg', 'feature="alloc"', '-Dwarnings', str(ROOT / 'libs/gpu-abi/src/lib.rs'), '-o', str(p / 'libgpu_abi.rlib')], check=True)
            subprocess.run(priority + ['rustc', '--edition=2021', '--extern', f'gpu_abi={p / "libgpu_abi.rlib"}', '-Dwarnings', str(p / 'check.rs'), '-o', str(p / 'check')], check=True)
            subprocess.run([str(p / 'check')], check=True)


if __name__ == '__main__':
    unittest.main()
