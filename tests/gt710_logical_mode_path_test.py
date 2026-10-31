"""Execute the native EVO mode transaction and high-console aperture setup."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'tests'))
from gt710_gr_live_unwind_test import item

class LogicalModePath(unittest.TestCase):
    def test_viewport_is_in_the_notified_update_and_high_console_avoids_userd(self):
        native=(ROOT/'kernel/src/gpu/gk208/native.rs').read_text()
        present=item(native,'pub(in crate::gk208) fn present(').replace('pub(in crate::gk208)','').replace('super::present::','present::').replace('super::display_evidence::','display_evidence::').replace('super::cost::','cost::')
        console=item(native,'pub(super) fn logical_console(').replace('pub(super)','').replace('crate::memory::map_mmio','host_map').replace('crate::console::display::linear::map_native_write_combining','host_wc').replace('super::present::','present::').replace('super::vm::TABLE_BYTES','vm::TABLE_BYTES').replace('super::display_evidence::','display_evidence::')
        binding=item(native,'pub fn console_binding(').replace('pub fn','fn').replace('surface: &ConsoleSurface','surface: &ConsoleSurface, regs: &mut Registers').replace('    let mut regs = Registers(surface.regs);','').replace('view.verify(&mut regs,','view.verify(regs,')
        fixture=r'''
#![allow(dead_code)] // Production modules include hardware APIs outside this fixture.
extern crate alloc;
#[macro_export] macro_rules! serial_println {($($arg:tt)*)=>{let _=format!($($arg)*);};}
use std::collections::BTreeMap;
#[path="@ROOT@/kernel/src/gpu/gk208/edid.rs"] mod edid;
#[path="@ROOT@/kernel/src/gpu/gk208/modeset.rs"] mod modeset;
#[path="@ROOT@/kernel/src/gpu/gk208/memory.rs"] mod memory;
#[path="@ROOT@/kernel/src/gpu/gk208/present.rs"] mod present;
#[path="@ROOT@/kernel/src/gpu/gk208/vm.rs"] mod vm;
#[derive(Default)] struct Registers {r:BTreeMap<usize,u32>,vram:BTreeMap<u64,u32>,staged:Vec<(u32,u32)>,stale:bool,updates:usize}
impl Registers {
 fn read(&self,r:usize)->u32 {if (0x700000..0x800000).contains(&r) {
  *self.vram.get(&((u64::from(*self.r.get(&0x1700).unwrap_or(&0))<<16)+(r-0x700000) as u64)).unwrap_or(&0)
 } else {*self.r.get(&r).unwrap_or(&0)}}
 fn write(&mut self,r:usize,v:u32) {if (0x700000..0x800000).contains(&r) {
  self.vram.insert((u64::from(self.read(0x1700))<<16)+(r-0x700000) as u64,v);
 } else {self.r.insert(r,v);}}
}
impl modeset::Io for Registers {
 fn read(&mut self,r:usize)->u32 {Registers::read(self,r)}
 fn write(&mut self,r:usize,v:u32){Registers::write(self,r,v)}
 fn delay_us(&mut self,_:u32){}
}
impl memory::Access for Registers {
 fn read(&mut self,r:usize)->u32 {Registers::read(self,r)}
 fn write(&mut self,r:usize,v:u32){Registers::write(self,r,v)}
}
mod cost {
 pub enum Slot { Evo }
 pub fn phase(_:Slot) {}
}
// Retained same-mode updates use a separate capabilities acknowledgement.
fn acknowledge(_: &mut Evo, _: &mut Registers)->Result<u32,&'static str>{Ok(1)}
mod display_evidence {
 pub fn record(_: &mut super::Registers,_:u32,_:super::present::Surface,_:&super::present::Receipt){}
 pub fn transition(_: &mut super::Registers,_:u32,s:super::present::Surface,r:&super::present::Receipt,
  allocation:[u64;2],console:[u64;2],event:&'static str)->Result<u64,&'static str>{
  assert_eq!(event,"handoff");assert_eq!(allocation,console);assert_eq!(allocation,[s.address,16384000]);
  assert_eq!(r.active,r.armed);Ok(1)
 }
}
struct Evo {head:u32,sor:u32,total:u64,push:u64,put:u32,notifier:u64,limits:(),coefficients:(),scripts:[Vec<modeset::Op>;4]}
#[derive(Clone,Copy)] struct ConsoleSurface {pointer:usize,bytes:usize,width:usize,height:usize,pitch:usize,vram:u64,phys:u64,regs:usize,head:u32,firmware_baseline:u64}
fn host_map(_:u64,n:usize)->Result<usize,&'static str>{assert_eq!(n,32<<20);Ok(Box::leak(vec![0u32;n/4].into_boxed_slice()).as_mut_ptr() as usize)}
fn host_wc(_:u64,n:u64)->Result<(),&'static str>{assert!(n>=32<<20);Ok(())}
fn write_words(r:&mut Registers,_:u64,a:u64,words:&[u32])->Result<(),&'static str>{for (i,&w) in words.iter().enumerate(){r.vram.insert(a+i as u64*4,w);}Ok(())}
fn read_word(r:&mut Registers,_:u64,a:u64)->Result<u32,&'static str>{Ok(*r.vram.get(&a).unwrap_or(&0))}
fn submit(r:&mut Registers,_:u64,_:u64,put:&mut u32,m:&[(u32,u32)])->Result<(),&'static str>{
 assert_eq!(&m[m.len()-2..],&[(0x84,0x80000000),(0x80,0)]);
 assert!(m.iter().any(|&(a,_)|a==0x4b8));assert!(m.iter().any(|&(a,_)|a==0x4c0));
 r.staged=m.to_vec();*put+=m.len() as u32*8;r.r.insert(0x640004,*put);Ok(())
}
fn complete(r:&mut Registers,_:u64,_:u64,_:&(),_:&(),_:(u32,u32,bool,bool),_:&[Vec<modeset::Op>;4])->Result<u32,&'static str>{
 r.updates+=1;for &(m,v) in &r.staged {r.r.insert(0x640000+m as usize,v);if !(r.stale && m==0x4b8) {r.r.insert(0x660000+m as usize,v);}} Ok(1)
}
// PRESENT
// CONSOLE
// BINDING
fn hardware()->(Registers,Evo){
 let mut r=Registers::default();for (a,v) in [(0x64041c,359|40<<16),(0x640420,1639|1064<<16),(0x616308,1),(0x61c004,1),(0x100c80,0x10000|0x8000)]{r.r.insert(a,v);}
 (r,Evo{head:0,sor:0,total:1<<30,push:0x10000,put:0,notifier:0x11000,limits:(),coefficients:(),scripts:std::array::from_fn(|_|vec![])})
}
fn main(){
 for (w,h) in [(1280,800),(2560,1600),(1024,768),(640,480)] {
  let (mut r,mut e)=hardware();let s=present::Surface{resource:1,address:0x1000000,width:w,height:h,pitch:w*4,block_height:0,format:0,dma:1};
  present(&mut e,&mut r,s).unwrap();assert_eq!(r.updates,1);
  assert_eq!(r.read(0x6604b8),w|h<<16);
  let out=if w==1280||w==2560 {1280|800<<16}else{w|h<<16};
  assert_eq!(r.read(0x6604c0),out);
  assert_eq!(r.read(0x640420),1639|1064<<16); // raster untouched
  let (mut r,mut e)=hardware();r.stale=true;assert!(present(&mut e,&mut r,s).is_err());
 }
 let (mut r,mut e)=hardware();let mut layout=memory::Layout::new(1<<30,&[],1280,1024).unwrap();
 let old=ConsoleSurface{pointer:0x10010000,phys:0x20010000,bytes:1280*1024*4,width:1280,height:1024,pitch:5120,vram:layout.objects[0].range.start,regs:0,head:0,firmware_baseline:9};
 let instance=layout.objects[4].range.start;
 let slot=instance+0x4000+(((8<<20)-4096)/4096)*8;r.vram.insert(slot,0x1234);
 let high=logical_console(&mut r,&mut layout,&mut e,old,0x10000000,128<<20).unwrap();
 assert_eq!((high.width,high.height,high.pitch,high.bytes),(2560,1600,10240,16384000));
 let low=layout.objects.iter().find(|o|o.name=="logical-low").unwrap().range;
 assert_eq!(low.size,1280*800*4);assert_ne!(low.start,layout.objects[1].range.start);
 assert_eq!(high.phys,0x21000000);assert_eq!(high.firmware_baseline,9);
 console_binding(&high,&mut r).unwrap();
 r.r.insert(0x6604b8,1280|800<<16);
 assert!(console_binding(&high,&mut r).is_err(),"console accepted low armed mode");
 r.r.insert(0x6604b8,2560|1600<<16);
 assert_eq!(r.vram[&slot],0x1234,"high console overwrote USERD PTE");
 assert_eq!(r.vram[&(instance+0x2208)],(32<<20)-1);
 // G2: vmmgk104.c's 128 MiB PDE coverage, also vm.rs COVERAGE=1<<27.
 let table=layout.objects.iter().find(|o|o.name=="bar1-table").unwrap().range;
 assert_eq!(table.size,vm::TABLE_BYTES);
 assert_eq!(r.vram[&(instance+0x3004)],(table.start>>8) as u32|1);
 assert_eq!(r.vram[&(table.start+(((8<<20)-4096)/4096)*8)],0x1234);
 for i in [0,2048,3999] {let at=table.start+((16<<20)/4096+i)*8;
  assert_eq!(r.vram[&at],((high.vram+i*4096)>>8) as u32|1);
 }
 assert_eq!(r.vram[&(table.start+((32<<20)/4096-1)*8)],0);
 unsafe {assert_eq!(*(high.pointer as *const u32),0xff000000);assert_eq!(*((high.pointer+high.bytes-4) as *const u32),0xff000000);}
 assert!(logical_console(&mut r,&mut layout,&mut e,old,0x10000000,16<<20).is_err());
}
'''.replace('@ROOT@',str(ROOT)).replace('// PRESENT',present).replace('// CONSOLE',console).replace('// BINDING',binding)
        priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp);(p/'modes.rs').write_text(fixture)
            subprocess.run(priority+['rustc','--edition=2021','-Dwarnings',str(p/'modes.rs'),'-o',str(p/'modes')],check=True)
            subprocess.run([str(p/'modes')],check=True)
    def test_mode_hook_publishes_only_confirmed_switches(self):
        source=(ROOT/'kernel/src/gpu/display.rs').read_text()
        function=item(source,'pub(super) fn set_mode(').replace('pub(super)','').replace('#[cfg(target_arch = "x86_64")]','').replace('super::arch::','arch::')
        fixture=r'''
use std::{cell::RefCell,sync::atomic::{AtomicBool,AtomicU32,Ordering}};
const EINVAL:i64=-22;const EPERM:i64=-1;const ENODEV:i64=-19;const EIO:i64=-5;
mod abi {pub struct SetMode {pub index:u32,pub reserved:[u32;3]}}
static INPUT:AtomicU32=AtomicU32::new(0);static FAIL:AtomicBool=AtomicBool::new(false);
static CALLED:AtomicU32=AtomicU32::new(0);static ENABLED:AtomicBool=AtomicBool::new(false);
struct Owner{mode:u32} struct Display{mode:u32,stack:Vec<Owner>,known:bool}
struct Core{display:Display,retired:u32}
impl Core {fn native_console_restored(&mut self){self.retired+=1;self.display.known=true;}}
thread_local!{static CORE:RefCell<Core>=RefCell::new(Core{display:Display{mode:1,stack:vec![Owner{mode:1}],known:true},retired:0});}
fn with_core<R>(f:impl FnOnce(&mut Core)->R)->Option<R>{Some(CORE.with(|c|f(&mut c.borrow_mut())))}
fn caller()->Result<u64,i64>{Ok(7)}fn owner(_:u64)->bool{true}
fn read_arg(_:u64,_:u64)->Result<abi::SetMode,i64>{Ok(abi::SetMode{index:INPUT.load(Ordering::Acquire),reserved:[0;3]})}
struct Transition;
impl Transition{fn for_observation(_:u64)->Result<Self,i64>{Ok(Self)}}
mod arch {pub fn with_interrupts<R>(f:impl FnOnce()->R)->R{super::ENABLED.store(true,super::Ordering::Release);f()}}
mod gk208 {pub fn set_mode(index:u32)->Result<(), &'static str>{
 assert!(super::ENABLED.load(super::Ordering::Acquire));
 super::with_core(|c|{assert_eq!(c.retired,0);assert_eq!(c.display.mode,1);});
 super::CALLED.store(index+1,super::Ordering::Release);
 if super::FAIL.load(super::Ordering::Acquire){Err("uncertain latch")}else{Ok(())}
}}
// FUNCTION
fn main(){
 INPUT.store(2,Ordering::Release);assert_eq!(set_mode(0,16),Err(EINVAL));assert_eq!(CALLED.load(Ordering::Acquire),0);
 INPUT.store(0,Ordering::Release);FAIL.store(true,Ordering::Release);
 assert_eq!(set_mode(0,16),Err(EIO));
 with_core(|c|{assert_eq!(c.display.mode,1);assert_eq!(c.display.stack[0].mode,1);assert_eq!(c.retired,0);assert!(!c.display.known,"uncertain native latch left old binding known");});
 FAIL.store(false,Ordering::Release);assert_eq!(set_mode(0,16),Ok(0));
 with_core(|c|{assert_eq!(c.display.mode,0);assert_eq!(c.display.stack[0].mode,0);assert_eq!(c.retired,1);assert!(c.display.known);});
}
'''.replace('// FUNCTION',function)
        priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp);(p/'hook.rs').write_text(fixture)
            subprocess.run(priority+['rustc','--edition=2021','-Dwarnings',str(p/'hook.rs'),'-o',str(p/'hook')],check=True)
            subprocess.run([str(p/'hook')],check=True)
if __name__=='__main__':unittest.main()
