"""Run the production pointer-order case with delayed FIFO focus-click delivery.

Derived from the W13 acceptance reproduction. Only the clock, host transport,
and client storage are modeled; press_order, focus, click and await_event are
compiled from the userspace sources without changing their bodies.
"""
from pathlib import Path
import subprocess,tempfile,sys
sys.path.insert(0,str(Path(__file__).resolve().parent))
from gt710_gr_live_unwind_test import item
root=Path(__file__).resolve().parents[1]
cases=(root/'userspace/programs/src/wm_suite/cases.rs').read_text()
desktop=(root/'userspace/programs/src/wm_suite/desktop.rs').read_text()
prelude=r'''
use std::sync::Mutex;
use breengel_abi::{self as abi, event_kind as kind, WindowEvent, Plain};
type CaseResult=Result<(),String>;
fn now()->u64 {STATE.lock().unwrap().now*1_000_000}
fn elapsed(_: &str, _:u64, _:u64){}
fn check(b:bool, s:&str)->CaseResult { if b {Ok(())} else {Err(s.into())} }
struct State { now:u64, focus_release:u64, packet:usize, pending:Vec<(u64,WindowEvent)>, history:Vec<WindowEvent>, delivered:Vec<u64> }
static STATE:Mutex<State>=Mutex::new(State { now:0,focus_release:0,packet:0,pending:Vec::new(),history:Vec::new(),delivered:Vec::new() });
struct Desktop;
struct Client { events:Vec<WindowEvent> }
fn until(_ms:u64,_s:&str,mut f:impl FnMut()->Result<bool,String>)->CaseResult {
 for _ in 0..=200 { if f()? {return Ok(());} STATE.lock().unwrap().now+=5; }
 Err("deadline".into())
}
mod time { pub fn sleep_ms(n:u64)->Result<(),String> {super::STATE.lock().unwrap().now+=n;Ok(())} }
fn one()->Result<(Desktop,Client),String> {Ok((Desktop,Client {events:Vec::new()}))}
impl Client {
 fn info(&self)->Result<abi::WinInfo,String> { let mut w=abi::WinInfo::ZERO;w.x=100;w.y=100;w.width=240;w.height=160;Ok(w) }
 fn clear(&mut self)->CaseResult { STATE.lock().unwrap().history.clear();self.events.clear();Ok(()) }
 fn snapshot(&mut self)->CaseResult {
  let mut s=STATE.lock().unwrap();
  while !s.pending.is_empty() && s.pending[0].0<=s.now {
   let (_,e)=s.pending.remove(0);s.delivered.push(e.serial);s.history.push(e);
  }
  self.events=s.history.clone();Ok(())
 }
'''
await_event=item(desktop,'    pub fn await_event(')
program=prelude+await_event+'\n}\nimpl Desktop {\n'
program+=item(desktop,'    pub fn click(')+'\n'+item(desktop,'    pub fn focus(')
program+=r'''
 fn pointer(&self,x:i32,y:i32,_buttons:u32,_wheel:i32)->CaseResult {
  let mut s=STATE.lock().unwrap();
  // FIFO source and delivery order. Only the old focus release crosses CLEAR.
  let (at,serial,k,pressed)=match s.packet {
   0=>(0,10,kind::POINTER_BUTTON,1),
   1=>(s.focus_release,20,kind::POINTER_BUTTON,0),
   2=>(400,30,kind::POINTER_BUTTON,1),
   3=>(450,40,kind::POINTER_MOVE,0),
   4=>(500,50,kind::POINTER_BUTTON,0),_=>panic!("extra packet")
  };
  let mut e=WindowEvent::new(k);e.serial=serial;e.pressed=pressed;e.x=x-100;e.y=y-100;
  s.pending.push((at,e));s.packet+=1;Ok(())
 }
}
'''
program+=item(cases,'pub fn press_order(')
program+=r'''
fn main() {
 for (delay, older_release) in [(150,false),(350,false),(350,true)] {
  *STATE.lock().unwrap()=State {now:0,focus_release:delay,packet:0,pending:Vec::new(),history:Vec::new(),delivered:Vec::new()};
  if older_release {
   let mut e=WindowEvent::new(kind::POINTER_BUTTON);e.serial=5;
   STATE.lock().unwrap().pending.push((0,e));
  }
  println!("focus release delivered at {delay}ms; older release={older_release}");
  let result=press_order();let s=STATE.lock().unwrap();
  assert!(s.delivered.windows(2).all(|p|p[0]<p[1]),"transport reordered events");
  println!("result={result:?}; FIFO delivered={:?}; pending={:?}",s.delivered,s.pending.iter().map(|(_,e)|e.serial).collect::<Vec<_>>());
  assert_eq!(result,Ok(()),"FIFO focus release at {delay}ms must not contaminate the gesture");
  let expected=if older_release {vec![5,10,20,30,40,50]} else {vec![10,20,30,40,50]};
  assert_eq!(s.delivered,expected);
  assert!(s.pending.is_empty());
 }
}
'''
with tempfile.TemporaryDirectory(prefix='e21-4-press-order-') as d:
 p=Path(d);compiler=(['taskpolicy','-b'] if sys.platform=='darwin' else [])+['rustc','--edition=2021','-Dwarnings']
 subprocess.run(compiler+['--crate-name','breengel_abi','--crate-type','rlib',str(root/'libs/breengel-abi/src/lib.rs'),'--out-dir',d],check=True)
 (p/'check.rs').write_text(program)
 subprocess.run(compiler+[str(p/'check.rs'),'--extern',f'breengel_abi={p}/libbreengel_abi.rlib','-o',str(p/'check')],check=True)
 subprocess.run([str(p/'check')],check=True)
