"""Execute BWM's startup block and force font/target initialization to overlap."""
from pathlib import Path
import os
import subprocess
import unittest
from suite_collection_test import run_rust
ROOT = Path(__file__).resolve().parents[1]

class FontOverlap(unittest.TestCase):
    def test_font_coverage_is_ready_alongside_targets(self):
        rel = 'userspace/programs/src/bwm.rs'
        old = os.environ.get('UITEST_OLD_COMMIT')
        source = subprocess.check_output(['git','show',f'{old}:{rel}'], cwd=ROOT, text=True) if old else (ROOT/rel).read_text()
        block = source[source.index('    // The GPU core composes the desktop'):source.index('    // Paint the decorative background')]
        run_rust(self, r'''
use std::sync::{Mutex,mpsc::{channel,Sender,Receiver}};
static READY:Mutex<Option<Receiver<()>>>=Mutex::new(None);
static STARTED:Mutex<Option<Sender<()>>>=Mutex::new(None);
static RESUME:Mutex<Option<Sender<()>>>=Mutex::new(None);
static HELD:Mutex<Option<Receiver<()>>>=Mutex::new(None);
static TRACE:Mutex<Vec<String>>=Mutex::new(Vec::new());
pub mod libuitest {pub mod startup {pub fn mark(name:&str){super::super::TRACE.lock().unwrap().push(name.into());}}}
pub mod process {pub fn exit(_code:i32)->!{panic!("startup exited")}}
const APPBAR_HEIGHT:usize=36;
fn px(v:usize)->usize{v*2}
struct Ui{scale:usize}fn ui()->Ui{Ui{scale:2}}
fn ttf_font_size()->f32{14.0}
struct Compositor;
impl Compositor{fn open(w:u32,h:u32,appbar:u32,scale:u32)->Result<Self,String>{
 assert_eq!((w,h,appbar,scale),(2560,1600,72,2));
 READY.lock().unwrap().as_ref().unwrap().recv_timeout(std::time::Duration::from_millis(250)).expect("cold font work is still serialized after GPU target creation");
 RESUME.lock().unwrap().take().unwrap().send(()).unwrap();Ok(Self)
}}
#[derive(Debug)]struct FontConfig{display_path:String}
impl FontConfig{fn load()->Self{Self{display_path:"/dev/null".into()}}}
struct Font;impl Font{fn parse(_bytes:&[u8])->Result<Self,()>{Ok(Self)}}
struct CachedFont{prepared:bool}
impl CachedFont {
 fn new(_font:Font,cap:usize)->Self{assert_eq!(cap,256);Self{prepared:false}}
 fn observe_phases(&mut self,observer:fn(&'static str)){observer("font-observed");}
 fn prepare_subpixel(&mut self,texts:&[&[u8]],size:f32)->Result<(),String>{
  assert_eq!(texts,&[b"Breenix".as_slice(),b"00:00:00 ET".as_slice()]);assert_eq!(size,28.0);
  STARTED.lock().unwrap().take().unwrap().send(()).unwrap();
  HELD.lock().unwrap().as_ref().unwrap().recv_timeout(std::time::Duration::from_millis(250)).expect("target work did not overlap font preparation");
  self.prepared=true;Ok(())
 }
}
#[test]fn targets_and_cold_taskbar_prepare_concurrently(){
 let(tx,rx)=channel();*STARTED.lock().unwrap()=Some(tx);*READY.lock().unwrap()=Some(rx);
 let(tx,rx)=channel();*RESUME.lock().unwrap()=Some(tx);*HELD.lock().unwrap()=Some(rx);
 let _ = CachedFont::prepare_subpixel; let _ = ttf_font_size();
 let(screen_w,screen_h)=(2560usize,1600usize);let clock_text=*b"00:00:00 ET";
 @BLOCK@
 assert!(ui_font.as_mut().unwrap().prepared,"cold taskbar coverage stayed on drawing path");
 let _=&mut gpu;let _=clock_text;
 assert!(TRACE.lock().unwrap().iter().any(|s|s=="fonts-ready"));
}
'''.replace('@BLOCK@',block))

if __name__ == '__main__': unittest.main()
