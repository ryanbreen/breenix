extern crate alloc;
use alloc::{format, string::String, vec::Vec};
use std::cell::RefCell;
thread_local! { static EVENTS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) }; static FAILURE: RefCell<&'static str> = const { RefCell::new("") }; }
fn event(s: &str) { EVENTS.with(|v| v.borrow_mut().push(s.into())); }
fn fail(s: &str) -> bool { FAILURE.with(|v| *v.borrow()==s) }
fn hash(_: &[u8]) -> String { "00".repeat(32) }
fn hex(b: &[u8]) -> String { b.iter().map(|v|format!("{v:02x}")).collect() }
macro_rules! serial_println { ($($arg:tt)*) => { println!($($arg)*) }; }
pub(crate) use serial_println;
mod bulk { pub fn line(s:String)->Result<(),&'static str>{println!("{s}");Ok(())} pub fn used()->usize{865396} }
mod context { #[derive(Clone,Copy)] pub struct Buffer {pub va:u64} }
mod shaders {
    pub const SEGMENT_SIZE:usize=0x280; pub const CODE_REGION_SIZE:usize=0x480;
    pub const CONSTANT_COLOUR_FRAGMENT_OFFSET:usize=0x2b0; pub const DROP_COLOUR_VERTEX_OFFSET:usize=0x3b0;
    pub const VERTEX_OFFSET:usize=0x30; pub const FRAGMENT_OFFSET:usize=0x1b0;
    pub const CONSTANT_COLOUR_FRAGMENT_HEADER:&[u8;80]=&[1;80]; pub const CONSTANT_COLOUR_FRAGMENT_CODE:&[u8;48]=&[2;48];
    pub const DROP_COLOUR_VERTEX_HEADER:&[u8;80]=&[3;80]; pub const DROP_COLOUR_VERTEX_CODE:&[u8;88]=&[4;88];
}
mod draw {
    pub const VERTEX_WORDS:[u32;24]=[0;24];
    pub struct Draw { pub target_va:u64,pub pitch:u32,pub width:u32,pub height:u32,pub vertex_va:u64,pub vertex_program:u32,pub fragment_program:u32,pub report_va:u64,pub epoch:u64,pub nonce:u32 }
}
mod binding {
    pub const TRAP_REGISTERS:[usize;4]=[0x400100,0x400108,0x400110,0x400118];
    pub struct Receipt {pub target:u64,pub epoch:u64,pub nonce:u32,pub before:[u32;4],pub after:[u32;4],pub observation:Option<super::channel::Observation>}
}
struct Registers;
struct Layout {allocator:Allocator}
struct Allocator;
impl Allocator {fn release(&mut self,_:u64)->Result<(),&'static str>{event("release");assert!(events().contains(&"reset".into()));Ok(())}}
struct VmIo<'a> {regs:&'a mut Registers,total:u64}
struct Context;
impl Context {
    unsafe fn confirm_reset(&mut self){assert!(events().contains(&"reset".into()));}
    fn retire(&mut self,_:&mut Allocator,io:&mut VmIo<'_>)->Result<(),&'static str>{let _=(&io.regs,io.total);event("retire");Ok(())}
}
struct Reader;
impl Reader {fn quiesce(&mut self,_:&mut Registers,_:&mut Layout)->Result<(),&'static str>{panic!("separate CE reader after GR admission")} fn finish(self,_:&mut Registers,_:&mut Layout)->Result<(),&'static str>{panic!("separate CE reader")}}
mod ctxctl {pub fn reset(_:&mut super::Registers)->Result<(),&'static str>{super::event("reset");assert!(super::events().contains(&"quiesce".into()));Ok(())}}
fn hardware_error(s:&'static str)->&'static str{s}
struct Hardware<'a> {regs:&'a mut Registers,layout:&'a mut Layout,total:u64,context:Context,graphics:Option<channel::Session>,reader:Option<Reader>,backing:Vec<(u64,u64)>}
impl Hardware<'_> { // FINISH
}
mod channel {
    use super::*;
    #[derive(Clone,Copy)] pub struct Observation {pub value:u32,pub epoch:u64,pub consumed:bool,pub accepted:bool,pub elapsed_us:u64}
    pub struct Session {pub next:u32,pub uploads:Vec<(u64,Vec<u8>)>}
    impl Session {
        pub fn allocate_draw(&mut self,_:&mut Hardware<'_>,name:&'static str,bytes:u64)->Result<context::Buffer,&'static str>{event(name);assert!(bytes==16384||bytes==96);Ok(context::Buffer{va:if bytes==96{0x20000}else{0x10000}})}
        pub fn upload(&mut self,_:&mut Hardware<'_>,va:u64,data:&[u8])->Result<(),&'static str>{event(&format!("upload-{va:x}-{}",data.len()));self.uploads.push((va,data.into()));Ok(())}
        pub fn readback(&mut self,_:&mut Hardware<'_>,va:u64,size:usize)->Result<[Vec<u8>;2],&'static str>{event(&format!("read-{va:x}-{size}"));let data=self.uploads.iter().rev().find(|(a,_)|*a==va).unwrap().1.clone();let mut images=[data.clone(),data];if fail("mismatch")&&va==0x10000{images[1][0]^=1;}Ok(images)}
        pub fn draw(&mut self,_:&mut Hardware<'_>,r:draw::Draw,receipt:&mut binding::Receipt)->Result<(),&'static str>{
            event(&format!("draw-{:x}-{:x}",r.vertex_program,r.fragment_program));
            assert_eq!((r.target_va,r.vertex_va,r.width,r.height,r.pitch),(0x10000,0x20000,64,64,256));
            assert_eq!((r.report_va,r.epoch,r.nonce),(0,0,0));
            let last=self.uploads.last().unwrap();assert_eq!(last.0,0x10000);assert!(last.1.iter().all(|v|*v==0));
            receipt.target=0x30000;receipt.epoch=42;receipt.nonce=self.next;self.next+=1;
            receipt.observation=Some(Observation{value:receipt.nonce,epoch:42,consumed:true,accepted:!fail("timeout"),elapsed_us:7});
            if fail("timeout"){return Err("injected receipt timeout");} Ok(())
        }
        pub fn quiesce(&mut self,_:&mut Registers,_:u64)->Result<(),&'static str>{event("quiesce");if fail("drain"){return Err("injected drain timeout");}Ok(())}
        pub fn finish(self,_:&mut Registers,_:&mut Layout,_:u64)->Result<(),&'static str>{event("session-reclaim");assert!(events().contains(&"reset".into()));Ok(())}
    }
}
// FLOW
// EVIDENCE
fn events()->Vec<String>{EVENTS.with(|v|v.borrow().clone())}
fn run(failure:&'static str)->Result<(),&'static str>{
    EVENTS.with(|v|v.borrow_mut().clear());FAILURE.with(|v|*v.borrow_mut()=failure);
    let mut regs=Registers;let mut layout=Layout{allocator:Allocator};
    let mut h=Hardware{regs:&mut regs,layout:&mut layout,total:1<<24,context:Context,graphics:None,reader:None,backing:vec![(0,1)]};
    let mut s=channel::Session{next:6,uploads:vec![]};let mut evidence=DrawEvidence::default();
    let result=draw_probe(&mut h,&mut s,0x40000,&mut evidence);
    h.graphics=Some(s);
    if failure.is_empty(){evidence.emit(None)?;}
    let unwind=h.finish();result.and(unwind)
}
#[test] fn order_and_evidence(){
    assert!(run("").is_ok());
    let e=events();let draws:Vec<_>=e.iter().filter(|s|s.starts_with("draw-")).cloned().collect();
    assert_eq!(draws,["draw-30-1b0","draw-30-2b0","draw-30-1b0","draw-3b0-1b0","draw-30-1b0"]);
    assert_eq!(e.iter().filter(|s|s.as_str()=="read-10000-16384").count(),5);
    let first=e.iter().position(|s|s=="draw-target").unwrap();let upload=e.iter().position(|s|s=="upload-20000-96").unwrap();assert!(first<upload);
}
#[test] fn receipt_timeout_retains_channel_until_reset(){
    assert_eq!(run("timeout"),Err("injected receipt timeout"));let e=events();
    assert_eq!(e.iter().filter(|s|s.starts_with("draw-")).count(),1);
    assert!(!e.contains(&"read-10000-16384".into()));
    assert!(e.iter().position(|s|s=="reset").unwrap()<e.iter().position(|s|s=="session-reclaim").unwrap());
}
#[test] fn mismatch_stops_draws_and_resets_before_reclaim(){
    assert!(run("mismatch").is_err());let e=events();assert_eq!(e.iter().filter(|s|s.starts_with("draw-")).count(),1);
    assert!(e.iter().position(|s|s=="reset").unwrap()<e.iter().position(|s|s=="release").unwrap());
}
#[test] fn failed_drain_retains_backing(){assert!(run("drain").is_err());let e=events();assert!(!e.contains(&"reset".into()));assert!(!e.contains(&"release".into()));assert!(!e.contains(&"retire".into()));}
