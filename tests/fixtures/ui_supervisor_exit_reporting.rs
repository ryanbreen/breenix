// Host syscall boundary for the production supervisor launch/collect/reap functions.
use std::collections::BTreeMap;
#[derive(Clone, Copy)] struct Fd(i32);
impl Fd { const STDIN: Self=Self(0); const STDOUT: Self=Self(1); const STDERR: Self=Self(2); }
#[derive(Debug, PartialEq)] enum Errno { EINTR, EAGAIN, ECHILD, Other }
#[derive(Debug)] enum Error { Os(Errno) }
impl std::fmt::Display for Error { fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result { write!(f,"{self:?}") } }
mod native {
    extern "C" {
        pub fn pipe(f:*mut i32)->i32; pub fn fcntl(f:i32,cmd:i32,...)->i32;
        pub fn read(f:i32,b:*mut u8,n:usize)->isize; pub fn write(f:i32,b:*const u8,n:usize)->isize;
        pub fn close(f:i32)->i32; pub fn open(p:*const u8,f:i32,...)->i32;
        pub fn fork()->i32; pub fn dup2(a:i32,b:i32)->i32; pub fn _exit(s:i32)->!;
        pub fn execve(p:*const u8,a:*const *const u8,e:*const *const u8)->i32;
        pub fn waitpid(p:i32,s:*mut i32,o:i32)->i32;
    }
}
fn error()->Error { Error::Os(match std::io::Error::last_os_error().raw_os_error().unwrap() {4=>Errno::EINTR,35=>Errno::EAGAIN,10=>Errno::ECHILD,_=>Errno::Other}) }
mod io {
    use super::*;
    pub mod status_flags {pub const O_CLOEXEC:i32=0x1000000;pub const O_NONBLOCK:i32=4;}
    pub fn pipe2(_:i32)->Result<(Fd,Fd),Error>{let mut f=[0;2];if unsafe{native::pipe(f.as_mut_ptr())}<0{return Err(error())}for fd in f {unsafe{native::fcntl(fd,2,1);}} Ok((Fd(f[0]),Fd(f[1])))}
    pub fn fcntl_setfl(f:Fd,v:i32)->Result<(),Error>{if unsafe{native::fcntl(f.0,4,v)}<0{Err(error())}else{Ok(())}}
    pub fn close(f:Fd)->Result<(),Error>{unsafe{native::close(f.0);}Ok(())}
    pub fn read(f:Fd,b:&mut[u8])->Result<usize,Error>{let n=unsafe{native::read(f.0,b.as_mut_ptr(),b.len())};if n<0{Err(error())}else{Ok(n as usize)}}
    pub fn write(f:Fd,b:&[u8])->Result<usize,Error>{let n=unsafe{native::write(f.0,b.as_ptr(),b.len())};if n<0{Err(error())}else{Ok(n as usize)}}
}
mod fs {use super::*;pub const O_RDWR:i32=2;pub fn open(p:&str,f:i32)->Result<Fd,Error>{let p=std::ffi::CString::new(p).unwrap();let fd=unsafe{native::open(p.as_ptr().cast(),f)};if fd<0{Err(error())}else{Ok(Fd(fd))}}}
#[derive(Clone,Copy)] struct Pid(i32); impl Pid {fn raw(self)->u64{self.0 as u64}}
mod process {use super::*;pub const WNOHANG:i32=1;
 pub fn execve(p:&[u8],a:*const *const u8,e:*const *const u8)->Result<(),Error>{unsafe{native::execve(p.as_ptr(),a,e);}Err(error())}
 pub fn waitpid(p:i32,s:&mut i32,o:i32)->Result<Pid,Error>{let n=unsafe{native::waitpid(p,s,o)};if n<0{Err(error())}else{Ok(Pid(n))}}
}
enum Run<'a>{Call(&'a mut dyn FnMut()->i32)}
fn spawn(fds:&[(Fd,Fd)],run:Run<'_>)->Result<Pid,Error>{let pid=unsafe{native::fork()};if pid<0{return Err(error())}if pid==0{for (a,b) in fds{unsafe{native::dup2(a.0,b.0);}}let Run::Call(f)=run;unsafe{native::_exit(f())}}Ok(Pid(pid))}
struct Display; enum Event {Died,Recover} struct Ownership;
impl Ownership {fn apply(&mut self,_:Event,_:&mut Display)->Result<(),String>{Ok(())}}
#[derive(Debug)] enum Message {Output{pid:u64,bytes:Vec<u8>},Exited{pid:u64,status:i32}}
#[derive(Default)] struct Channel {exits:Vec<(u64,i32)>,bytes:usize}
impl Channel {fn queue(&mut self,m:&Message)->Result<(),String>{match m{Message::Output{pid,bytes}=>{assert!(*pid>0);self.bytes+=bytes.len();},Message::Exited{pid,status}=>self.exits.push((*pid,*status))}Ok(())}}
// The four fixture launches pass uid 0; Breenix-only credential calls must not run.
#[export_name="setresuid"] extern "C" fn host_setresuid(_:u32,_:u32,_:u32)->i32{panic!("unexpected credential switch")}
#[export_name="setresgid"] extern "C" fn host_setresgid(_:u32,_:u32,_:u32)->i32{panic!("unexpected credential switch")}
#[export_name="getresuid"] extern "C" fn host_getresuid(_:*mut u32,_:*mut u32,_:*mut u32)->i32{panic!("unexpected credential query")}
#[export_name="getresgid"] extern "C" fn host_getresgid(_:*mut u32,_:*mut u32,_:*mut u32)->i32{panic!("unexpected credential query")}
// PRODUCTION_RUNTIME
fn main(){
 for (script,expected,connected) in [
   ("printf 'child-stderr-cause\\n' >&2; printf 'child-stdout-tail\\n'; exit 1",256,true),
   ("printf successful-child >&2; exit 0",0,true),
   ("printf signal-cause >&2; kill -TERM $$",15,false),
   ("printf discarded-prefix; printf discarded-prefix >&2; i=0; while [ $i -lt 5000 ]; do printf x; printf y >&2; i=$((i+1)); done; printf child-stdout-tail; printf child-stderr-cause >&2; exit 1",256,false),
 ] {
   let (pid,output)=launch(&[b"/bin/sh".to_vec(),b"-c".to_vec(),script.as_bytes().to_vec()],&[],0,false).unwrap();
   let mut outputs_state=BTreeMap::from([(pid,output)]);let mut channel=Channel::default();let mut ownership=Ownership;
   let end=std::time::Instant::now()+std::time::Duration::from_secs(10);
   loop {
     outputs(&mut outputs_state,&mut channel,connected).unwrap();
     let empty=reap(&mut channel,&mut outputs_state,connected,None,&mut ownership,false).unwrap();
     outputs(&mut outputs_state,&mut channel,connected).unwrap();
     if empty && outputs_state.values().all(|o|o.stdout.is_none()&&o.stderr.is_none()){break}
     assert!(std::time::Instant::now()<end);std::thread::sleep(std::time::Duration::from_millis(1));
   }
   assert_eq!(outputs_state[&pid].status,Some(expected));
   assert_eq!(channel.exits,if connected{vec![(pid,expected)]}else{vec![]});
 }
}
