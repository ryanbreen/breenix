"""A refused frame preserves the live document and session."""
import subprocess
import tempfile
import unittest
from pathlib import Path
from gt710_gr_live_unwind_test import item
ROOT = Path(__file__).resolve().parents[1]

class PresentError(unittest.TestCase):
    def test_acquire_and_commit_errors_return_to_the_event_loop(self):
        source = (ROOT / 'userspace/programs/src/guskit.rs').read_text()
        if 'fn present_frame(' in source:
            function = item(source, 'fn present_frame(')
        else:
            block = item(source[source.index('fn main()'):], 'if redraw {')
            function = '''fn present_frame(win: &mut Window, app: &mut App, menu: &MenuBar,
                font: &mut Option<CachedFont>, font_size: f32) -> Result<(), libbreenix::error::Error> {'''
            function += block[block.index('{')+1:-1].replace('&mut win', 'win').replace('&mut app', 'app').replace('&mut font', 'font').replace('&menu', 'menu') + '\nOk(())\n}'
        code = r'''
#[derive(Clone,Copy)] struct Clip {x:i32,y:i32,w:i32,h:i32}
impl Clip {fn new(x:i32,y:i32,w:i32,h:i32)->Self {Self{x,y,w,h}} fn is_empty(&self)->bool {self.w<=0||self.h<=0}}
mod libbreenix {pub mod error {#[derive(Debug,PartialEq)] pub struct Error;} pub mod window {pub mod abi {pub type Rect=crate::Clip;}}}
use libbreenix::error::Error;
impl std::fmt::Display for Error {fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result {f.write_str("refused")}}
struct App {damage:Option<Clip>, drawing:Vec<u8>, session:bool}
struct MenuBar; struct CachedFont; struct Surface {fail:bool}
struct Acquired {replay_full:bool,replay:Vec<Clip>}
struct Window {fail:u8,commits:usize}
impl Window {
 fn width(&self)->u32 {2296} fn height(&self)->u32 {1260}
 fn acquire(&mut self,_:u64)->Result<Acquired,Error> {if self.fail==1 {Err(Error)} else {Ok(Acquired{replay_full:false,replay:vec![]})}}
 fn surface(&mut self)->Result<&mut Surface,Error> {Ok(Box::leak(Box::new(Surface{fail:self.fail==3})))}
 fn commit(&mut self,_:&[Clip])->Result<(),Error> {if self.fail==2 {Err(Error)} else {self.commits+=1;Ok(())}}
}
fn render(surface: &mut Surface,_:&mut App,_:&MenuBar,_:&mut Option<CachedFont>,_:f32)->Result<(),String> {if surface.fail {Err("refused".into())} else {Ok(())}}
fn publish(_:&mut Window,_:&App,_:&MenuBar,_:f32) {}
UNION
FUNCTION
fn main() {
 for failure in [1,2,3] {
  let mut app=App{damage:None,drawing:vec![7,8,9],session:true};let mut win=Window{fail:failure,commits:0};
  assert_eq!(present_frame(&mut win,&mut app,&MenuBar,&mut None,14.0),Err("refused".into()));
  assert_eq!(app.drawing,[7,8,9]);assert!(app.session);
  win.fail=0;assert!(present_frame(&mut win,&mut app,&MenuBar,&mut None,14.0).is_ok());assert_eq!(win.commits,1);
 }
}
'''.replace('UNION', item(source,'fn union(')).replace('FUNCTION',function)
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp);(p/'check.rs').write_text(code)
            subprocess.run(['taskpolicy','-b','rustc','--edition=2021','-Dwarnings',str(p/'check.rs'),'-o',str(p/'check')],check=True)
            r=subprocess.run([str(p/'check')],capture_output=True,text=True)
            self.assertEqual(r.returncode,0,r.stdout+r.stderr)
if __name__=='__main__':unittest.main()
