//! Bound the real per-PDB invalidation sequence, not a substitute engine probe.
#![allow(dead_code)] // Complete pure hardware modules are imported by this host harness.
extern crate alloc;
#[path = "../kernel/src/gpu/gk208/edid.rs"] mod edid;
#[path = "../kernel/src/gpu/gk208/modeset.rs"] mod modeset;
#[path = "../kernel/src/gpu/gk208/vm_control.rs"] mod vm_control;
#[derive(Default)]
struct Regs { writes:Vec<(usize,u32)>, no_slot:bool, no_ack:bool, delays:usize }
impl modeset::Io for Regs {
    fn read(&mut self,r:usize)->u32 {
        if r!=0x100c80 {return 0;}
        (if self.no_slot {0} else {0x10000}) | (if self.no_ack {0} else {0x8000})
    }
    fn write(&mut self,r:usize,v:u32) {self.writes.push((r,v));}
    fn delay_us(&mut self,us:u32) {assert_eq!(us,100);self.delays+=1;}
}
#[test]
fn per_directory_invalidation_flushes_before_queueing_and_bounds_both_waits() {
    let mut r=Regs::default();
    assert_eq!(vm_control::invalidate(&mut r,0x12345000).unwrap(),0x18000);
    assert_eq!(r.writes,[(0x70000,1),(0x100cb8,0x123450),(0x100cbc,0x80000001)]);
    let mut r=Regs{no_slot:true,..Regs::default()};
    assert!(vm_control::invalidate(&mut r,0x12345000).is_err());
    assert_eq!(r.writes,[(0x70000,1)]);assert_eq!(r.delays,20000);
    let mut r=Regs{no_ack:true,..Regs::default()};
    assert!(vm_control::invalidate(&mut r,0x12345000).is_err());assert_eq!(r.delays,20000);
    let mut r=Regs::default();
    for directory in [1,1<<40,u64::MAX] {assert!(vm_control::invalidate(&mut r,directory).is_err());}
    assert!(r.writes.is_empty());
}
