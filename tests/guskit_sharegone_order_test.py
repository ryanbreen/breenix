"""Replay production save/input ordering with a deterministic recovery schedule."""
from pathlib import Path
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust

ROOT = Path(__file__).resolve().parents[1]

class ShareGoneOrder(unittest.TestCase):
    def test_recovery_and_error_commit_precede_the_normal_input_check(self):
        source = (ROOT / 'userspace/programs/src/guiapps_suite/guskit.rs').read_text()
        flow = 'let key_result =' + source.split('    let key_result =', 1)[1].split('    // tests/atomic_file.rs', 1)[0]
        app = (ROOT / 'libs/libuitest/src/app.rs').read_text()
        method = item(app, 'pub fn key_wait_node(') if 'pub fn key_wait_node(' in app else ''
        failure_check = r'''
#[test] fn failed_result_still_releases_every_held_key() {
 let mut app=App::new(); app.fail_result=true;
 assert!(app.key_wait_node(31,&[Modifier::Ctrl],"status",|_|true).is_err());
 EVENTS.with(|e|assert_eq!(*e.borrow(),[(29,true),(31,true),(31,false),(29,false)]));
}
''' if method else ''
        run_rust(self, r'''
use std::cell::{Cell,RefCell};
thread_local! {
 static RECOVERED: Cell<bool> = const { Cell::new(false) };
 static EVENTS: RefCell<Vec<(u16,bool)>> = const { RefCell::new(Vec::new()) };
}
#[derive(Clone,Copy,PartialEq)] enum Modifier { Ctrl }
impl Modifier { fn code(self)->u16 { match self { Self::Ctrl=>29 } } }
mod serde_json {
 pub struct Value;
 impl std::ops::Index<&str> for Value { type Output=Value; fn index(&self,_:&str)->&Value { self } }
 impl PartialEq<bool> for Value { fn eq(&self,_:&bool)->bool { true } }
 impl PartialEq<&str> for Value { fn eq(&self,_:&&str)->bool { true } }
 impl Value { pub fn as_str(&self)->Option<&str> { Some("EIO") } }
}
struct Event { serial:u64 }
struct Delivery { window:u64 }
mod input {
 use super::*;
 pub fn delivered(serial:u64)->Result<Delivery,String> {
  if serial!=31 || RECOVERED.with(Cell::get) { Ok(Delivery{window:16}) }
  else { Err("input unprocessed within 2 s; recovery takes 3 s".into()) }
 }
}
struct Keys;
impl Keys {
 fn backend_mut(&mut self)->&mut Self { self }
 fn send_key(&mut self,code:u16,pressed:bool)->Result<Event,String> {
  EVENTS.with(|e|e.borrow_mut().push((code,pressed)));
  Ok(Event{serial:if pressed { u64::from(code) } else {0}})
 }
}
struct App { keys:Keys,window:u64,fail_result:bool }
impl App {
 fn new()->Self { Self{keys:Keys,window:16,fail_result:false} }
 fn key(&mut self,_:u16,_:&[Modifier])->Result<(),String> {
  input::delivered(31).map(|_|())
 }
 fn wait_node(&mut self,_:&str,p:impl Fn(&serde_json::Value)->bool)->Result<(),String> {
  RECOVERED.with(|r|r.set(true)); assert!(p(&serde_json::Value));
  if self.fail_result {Err("save result watchdog".into())} else {Ok(())}
 }
 METHOD
}
struct Watcher;
impl Watcher { fn join(self)->Result<Result<(),String>,()> { RECOVERED.with(|r|r.set(true)); Ok(Ok(())) } }
struct Restart(Option<u64>);
fn scenario()->Result<(),String> {
 let mut t=App::new(); let watcher=Watcher; let mut restart=Restart(Some(1));
 // The pre-fix flow has no explicit low-level injection. Exercise the fixture
 // backend without changing recovery state so both versions compile cleanly.
 assert_eq!(t.keys.backend_mut().send_key(29,true)?.serial,29);
 assert_eq!(t.window,16); assert_eq!(Modifier::Ctrl.code(),29);
 FLOW
 assert!(restart.0.is_none());
 t.key(25,&[])
}
#[test] fn interrupted_save_then_input() { scenario().unwrap(); }
FAILURE_CHECK
'''.replace('FLOW', flow).replace('METHOD', method).replace('FAILURE_CHECK', failure_check))

    def test_error_return_still_tears_down_the_owned_app_session(self):
        source = (ROOT / 'libs/libuitest/src/pointer_live.rs').read_text()
        cleanup = item(source, 'impl Drop for SessionHandle')
        cleanup = cleanup.replace("SessionHandle<'_>", 'SessionHandle')
        run_rust(self, r'''
use std::sync::{Arc,atomic::{AtomicUsize,Ordering}};
struct Session { processes:Arc<AtomicUsize>,windows:Arc<AtomicUsize> }
impl Session { fn teardown(&mut self)->Result<(),String> {
 self.processes.store(0,Ordering::SeqCst); self.windows.store(0,Ordering::SeqCst); Ok(())
} }
enum SessionHandle { Owned(Session),Borrowed(()) }
CLEANUP
fn failed_case(processes:Arc<AtomicUsize>,windows:Arc<AtomicUsize>)->Result<(),String> {
 let _borrowed=SessionHandle::Borrowed(());
 let _app=SessionHandle::Owned(Session{processes,windows});
 Err("failed save assertion".into())
}
#[test] fn error_return_has_no_app_or_window() {
 let processes=Arc::new(AtomicUsize::new(1)); let windows=Arc::new(AtomicUsize::new(1));
 assert!(failed_case(processes.clone(),windows.clone()).is_err());
 assert_eq!(processes.load(Ordering::SeqCst),0); assert_eq!(windows.load(Ordering::SeqCst),0);
}
'''.replace('CLEANUP', cleanup))

if __name__ == '__main__': unittest.main()
