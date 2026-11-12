use std::cell::RefCell;
type CaseResult = Result<(), String>;
#[derive(Default)]
struct State { clock:u64, fail:&'static str, case:&'static str, killed:bool, output:Vec<u8> }
thread_local! {static STATE:RefCell<State> = RefCell::new(State::default());}
fn now()->u64 {STATE.with(|s|s.borrow().clock)}
fn advance(ns:u64){STATE.with(|s|s.borrow_mut().clock+=ns);}
fn failing(phase:&str)->bool {STATE.with(|s|s.borrow().fail==phase)}
fn pause(){advance(5_000_000);}
fn case_ms_left()->u64 {10000}
mod time {pub fn sleep_ms(ms:u64)->Result<(),String>{super::advance(ms*1_000_000);Ok(())}}
mod signal {
 pub const SIGKILL:i32=9;
 pub fn kill(_:i32,_:i32)->Result<(),String>{super::STATE.with(|s|s.borrow_mut().killed=true);Ok(())}
}
mod breenix_suite {pub mod suite {
 pub fn running_case()->Option<&'static str>{Some(crate::STATE.with(|s|s.borrow().case))}
}}
mod guest_fs {
 pub struct OpenOptions;
 impl OpenOptions {
  pub fn new()->Self{Self}
  pub fn write(&mut self,_:bool)->&mut Self{self}
  pub fn open(&self,path:&str)->Result<Sink,String>{assert_eq!(path,"/dev/screen-evidence");Ok(Sink)}
 }
 pub struct Sink;
 impl std::io::Write for Sink {
  fn write(&mut self,b:&[u8])->std::io::Result<usize>{
   // Reporting consumes time on a UART. It must not inflate the interval it reports.
   crate::STATE.with(|s|s.borrow_mut().output.extend_from_slice(b));
   crate::advance(100_000_000);Ok(b.len())
  }
  fn flush(&mut self)->std::io::Result<()>{Ok(())}
 }
}
mod abi {
 pub mod event_kind {pub const POINTER_BUTTON:u16=1;}
 #[derive(Clone,Copy)]pub struct WindowEvent {pub kind:u16,pub pressed:u32,pub button:u32,pub serial:u64}
 pub struct WinInfo {pub x:i32,pub y:i32}
}
struct Child {pid:i32,reap_ns:u64}
impl Child {
 fn signal(&self,sig:i32)->CaseResult {signal::kill(self.pid,sig)}
 fn status(&mut self)->Result<Option<i32>,String>{advance(6_125_000);Ok((!failing("reap")).then_some(signal::SIGKILL))}
 STOP
}
struct Client {events:Vec<abi::WindowEvent>}
impl Client {
 fn start(_:u32)->Result<Self,String>{Ok(Self{events:vec![]})}
 fn info(&self)->Result<abi::WinInfo,String>{Ok(abi::WinInfo{x:10,y:10})}
 fn clear(&mut self)->CaseResult{self.events.clear();Ok(())}
 fn snapshot(&mut self)->CaseResult {
  advance(12_345_000);
  if !failing("click") {self.events=vec![abi::WindowEvent{kind:1,pressed:1,button:1,serial:1},abi::WindowEvent{kind:1,pressed:0,button:1,serial:2}];}
  Ok(())
 }
 AWAIT
}
struct Desktop {wm:Child}
impl Desktop {
 fn launch(_:bool)->Result<Self,String>{Ok(Self{wm:Child{pid:42,reap_ns:0}})}
 fn shown(&mut self,_:&Client)->CaseResult{Ok(())}
 fn pointer(&self,_:i32,_:i32,_:u32,_:i32)->CaseResult{Ok(())}
 FOCUS
}
fn children(_:i32,_:&str)->Result<Vec<i32>,String> {
 if STATE.with(|s|s.borrow().killed) {advance(125_750_000);Ok(vec![if failing("supervisor"){7}else{8}])} else {Ok(vec![7])}
}
fn content(_:&mut Desktop,_:&Client)->CaseResult {
 advance(386_787_000);
 if failing("composite"){Err("first composite failed".into())}else{Ok(())}
}
REPORT
DIAGNOSTIC
UNTIL
SUPERVISOR
fn reset(case:&'static str,fail:&'static str){STATE.with(|s|*s.borrow_mut()=State{case,fail,..State::default()});assert_eq!(breenix_suite::suite::running_case(),Some(case));}
fn rows()->Vec<Vec<String>> {
 STATE.with(|s| {
  let s=s.borrow();assert!(s.output.len()<=2048,"{} bytes exceeds per-case bound",s.output.len());
  String::from_utf8(s.output.clone()).unwrap().lines().filter(|l|l.starts_with("VALUE wm-elapsed ")).map(|l|l.split_whitespace().map(str::to_string).collect()).collect()
 })
}
fn expected(case:&str,wait:&str,ms:&str,deadline:&str)->Vec<String>{["VALUE","wm-elapsed",case,wait,ms,deadline].map(str::to_string).to_vec()}
#[test]fn supervisor_reports_pass_and_failure(){
 for fail in ["","supervisor","composite"] {
  reset("recovery/supervisor-restarts",fail);
  assert_eq!(supervisor_restarts().is_ok(),fail.is_empty());
  let r=rows();assert_eq!(r.len(),if fail=="supervisor"{1}else{2},"{r:?}");
  assert_eq!(r[0][2],"recovery/supervisor-restarts");assert_eq!(r[0][3],"supervisor-restart");assert_eq!(r[0][5],"2000.000");
  if fail!="supervisor" {
   assert_eq!(r[0][4],"125.750");
   assert_eq!(r[1],expected("recovery/supervisor-restarts","first-composite","386.787","2000.000"));
  }else{assert!(r[0][4].parse::<f64>().unwrap()>=2000.0);}
 }
}
#[test]fn positive_click_reports_pass_and_failure(){
 for fail in ["","click"] {
  reset("pointer/press-order",fail);
  let mut d=Desktop::launch(false).unwrap();let mut c=Client::start(11).unwrap();
  assert_eq!(d.focus(&mut c).is_ok(),fail.is_empty());
  let r=rows();assert_eq!(r.len(),if fail.is_empty(){2}else{1},"{r:?}");
  assert_eq!(r[0][3],"positive-click");assert_eq!(r[0][5],"1000.000");
  if fail.is_empty(){
   assert_eq!(r[0][4],"12.345");
   assert_eq!(r[1],expected("pointer/press-order","click-release","12.345","1000.000"));
   // Failure diagnostics share the case budget with compact timing reports.
   for _ in 0..100 {failure_diagnostic("readback",&"x".repeat(1000));}
   rows();
  }else{assert!(r[0][4].parse::<f64>().unwrap()>=1000.0);}
 }
}
#[test]fn reap_reports_pass_and_failure(){
 for fail in ["","reap"] {
  reset("recovery/restart-adopts",fail);
  let mut child=Child{pid:42,reap_ns:0};assert_eq!(child.stop().is_ok(),fail.is_empty());
  let r=rows();assert_eq!(r.len(),1,"{r:?}");assert_eq!(r[0][3],"reap");assert_eq!(r[0][5],"500.000");
  if fail.is_empty(){assert_eq!(r[0][4],"6.125");}else{assert!(r[0][4].parse::<f64>().unwrap()>=500.0);}
 }
}
