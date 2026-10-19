#![cfg(feature = "alloc")]
use gpu_abi::{draw::{self, Instance}, pipeline, stream::{self, Resolver, SourceInfo, TargetInfo, Words}};
use std::cell::Cell;
struct Resources { reads: Cell<usize> }
impl Resolver for Resources {
    fn target(&self, _: u64) -> Option<TargetInfo> { Some(TargetInfo { width: 16384, height:16384, format:0, invalid:false }) }
    fn source(&self, _: u64) -> Option<SourceInfo> { Some(SourceInfo { width:16384, height:16384, format:0, invalid:false }) }
    fn buffer_size(&self, _: u64) -> Option<u64> { Some(1028) }
    fn buffer_rect(&self, _: u64, _: u64) -> [i32;4] { unreachable!() }
    fn buffer_word(&self, _: u64, _: u64) -> u32 { self.reads.set(self.reads.get()+1); 0 }
}
#[test]
fn repeated_coverage_table_is_validated_once() {
    let r=Resources { reads:Cell::new(0) };
    let mut words=Vec::new();
    stream::encode::begin(&mut words,1);
    let mut params=[0;16]; params[2]=64; params[3]=32;
    let instances:Vec<_>=(0..32).map(|x|Instance {rect:[x,0,x+1,1],params}).collect();
    draw::encode(&mut words,pipeline::COVERAGE_SPANS,0,0,2,1028,&instances);
    let plan=stream::validate(&words[..],&r).unwrap();
    assert_eq!(plan.drawn,32);
    assert!(r.reads.get()<700,"{} buffer reads for one table",r.reads.get());
}
struct Expensive { reads:Cell<usize> }
impl Words for Expensive {
    fn len(&self)->usize {4+draw::HEADER+draw::INSTANCE*65536}
    fn word(&self,i:usize)->u32 {
        self.reads.set(self.reads.get()+1);
        if i<4 {return [stream::cmd::BEGIN,4,1,0][i];}
        let i=i-4;
        if i<draw::HEADER {return [draw::COMMAND,(self.len()-4) as u32,pipeline::OVER,0,2,0,0,0,0,65536][i];}
        match (i-draw::HEADER)%draw::INSTANCE {2|3=>16384,_=>0}
    }
}
#[test]
fn over_budget_instance_is_refused_before_collecting_the_command() {
    let words=Expensive {reads:Cell::new(0)};
    let r=Resources {reads:Cell::new(0)};
    let refusal=stream::validate(&words,&r).unwrap_err();
    assert_eq!(refusal.error,stream::error::WORK_LIMIT);
    assert!(words.reads.get()<100,"walked {} words before refusing",words.reads.get());
}
