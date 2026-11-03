"""Native stage records must bracket the latch, not GET acceptance alone."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class StageCounts(unittest.TestCase):
    def test_native_snapshot_waits_for_the_count_it_brackets(self):
        core = (ROOT / 'kernel/src/gpu/mod.rs').read_text()
        display = (ROOT / 'kernel/src/gpu/display.rs').read_text()
        producer = item(core, 'pub fn present_records(')
        helper = ''
        if 'pub(super) fn present_counts(' in display:
            helper = ('mod display { use super::*;\n' +
                      item(display, 'pub(super) fn present_counts(') + '\n}')
        fixture = r'''
#![allow(dead_code)] // Native wait support is unused by the pre-fix producer.
extern crate alloc;
use std::{cell::{Cell, RefCell}, collections::BTreeMap};
const EIO:i64=5; const EINTR:i64=4; const ETIMEDOUT:i64=110;
mod abi {pub mod backend {pub const GK208:u32=6;}}
mod dev {pub fn backend()->u32{crate::BACKEND.with(Cell::get)} use std::cell::Cell;}
struct Proc {presents:u64}
struct Display {outstanding:u32}
struct Core {display:Display, procs:BTreeMap<u64,Proc>, stalled:bool}
fn fresh()->Core{Core{display:Display{outstanding:0},procs:BTreeMap::from([(1,Proc{presents:19})]),stalled:false}}
thread_local! {
 static CORE:RefCell<Core>=RefCell::new(fresh());
 static BACKEND:Cell<u32>=const{Cell::new(6)};
 static FAILURE:Cell<u32>=const{Cell::new(0)};
 static WAITS:Cell<u32>=const{Cell::new(0)};
}
fn now_ns()->u64{26_467_591_912}
fn with_core<T>(f:impl FnOnce(&mut Core)->T)->Option<T>{CORE.with(|c|Some(f(&mut c.borrow_mut())))}
enum Stop{Timeout,Interrupted}
fn wait_for<T>(deadline:Option<u64>,interruptible:bool,mut f:impl FnMut(&mut Core)->Option<T>)->Result<T,Stop>{
 assert_eq!(deadline,Some(now_ns()+1_000_000_000)); assert!(interruptible);
 WAITS.with(|n|n.set(n.get()+1));
 if let Some(v)=with_core(&mut f).flatten(){return Ok(v);}
 match FAILURE.with(Cell::get){1=>return Err(Stop::Timeout),2=>return Err(Stop::Interrupted),_=>{}}
 // The real capture published count 20 at 26_480_396_585 ns. Until this
 // device completion, GET acceptance leaves the old count and one flip pending.
 with_core(|c|{assert_eq!(c.display.outstanding,1);c.display.outstanding=0;c.procs.get_mut(&1).unwrap().presents+=1;});
 Ok(with_core(f).flatten().expect("latch must make the count snapshot ready"))
}
HELPER
PRODUCER
trait Record {fn result(self)->Result<String,i64>;}
impl Record for String {fn result(self)->Result<String,i64>{Ok(self)}}
impl Record for Result<String,i64> {fn result(self)->Result<String,i64>{self}}
#[test]
fn live_draw_delta_includes_its_native_latch_and_read_failure_is_not_a_count(){
 let before=present_records().result().unwrap();assert_eq!(before,"1 19\n");
 with_core(|c|c.display.outstanding=1); // The requested frame was GET-accepted.
 let after=present_records().result().unwrap();
 assert_eq!(after,"1 20\n","post-draw snapshot returned before its native latch");
 assert_eq!(present_records().result().unwrap(),after,"late count escaped the request bracket");
 for (mode,errno) in [(1,ETIMEDOUT),(2,EINTR)] {
  with_core(|c|c.display.outstanding=1);FAILURE.with(|v|v.set(mode));
  assert_eq!(present_records().result(),Err(errno));
  CORE.with(|c|assert_eq!(c.borrow().procs[&1].presents,20,"failed wait fabricated a count"));
 }
 with_core(|c|{c.display.outstanding=0;c.stalled=true;});
 assert_eq!(present_records().result(),Err(EIO));
 // Virtual backends retain their immediate completed-count snapshot.
 with_core(|c|{*c=fresh();c.display.outstanding=1;});BACKEND.with(|b|b.set(4));
 let waits=WAITS.with(Cell::get);
 assert_eq!(present_records().result().unwrap(),"1 19\n");
 assert_eq!(WAITS.with(Cell::get),waits);
}
'''.replace('HELPER', helper).replace('PRODUCER', producer)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory(prefix='gt710-stage-counts-') as directory:
            path = Path(directory)
            (path / 'counts.rs').write_text(fixture)
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                           str(path / 'counts.rs'), '-o', str(path / 'counts')], check=True)
            subprocess.run([str(path / 'counts'), '--nocapture'], check=True)


if __name__ == '__main__':
    unittest.main()
