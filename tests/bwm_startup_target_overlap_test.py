"""Force target allocation to finish while the replacement claim is pending."""
from pathlib import Path
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust

ROOT = Path(__file__).resolve().parents[1]


class TargetOverlap(unittest.TestCase):
    def test_targets_precede_claim_join_and_reserved_context_transfer(self):
        source = (ROOT / 'userspace/programs/src/bwm/compositor.rs').read_text()
        methods = [item(source, '    pub fn open('),
                   item(source, '    fn make_targets('),
                   item(source, '    fn destroy_targets(')]
        for name in ['    pub fn open_during<', '    fn allocate(', '    fn clear_targets(']:
            if name in source:
                methods.append(item(source, name))
        scheduled = 'open_during' in source
        call = ('let (opened, epoch) = Compositor::open_during(2560,1600,72,2, claim);'
                if scheduled else
                'let epoch=claim();let opened=Compositor::open(2560,1600,72,2);')
        run_rust(self, r'''
use std::sync::atomic::{AtomicUsize,Ordering};
static TARGETS:AtomicUsize=AtomicUsize::new(0);
static CONTEXTS:AtomicUsize=AtomicUsize::new(0);
static CLEARS:AtomicUsize=AtomicUsize::new(0);
static DROPPED:AtomicUsize=AtomicUsize::new(0);
mod render_requirements{pub const SCENE_PIPELINES:u32=3;}
mod abi{pub mod format{pub const B8G8R8A8:u32=1;}}
mod gpu{
 pub fn context_create_for(pipelines:u32)->Result<u64,i64>{assert_eq!(pipelines,3);super::CONTEXTS.fetch_add(1,super::Ordering::SeqCst);Ok(7)}
 pub fn context_destroy(_:u64){}
 pub fn resource_destroy(_:u64){super::DROPPED.fetch_add(1,super::Ordering::SeqCst);}
 pub fn scanout_target_create(_:u32,_:u32,_:u32)->Result<u64,i64>{Ok(10+super::TARGETS.fetch_add(1,super::Ordering::SeqCst) as u64)}
 pub fn target_create(f:u32,w:u32,h:u32)->Result<u64,i64>{scanout_target_create(f,w,h)}
 pub struct Submit{pub fence:u64}pub struct Wait{pub status:i64}
 pub fn submit(ctx:u64,words:&[u32])->(i64,Submit){assert_eq!(ctx,7);assert_eq!(words.len(),6);super::CLEARS.fetch_add(1,super::Ordering::SeqCst);(0,Submit{fence:1})}
 pub fn wait(_:u64,_:u64,_:u64)->(i64,Wait){(0,Wait{status:0})}
}
enum Format{BGRX}struct Surface;
impl Surface{fn new(_:usize,_:usize,_:Format)->Result<Self,i64>{TARGETS.fetch_add(1,Ordering::SeqCst);Ok(Self)}}
mod encode{pub fn begin(w:&mut Vec<u32>,t:u64){w.push(t as u32);}pub fn clear(w:&mut Vec<u32>,_:u32){w.push(0);}}
mod libuitest{pub mod startup{pub fn mark(_:&str){}}}
mod pace{#[derive(Default)]pub struct Timing;}
#[derive(Default)]struct CheckWorkspace;
const BLACK:u32=0;
fn wait_for_admission()->Result<(),String>{Ok(())}
struct Compositor{context:u64,width:u32,height:u32,scene:u64,scanout:[u64;2],last_serial:u64,last_timing:pace::Timing,next:usize,desktop:Surface,background:Surface,sampled:Vec<()>,check_first:u64,check_workspace:CheckWorkspace,appbar_top:i32,scale:i32,full:bool}
impl Compositor{@METHODS@}
@DROP@
#[test]fn claim_can_remain_pending_until_all_targets_exist(){
 let claim=||{assert_eq!(TARGETS.load(Ordering::SeqCst),5,"replacement still waits for the claim before allocating its five targets");assert_eq!(CONTEXTS.load(Ordering::SeqCst),0,"reserved context transferred before claim completed");assert_eq!(CLEARS.load(Ordering::SeqCst),0,"drawing began before claim completed");19};
 @CALL@
 assert_eq!(epoch,19);let gpu=opened.unwrap();
 assert_eq!(CONTEXTS.load(Ordering::SeqCst),1);assert_eq!(CLEARS.load(Ordering::SeqCst),1);
 assert_eq!((gpu.context,gpu.width,gpu.height,gpu.appbar_top,gpu.scale),(7,2560,1600,1528,2));
 assert_eq!((gpu.last_serial,gpu.next,gpu.check_first,gpu.full),(0,0,0,true));
 let _=(&gpu.desktop,&gpu.background,&gpu.last_timing,&gpu.sampled,&gpu.check_workspace);
 drop(gpu);assert_eq!(DROPPED.load(Ordering::SeqCst),3);
 // Recovery uses the same allocation/clear path without a claim wait.
 drop(Compositor::open(2560,1600,72,2).unwrap());
 assert_eq!(CLEARS.load(Ordering::SeqCst),2);
}
'''.replace('@METHODS@', '\n'.join(methods).replace('super::pace::', 'crate::pace::').replace('super::render_requirements::', 'crate::render_requirements::')).replace(
            '@DROP@', item(source, 'impl Drop for Compositor')).replace('@CALL@', call))
        main = (ROOT / 'userspace/programs/src/bwm.rs').read_text()
        start = main.index('let claim = scope.spawn(session::claim)')
        allocate = main.index('Compositor::open_during(', start)
        joined = main.index('claim.join()', allocate)
        self.assertLess(start, allocate)
        self.assertLess(allocate, joined)


if __name__ == '__main__':
    unittest.main()
