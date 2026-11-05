//! Execute the production replay descriptor with only authority and the driver replaced.
extern crate alloc;
extern crate self as spin;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
static LOCK_DEPTH:AtomicU32=AtomicU32::new(0);
#[derive(Debug)]
pub struct Mutex<T>(std::sync::Mutex<T>);
impl<T> Mutex<T> {
    pub const fn new(value:T)->Self { Self(std::sync::Mutex::new(value)) }
    pub fn lock(&self)->Guard<'_,T> {
        let guard=self.0.lock().unwrap();
        LOCK_DEPTH.fetch_add(1,Ordering::Relaxed);
        Guard(guard)
    }
}
pub struct Guard<'a,T>(std::sync::MutexGuard<'a,T>);
impl<T> std::ops::Deref for Guard<'_,T> { type Target=T;fn deref(&self)->&T { &self.0 } }
impl<T> std::ops::DerefMut for Guard<'_,T> { fn deref_mut(&mut self)->&mut T { &mut self.0 } }
impl<T> Drop for Guard<'_,T> { fn drop(&mut self) { LOCK_DEPTH.fetch_sub(1,Ordering::Relaxed); } }
#[path = "../kernel/src/input/control.rs"] mod control;
mod pointer { pub fn screen() -> Option<(u32,u32)> { Some((2560,1600)) } }
mod test_control {
    pub static ENABLED: super::AtomicBool = super::AtomicBool::new(false);
    pub fn enabled() -> bool { ENABLED.load(super::Ordering::Relaxed) }
}
mod window {
    pub static ROOT: super::AtomicBool = super::AtomicBool::new(true);
    pub struct Caller;
    impl Caller {
        pub fn current() -> Result<Self,i32> { Ok(Self) }
        pub fn is_root(&self) -> bool { ROOT.load(super::Ordering::Relaxed) }
    }
}
mod syscall {
    pub mod errno { pub const EPERM:i32=1; pub const ENODEV:i32=19; pub const EINVAL:i32=22; pub const ENOSYS:i32=38; }
    pub mod fs {
        pub const S_IFREG:u32=0o100000;
        #[derive(Default)] pub struct Stat { pub st_mode:u32, pub st_nlink:u64, pub st_blksize:i64 }
    }
    pub mod userptr {
        pub fn write_user_bytes(out:u64, source:*const u8, n:usize) -> Result<(),u64> {
            unsafe { std::ptr::copy_nonoverlapping(source,out as *mut u8,n); } Ok(())
        }
    }
}
mod ipc { pub mod object {
    pub trait IpcFile: std::fmt::Debug + Send + Sync {
        fn as_any(&self) -> &dyn core::any::Any;
        fn fill_stat(&self, st:&mut crate::syscall::fs::Stat);
        fn statfs_magic(&self) -> u64;
        fn read(&self, buf:u64, count:usize) -> Result<u64,u64>;
        fn write(&self, bytes:&[u8], append:bool) -> Result<u64,u64>;
    }
} }
struct Driver;
static CALL: AtomicU32 = AtomicU32::new(0);
impl control::Replay for Driver {
    fn replay_keyboard(&self, suppress:bool) -> Result<control::Receipt,u64> {
        assert_eq!(LOCK_DEPTH.load(Ordering::Relaxed),0,"descriptor guard held across a blocking driver replay");
        CALL.store(if suppress {0x101} else {1},Ordering::Relaxed);
        Ok(control::Receipt {calls:if suppress {0} else {2},posted:4,ring:4})
    }
    fn replay_pointer(&self,x:i32,y:i32,suppress:bool) -> Result<control::Receipt,u64> {
        assert_eq!(LOCK_DEPTH.load(Ordering::Relaxed),0,"descriptor guard held across a blocking driver replay");
        assert_eq!((x,y),(2559,1599));
        CALL.store(if suppress {0x102} else {2},Ordering::Relaxed);
        Ok(control::Receipt {calls:if suppress {0} else {2},posted:4,ring:4})
    }
}
static DRIVER: Driver = Driver;
#[test]
fn descriptor_latches_authority_validates_and_dispatches_each_selector() {
    assert!(control::open("/proc/unrelated").unwrap().is_none());
    assert_eq!(control::open("/proc/breenix/input-replay").unwrap_err(),38);
    test_control::ENABLED.store(true,Ordering::Relaxed);
    let file=control::open("/proc/breenix/input-replay").unwrap().unwrap();
    let mut stat=syscall::fs::Stat::default(); file.fill_stat(&mut stat);
    assert_eq!(stat.st_mode,syscall::fs::S_IFREG|0o600);
    assert_eq!((stat.st_nlink,stat.st_blksize,file.statfs_magic()),(1,64,0x9fa0));
    assert!(file.as_any().is::<control::Control>());
    let mut request=[0u8;64]; request[0]=1; request[10]=4;
    assert_eq!(file.write(&request,false),Err(19));
    control::register(&DRIVER,false); control::register(&DRIVER,true);
    for which in [1u32,2,0x101,0x102] {
        request[..4].copy_from_slice(&which.to_le_bytes());
        request[16..20].copy_from_slice(&2559i32.to_le_bytes());
        request[20..24].copy_from_slice(&1599i32.to_le_bytes());
        assert_eq!(file.write(&request,false),Ok(64));
        assert_eq!(CALL.load(Ordering::Relaxed),which);
        let mut reply=[0u8;64];
        assert_eq!(file.read(reply.as_mut_ptr() as u64,17),Ok(17));
        assert_eq!(file.read(reply[17..].as_mut_ptr() as u64,47),Ok(47));
        assert_eq!(file.read(reply.as_mut_ptr() as u64,64),Ok(0));
        assert_eq!(&reply[..32],&request[..32]);
        assert_eq!(u32::from_le_bytes(reply[32..36].try_into().unwrap()),if which&0x100!=0 {0} else {2});
        assert_eq!(&reply[36..44],&[4,0,0,0,4,0,0,0]);
    }
    for at in [4,9,24,32,63] {
        let mut bad=request; bad[at]=1; assert_eq!(file.write(&bad,false),Err(22));
    }
    for which in [0u32,3,0x201,0x103] {
        let mut bad=request; bad[..4].copy_from_slice(&which.to_le_bytes()); assert_eq!(file.write(&bad,false),Err(22));
    }
    request[..4].copy_from_slice(&2u32.to_le_bytes());
    request[16..20].copy_from_slice(&2560i32.to_le_bytes());
    assert_eq!(file.write(&request,false),Err(22));
    assert_eq!(file.write(&request[..63],false),Err(22));
    window::ROOT.store(false,Ordering::Relaxed);
    assert_eq!(control::open("/proc/breenix/input-replay").unwrap_err(),1);
    assert_eq!(file.write(&request,false),Err(1));
    assert_eq!(file.read(0,64),Err(1));
    test_control::ENABLED.store(false,Ordering::Relaxed);
    assert_eq!(file.write(&request,false),Err(38));
    assert_eq!(file.read(0,64),Err(38));
}
