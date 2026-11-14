use std::cell::RefCell;
pub use gpu_abi::stream::{self, encode};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Work { pub submits: usize, pub clears: usize, pub clear_bytes: u64, pub copies: usize, pub copy_bytes: u64 }
type CaseError = String;
type R<T> = Result<T, CaseError>;
type CaseResult = R<()>;
// CONSTANTS
#[derive(Default)]
pub struct State {
    pub mode: [u32; 2], pub targets: Vec<([u32; 2], u32, u32)>, pub screen: u64, pub timed: bool,
    pub views: usize, pub timed_views: usize, pub timed_allocations: usize,
    pub work: Work, pub presents: usize, pub timed_presents: usize, pub corners: usize, pub full: usize, pub timed_copies: usize,
}
thread_local! { static STATE: RefCell<State> = RefCell::new(State::default()); }
pub fn ensure(ok: bool, why: impl FnOnce() -> String) -> R<()> { if ok { Ok(()) } else { Err(why()) } }
pub fn fail_msg(s: String) -> String { s }
pub fn errno(r: i64) -> String { r.to_string() }
pub fn begin() -> R<()> { Ok(()) }
pub fn take_display_2560() -> R<()> { Ok(()) }
pub fn context() -> R<u64> { Ok(1) }
pub fn native_success() -> R<u32> { Ok(0) }
pub fn wait_ok(_: u64, _: u64, _: u64) -> R<()> { Ok(()) }
pub fn pass() -> CaseResult { Ok(()) }
pub fn bgra(b:u8,g:u8,r:u8,a:u8)->u32 { u32::from_le_bytes([b,g,r,a]) }
pub fn value(_: &str, _: i64, _: &str, bound: Option<(i64,i64)>) { assert_eq!(bound, Some((60,i64::MAX))); }
pub fn now_ns() -> u64 { STATE.with(|s| { s.borrow_mut().timed = true; }); 0 }
pub fn us_since(_: u64) -> u64 { STATE.with(|s| { s.borrow_mut().timed = false; }); 1_000 }
pub mod format { pub const B8G8R8A8: u32 = 1; }
pub mod abi {
    pub struct Mode { pub width:u32, pub height:u32 }
    pub struct Modes;
    impl Modes { pub fn current_mode(&self)->Option<Mode> { super::STATE.with(|s| { let m=s.borrow().mode; Some(Mode {width:m[0],height:m[1]}) }) } }
    pub struct Present { pub fence:u64, pub answer:u32 }
}
pub mod gpu {
    use super::*;
    pub fn modes()->(i64,abi::Modes) { (0,abi::Modes) }
    pub fn present(_:u64,t:u64)->(i64,abi::Present) {
        STATE.with(|s| { let mut s=s.borrow_mut(); assert_eq!(s.targets[t as usize-1].0,s.mode);
            if s.timed && s.mode[1]>H {assert_eq!(s.targets[t as usize-1].2,0xff181512,"outside the measured view changed");}
            s.screen=t; s.presents+=1; if s.timed {s.timed_presents+=1;} });
        (0,abi::Present {fence:1,answer:0})
    }
    pub fn resource_destroy(_:u64)->i64 {0}
}
pub fn scanout_target(_:u32,w:u32,h:u32)->R<u64> {
    STATE.with(|s| { let mut s=s.borrow_mut(); if s.timed {s.timed_allocations+=1;}
        s.targets.push(([w,h],0,0)); Ok(s.targets.len() as u64) })
}
pub fn scene(t:u64,c:u32,_:&[()])->Vec<u32> {let mut s=Vec::new();encode::begin(&mut s,t);encode::clear(&mut s,c);s}
pub fn submit(_:u64,words:&[u32])->R<u64> {
    STATE.with(|s| {let mut s=s.borrow_mut(); let mut at=0; let mut target=0; let mut scissor=None;
        if s.timed {s.work.submits+=1;}
        while at<words.len() {match words[at] {
            stream::cmd::BEGIN=>{target=words[at+2] as usize-1;scissor=None;},
            stream::cmd::SCISSOR=>{scissor=Some([2,3,4,5].map(|i| words[at+i] as i32));},
            stream::cmd::CLEAR=>{
                let [w,h]=s.targets[target].0;
                let r=stream::clipped([0,0,w as i32,h as i32],w,h,scissor).unwrap();
                if s.timed {s.work.clears+=1;s.work.clear_bytes+=u64::from(r[2]-r[0])*u64::from(r[3]-r[1])*4;}
                assert_eq!([r[0],r[1]],[0,0]);
                s.targets[target].1=words[at+2];
                if r[3]==h {s.targets[target].2=words[at+2];}
            },
            stream::cmd::COPY=>{
                assert_eq!(&words[at+4..at+8],&[0,0,W,H]);assert_eq!(&words[at+10..at+12],&[0,0]);
                if s.timed {s.timed_copies+=1;s.work.copies+=1;s.work.copy_bytes+=u64::from(W)*u64::from(H)*8;}
                let src=words[at+2] as usize-1;let dst=words[at+8] as usize-1;
                s.targets[dst].1=s.targets[src].1;
            },
            _=>panic!("bad stream"),
        } at+=words[at+1] as usize;}
    }); Ok(1)
}
pub fn frame(ctx:u64,s:&[u32])->R<u64> {submit(ctx,s)}
pub fn present_view(_:u64,t:u64)->R<Option<abi::Present>> {
    STATE.with(|s| {let mut s=s.borrow_mut();s.views+=1;s.screen=t;if s.timed {s.timed_views+=1;}
        assert_eq!(s.targets[t as usize-1].0,[W,H]);});
    Ok(Some(abi::Present {fence:1,answer:0}))
}
pub fn read_rect(t:u64,r:[u32;4])->R<Vec<u8>> {
    STATE.with(|s| {let mut s=s.borrow_mut();assert_eq!(t,0);
        if r==[W-16,H-16,16,16] {s.corners+=1;} else {assert_eq!(r,[0,0,W,H]);assert!(!s.timed);s.full+=1;}
        Ok(s.targets[s.screen as usize-1].1.to_le_bytes().to_vec()) })
}
pub fn all(px:&[u8],c:u32,_:&str)->R<()> {assert_eq!(px,c.to_le_bytes());Ok(())}
pub fn all_judged(px:Vec<u8>,c:u32,w:&str,width:u32,height:u32)->CaseResult {assert_eq!([width,height],[W,H]);all(&px,c,w)}
// PRESENT
// Main 9a7f88cb (also a85904b6), performance.rs:115-124: clear the presented target.
fn main_clear_work() -> Work {
    STATE.with(|s| *s.borrow_mut()=State {mode:[W,H],..State::default()});
    let targets=[scanout_target(format::B8G8R8A8,W,H).unwrap(),scanout_target(format::B8G8R8A8,W,H).unwrap()];
    now_ns();
    for n in 0..FRAMES {
        let t=targets[(n%2) as usize];
        let mut s=Vec::new();encode::begin(&mut s,t);encode::clear(&mut s,colour(n));
        submit(1,&s).unwrap();present(1,t).unwrap();
    }
    STATE.with(|s| s.borrow().work)
}
fn main() {
    let case=std::env::args().nth(1).unwrap();
    let reference=main_clear_work();
    assert_eq!(reference,Work {submits:120,clears:120,clear_bytes:1_966_080_000,copies:0,copy_bytes:0});
    assert!(rate_met(120,2_000_000,60));assert!(!rate_met(120,2_000_001,60));
    for mode in [[2560,2048],[2560,1600]] {
        STATE.with(|s| *s.borrow_mut()=State {mode,..State::default()});
        if case=="clear" {clear_rate_2560().unwrap();} else {present_rate_2560().unwrap();}
        STATE.with(|s| {let s=s.borrow();
            assert_eq!(s.timed_views,0,"per-frame exact view verification in the measured loop");
            assert_eq!(s.timed_allocations,0,"per-frame scanout allocation");
            assert_eq!(s.timed_presents,120);
            assert_eq!(s.timed_copies,0,"rate frames must not copy the view");
            if case=="clear" {assert_eq!(s.work,reference,"per-frame commands and pixel bytes must match main at {mode:?}");}
            else {assert_eq!(s.work,Work::default(),"static presentation must not render during the sample");}
            println!("{case} mode={mode:?} work={:?} main={reference:?}",s.work);
            assert_eq!(s.corners,if case=="clear" {1} else {121},"original far-corner checks");
            assert_eq!(s.full,1,"final whole scanout check");
            assert_eq!(s.views,1,"one exact source-rectangle view check outside the sample");
        });
    }
}
