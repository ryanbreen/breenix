"""Window observations wait for latch and resolve resources from native head state."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class LatchedObservation(unittest.TestCase):
    def run_rust(self, source):
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory(prefix='gt710-latched-observation-') as directory:
            p = Path(directory)
            (p / 'check.rs').write_text(source)
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                           str(p / 'check.rs'), '-o', str(p / 'check')], check=True)
            subprocess.run([str(p / 'check'), '--nocapture'], check=True)

    def test_window_waits_for_latch_while_owner_keeps_g26d(self):
        display = (ROOT / 'kernel/src/gpu/display.rs').read_text()
        methods = '\n'.join(item(display, marker) for marker in [
            '    fn acquire() -> Result<Self, i64>',
            '    fn observe() -> Result<Self, i64>',
            'pub(super) fn for_window_observer()',
            'pub(super) fn for_observation(',
        ]).replace('pub(super)', 'pub(crate)')
        self.run_rust(r'''
#![allow(dead_code)] // Production predicates include virtual-backend alternatives.
mod abi {pub mod backend {pub const GK208:u32=6;}}
mod dev {pub fn backend()->u32{6}}
const EINTR:i64=4; const EPERM:i64=1;
mod signal {pub fn monotonic_nanos()->u64{100}}
mod gk208{pub mod timing {pub fn scanout_wait(_:bool,_:u64){}}}
fn with_core<T>(f:impl FnOnce(&mut Core)->T)->Option<T>{CORE.with(|c|Some(f(&mut c.borrow_mut())))}
struct Display {changing:bool,outstanding:u32,known:bool,accepted:u64,sent:u64}
struct Core {display:Display}
thread_local! {static CORE:std::cell::RefCell<Core>=const{std::cell::RefCell::new(Core{
 display:Display{changing:false,outstanding:1,known:true,accepted:2,sent:2}})};}
fn owner(group:u64)->bool{group==7}
fn wait_for<T>(_:Option<u64>,_:bool,f:impl FnOnce(&mut Core)->Option<T>)->Result<T,()>{
 CORE.with(|c|f(&mut c.borrow_mut()).ok_or(()))
}
struct Transition;
impl Transition {METHODS}
#[test] fn accepted_is_enough_for_owner_but_not_for_window(){
 assert!(Transition::for_window_observer().is_err(),"window accepted an unlatched binding");
 CORE.with(|c|assert!(!c.borrow().display.changing));
 assert!(Transition::for_observation(7).is_ok(),"G26(d) owner waited for latch");
 CORE.with(|c|{let mut c=c.borrow_mut();c.display.changing=false;c.display.outstanding=0;});
 assert!(Transition::for_window_observer().is_ok());
 CORE.with(|c|assert!(c.borrow().display.changing));
}
'''.replace('METHODS', methods))

    def test_head_state_selects_the_resource_and_refuses_unknown_or_mixed_banks(self):
        self.run_rust(r'''
#![allow(dead_code)] // Only the read-only surface resolver is under test.
extern crate alloc;
#[path="PRESENT"] mod present;
use present::{Surface,Io};
struct Head {active:[u32;5],armed:[u32;5],reads:Vec<usize>}
impl Io for Head {
 fn clear_notifier(&mut self)->Result<(),&'static str>{panic!("observation wrote notifier")}
 fn submit(&mut self,_:&[(u32,u32)])->Result<(),&'static str>{panic!("observation submitted UPDATE")}
 fn complete(&mut self)->Result<u32,&'static str>{panic!("observation consumed notifier")}
 fn read(&mut self,r:usize)->u32{
  self.reads.push(r);
  let methods=[0x460,0x468,0x46c,0x470,0x474];
  for(i,m)in methods.into_iter().enumerate(){
   if r==0x640000+m+0x300{return self.active[i];}
   if r==0x660000+m+0x300{return self.armed[i];}
  }
  panic!("unexpected head register {r:x}")
 }
}
fn surface(id:u32,address:u64)->Surface{Surface{resource:id,address,width:2560,height:1600,
 pitch:10240,block_height:16,format:0,dma:3}}
fn head(s:Surface)->Head{let m=s.methods(1).unwrap();let words=core::array::from_fn(|i|m[i].1);
 Head{active:words,armed:words,reads:vec![]}}
#[test]fn resource_comes_from_head_not_candidate_order(){
 let a=surface(71,0x1000000);let b=surface(72,0x2000000);
 let mut io=head(a);
 assert_eq!(present::observed_surface(&mut io,1,[b,a].into_iter()),Ok(a));
 assert_eq!(io.reads.len(),10,"read each bank once, not once per resource");
 let mut io=head(b);assert_eq!(present::observed_surface(&mut io,1,[a,b].into_iter()),Ok(b));
}
#[test]fn each_surface_field_and_both_banks_are_required(){
 let a=surface(71,0x1000000);let b=surface(72,0x2000000);
 for field in 0..5{for bank in 0..2{
  let mut io=head(a);if bank==0{io.active[field]^=1;}else{io.armed[field]^=1;}
  assert!(present::observed_surface(&mut io,1,[a,b].into_iter()).is_err());
 }}
 let mut io=head(b);assert!(present::observed_surface(&mut io,1,[a].into_iter()).is_err());
 let mut io=head(a);assert!(present::observed_surface(&mut io,4,[a].into_iter()).is_err());
 assert!(io.reads.is_empty(),"invalid head read MMIO");
}
'''.replace('PRESENT', str(ROOT / 'kernel/src/gpu/gk208/present.rs')))

    def test_native_counts_keep_pending_cost_and_confirmed_refused_results(self):
        self.run_rust(r'''extern crate alloc;
#[path="TIMING"] pub mod timing;
#[test]fn scanout_counts_are_retained_without_detailed_timing(){
 let mut r=timing::Recorder::new();
 assert!(!timing::collecting(false,false));
 r.scanout_wait(true,123);r.scanout_wait(false,999);r.scanout_wait(true,456);
 r.scanout_result(true);r.scanout_result(false);r.scanout_result(true);
 let text=r.drain(1).unwrap();
 assert!(text.contains(r#""scanout_fields":["confirmed","refused","pending","wait_ns","wait_max_ns"]"#));
 assert!(text.contains(r#""scanout_counts":[2,1,2,579,456]"#),"{text}");
 assert!(r.drain(2).is_none(),"counts were repeated in the next batch");
 r.scanout_result(false);assert!(r.drain(3).unwrap().contains(r#""scanout_counts":[0,1,0,0,0]"#));
}
'''.replace('TIMING', str(ROOT / 'kernel/src/gpu/gk208/timing.rs')))

    def test_scanout_target_mismatch_is_eio_before_record_or_pixels(self):
        core = (ROOT / 'kernel/src/gpu/mod.rs').read_text()
        source = item(core, 'fn read_back(')
        begin = source.index('    let (id, fmt, tw, th, pitch, backing) =')
        end = source.index('    // Privileged window observation', begin)
        select = source[begin:end]
        self.run_rust(r'''
#![allow(dead_code)] // The real selection returns format, geometry and backing too.
use std::collections::BTreeMap;
const EPERM:i64=1;const EIO:i64=5;const EINVAL:i64=22;const ENODEV:i64=19;
mod abi{pub mod errno{pub const EPERM:i64=1;}pub mod backend{pub const GK208:u32=6;}}
mod readback_reason{pub const BINDING_UNKNOWN:u32=10;}
mod dev{pub fn backend()->u32{6}}
mod kind{pub const TARGET:u32=1;pub const TEXTURE:u32=2;}
#[derive(Debug,PartialEq)]enum Refused{Errno(i64),Eio(u32,u64)}
impl From<i64> for Refused{fn from(e:i64)->Self{Self::Errno(e)}}
struct Arg{target:u64}#[derive(Clone,Copy)]struct Binding{id:u32}struct Display{binding:Option<Binding>}
struct Resource{kind:u32,format:u32,width:u32,height:u32,pitch:u32,backing:()}
struct Core{display:Display,resources:BTreeMap<u32,Resource>}
impl Core{fn res_of(&self,_:u64,target:u64)->Option<u32>{Some(target as u32)}}
mod display{pub fn owner(_:u64)->bool{true}}
thread_local!{static HARDWARE:std::cell::Cell<Result<u32,i64>>=const{std::cell::Cell::new(Ok(71))};static QUERIES:std::cell::Cell<u32>=const{std::cell::Cell::new(0)};}
mod gk208{pub fn observed_scanout()->Result<u32,&'static str>{super::QUERIES.with(|q|q.set(q.get()+1));super::HARDWARE.with(|h|h.get().map_err(|_|"unknown head"))}}
fn with_core<T>(f:impl FnOnce(&mut Core)->T)->Option<T>{let mut c=Core{
 display:Display{binding:Some(Binding{id:72})},resources:[71,72].into_iter().map(|id|(id,
 Resource{kind:1,format:0,width:2560,height:1600,pitch:10240,backing:()})).collect()};Some(f(&mut c))}
fn select(window:bool,target:u64)->Result<u32,Refused>{let tgid=7;let a=Arg{target};
 SELECT
 let _=(fmt,tw,th,pitch,backing);Ok(id)
}
#[test]fn mismatch_refuses_and_owner_does_not_query_hardware(){
 assert_eq!(select(true,0),Err(Refused::Eio(10,0)));
 HARDWARE.with(|h|h.set(Err(EIO)));assert_eq!(select(true,0),Err(Refused::Eio(10,0)));
 HARDWARE.with(|h|h.set(Ok(72)));assert_eq!(select(true,0),Ok(72));
 QUERIES.with(|q|q.set(0));HARDWARE.with(|h|h.set(Ok(71)));
 assert_eq!(select(false,0),Ok(72),"owner target 0 must name latest accepted binding");
 assert_eq!(select(true,71),Ok(71));QUERIES.with(|q|assert_eq!(q.get(),0));
}
'''.replace('SELECT', select))


if __name__ == '__main__':
    unittest.main()
