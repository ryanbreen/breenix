use std::{cell::{Cell, RefCell}, ops::{Deref, DerefMut}, sync::{Mutex, MutexGuard}};
thread_local! {
    static LOCKED: Cell<bool> = const { Cell::new(false) };
    static FAIL: Cell<u8> = const { Cell::new(0) };
    static EVENTS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}
fn event(value: &str) { EVENTS.with(|e| e.borrow_mut().push(value.into())); }
fn locked() -> bool { LOCKED.with(Cell::get) }
fn fail(at: u8) -> bool { FAIL.with(|f| f.get() == at) }
macro_rules! serial_println {
    ($($arg:tt)*) => {{
        assert!(!crate::locked(), "presentation diagnostic exported under SESSION");
        crate::event(&format!($($arg)*));
    }};
}
pub(crate) use serial_println;
mod spin {
    pub struct Mutex<T>(std::sync::Mutex<T>);
    impl<T> Mutex<T> {
        pub const fn new(value: T) -> Self { Self(std::sync::Mutex::new(value)) }
        pub fn lock(&self) -> std::sync::MutexGuard<'_, T> { self.0.lock().unwrap() }
    }
}
mod time { pub mod wakeup {
    pub fn stats() -> [u64; 3] { [u64::from(super::super::locked()), 2, 3] }
}}
mod presentation_trace {
    use crate::spin;
    // TRACE
}
mod present {
    #[derive(Clone, Copy)]
    pub struct Surface { pub resource:u32, pub address:u64, pub width:u32, pub height:u32,
        pub pitch:u32, pub block_height:u32, pub format:u32, pub dma:u32 }
    pub struct Receipt;
}
struct Registers(usize);
#[derive(Clone, Copy)] struct Range { start:u64, size:u64 }
struct Object { name:&'static str, range:Range }
struct Layout { objects:Vec<Object> }
#[derive(Clone, Copy)] struct Console { vram:u64, bytes:usize }
struct Session { pending_present:Option<present::Surface>, ready_present:Option<present::Receipt>,
    evo:native::Evo, registers:usize, console:Console, layout:Layout, mode:u32, scanout:Vec<u32> }
struct Owner(Mutex<Option<Session>>);
struct Guard<'a>(MutexGuard<'a, Option<Session>>);
impl Owner {
    fn lock(&self) -> Guard<'_> {
        let guard = self.0.lock().unwrap();
        assert!(!LOCKED.with(|v| v.replace(true)));
        event("lock");
        Guard(guard)
    }
}
impl Deref for Guard<'_> { type Target=Option<Session>; fn deref(&self)->&Self::Target { &self.0 } }
impl DerefMut for Guard<'_> { fn deref_mut(&mut self)->&mut Self::Target { &mut self.0 } }
impl Drop for Guard<'_> {
    fn drop(&mut self) {
        LOCKED.with(|v| v.set(false)); event("unlock");
        // Force a new frame between session unlock and serial export.
        presentation_trace::record([999;15]);
    }
}
static SESSION:Owner=Owner(Mutex::new(None));
mod native {
    use super::*;
    pub struct Evo { pub head:u32 }
    pub fn wait_present(_: &mut Evo, _: &mut Registers, _:present::Surface)->Result<present::Receipt,&'static str> {
        assert!(locked()); event("drain"); Ok(present::Receipt)
    }
    pub fn present(_: &mut Evo, regs: &mut Registers, surface:present::Surface)->Result<present::Receipt,&'static str> {
        assert!(locked()); assert_eq!(regs.0,0); event("present");
        assert_eq!((surface.resource,surface.pitch,surface.block_height,surface.format,surface.dma),
            (u32::MAX,surface.width*4,0,0,1));
        assert!(surface.height>0);
        if fail(1) { return Err("presentation failed"); }
        let head=0; let receipt=present::Receipt;
        // NATIVE_TAIL
        Ok(receipt)
    }
}
mod display_evidence {
    use super::*;
    pub fn record(_: &mut Registers, _:u32, _:present::Surface, _:&present::Receipt) {
        assert!(locked());
        for i in 0..10 { let mut row=[0;15]; row[0]=i; presentation_trace::record(row); }
        event("record");
    }
    pub fn transition(_: &mut Registers, _:u32, _:present::Surface, _:&present::Receipt,
        _:[u64;2], _:[u64;2], _: &str)->Result<(),&'static str> {
        assert!(locked()); event("transition");
        if fail(2) { Err("transition failed") } else { Ok(()) }
    }
}
// MODE
fn setup() {
    presentation_trace::dump();
    *SESSION.0.lock().unwrap()=Some(Session { pending_present:Some(present::Surface {resource:7,address:100,
        width:1280,height:800,pitch:5120,block_height:0,format:0,dma:1}),ready_present:None,
        evo:native::Evo{head:0},registers:0,console:Console{vram:100,bytes:100},
        layout:Layout{objects:vec![Object{name:"logical-low",range:Range{start:100,size:100}}]},
        mode:1,scanout:vec![7] });
    EVENTS.with(|e|e.borrow_mut().clear());
}
#[test] fn handback() {
    for fault in [0,1,2] {
        for index in [0,1] {
            setup(); FAIL.with(|f|f.set(fault));
            assert_eq!(set_mode(index).is_err(),fault!=0);
            let events=EVENTS.with(|e|e.borrow().clone());
            assert_eq!(&events[..3],["lock","drain","present"]);
            let unlock=events.iter().position(|e|e=="unlock").unwrap();
            if fault!=1 {
                let transition=events.iter().position(|e|e=="transition").unwrap();
                assert!(transition<unlock);
                let exports=&events[unlock+1..];
                assert!(exports[0].starts_with("TIMER_WAKE requests=1 "),"wakeup stats were not snapshotted under SESSION");
                assert_eq!(exports.iter().filter(|e|e.starts_with("GT710_PRESENT_EDGE ")).count(),8);
                assert!(exports.iter().any(|e|e.contains("seq=10 resource=9")));
                assert!(!exports.iter().any(|e|e.contains("resource=999")));
                EVENTS.with(|e|e.borrow_mut().clear());
                presentation_trace::dump();
                let next=EVENTS.with(|e|e.borrow().clone());
                assert!(next.iter().any(|e|e.contains("count=1 ")));
                assert!(next.iter().any(|e|e.contains("seq=1 resource=999")));
            } else { assert_eq!(events.last().unwrap(),"unlock"); }
            let owner=SESSION.0.lock().unwrap(); let s=owner.as_ref().unwrap();
            if fault==0 { assert_eq!(s.mode,index); assert!(s.scanout.is_empty()); }
            else { assert_eq!(s.mode,1); assert_eq!(s.scanout,[7]); }
        }
    }
    setup(); assert!(set_mode(2).is_err()); assert!(EVENTS.with(|e|e.borrow().is_empty()));
    *SESSION.0.lock().unwrap()=None; assert!(set_mode(0).is_err());
}
