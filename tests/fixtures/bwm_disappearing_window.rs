use std::cell::RefCell;
pub mod wabi {
 pub mod errno {pub const EPERM:i32=1;pub const ENOENT:i32=2;pub const EAGAIN:i32=11;pub const EBUSY:i32=16;pub const EINVAL:i32=22;pub const EIO:i32=5;}
 #[derive(Clone,Copy,Default)]pub struct SceneSnapshot {pub context:u64,pub width:u32,pub height:u32,pub window_count:u32,pub damage_count:u32,pub windows:u64,pub window_capacity:u32,pub damage:u64,pub damage_capacity:u32,pub flags:u32,pub snapshot:u64}
 impl SceneSnapshot {pub const ZERO:Self=Self{context:0,width:0,height:0,window_count:0,damage_count:0,windows:0,window_capacity:0,damage:0,damage_capacity:0,flags:0,snapshot:0};}
 #[derive(Clone,Copy,Default)]pub struct SceneWindow {pub id:u64,pub sample:u64,pub commit_id:u64,pub width:u32,pub height:u32}
 impl SceneWindow {pub const ZERO:Self=Self{id:0,sample:0,commit_id:0,width:0,height:0};}
 #[derive(Clone,Copy)]pub struct Rect;impl Rect {pub const ZERO:Self=Self;}
 pub struct Composite {pub snapshot:u64,pub target:u64,pub stream:u64,pub length:u64,pub scene_target:u64,pub serial:u64,pub reason:u32,pub phase:u32,pub error:u32,pub error_index:u32}
 impl Composite {pub const ZERO:Self=Self{snapshot:0,target:0,stream:0,length:0,scene_target:0,serial:0,reason:0,phase:0,error:0,error_index:0};}
}
mod op {pub const COMPOSITE_SNAPSHOT:u32=29;pub const COMPOSITE:u32=19;}
mod cmd {pub const SCISSOR:u32=99;}
mod abi {pub mod pipeline {pub const SOLID:u32=1;}}
const RETRY_MS:u64=2000;const CURSOR_WHITE:u32=0xffffff;const CURSOR_BLACK:u32=0;
#[derive(Clone,Copy)]pub struct Pointer;
pub struct Layer {pub window_id:u64,pub content:[i32;4],pub frame:[[i32;4];1],pub frame_len:usize}
#[derive(Debug)]enum FrameError {NotOwner,Failed(String)}
pub struct Sampled {pub id:u64,pub sample:u64,pub commit_id:u64,pub width:u32,pub height:u32}
#[derive(Default)]pub struct Timing {pub presented_ns:u64,pub prepare_ns:u64}
mod session {pub fn timing(_:u64)->Result<super::Timing,()>{Ok(super::Timing::default())}}
mod libuitest {pub mod startup {pub fn mark(_:&str){}pub fn at(_:&str,_:u64){}}}
mod libbreenix {pub mod time {pub fn sleep_ms(_:u64)->Result<(),()>{Ok(())}}}
fn monotonic_ns()->u64{0}fn monotonic_ms()->u64{0}
struct Desktop;impl Desktop {fn wait(&self)->Result<(),i64>{Ok(())}fn handle(&self)->u64{1}}
struct Workspace;impl Workspace {fn prepare_clients(&mut self,_:&[Sampled]){}}
fn pointer_rects(_:Pointer,_:i32)->(Vec<[i32;4]>,Vec<[i32;4]>){(vec![],vec![])}
mod encode {
 pub fn begin(_: &mut Vec<u32>,_:u64){}
 pub fn copy(out:&mut Vec<u32>,source:u64,_:[u32;4],_:u64,_:[u32;2]){out.extend([0xfeee,source as u32]);}
 pub fn draw(_:&mut Vec<u32>,_:u32,_:u32,_:&[[i32;4]]){}
}
#[derive(Default)]struct State {alive:bool,first_error:i64,snapshots:usize,submitted:Vec<Vec<u32>>}
thread_local! {static STATE:RefCell<State>=RefCell::new(State::default());}
fn call<T>(operation:u32,value:&mut T)->i64 {
 STATE.with(|cell| {let mut state=cell.borrow_mut();match operation {
  op::COMPOSITE_SNAPSHOT=>{
   let s=unsafe{&mut *(value as *mut T).cast::<wabi::SceneSnapshot>()};assert_eq!(s.context,41);
   s.width=128;s.height=80;s.window_count=u32::from(state.alive);s.damage_count=1;
   if s.window_capacity!=0 || s.damage_capacity!=0 {
    state.snapshots+=1;s.snapshot=state.snapshots as u64;
    if state.alive {unsafe{*(s.windows as *mut wabi::SceneWindow)=wabi::SceneWindow{id:7,sample:77,commit_id:9,width:10,height:10};}}
   }
   0
  },
  op::COMPOSITE=>{
   let c=unsafe{&mut *(value as *mut T).cast::<wabi::Composite>()};
   assert_eq!(c.snapshot,state.snapshots as u64);
   let words=unsafe{std::slice::from_raw_parts(c.stream as *const u32,c.length as usize/4)};
   state.submitted.push(words.to_vec());
   if state.submitted.len()==1 {state.alive=false;state.first_error} else {c.serial=42;0}
  },_=>panic!("unexpected operation")
 }})
}
struct Compositor {desktop:Desktop,context:u64,width:u32,height:u32,full:bool,scanout:[u64;2],next:usize,scene:u64,last_timing:Timing,last_serial:u64,sampled:Vec<Sampled>,check_workspace:Workspace,appbar_top:i32,scale:i32}
impl Compositor {
 fn new()->Self{Self{desktop:Desktop,context:41,width:128,height:80,full:true,scanout:[4,5],next:0,scene:3,last_timing:Timing::default(),last_serial:0,sampled:vec![],check_workspace:Workspace,appbar_top:70,scale:1}}
 fn damage_rects(&self,_:bool,_:&[[i32;4]],_:&[wabi::Rect])->Vec<[i32;4]>{vec![[0,0,128,80]]}
 // COMPOSE_METHODS
}
// CONTENT_RECT
fn layer()->Layer{Layer{window_id:7,content:[0,0,10,10],frame:[[0;4]],frame_len:0}}
fn setup(error:i64){STATE.with(|s|*s.borrow_mut()=State{alive:true,first_error:error,..State::default()});}
#[test]fn destroyed_source_retries_without_reopening(){
 setup(-2);let mut gpu=Compositor::new();
 let result=gpu.compose(&[layer()],None,&[]);
 assert!(result.is_ok(),"destroyed source would enter GPU reopen/exit recovery: {result:?}");
 STATE.with(|s|{let s=s.borrow();assert_eq!(s.snapshots,2);assert_eq!(s.submitted.len(),2);assert!(s.submitted[0].windows(2).any(|w|w==[0xfeee,77]));assert!(!s.submitted[1].windows(2).any(|w|w==[0xfeee,77]),"removed window was still sampled");});
 assert_eq!(gpu.context,41);assert_eq!(gpu.last_serial,42);assert!(gpu.sampled.is_empty());assert_eq!(gpu.next,1);
 // A real GPU error keeps the failure/reopen route and is not silently retried.
 setup(-5);let mut gpu=Compositor::new();
 match gpu.compose(&[layer()],None,&[]){Err(FrameError::Failed(error))=>assert!(error.contains("EIO")),other=>panic!("GPU error changed: {other:?}")}
 STATE.with(|s|assert_eq!(s.borrow().submitted.len(),1));
}
