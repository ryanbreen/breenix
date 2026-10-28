extern crate alloc;
use alloc::{format, string::String, vec::Vec};
use std::cell::RefCell;
thread_local! {static EVENTS:RefCell<Vec<String>>=const{RefCell::new(Vec::new())}; static FAIL:RefCell<&'static str>=const{RefCell::new("")};}
fn event(s:String){EVENTS.with(|v|v.borrow_mut().push(s));}
fn fails(s:&str)->bool{FAIL.with(|f|*f.borrow()==s)}
#[path="ROOT/libs/libbreenix/src/crypto/sha256.rs"] mod sha256;
fn hash(data:&[u8])->String{sha256::sha256(data).iter().map(|v|format!("{v:02x}")).collect()}
macro_rules! serial_println {($($arg:tt)*)=>{println!($($arg)*)};}
pub(crate) use serial_println;
mod bulk {pub fn line(s:String)->Result<(),&'static str>{println!("{s}");Ok(())} pub fn used()->usize{1_326_749}}
#[path="ROOT/kernel/src/gpu/gk208/gr/channel.rs"] mod wire;
#[path="ROOT/kernel/src/gpu/gk208/gr/shaders.rs"] mod shaders;
#[path="ROOT/kernel/src/gpu/gk208/gr/draw.rs"] mod draw;
#[path="ROOT/kernel/src/gpu/gk208/gr/binding.rs"] mod binding;
struct Hardware<'a>{marker:&'a ()}
mod channel {
    pub use super::wire::*;
    use super::*;
    pub struct Session{pub id:usize,pub next:u32,pub target:Vec<u8>,pub vertices:Vec<u8>}
    impl Session {
        pub fn upload(&mut self,h:&mut Hardware<'_>,va:u64,data:&[u8])->Result<(),&'static str>{
            let _=h.marker;
            event(format!("upload-{}-{va:x}",self.id));
            if va==target(self.id){assert!(data.iter().all(|v|*v==0));self.target=data.to_vec();}
            else {assert_eq!(va,vertex(self.id));self.vertices=data.to_vec();}
            Ok(())
        }
        pub fn readback(&mut self,_:&mut Hardware<'_>,va:u64,size:usize)->Result<[Vec<u8>;2],&'static str>{
            assert_eq!((va,size),(target(self.id),16384));event(format!("read-{}",self.id));
            let mut pair=[self.target.clone(),self.target.clone()];
            if fails("poison"){pair[1][0]^=1;} Ok(pair)
        }
        pub fn context_draw(&mut self,_:&mut Hardware<'_>,r:draw::Draw,full:bool,
                            receipt:&mut binding::Receipt,current:&mut [u32;2])->Result<(),&'static str>{
            event(format!("draw-{}-{full}",self.id));
            assert_eq!(r.target_va,target(self.id));assert_eq!(r.vertex_va,vertex(self.id));
            let r=draw::Draw{report_va:0x900000+self.id as u64*4096,epoch:42+self.id as u64,nonce:self.next,..r};
            self.next+=1;
            let stream=if full {draw::commands(r)} else {draw::draw_commands(r)}.unwrap();
            let methods=methods(&stream);
            if !full {assert!(methods.iter().all(|m|[0x021c,0x1618,0x0d74,0x0d78,0x1614,0x0110,0x1b00,0x1b04,0x1b08,0x1b0c].contains(m)),"state leaked into restore stream");}
            if self.id==0 {
                let words=if full {draw::VERTEX_WORDS} else {draw::V2_VERTEX_WORDS};
                assert_eq!(self.vertices,words.iter().flat_map(|w|w.to_le_bytes()).collect::<Vec<_>>());
                self.target=if full {include_bytes!("v1.bin").to_vec()} else {include_bytes!("v2.bin").to_vec()};
            } else {
                assert_eq!(r.fragment_program,draw::GRADIENT_PROGRAM);
                assert_eq!(self.vertices,draw::FULL_VIEWPORT_VERTEX_WORDS.iter().flat_map(|w|w.to_le_bytes()).collect::<Vec<_>>());
                if full {assert_eq!(packet(&stream,0x214c),vec![shaders::GRADIENT_FRAGMENT_GPRS]);}
                self.target=(0..64).flat_map(|y|(0..64).flat_map(move|x|[4*x,4*y,2*(x+y),255])).collect();
            }
            *current=[0x80000100+self.id as u32;2];
            receipt.target=r.report_va;receipt.epoch=r.epoch;receipt.nonce=r.nonce;
            receipt.observation=Some(Observation{value:r.nonce,epoch:r.epoch,consumed:true,accepted:!fails("release"),elapsed_us:1});
            if fails("release"){return Err("injected missing release");} Ok(())
        }
    }
}
fn target(i:usize)->u64{0x100000+i as u64*0x100000}
fn vertex(i:usize)->u64{0x300000+i as u64*4096}
fn methods(words:&[u32])->Vec<u32>{let mut out=vec![];let mut i=0;while i<words.len(){let h=words[i];assert_eq!(h&0xe0000000,0x20000000);let n=((h>>16)&0x1fff)as usize;out.extend((0..n).map(|j|(h&0x1fff)*4+j as u32*4));i+=n+1;}out}
fn packet(words:&[u32],m:u32)->Vec<u32>{let mut i=0;while i<words.len(){let h=words[i];let n=((h>>16)&0x1fff)as usize;if(h&0x1fff)*4==m{return words[i+1..i+n+1].to_vec();}i+=n+1;}panic!("missing packet")}
// FLOW
// EVIDENCE
fn run(failure:&'static str)->Result<(),&'static str>{
    EVENTS.with(|v|v.borrow_mut().clear());FAIL.with(|v|*v.borrow_mut()=failure);
    let mut a=channel::Session{id:0,next:6,target:vec![0;16384],vertices:draw::VERTEX_WORDS.iter().flat_map(|w|w.to_le_bytes()).collect()};
    let mut b=channel::Session{id:1,next:6,target:vec![0;16384],vertices:draw::FULL_VIEWPORT_VERTEX_WORDS.iter().flat_map(|w|w.to_le_bytes()).collect()};
    let requests=core::array::from_fn(|i|draw::Draw{target_va:target(i),pitch:256,width:64,height:64,vertex_va:vertex(i),quarantine_va:0x400000+i as u64*(1<<20),vertex_program:shaders::VERTEX_OFFSET as u32,fragment_program:if i==0{shaders::FRAGMENT_OFFSET as u32}else{draw::GRADIENT_PROGRAM},report_va:0,epoch:0,nonce:0});
    let mut evidence=ContextsEvidence::default();
    let result=contexts_sequence(&mut Hardware{marker:&()},&mut a,&mut b,requests,&mut evidence);
    if failure.is_empty(){evidence.emit(None)?;} result
}
#[test]fn alternation(){assert!(run("").is_ok());let e=EVENTS.with(|v|v.borrow().clone());assert_eq!(e,vec!["read-1","draw-0-true","read-0","read-1","read-0","draw-1-true","read-1","read-0","upload-0-100000","upload-0-300000","read-1","draw-0-false","read-0","read-1","read-0","draw-1-false","read-1","read-0"]);}
#[test]fn missing_release_stops_before_ce(){assert!(run("release").is_err());assert_eq!(EVENTS.with(|v|v.borrow().clone()),["read-1","draw-0-true"]);}
#[test]fn poison_mismatch_stops_before_draw(){assert!(run("poison").is_err());assert_eq!(EVENTS.with(|v|v.borrow().clone()),["read-1"]);}
