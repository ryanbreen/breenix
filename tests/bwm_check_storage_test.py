"""Scene admission must prepare capture storage before deferred validation runs."""
from pathlib import Path
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust
ROOT=Path(__file__).resolve().parents[1]
class CheckStorage(unittest.TestCase):
 def test_admission_reserves_mixed_client_sizes_without_steady_growth(self):
  source=(ROOT/'userspace/programs/src/bwm/compositor.rs').read_text()
  compose=item(source,'pub fn compose(')
  start=compose.index('self.sampled.clear();')
  finish=compose.index('return Ok(());',start)+len('return Ok(());')
  workspace=item(source,'struct CheckWorkspace {')
  prepare=item(source,'impl CheckWorkspace {') if 'impl CheckWorkspace {' in source else ''
  buffer=item(source,'fn client_buffer(') if 'fn client_buffer(' in source else ''
  run_rust(self,r'''
use std::sync::atomic::{AtomicUsize,Ordering};
static ALLOCATED:AtomicUsize=AtomicUsize::new(0);
fn prefaulted_bytes(n:usize)->Vec<u8>{ALLOCATED.fetch_add(1,Ordering::Relaxed);vec![0;n]}
@RESIZE@
#[derive(Clone,Copy)]struct Sampled{id:u64,sample:u64,commit_id:u64,width:u32,height:u32}
#[derive(Default)]@WORKSPACE@
@PREPARE@
@BUFFER@
struct Compositor{sampled:Vec<Sampled>,check_workspace:CheckWorkspace}
impl Compositor{fn admit(&mut self,windows:&[Sampled])->Result<(),()>{@ADMIT@}}
#[test]fn capture_capacity_is_ready_with_the_scene(){
 let windows:Vec<_>=(0..8).map(|n|Sampled{id:n+1,sample:1,commit_id:7,width:if n==3{640}else{240},height:if n==3{480}else{160}}).collect();
 let mut scratch=Vec::new();resize_readback(&mut scratch,1);assert_eq!(scratch.len(),1);ALLOCATED.store(0,Ordering::Relaxed);
 let mut c=Compositor{sampled:vec![],check_workspace:CheckWorkspace::default()};
 assert!(c.check_workspace.band.is_empty() && c.check_workspace.desktop.is_empty());assert_eq!(c.check_workspace.costs,[0;5]);
 c.admit(&windows).unwrap();
 assert_eq!(c.check_workspace.clients.len(),8,"first deferred check would allocate after steady-state warmup");
 assert_eq!(ALLOCATED.load(Ordering::Relaxed),8);
 for _ in 0..1000{c.admit(&windows).unwrap();}
 assert_eq!(ALLOCATED.load(Ordering::Relaxed),8,"unchanged scenes must retain the existing buffers");
 @CAPTURE@
}
'''.replace('@RESIZE@',item(source,'fn resize_readback(')).replace('@WORKSPACE@',workspace).replace('@PREPARE@',prepare).replace('@BUFFER@',buffer).replace('@ADMIT@',compose[start:finish]).replace('@CAPTURE@',r'''
 let allocated=ALLOCATED.load(Ordering::Relaxed);
 for (index,w) in windows.iter().enumerate(){let bytes=w.width as usize*w.height as usize*4;let slot=client_buffer(&mut c.check_workspace.clients,index,bytes);assert!(slot.3.capacity()>=bytes);}
 assert_eq!(ALLOCATED.load(Ordering::Relaxed),allocated,"capture order must not discard admission's size reservations");
''' if buffer else ''))
if __name__=='__main__':unittest.main()
