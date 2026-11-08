"""Check diagnostic phase publication around the actual glyph submission and wait."""
from pathlib import Path
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust
ROOT=Path(__file__).resolve().parents[1]
class FontPhases(unittest.TestCase):
    def test_success_and_refusal_distinguish_submission_from_wait(self):
        source=(ROOT/'libs/libgfx/src/ttf_font.rs').read_text()
        phase=item(source,'fn phase(&self,') if 'fn phase(&self,' in source else 'fn phase(&self,_:&\'static str){}'
        run_rust(self,r'''
use std::sync::{Arc,Mutex,atomic::{AtomicBool,Ordering}};
static EVENTS:Mutex<Vec<&'static str>>=Mutex::new(Vec::new());
static SEND_FAIL:AtomicBool=AtomicBool::new(false);static WAIT_FAIL:AtomicBool=AtomicBool::new(false);
fn event(s:&'static str){EVENTS.lock().unwrap().push(s)}
#[derive(Debug)]enum Error{Device(i64)}
type AtlasGlyph=u32;
#[derive(Default)]struct Run;
struct Fence;impl Fence{fn wait(self,_:u64)->Result<(),i64>{event("wait");if WAIT_FAIL.load(Ordering::Relaxed){Err(-5)}else{Ok(())}}}
impl Run{fn send(self)->Result<Option<Fence>,Error>{event("send");if SEND_FAIL.load(Ordering::Relaxed){Err(Error::Device(-5))}else{Ok(Some(Fence))}}}
struct Entry{slot:Option<(u64,usize)>}struct Stats{entries:usize}
struct Batch{run:Run,draws:Vec<(Arc<AtlasGlyph>,i64)>,slots:Vec<(u64,usize)>,first:u64}
struct CachedFont{clock:u64,entries:Vec<Entry>,stats:Stats,phase_observer:Option<fn(&'static str)>}
impl CachedFont{fn free(&mut self,_:u64,_:usize){event("free")}@PHASE@ @FINISH@}
#[test]fn phases_and_original_cleanup(){
 for (observed,send_fail,wait_fail) in [(false,false,false),(true,false,false),(true,true,false),(true,false,true)]{
  SEND_FAIL.store(send_fail,Ordering::Relaxed);WAIT_FAIL.store(wait_fail,Ordering::Relaxed);
  let mut font=CachedFont{clock:7,entries:vec![Entry{slot:Some((4,2))}],stats:Stats{entries:1},phase_observer:observed.then_some(event)};
  // Read the observer field and exercise the helper on both producer versions.
  let _=font.phase_observer;font.phase("setup");EVENTS.lock().unwrap().clear();
  let mut batch=Batch{run:Run,draws:vec![(Arc::new(9),3)],slots:vec![(4,2)],first:0};
  font.finish(&mut batch,&mut |value|match value{Ok((v,dx))=>{assert_eq!((*v,dx),(9,3));event("image")},Err(Error::Device(e))=>{assert_eq!(e,-5);event("error")}});
  let expected=if !observed{vec!["send","wait","image"]}else if send_fail{vec!["font-submit-begin","send","font-submit-return","free","error"]}else if wait_fail{vec!["font-submit-begin","send","font-submit-return","wait","font-wait-return","free","error"]}else{vec!["font-submit-begin","send","font-submit-return","wait","font-wait-return","image"]};
  assert_eq!(*EVENTS.lock().unwrap(),expected);
  assert!(batch.draws.is_empty()&&batch.slots.is_empty());assert_eq!(batch.first,7);
  assert_eq!(font.entries.len(),usize::from(!send_fail&&!wait_fail));assert_eq!(font.stats.entries,font.entries.len());
 }
}
'''.replace('@PHASE@',phase).replace('@FINISH@',item(source,'fn finish(&mut self, batch:')))
if __name__=='__main__':unittest.main()
