"""Replay the interrupted-save case's ordering with a deterministic recovery clock."""
from pathlib import Path
import unittest
from suite_collection_test import run_rust

ROOT = Path(__file__).resolve().parents[1]

class ShareGoneOrder(unittest.TestCase):
    def test_recovery_and_error_commit_precede_the_normal_input_check(self):
        source = (ROOT / 'userspace/programs/src/guiapps_suite/guskit.rs').read_text()
        flow = source.split('    let key_result =', 1)[1].split('    // tests/atomic_file.rs', 1)[0]
        flow = 'let key_result =' + flow
        waiter = r'''
 fn key_wait_node(&mut self,_:u16,_:&[Modifier],node:&str,p:impl Fn(&Value)->bool)->Result<(),String> {
  self.wait_node(node,p)
 }
''' if 'key_wait_node' in flow else ''
        run_rust(self, r'''
use std::cell::Cell;
thread_local! { static RECOVERED: Cell<bool> = const { Cell::new(false) }; }
enum Modifier { Ctrl }
struct Value;
impl std::ops::Index<&str> for Value { type Output=Value; fn index(&self,_:&str)->&Value { self } }
impl PartialEq<bool> for Value { fn eq(&self,_:&bool)->bool { true } }
impl Value { fn as_str(&self)->Option<&str> { Some("EIO") } }
struct App;
impl App {
 fn key(&mut self, _:u16, _:&[Modifier])->Result<(),String> {
  if RECOVERED.with(Cell::get) { Ok(()) } else { Err("input unprocessed within 2 s; recovery takes 3 s".into()) }
 }
 fn wait_node(&mut self,_:&str,p:impl Fn(&Value)->bool)->Result<(),String> {
  RECOVERED.with(|r|r.set(true)); assert!(p(&Value)); Ok(())
 }
 WAITER
}
struct Watcher;
impl Watcher { fn join(self)->Result<Result<(),String>,()> { RECOVERED.with(|r|r.set(true)); Ok(Ok(())) } }
struct Restart(Option<u64>);
fn scenario()->Result<(),String> {
 let mut t=App; let watcher=Watcher; let mut restart=Restart(Some(1));
 FLOW
 assert!(restart.0.is_none());
 // A fresh physical input still uses the unchanged normal input contract.
 t.key(25,&[])
}
#[test] fn interrupted_save_then_input() { scenario().unwrap(); }
'''.replace('FLOW', flow).replace('WAITER', waiter))

if __name__ == '__main__': unittest.main()
