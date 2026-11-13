use std::cell::RefCell;
#[derive(Default)]
struct State { tick: u64, generation: u64, screens: u64, clients: u64, stale: bool, catches_up: bool }
thread_local! { static STATE: RefCell<State> = RefCell::new(State::default()); }
#[derive(Clone, Copy, PartialEq, PartialOrd)]
struct Clock(u64);
impl Clock {
    fn now() -> Self { STATE.with(|s| { let mut s=s.borrow_mut(); s.tick+=100; Self(s.tick) }) }
}
impl std::ops::Add<std::time::Duration> for Clock {
    type Output=Self;
    fn add(self, d:std::time::Duration)->Self { Self(self.0+d.as_millis() as u64) }
}
mod libbreenix { pub mod window {
    pub enum ObservationKind { Composed }
    pub mod abi { #[derive(Debug)] pub struct CompositeEntry { pub id:u64, pub commit_id:u64, pub pixel_gen:u64 } }
} }
mod libuitest {
    pub mod input {
        pub trait Backend { fn move_to(&mut self, p:(i32,i32))->Result<(),String>; }
        pub fn dispositions(_: ())->Result<(),String> { Ok(()) }
    }
    pub mod settle {
        pub fn settle_session(_: &mut super::super::Session, _: &[(u64,u64)], _: u64, _: &mut [()])->Result<(),String> { Ok(()) }
    }
}
struct Session;
impl Session {
    fn next_checkpoint(&mut self)->Result<(),String> { Ok(()) }
    fn arena_mut(&mut self)->&mut Self { self }
    fn read(&mut self, _: Slot)->Result<SlotRead,String> { Ok(SlotRead::Final { bytes: vec![0;4] }) }
}
struct LivePointer<'a>(&'a mut Session);
impl LivePointer<'_> {
    fn session(&mut self)->&mut Session { self.0 }
    fn desktop_tree(&mut self)->Result<(),String> { Ok(()) }
    fn serials(&self) {}
}
impl libuitest::input::Backend for LivePointer<'_> {
    fn move_to(&mut self, _: (i32,i32))->Result<(),String> { Ok(()) }
}
enum Slot { Tree, Screen }
#[derive(Debug)]
enum SlotRead { Final { bytes:Vec<u8> }, Incomplete }
mod libbui { pub mod tree { pub struct Tree; impl Tree { pub fn parse(_: &[u8])->Result<Self,String> { Ok(Self) } } } }
struct Frame;
enum Format { Bgrx }
impl Frame { fn new(_: &[u8], _:usize, _:usize, _:usize, _:Format)->Result<Self,()> { Ok(Self) } }
mod wm_checks {
    FRONT
    pub fn desktop(_: &(), _: [u64;2], _: [u64;2], _:u32, _: [u32;2], _:usize)->Result<(),String> { Ok(()) }
    pub fn entry(_: &super::libbui::tree::Tree, _: &str, _:u32)->Result<(),String> { Ok(()) }
    pub fn screen(_: super::Frame, _:u32, _:usize)->Result<(),String> { Ok(()) }
}
struct Window { pixel_gen:u64 }
fn generation(id:u64)->u64 { STATE.with(|s| if id==143 {s.borrow().generation} else {5}) }
fn wm_window(id:u64)->Result<Window,String> { Ok(Window { pixel_gen:generation(id) }) }
fn wm_composite(_:u64)->Result<Vec<libbreenix::window::abi::CompositeEntry>,String> {
    Ok([143,144].map(|id| { let g=if id==143 && STATE.with(|s| { let s=s.borrow(); s.stale || s.catches_up && s.screens == 2 }) {8} else {generation(id)};
        libbreenix::window::abi::CompositeEntry { id, commit_id:g, pixel_gen:g }
    }).into())
}
struct Record { commit:u64, composite:u64, width:u32, height:u32 }
mod capture {
    use super::*;
    pub fn window(_: &mut Session, _: &str, id:u64)->Result<Record,String> {
        STATE.with(|s|s.borrow_mut().clients+=1);
        Ok(Record { commit:generation(id), composite:0, width:1, height:1 })
    }
    pub fn screen(_: &mut Session, _: &str, _:libbreenix::window::ObservationKind)->Result<Record,String> {
        STATE.with(|s| {let mut s=s.borrow_mut(); s.screens+=1; s.generation=9;});
        Ok(Record { commit:0, composite:989, width:1, height:1 })
    }
    pub fn screen_windows(_: &Record)->Result<Vec<(u64,u64)>,String> {
        Ok(wm_composite(989)?.iter().map(|e|(e.id,e.commit_id)).collect())
    }
}
CHECKPOINT
BINDING
fn main() {
    // Exercise the incomplete-slot variant too, without entering that error path.
    assert!(matches!(SlotRead::Incomplete, SlotRead::Incomplete));
    let mut session=Session;
    for (stale,catches_up) in [(false,false),(false,true),(true,false)] {
        STATE.with(|s|*s.borrow_mut()=State { generation:8,stale,catches_up,..State::default() });
        let result=wm_checkpoint(&mut LivePointer(&mut session),[143,144],[1,2],1,[1280,800],1,["",""]);
        if stale || catches_up {
            assert!(result.is_err(), "stable stale composite must fail even when a later sample catches up");
            STATE.with(|s| assert_eq!(s.borrow().screens,2, "only the superseded first sample may retry"));
        }
        else {
            assert_eq!(result,Ok(()), "publication 8 -> 9 must reacquire the complete snapshot");
            STATE.with(|s| { let s=s.borrow(); assert_eq!(s.screens,2); assert_eq!(s.clients,4); });
        }
    }
}
