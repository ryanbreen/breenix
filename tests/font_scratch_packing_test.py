"""Drive actual LCD scratch allocation with cold-run rectangles and ownership changes."""
from pathlib import Path
import os
import subprocess
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust
ROOT=Path(__file__).resolve().parents[1]
class ScratchPacking(unittest.TestCase):
    def test_disjoint_lcd_regions_share_targets_and_respect_context_owner(self):
        rel='libs/libgfx/src/ttf_font.rs'
        old=os.environ.get('UITEST_OLD_COMMIT')
        source=subprocess.check_output(['git','show',f'{old}:{rel}'],cwd=ROOT,text=True) if old else (ROOT/rel).read_text()
        run_rust(self,r'''
use std::{cell::UnsafeCell,sync::{Arc,atomic::{AtomicU64,Ordering}}};
enum Error{Device(i64)}
impl std::fmt::Debug for Error{fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{let Self::Device(e)=self;write!(f,"device {e}")}}
static COUNT:AtomicU64=AtomicU64::new(0);
static CONTEXT:AtomicU64=AtomicU64::new(1);
static OWNER:AtomicU64=AtomicU64::new(1);
struct Surface{id:u64,w:usize,h:usize,owner:u64}
impl Surface{fn owned_here(&self)->bool{self.owner==OWNER.load(Ordering::Relaxed)}fn width(&self)->usize{self.w}fn height(&self)->usize{self.h}}
fn target(w:usize,h:usize)->Result<Surface,Error>{Ok(Surface{id:COUNT.fetch_add(1,Ordering::Relaxed)+1,w,h,owner:OWNER.load(Ordering::Relaxed)})}
struct Context;impl Context{fn handle(&self)->u64{CONTEXT.load(Ordering::Relaxed)}}
struct Device{context:Context}
mod device{pub fn with<T>(f:impl FnOnce(&mut super::Device)->Result<T,i64>)->Result<T,i64>{f(&mut super::Device{context:super::Context})}}
fn scratch_lock()->Result<(),Error>{Ok(())}
struct Slots{targets:UnsafeCell<[Option<Scratch>;2]>}
// The fixture runs the single allocation sequence on one thread.
unsafe impl Sync for Slots{}
static SCRATCH:Slots=Slots{targets:UnsafeCell::new([None,None])};
const SCRATCH_SIDE:usize=256;
@SCRATCH@
struct Region{target:Arc<Surface>,x:usize,y:usize}
@REGION@
#[test]fn cold_lcd_and_rollover(){
 let mut held:Vec<(Region,usize,usize)>=Vec::new();
 // Five scratch allocations per LCD glyph, in the production caller's order.
 for _ in 0..20 {for(which,w,h)in[(0,60,28),(0,20,28),(1,62,28),(1,20,28),(1,20,28)]{
  let r=region(which,w,h).unwrap();
  assert!(r.x+w<=r.target.width()&&r.y+h<=r.target.height());
  for(other,ow,oh)in &held{if other.target.id==r.target.id{assert!(r.x+w<=other.x||other.x+ow<=r.x||r.y+h<=other.y||other.y+oh<=r.y,"scratch coverage or its padding was reused");}}
  held.push((r,w,h));
 }}
 assert!(COUNT.load(Ordering::Relaxed)<=3,"cold LCD run replaced {} targets because small glyphs consumed whole rows",COUNT.load(Ordering::Relaxed));
 let wide=region(0,400,17).unwrap();assert_eq!(wide.target.width(),400);assert!(wide.x+400<=wide.target.width());
 let tall=region(1,13,400).unwrap();assert_eq!(tall.target.height(),400);assert!(tall.y+400<=tall.target.height());
 CONTEXT.store(2,Ordering::Relaxed);
 let fresh=region(0,20,28).unwrap();assert_ne!(fresh.target.id,wide.target.id);
 OWNER.store(2,Ordering::Relaxed);
 let child=region(0,20,28).unwrap();assert_ne!(child.target.id,fresh.target.id);assert_eq!(child.target.owner,2);
}
'''.replace('@SCRATCH@',item(source,'struct Scratch {')).replace('@REGION@',item(source,'fn region(')))
if __name__=='__main__':unittest.main()
