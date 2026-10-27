//! CPU table and ordering checks; these do not execute GPU translations.
#![allow(dead_code)] // Standalone host harness imports the complete memory modules.
extern crate alloc;
#[path = "../kernel/src/gpu/gk208/memory.rs"] mod memory;
#[path = "../kernel/src/gpu/gk208/vm.rs"] mod vm;
use memory::{allocator::Allocator, Range};
use vm::{Flags, Io, Space, LIMIT, TABLE_BYTES};
use std::collections::BTreeMap;

#[derive(Default)]
struct Model { words:BTreeMap<u64,u64>, events:Vec<(char,u64,u64)>, fail:bool, regs:BTreeMap<usize,u32> }
impl Io for Model {
    fn read(&mut self,a:u64)->Result<u64,&'static str> {Ok(*self.words.get(&a).unwrap_or(&0xdeadbeef))}
    fn write(&mut self,a:u64,v:u64)->Result<(),&'static str> {self.words.insert(a,v);self.events.push(('w',a,v));Ok(())}
    fn flush(&mut self)->Result<(),&'static str> {self.events.push(('f',0,0));Ok(())}
    fn invalidate(&mut self,pd:u64)->Result<u32,&'static str> {
        self.events.push(('i',pd,0));if self.fail {Err("invalidation timeout")} else {Ok(0x18000)}
    }
    fn register(&mut self,r:usize)->u32 {*self.regs.get(&r).unwrap_or(&0)}
}
#[test]
fn private_spaces_cross_a_directory_boundary_and_remap_only_after_visibility() {
    let mut a=Allocator::new(1<<30,&[Range{start:0,size:128<<20}]).unwrap();
    let baseline=a.stats();let mut io=Model::default();
    let mut x=Space::new(&mut a,&mut io).unwrap();let mut y=Space::new(&mut a,&mut io).unwrap();
    assert_ne!(x.instance.range,y.instance.range);assert_ne!(x.directory.range,y.directory.range);
    for s in [&x,&y] {
        assert_eq!(io.words[&(s.instance.range.start+0x200)],s.directory.range.start);
        assert_eq!(io.words[&(s.instance.range.start+0x208)],LIMIT-1);
        assert!((0..65536).step_by(8).all(|o|io.words[&(s.directory.range.start+o)]==0));
    }
    let va=(128<<20)-4096;
    let flags=Flags{read_only:true,privileged:true,volatile:false};
    x.map(&mut a,&mut io,va,Range{start:512<<20,size:12288},flags).unwrap();
    y.map(&mut a,&mut io,va,Range{start:640<<20,size:12288},Flags::default()).unwrap();
    for (s,base,bits) in [(&x,512<<20,7),(&y,640<<20,1)] {
        for n in 0..3 {
            assert_eq!(io.words[&s.pte_address(va+n*4096).unwrap()],((base+n*4096)>>8)|bits);
        }
        assert_eq!(io.words[&s.pte_address(va-4096).unwrap()],0);
        assert_eq!(io.words[&s.pte_address(va+12288).unwrap()],0);
        for pdi in 0..2 {
            let pde=io.words[&(s.directory.range.start+pdi*8)];
            assert_eq!(pde as u32,0);assert_eq!((pde>>32)&15,1);
            let table=(pde>>36)<<12;
            assert_eq!(table%4096,0);
            assert_eq!(io.words[&table],if pdi==1 {((base+4096)>>8)|bits} else {0});
            assert_eq!(io.words[&(table+TABLE_BYTES-8)],if pdi==0 {(base>>8)|bits} else {0});
        }
    }
    let unchanged:Vec<_>=(0..3).map(|n|io.words[&y.pte_address(va+n*4096).unwrap()]).collect();
    io.events.clear();
    x.remap(&mut io,va,Range{start:768<<20,size:12288},Flags{volatile:true,..Flags::default()}).unwrap();
    assert_eq!(&io.events[io.events.len()-2..],&[('f',0,0),('i',x.directory.range.start,0)]);
    assert_eq!(io.events.iter().filter(|e|e.0=='w').count(),3);
    for n in 0..3 {assert_eq!(io.words[&x.pte_address(va+n*4096).unwrap()],(((768<<20)+n*4096)>>8)|(1<<32)|1);}
    assert_eq!((0..3).map(|n|io.words[&y.pte_address(va+n*4096).unwrap()]).collect::<Vec<_>>(),unchanged);
    x.unmap(&mut io,va,12288).unwrap();
    assert!((0..3).all(|n|io.words[&x.pte_address(va+n*4096).unwrap()]==0));
    x.retire(&mut a,&mut io).unwrap();y.retire(&mut a,&mut io).unwrap();assert_eq!(a.stats(),baseline);
}

#[test]
fn rejected_maps_do_not_mutate_and_failed_invalidation_never_recycles_storage() {
    let mut a=Allocator::new(1<<30,&[]).unwrap();let mut io=Model::default();
    let mut s=Space::new(&mut a,&mut io).unwrap();
    let before=a.stats();io.events.clear();
    for (va,physical,size) in [(1,4096,4096),(0,1,4096),(0,4096,0),(0,4096,4097),
                              (LIMIT,4096,4096),(LIMIT-4096,4096,8192),(0,1<<30,4096),
                              (u64::MAX-4095,4096,8192),(0,u64::MAX-4095,8192)] {
        assert!(s.map(&mut a,&mut io,va,Range{start:physical,size},Flags::default()).is_err());
        assert_eq!(a.stats(),before);assert!(io.events.is_empty());
    }
    s.map(&mut a,&mut io,LIMIT-4096,Range{start:4096,size:4096},Flags::default()).unwrap();
    assert_eq!(io.words[&(s.directory.range.start+8191*8)]&0xffffffff,0);
    let used=a.stats();io.events.clear();
    assert!(s.map(&mut a,&mut io,LIMIT-4096,Range{start:8192,size:4096},Flags::default()).is_err());
    assert!(s.remap(&mut io,LIMIT-8192,Range{start:8192,size:8192},Flags::default()).is_err());
    assert!(s.unmap(&mut io,LIMIT-8192,8192).is_err());
    assert!(io.events.is_empty());assert_eq!(a.stats(),used);
    io.fail=true;
    assert!(s.remap(&mut io,LIMIT-4096,Range{start:8192,size:4096},Flags::default()).is_err());
    assert!(s.unmap(&mut io,LIMIT-4096,4096).is_err());
    assert!(s.retire(&mut a,&mut io).is_err());
    assert_eq!(a.stats(),used,"uncertain device access must retain all table charges");
}

#[test]
fn fault_records_keep_raw_words_and_decode_only_pending_units_without_acknowledging() {
    let mut io=Model::default();
    io.regs.insert(0x259c,(1<<5)|(1<<21));
    let raw=[0x12345,0x89abcdef,0x23,0x17001bc6];
    for unit in [5,21] {for (i,v) in raw.into_iter().enumerate(){io.regs.insert(0x2800+unit*16+i*4,v);}}
    let (mask,records)=vm::faults(&mut io);
    assert_eq!(mask,(1<<5)|(1<<21));assert_eq!(records.len(),2);
    for (record,unit) in records.iter().zip([5,21]) {
        assert_eq!(record.unit,unit);assert_eq!(record.raw,raw);
        assert_eq!(record.instance,0x12345000);assert_eq!(record.address,0x2389abcdef);
        assert_eq!(record.reason,6);assert_eq!(record.client,27);assert_eq!(record.gpc,23);
        assert!(record.hub);assert!(record.write);
    }
    assert!(io.events.is_empty(),"fault observation must not acknowledge or reset hardware");
}
