extern crate self as libbreenix;
use std::sync::Mutex;
static STATE: Mutex<State> = Mutex::new(State { now: 0, opened: false, batches: Vec::new(), mouse: (0,0,0) });
struct State { now: u64, opened: bool, batches: Vec<(u64, Vec<input::Event>)>, mouse: (u32,u32,u32) }
fn case_ms_left() -> u64 { 10000u64.saturating_sub(STATE.lock().unwrap().now / 1000000) }
fn monotonic_ns() -> Option<i128> { Some(i128::from(STATE.lock().unwrap().now)) }
pub mod input {
    pub mod kind { pub const SYN:u16=0; pub const KEY:u16=1; pub const REL:u16=2; pub const ABS:u16=3; }
    pub mod syn { pub const REPORT:u16=0; pub const DROPPED:u16=3; }
    pub mod axis { pub const X:u16=0; pub const Y:u16=1; pub const WHEEL:u16=8; }
    #[derive(Clone, Copy)] pub struct Event { pub kind:u16, pub code:u16, pub value:i32, pub device:u32, pub time_ns:u64 }
    #[derive(Default,Clone,Copy)] pub struct Capability { pub kind:u16,pub code:u16,pub minimum:i32,pub maximum:i32 }
    pub struct Record { pub event:Event }
    pub struct Fd;
    impl Fd { pub fn raw(&self) -> u64 { 3 } }
    pub struct Reader;
    impl Reader {
        pub fn open(nonblocking:bool) -> Result<Self,String> {
            assert!(nonblocking); let mut s=super::STATE.lock().unwrap();
            assert!(!s.opened); s.opened=true; s.batches.clear(); Ok(Self)
        }
        pub fn fd(&self) -> Fd { Fd }
        pub fn capabilities(&self,_: u32,caps:&mut [Capability])->Result<usize,String> { caps[0]=Capability{kind:3,code:0,minimum:0,maximum:32767}; caps[1]=Capability{kind:3,code:1,minimum:0,maximum:32767}; Ok(2) }
        pub fn read(&self, _: &mut [u8]) -> Result<Vec<Result<Record,String>>,String> {
            let mut s=super::STATE.lock().unwrap();
            assert!(s.now>=s.batches[0].0);
            Ok(s.batches.remove(0).1.into_iter().map(|event|Ok(Record{event})).collect())
        }
    }
}
pub mod io {
    pub struct PollFd { pub fd:i32, pub events:i16, pub revents:i16 }
    pub fn poll(p:&mut [PollFd], ms:i32) -> Result<usize,String> {
        assert_eq!((p[0].fd,p[0].events),(3,1));
        let mut s=super::STATE.lock().unwrap(); s.now+=ms as u64*1000000;
        p[0].revents=i16::from(s.batches.first().is_some_and(|b|b.0<=s.now));
        Ok(p[0].revents as usize)
    }
}
pub mod graphics {
    pub fn mouse_state() -> Result<(u32,u32,u32),String> { Ok(super::STATE.lock().unwrap().mouse) }
}
#[path="HOST_INPUT_PATH"] mod host_input;
fn reset() { *STATE.lock().unwrap()=State {now:0,opened:false,batches:Vec::new(),mouse:(2520,1500,0)}; }
fn event(kind:u16, code:u16, value:i32, time_ns:u64) -> input::Event { input::Event {kind,code,value,device:7,time_ns} }
fn request(batches: Vec<(u64,Vec<input::Event>)>) -> impl FnOnce()->Result<u64,String> {
    move || { let mut s=STATE.lock().unwrap(); assert!(s.opened,"observe before emit"); s.batches=batches; Ok(1) }
}
#[test] fn native_keys_require_every_press_and_release_and_return_delivery_time() {
    reset();
    let mut events=Vec::new();
    // Include an orphan release and repeated e: neither can stand in for a chord.
    events.push(event(1,35,0,1));
    for code in [35,18,18] { events.push(event(1,code,1,2150000000)); events.push(event(1,code,0,2150000000)); }
    assert_eq!(host_input::keys("shift-h,e,e",request(vec![(2650000000,events)])),Ok(2150000000));
    assert_eq!(STATE.lock().unwrap().now,2650000000);
    reset();
    assert!(host_input::keys("h,e",request(vec![(1,vec![event(1,35,1,1),event(1,35,0,1),event(1,18,1,1)])])).unwrap_err().contains("deadline"));
}
#[test] fn native_park_waits_for_a_new_complete_motion_batch() {
    reset();
    let partial=vec![event(3,0,1,150000000),event(3,1,1,150000000),event(0,0,0,150000000)];
    let complete=vec![event(3,0,(2520*32767+2558)/2559,202000000),event(3,1,(1500*32767+1598)/1599,202000000),event(0,0,0,202000000)];
    assert_eq!(host_input::pointer(2520,1500,2560,1600,0,0,request(vec![(150000000,partial),(202000000,complete)])),Ok(202000000));
    reset();
    let complete=vec![event(3,0,1,1),event(3,1,1,1),event(0,0,0,1)];
    assert!(host_input::pointer(1,2,2560,1600,0,0,request(vec![(1,complete)])).unwrap_err().contains("deadline"));
}
#[test] fn observer_loss_and_invalid_timestamps_fail() {
    reset();
    assert!(host_input::keys("h",request(vec![(1,vec![event(0,3,0,1)])])).unwrap_err().contains("overflowed"));
    reset();
    assert!(host_input::keys("h",request(vec![(1,vec![event(1,35,1,1),event(1,35,0,9999999999)])])).unwrap_err().contains("timestamp"));
}
