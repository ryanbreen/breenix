"""Execute the shared display-hold retirement after a native viewing copy."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'tests'))
from gt710_gr_live_unwind_test import item
class PresentRetirement(unittest.TestCase):
    def test_completed_native_repaint_retires_the_logical_client_binding(self):
        source=(ROOT/'kernel/src/gpu/console.rs').read_text()
        # Extract the actual completed-frame publication, not a surrogate call:
        # at the old head it announces the VT frame but leaves the GPU view held.
        render=(ROOT/'kernel/src/console/mod.rs').read_text()
        completed=item(render,'            if screen.complete {\n                vt::drawn(')
        completed=completed.replace('#[cfg(target_arch = "x86_64")]','')
        helper=(item(source,'pub(crate) fn native_repainted(')
                if 'pub(crate) fn native_repainted(' in source else '')
        describe=item(source,'pub(super) fn describe(').replace('pub(super)','pub(crate)')
        describe=describe.replace('#[cfg(target_arch = "x86_64")]','')
        method=item((ROOT/'kernel/src/gpu/display.rs').read_text(),
                    'fn native_console_restored(')
        modes=item((ROOT/'kernel/src/gpu/display.rs').read_text(),
                   'pub(super) fn modes(').replace('pub(super)', 'pub(crate)')
        modes=modes.replace('#[cfg(target_arch = "x86_64")]','')
        harness=r'''
#![allow(dead_code)] // Extracted console diagnostics have controlled boundaries.
use std::{cell::RefCell,collections::BTreeSet,sync::atomic::{AtomicBool,AtomicU64,Ordering}};
use gpu_abi as abi;use gpu_abi::Plain;
#[derive(Clone,Copy,Debug,PartialEq)]
struct Binding {id:u32}
struct Caps {max_width:u32,max_height:u32}
struct Display {mode:u32,owner_sequence:u64,holds:BTreeSet<u32>,binding:Option<Binding>,outstanding:u32,known:bool,latest_ok:bool,stack:Vec<u64>,changing:bool}
struct Core {caps:Caps,display:Display,released:Vec<u32>}
impl Core {fn unhold(&mut self,id:u32){self.released.push(id);} // METHOD
}
thread_local! {static CORE:RefCell<Core>=RefCell::new(Core{caps:Caps{max_width:2560,max_height:1600},display:Display{mode:0,owner_sequence:0,holds:[731].into_iter().collect(),binding:Some(Binding{id:731}),outstanding:0,known:true,latest_ok:true,stack:vec![],changing:false},released:vec![]});}
fn with_core<R>(f:impl FnOnce(&mut Core)->R)->Option<R>{Some(CORE.with(|c|f(&mut c.borrow_mut())))}
fn notify(){NOTIFIED.fetch_add(1,Ordering::Relaxed);}
static NOTIFIED:AtomicU64=AtomicU64::new(0);
static FIXTURE:AtomicU64=AtomicU64::new(0);static HASH:AtomicU64=AtomicU64::new(0);
static HANDOFF:AtomicBool=AtomicBool::new(false);static BOUND:AtomicBool=AtomicBool::new(true);static SCREEN_ACTIVE:AtomicBool=AtomicBool::new(false);
fn active()->bool{false}fn caller()->Result<u64,()>{Ok(1)}fn firmware_hash()->u64{0}
mod display {pub fn owner(_:u64)->bool{false}pub fn active()->bool{super::SCREEN_ACTIVE.load(super::Ordering::Acquire)}}
mod arch {pub fn firmware_writes()->u64{0}pub fn with_interrupts<R>(f:impl FnOnce()->R)->R{f()}}
mod screen {
 pub fn describe()->(u64,u64){(732,0)}
 pub fn await_binding()->Result<(),i64>{super::with_core(|c|{
  if c.display.binding.is_none(){c.display.binding=Some(super::Binding{id:732});super::SCREEN_ACTIVE.store(true,super::Ordering::Release);}
 });Ok(())}
}
mod gk208 {pub fn console_state()->Option<(gpu_abi::Mode,u64,bool,u64)>{Some((gpu_abi::Mode{width:1280,height:1024,scale:1,reserved:0},0xffff_ffff,super::BOUND.load(super::Ordering::Acquire),0))}}
mod gpu {use super::screen;pub mod console {use super::super::*; // HELPER
// DESCRIBE
pub fn restoring(_:u64)->bool{false}
}}
mod vt {pub fn drawn(_:u64){}}
struct Frame {complete:bool,epoch:u64}
fn repaint(complete:bool){let screen=Frame{complete,epoch:1};let snapshot=&screen;
// COMPLETED
}
const EINVAL:i64=-22;const ENODEV:i64=-19;const EINTR:i64=-4;
const MODES:[abi::Mode;2]=[abi::Mode{width:1280,height:800,scale:1,reserved:0},abi::Mode{width:2560,height:1600,scale:2,reserved:0}];
fn now_ns()->u64{0}
enum Stop{Interrupted}
fn wait_for(_:Option<u64>,_:bool,f:impl FnOnce(&mut Core)->Option<()>)->Result<(),Stop>{assert_eq!(with_core(f),Some(Some(())));Ok(())}
fn read_arg(_:u64,_:u64)->Result<abi::Modes,i64>{Ok(abi::Modes::ZERO)}
thread_local!{static ANSWER:RefCell<abi::Modes>=RefCell::new(abi::Modes::ZERO);}
fn write_arg(_:u64,a:&abi::Modes)->Result<(),i64>{ANSWER.with(|r|*r.borrow_mut()=*a);Ok(())}
mod dev{pub fn resource_id(id:u32)->Option<u32>{Some(id)}}
use gpu::console;
mod observed {use super::*;
 // MODES
}
fn modes()->abi::Modes{assert_eq!(observed::modes(0,256),Ok(0));ANSWER.with(|r|*r.borrow())}

fn held(){with_core(|c|{assert_eq!(c.display.binding,Some(Binding{id:731}));assert_eq!(c.display.holds.len(),1);assert!(c.released.is_empty());});assert_eq!(NOTIFIED.load(Ordering::Acquire),0);}
fn main(){
 // A healthy native allocation does not mean its copied client image has
 // returned to the console; polling MODES must preserve that distinction.
 let before=modes();assert_ne!(before.scanout_resource,before.console_resource);held();
 repaint(false);held(); // incomplete console frame
 BOUND.store(false,Ordering::Release);repaint(true);held(); // failed hardware binding
 BOUND.store(true,Ordering::Release);
 with_core(|c|c.display.outstanding=1);repaint(true);held(); // live GPU access
 with_core(|c|{c.display.outstanding=0;c.display.stack.push(1);});repaint(true);held();
 with_core(|c|{c.display.stack.clear();c.display.changing=true;});repaint(true);held();
 with_core(|c|c.display.changing=false);
 repaint(true);
 with_core(|c|{assert_eq!(c.display.binding,None,"completed native console repaint left the client scanout binding live");assert!(c.display.holds.is_empty());assert_eq!(c.released,[731]);});
 let after=modes();assert_eq!(after.owner_depth,0);assert_eq!(after.binding_known,1);
 assert_ne!(after.console_resource,0);assert_eq!(after.scanout_resource,after.console_resource);
 assert_eq!(NOTIFIED.load(Ordering::Acquire),1);
 repaint(true);assert_eq!(NOTIFIED.load(Ordering::Acquire),1); // no duplicate retirement
}
'''.replace('// MODES',modes).replace('// METHOD',method).replace('// HELPER',helper).replace('// DESCRIBE',describe).replace('// COMPLETED',completed)
        priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp);(p/'repaint.rs').write_text(harness)
            subprocess.run(priority+['rustc','--edition=2021','--crate-name','gpu_abi','--crate-type','rlib',str(ROOT/'libs/gpu-abi/src/lib.rs'),'-o',str(p/'libgpu_abi.rlib')],check=True)
            subprocess.run(priority+['rustc','--edition=2021','-Dwarnings','--extern',f'gpu_abi={p / "libgpu_abi.rlib"}',str(p/'repaint.rs'),'-o',str(p/'repaint')],check=True)
            subprocess.run([str(p/'repaint')],check=True)

    def test_return_to_console_releases_copy_sources_but_preserves_pending_count(self):
        source=(ROOT/'kernel/src/gpu/display.rs').read_text()
        method=item(source,'fn native_console_restored(')
        restore=item(source,'pub(super) fn restore()')
        self.assertIn('native_console_restored()',restore)
        harness=r'''
use std::collections::BTreeSet;
struct Display {holds:BTreeSet<u32>,binding:Option<u32>,outstanding:u32,known:bool,latest_ok:bool}
struct Core {display:Display,released:Vec<u32>}
impl Core {
 fn unhold(&mut self,id:u32){self.released.push(id);}
 // METHOD
}
fn main(){
 for outstanding in [0,2] {
  let mut c=Core{display:Display{holds:[7,9].into_iter().collect(),binding:Some(9),outstanding,known:false,latest_ok:false},released:vec![]};
  c.native_console_restored();
  assert_eq!(c.released,[7,9]);assert!(c.display.holds.is_empty());assert_eq!(c.display.binding,None);
  assert_eq!(c.display.outstanding,outstanding);assert_eq!(c.display.known,outstanding==0);assert!(c.display.latest_ok);
 }
}
'''.replace('// METHOD',method)
        priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp);(p/'retire.rs').write_text(harness)
            subprocess.run(priority+['rustc','--edition=2021','-Dwarnings',str(p/'retire.rs'),'-o',str(p/'retire')],check=True)
            subprocess.run([str(p/'retire')],check=True)
if __name__=='__main__':unittest.main()
