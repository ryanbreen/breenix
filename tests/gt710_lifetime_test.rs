//! CPU ownership checks. Model completions/reset receipts do not execute hardware.
#![allow(dead_code)] // Import complete production modules into this standalone harness.
extern crate alloc;
#[path = "../kernel/src/gpu/gk208/memory.rs"] mod memory;
#[path = "../kernel/src/gpu/gk208/vm.rs"] mod vm;
#[path = "../kernel/src/gpu/gk208/lifetime.rs"] mod lifetime;
use memory::allocator::{Allocator,Kind};
use lifetime::{Context,Consumer};
use std::collections::BTreeMap;
#[derive(Default)]
struct Model { words:BTreeMap<u64,u64>, mutations:usize, fail:bool }
impl vm::Io for Model {
 fn read(&mut self,a:u64)->Result<u64,&'static str>{Ok(*self.words.get(&a).unwrap_or(&0))}
 fn write(&mut self,a:u64,v:u64)->Result<(),&'static str>{self.mutations+=1;self.words.insert(a,v);Ok(())}
 fn flush(&mut self)->Result<(),&'static str>{self.mutations+=1;Ok(())}
 fn invalidate(&mut self,_:u64)->Result<u32,&'static str>{self.mutations+=1;if self.fail{Err("invalidation timeout")}else{Ok(0x18000)}}
 fn register(&mut self,_:usize)->u32{0}
}
fn fault(instance:u64)->vm::Fault {vm::Fault{unit:5,raw:[(instance>>12) as u32,0x1234,0,2],instance,address:0x1234,reason:2,client:0,gpc:0,hub:false,write:false}}
#[test]
fn consumers_pin_backing_and_tables_and_eio_never_means_completion() {
 let mut a=Allocator::new(1<<30,&[]).unwrap();let baseline=a.stats();let mut io=Model::default();
 let backing=a.allocate(12288,4096,Kind::Probe).unwrap();let replacement=a.allocate(12288,4096,Kind::Probe).unwrap();
 let mut c=Context::new(&mut a,&mut io).unwrap();
 c.map(&mut a,&mut io,(128<<20)-4096,backing,vm::Flags::default()).unwrap();
 let peak=a.stats();let before=io.mutations;
 let holds:Vec<_>=[Consumer::Queued,Consumer::Readback,Consumer::Scanout].into_iter().map(|k|c.hold(k).unwrap()).collect();
 assert_eq!(c.holds(),[1,1,1]);
 assert!(a.release(backing).is_err());
 for allocation in c.space().allocations(){assert!(a.release(allocation).is_err());}
 assert!(c.remap(&mut a,&mut io,(128<<20)-4096,replacement,vm::Flags::default()).is_err());
 assert!(c.unmap(&mut io,(128<<20)-4096).is_err());assert!(c.retire(&mut a,&mut io).is_err());
 assert_eq!(io.mutations,before);assert_eq!(a.stats(),peak);
 c.fail_eio();c.reset_failed();
 for h in holds{c.finish_cpu(h).unwrap();assert!(c.retire(&mut a,&mut io).is_err());assert_eq!(a.stats(),peak);}
 assert_eq!(io.mutations,before);
 c.cancel_unsubmitted().unwrap();
 c.remap(&mut a,&mut io,(128<<20)-4096,replacement,vm::Flags::default()).unwrap();
 a.release(backing).unwrap();assert!(a.release(replacement).is_err());
 c.unmap(&mut io,(128<<20)-4096).unwrap();a.release(replacement).unwrap();
 c.retire(&mut a,&mut io).unwrap();assert_eq!(a.stats(),baseline);
}
#[test]
fn faults_attribute_only_exact_live_instance_and_failed_reset_keeps_other_context_healthy() {
 let mut a=Allocator::new(1<<30,&[]).unwrap();let baseline=a.stats();let mut io=Model::default();
 let mut x=Context::new(&mut a,&mut io).unwrap();let mut y=Context::new(&mut a,&mut io).unwrap();
 let backing=a.allocate(4096,4096,Kind::Probe).unwrap();x.map(&mut a,&mut io,0,backing,vm::Flags::default()).unwrap();
 let f=fault(x.space().instance.range.start);
 assert!(!y.attribute(&f));assert!(x.attribute(&f));assert!(!x.attribute(&fault(1<<39)));
 assert!(x.hold(Consumer::Queued).is_err());assert!(x.retire(&mut a,&mut io).is_err());
 let h=y.hold(Consumer::Readback).unwrap();y.finish_cpu(h).unwrap();y.retire(&mut a,&mut io).unwrap();
 x.reset_failed();assert!(a.release(backing).is_err());
 x.cancel_unsubmitted().unwrap();x.retire(&mut a,&mut io).unwrap();a.release(backing).unwrap();assert_eq!(a.stats(),baseline);
}
#[test]
fn dropped_tickets_and_device_eio_cannot_bypass_drain_or_reset_and_foreign_receipts_fail() {
 let mut a=Allocator::new(1<<30,&[]).unwrap();let baseline=a.stats();let mut io=Model::default();
 let mut x=Context::new(&mut a,&mut io).unwrap();let mut y=Context::new(&mut a,&mut io).unwrap();
 let foreign=y.hold(Consumer::Queued).unwrap();assert!(x.finish_cpu(foreign).is_err());
 // The scheduler admission is modelled; there is no GPU engine in this test.
 unsafe{x.admit_device().unwrap();}
 let h=x.hold(Consumer::Queued).unwrap();drop(h);x.fail_eio();x.reset_failed();
 assert!(x.cancel_unsubmitted().is_err());assert!(x.retire(&mut a,&mut io).is_err());
 let peak=a.stats();assert!(a.release(x.space().instance).is_err());assert_eq!(a.stats(),peak);
 // The trusted model reset authorizes retirement; CPU cancellation must refuse it.
 unsafe{x.confirm_reset();y.confirm_reset();}
 x.retire(&mut a,&mut io).unwrap();y.retire(&mut a,&mut io).unwrap();assert_eq!(a.stats(),baseline);
}
#[test]
fn retire_timeout_and_remap_timeout_keep_all_charges_and_both_backings() {
 let mut a=Allocator::new(1<<30,&[]).unwrap();let mut io=Model::default();
 let mut x=Context::new(&mut a,&mut io).unwrap();let one=a.allocate(4096,4096,Kind::Probe).unwrap();let two=a.allocate(4096,4096,Kind::Probe).unwrap();
 x.map(&mut a,&mut io,0,one,vm::Flags::default()).unwrap();let used=a.stats();io.fail=true;
 assert!(x.remap(&mut a,&mut io,0,two,vm::Flags::default()).is_err());
 assert!(a.release(one).is_err());assert!(a.release(two).is_err());assert!(x.retire(&mut a,&mut io).is_err());assert_eq!(a.stats(),used);
 let mut io=Model::default();let mut y=Context::new(&mut a,&mut io).unwrap();let used=a.stats();io.fail=true;
 assert_eq!(y.retire(&mut a,&mut io),Err("invalidation timeout"));
 assert!(a.release(y.space().instance).is_err());assert!(a.release(y.space().directory).is_err());assert_eq!(a.stats(),used);
}

#[test]
fn dropping_an_unretired_owner_retains_backing_and_table_references() {
 let mut a=Allocator::new(1<<30,&[]).unwrap();let mut io=Model::default();
 let backing=a.allocate(4096,4096,Kind::Probe).unwrap();let instance;
 {
  let mut c=Context::new(&mut a,&mut io).unwrap();
  c.map(&mut a,&mut io,0,backing,vm::Flags::default()).unwrap();
  instance=c.space().instance;c.fail_eio();
 }
 let used=a.stats();assert!(a.release(backing).is_err());assert!(a.release(instance).is_err());assert_eq!(a.stats(),used);
}
#[test]
fn an_engine_completion_releases_only_its_consumer_and_still_requires_drain() {
 let mut a=Allocator::new(1<<30,&[]).unwrap();let baseline=a.stats();let mut io=Model::default();
 let mut c=Context::new(&mut a,&mut io).unwrap();unsafe{c.admit_device().unwrap();}
 let queued=c.hold(Consumer::Queued).unwrap();let readback=c.hold(Consumer::Readback).unwrap();
 assert!(c.finish_cpu(queued).is_err());assert_eq!(c.holds(),[1,1,0]);
 unsafe{c.finish_device(readback).unwrap();}assert_eq!(c.holds(),[1,0,0]);
 unsafe{c.confirm_drain();}assert!(c.retire(&mut a,&mut io).is_err());
 unsafe{c.confirm_reset();}c.retire(&mut a,&mut io).unwrap();assert_eq!(a.stats(),baseline);
}
#[test]
fn reset_keeps_scanout_resident_until_display_completion() {
 let mut a=Allocator::new(1<<30,&[]).unwrap();let baseline=a.stats();let mut io=Model::default();
 let backing=a.allocate(4096,4096,Kind::Probe).unwrap();
 let mut c=Context::new(&mut a,&mut io).unwrap();
 c.map(&mut a,&mut io,0,backing,vm::Flags::default()).unwrap();
 unsafe{c.admit_device().unwrap();}
 drop(c.hold(Consumer::Queued).unwrap());drop(c.hold(Consumer::Readback).unwrap());
 let scanout=c.hold(Consumer::Scanout).unwrap();let peak=a.stats();
 c.fail_eio();unsafe{c.confirm_reset();}
 assert_eq!(c.holds(),[0,0,1]);
 assert!(c.retire(&mut a,&mut io).is_err());assert!(a.release(backing).is_err());
 for allocation in c.space().allocations(){assert!(a.release(allocation).is_err());}
 assert_eq!(a.stats(),peak);
 // Model the display flipping away; an engine reset alone is not this receipt.
 unsafe{c.finish_device(scanout).unwrap();}
 c.retire(&mut a,&mut io).unwrap();a.release(backing).unwrap();assert_eq!(a.stats(),baseline);
}
