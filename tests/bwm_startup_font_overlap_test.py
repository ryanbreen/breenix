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
        block = source[source.index('    // The GPU core composes the desktop'):source.index('    let mut windows:')]
        if source.index('    let claim_epoch = match session::claim()') < source.index('    // The GPU core composes the desktop'):
            block = 'let claim_epoch=session::claim().unwrap();let scene_fd=0;let mut desktop_tree:Option<desktop_publish::Publisher>=None;let mut clock_wake:Option<()>=None;let _=clock.participate();\n'+block
        run_rust(self, r'''
use std::sync::{Mutex,mpsc::{channel,Sender,Receiver}};
#[path="@PREP@"] pub mod startup_prepare;
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
static DEVICE_READY:std::sync::atomic::AtomicBool=std::sync::atomic::AtomicBool::new(false);
pub mod libgfx{pub mod stage{pub fn context()->Result<u64,String>{super::super::DEVICE_READY.store(true,std::sync::atomic::Ordering::Relaxed);Ok(1)}}}
mod session{pub fn claim()->Result<u64,String>{super::READY.lock().unwrap().as_ref().unwrap().recv_timeout(std::time::Duration::from_millis(250)).expect("font preparation still begins after the blocking compositor claim");Ok(7)}pub fn scene_fd()->Result<i32,String>{Ok(9)}}
mod desktop_publish{pub struct Publisher;impl Publisher{pub fn new()->Result<Self,String>{Ok(Self)}}}
struct Clock;impl Clock{fn participate(&mut self)->Result<(),String>{Ok(())}fn begin_epoch(&mut self){}fn update_clock(&mut self)->Option<i64>{Some(0)}fn wall_sec(&self)->i64{0}}
fn format_clock(_:i64,out:&mut[u8;11]){*out=*b"00:00:00 ET";}
mod clock_wake{pub fn open()->Result<Option<()>,String>{Ok(None)}}
struct Surface;
fn paint_background(_fb:&mut Surface){RESUME.lock().unwrap().take().unwrap().send(()).unwrap();}
struct Compositor{background:Surface}
impl Compositor{fn open(w:u32,h:u32,appbar:u32,scale:u32)->Result<Self,String>{
 assert_eq!((w,h,appbar,scale),(2560,1600,72,2));
 assert!(DEVICE_READY.load(std::sync::atomic::Ordering::Relaxed),"font context must exist before reserving the compositor context");
 Ok(Self{background:Surface})
}
fn paint_background(&mut self)->Result<(),String>{paint_background(&mut self.background);Ok(())}
}
#[derive(Debug)]struct FontConfig{display_path:String}
impl FontConfig{fn load()->Self{Self{display_path:"/dev/null".into()}}}
struct Font;impl Font{fn parse(_bytes:&[u8])->Result<Self,()>{Ok(Self)}}
struct CachedFont{prepared:bool}
impl CachedFont {
 fn new(_font:Font,cap:usize)->Self{assert_eq!(cap,256);Self{prepared:false}}
 fn observe_phases(&mut self,observer:fn(&'static str)){observer("font-observed");}
 fn prepare_subpixel(&mut self,texts:&[&[u8]],size:f32)->Result<(),String>{
  assert_eq!(texts,&[b"Breenix".as_slice(),b"00:00:00 ET".as_slice(),b"0123456789: ET".as_slice(),b"x-".as_slice()]);assert_eq!(size,28.0);
  STARTED.lock().unwrap().take().unwrap().send(()).unwrap();
  HELD.lock().unwrap().as_ref().unwrap().recv_timeout(std::time::Duration::from_millis(250)).expect("background preparation remained serialized after the font join");
  self.prepared=true;Ok(())
 }
}
#[test]fn targets_and_cold_taskbar_prepare_concurrently(){
 let(tx,rx)=channel();*STARTED.lock().unwrap()=Some(tx);*READY.lock().unwrap()=Some(rx);
 let(tx,rx)=channel();*RESUME.lock().unwrap()=Some(tx);*HELD.lock().unwrap()=Some(rx);
 let _=Clock::begin_epoch;let _=Clock::update_clock;let _=Clock::wall_sec;let _=format_clock;
 let mut clock=Clock;let _=clock_wake::open;
 let _ = desktop_publish::Publisher::new;let _=session::scene_fd;let _=libgfx::stage::context;
 let _ = Compositor::paint_background;
 let _ = CachedFont::prepare_subpixel; let _ = ttf_font_size();
 let(screen_w,screen_h)=(2560usize,1600usize);let mut clock_text=*b"00:00:00 ET";
 let startup_titles:Vec<&[u8]>=Vec::new();
 @BLOCK@
 assert!(ui_font.as_mut().unwrap().prepared,"cold taskbar coverage stayed on drawing path");
 let _=&mut gpu;let _=&mut clock_text;let _=&mut desktop_tree;let _=&mut clock_wake;let _=(claim_epoch,scene_fd);
 assert!(TRACE.lock().unwrap().iter().any(|s|s=="fonts-ready"));
}
'''.replace('@BLOCK@',block).replace('@PREP@',str(ROOT/'userspace/programs/src/bwm/startup_prepare.rs')))

if __name__ == '__main__': unittest.main()
