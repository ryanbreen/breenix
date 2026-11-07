"""Run production LCD batching against a recording GPU and scalar draw oracle."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from gt710_gr_live_unwind_test import item
ROOT=Path(__file__).resolve().parents[1]

class LcdBatch(unittest.TestCase):
    def test_lcd_instances_overlap_order_and_eviction(self):
        source=(ROOT/'libs/libgfx/src/ttf_font.rs').read_text()
        code='\n'.join(item(source,n) for n in ['pub struct TextRun', 'pub fn draw_runs_subpixel(', 'fn draw_runs_mode('])
        fixture=r'''
use std::sync::Arc;
use std::collections::BTreeMap;
type Color=u32;
#[derive(Clone,Copy)]pub struct Box64{x0:i64,y0:i64,x1:i64,y1:i64}
impl Box64{fn wide(x:i64,y:i64,w:i64,h:i64)->Self{Self{x0:x,y0:y,x1:x+w,y1:y+h}}fn intersect(&self,o:&Self)->Self{Self{x0:self.x0.max(o.x0),y0:self.y0.max(o.y0),x1:self.x1.min(o.x1),y1:self.y1.min(o.y1)}}fn is_empty(&self)->bool{self.x0>=self.x1||self.y0>=self.y1}fn corners(&self)->[i32;4]{[self.x0 as i32,self.y0 as i32,self.x1 as i32,self.y1 as i32]}}
mod crate_surface{pub type Rect=super::Box64;}
mod surface{pub use super::crate_surface::Rect;}
pub struct Surface{clip:Box64,stack:Vec<Box64>,draws:Vec<(u32,u64,Vec<Instance>)>,marks:usize}
impl Surface{fn push_clip(&mut self,b:Box64){self.stack.push(self.clip);self.clip=self.clip.intersect(&b);}fn pop_clip(&mut self){self.clip=self.stack.pop().unwrap();}fn clip_box(&self)->Box64{self.clip}fn fail(&mut self,e:i64){panic!("device error {e}")}fn mark(&mut self,_:Box64){self.marks+=1;}fn submit(&mut self,f:impl FnOnce(&mut Context)){f(&mut Context(&mut self.draws));}}
#[derive(Clone)]struct Instance{rect:[i32;4],params:[i32;5]}
fn instance(rect:[i32;4],p:&[i32;5])->Instance{Instance{rect,params:*p}}
struct Context<'a>(&'a mut Vec<(u32,u64,Vec<Instance>)>);
impl Context<'_>{fn draw(&mut self,p:u32,c:u32,t:u64,_:u64,_:u64,i:&[Instance]){assert_eq!(p,7);self.0.push((c,t,i.to_vec()));}}
fn word(c:Color)->u32{c}
mod pipeline{pub const COVERAGE:u32=7;}
mod abi{pub mod limits{pub const WORK:u64=4_000_000;}}
mod work{pub fn add(_:u64){}}
struct Target(u64);impl Target{fn handle(&self)->u64{self.0}}
struct Region{x:usize,y:usize,target:Target}
struct AtlasGlyph{x_offset:i32,y_offset:i32,width:usize,height:usize,region:Option<Region>}
#[derive(Clone,Copy)]struct Stats{entries:usize,misses:u64}
pub struct CachedFont{calls:usize,evict:bool}
struct Font;impl Font{fn size26(_:f32)->Result<i32,()>{Ok(1024)}}
enum Error{Device(i64),Other}
fn image(g:u16)->Arc<AtlasGlyph>{Arc::new(AtlasGlyph{x_offset:0,y_offset:0,width:if g==b'W' as u16{12}else{6},height:8,region:Some(Region{x:g as usize*16,y:0,target:Target(if g==b'X' as u16{2}else{1})})})}
impl CachedFont{fn cache_stats(&self)->Stats{Stats{entries:if self.evict&&self.calls>0{0}else{100},misses:0}}fn run_mode(&mut self,p:&[(u16,i64)],_:i32,lcd:bool,mut out:impl FnMut(Result<(Arc<AtlasGlyph>,i64),Error>)){assert!(lcd,"LCD caller used grayscale coverage");self.calls+=1;for(g,x)in p{out(Ok((image(*g),*x)));}}}
fn decode_utf8(t:&[u8])->impl Iterator<Item=u32>+'_ {t.iter().map(|b|*b as u32)}
fn glyphs_of(_: &CachedFont,c:impl Iterator<Item=u32>)->impl Iterator<Item=u16>{c.map(|v|v as u16)}
fn visible(_: &Surface,_:&mut CachedFont,g:Vec<u16>,_:i32,_:i32,_:f32)->(Vec<(u16,i64)>,i32){(g.into_iter().enumerate().map(|(i,g)|(g,i as i64*8)).collect(),0)}
fn draw_text_bytes(_: &mut Surface,_:&mut CachedFont,_:&[u8],_:i32,_:i32,_:f32,_:Color){panic!("LCD fallback used grayscale");}
fn draw_glyphs_subpixel(f:&mut Surface,_:&mut CachedFont,g:Vec<u16>,x:i32,y:i32,_:f32,c:Color)->i32{for(i,g)in g.into_iter().enumerate(){let a=image(g);let b=Box64::wide(x as i64+i as i64*8,y as i64,a.width as i64,a.height as i64).intersect(&f.clip_box());if b.is_empty(){continue;}let [x0,y0,_,_]=b.corners();let r=a.region.as_ref().unwrap();f.draws.push((c,r.target.handle(),vec![instance(b.corners(),&[x0,y0,r.x as i32+(b.x0-x as i64-i as i64*8)as i32,r.y as i32+(b.y0-y as i64)as i32,1])]));f.mark(b);}0}
CODE
fn new()->Surface{Surface{clip:Box64::wide(0,0,1000,1000),stack:vec![],draws:vec![],marks:0}}
fn pixels(f:&Surface)->BTreeMap<(i32,i32),Vec<(u32,u64,i32,i32)>>{let mut p:BTreeMap<_,Vec<_>>=BTreeMap::new();for(c,t,instances)in &f.draws{for i in instances{let[x0,y0,x1,y1]=i.rect;for y in y0..y1{for x in x0..x1{p.entry((x,y)).or_default().push((*c,*t,i.params[2]+x-i.params[0],i.params[3]+y-i.params[1]));}}}}p}
fn main(){
 // Exercise error variants as values so this narrow fixture builds warning-free.
 let errors=[Error::Device(5),Error::Other];assert_eq!(errors.len(),2);
 for evict in [false,true]{for text in [&b"Breenix"[..],&b"WWXYZ"[..]]{
 let mut actual=new();let mut expected=new();let mut font=CachedFont{calls:0,evict};
 let runs=[TextRun{bytes:text,x:2,y:3,size:16.,fg:123,clip:Box64::wide(4,4,90,90)},TextRun{bytes:b"12:34:56 ET",x:200,y:3,size:16.,fg:123,clip:Box64::wide(200,3,400,400)}];
 for r in &runs{expected.push_clip(r.clip);draw_glyphs_subpixel(&mut expected,&mut font,r.bytes.iter().map(|b|*b as u16).collect(),r.x,r.y,r.size,r.fg);expected.pop_clip();}
 draw_runs_subpixel(&mut actual,&mut font,&runs);
 assert_eq!(pixels(&actual),pixels(&expected),"LCD sample positions or overlap order changed; eviction={evict}");
 if !evict{assert!(actual.draws.len()<expected.draws.len());}
 }}
}
'''.replace('CODE',code)
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp);(p/'check.rs').write_text(fixture)
            priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
            subprocess.run(priority+['rustc','--edition=2021','-Dwarnings',str(p/'check.rs'),'-o',str(p/'check')],check=True)
            subprocess.run([str(p/'check')],check=True)

if __name__=='__main__':unittest.main()
