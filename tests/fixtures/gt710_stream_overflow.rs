extern crate alloc;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU64, Ordering};
use gpu_abi::{self as abi, errno::{EINVAL, ENOMEM, EDQUOT}, format, limits, pipeline,
    stream::{self, Resolver, TargetInfo, SourceInfo}};
#[allow(dead_code)] // The other committed shader assets have kernel consumers.
#[path = "ROOT/kernel/src/gpu/gk208/gr/shaders.rs"] mod shaders;
#[path = "ROOT/kernel/src/gpu/gk208/encode.rs"] pub mod native;
#[path = "ROOT/kernel/src/gpu/gk208/caps.rs"] pub mod caps;
mod gk208 { pub use crate::native as encode; }
struct Words<'a>(&'a [u32]);
impl stream::Words for Words<'_> {
    fn len(&self)->usize { self.0.len() }
    fn word(&self,i:usize)->u32 { self.0[i] }
}
const MEMORY_QUOTA:u64 = 192 << 20;
static GPU_RESOURCE_BYTES: () = ();
fn gauge(_: &(), _: i64) {}
type Quota = Arc<Counts>;
// QUOTA
thread_local! { static QUOTA:Quota = Arc::new(Counts::default()); }
struct Charge(QuotaCharge);
impl Charge {
    fn new(_:u64, bytes:u64, copy:bool)->Result<Self,i64> {
        QUOTA.with(|q| QuotaCharge::try_new(q, bytes, copy).map(Self).ok_or(ENOMEM))
    }
    fn bytes(&self)->u64 { self.0.bytes }
}
struct Point;
impl Point { fn fire(self) {} }
#[derive(Clone,Debug,PartialEq)]
struct Picture { generation:u64, bytes:Vec<u8> }
struct View;
impl Resolver for View {
    fn target(&self,h:u64)->Option<TargetInfo> {
        (h==1 || h==2).then_some(TargetInfo {width:16,height:16,format:format::B8G8R8A8,invalid:false})
    }
    fn source(&self,_:u64)->Option<SourceInfo> { None }
    fn buffer_size(&self,h:u64)->Option<u64> { match h { 10=>Some(33<<20),11=>Some(32<<20),_=>None } }
    fn buffer_rect(&self,_:u64,_:u64)->[i32;4] { [0,0,16,16] }
}
impl native::Bindings for View {
    fn render_target(&self,h:u64)->Option<native::Target> {
        self.target(h).map(|t| native::Target { id:h as u32,address:h<<24,
            width:t.width,height:t.height,format:t.format,tile_mode:0x30 })
    }
    fn sampled_source(&self,_:u64)->Option<native::Target> { None }
}
struct Ring { frames:Vec<(native::Frame,Arc<Charge>)>, allocations:u32, pictures:[Picture;2] }
impl Ring {
    fn new()->Self { Self { frames:vec![],allocations:0,pictures:core::array::from_fn(|i|
        Picture {generation:17+i as u64,bytes:vec![0x55+i as u8;16*16*4]}) } }
    fn submit(&mut self,words:&[u32],a:&mut abi::Submit)->Result<u64,i64> {
        let words=Words(words);
        let validation=Words(words.0);
        let resolved=View;
        let injection:Option<(usize,u32,Point)>=None;
        let refuse=|a:&mut abi::Submit,index,error,errno| { a.error_index=index;a.error=error;Err(errno) };
        // VALIDATION
        assert_eq!(native::pipeline_refusal(&words),None);
        let corrupt:Option<u64>=None;
        let tgid=1;
        // RESERVATION
        assert!(charge.bytes()>0,"native backing must remain charged");
        self.allocations+=1;
        let frame=native::translate(&words,&resolved).unwrap();
        let emitted=frame.data.len() as u64+frame.chunks.iter().map(|c|
            (c.words.len()*4) as u64).sum::<u64>();
        assert!(charge.bytes()>=emitted,"reservation must cover native payload and methods");
        // Publication is the host boundary: even a clear in a valid prefix
        // changes both a generation and bytes if a refused stream reaches it.
        for p in &mut self.pictures { p.generation+=1;p.bytes.fill(0xcc); }
        self.frames.push((frame,charge));
        Ok(self.frames.len() as u64)
    }
}
fn submit_arg()->abi::Submit { unsafe { core::mem::zeroed() } }
fn held()->u64 { QUOTA.with(|q| q.held.load(Ordering::Acquire)) }
fn prefix()->Vec<u32> {
    let mut w=Vec::new();
    for t in [1,2] { stream::encode::begin(&mut w,t);stream::encode::clear(&mut w,0xff112233); }
    w
}
fn many(n:usize)->Vec<u32> {
    let mut w=Vec::with_capacity(9+4*n);stream::encode::begin(&mut w,1);
    w.extend([stream::cmd::DRAW,5+4*n as u32,pipeline::SOLID,0xffff0000,n as u32]);
    w.resize(9+4*n,0);w
}
#[test] fn accepted_draw_and_instance_boundaries_keep_real_charges() {
    assert_eq!(held(),0);
    let mut ring=Ring::new();let mut w=Vec::new();stream::encode::begin(&mut w,1);
    for _ in 0..limits::DRAWS { stream::encode::draw(&mut w,pipeline::SOLID,0,&[]); }
    let stream_charge=Charge::new(1,(w.len() as u64*4).next_multiple_of(4096),true).unwrap();
    assert_eq!(ring.submit(&w,&mut submit_arg()),Ok(1),"65,536 zero-instance draws are valid");
    assert_eq!(ring.frames[0].0.chunks.len(),1);
    assert!(ring.frames[0].0.chunks[0].words.is_empty());
    assert!(held()>stream_charge.bytes());
    drop(stream_charge);
    let w=many(limits::INSTANCES as usize);
    let stream_charge=Charge::new(1,(w.len() as u64*4).next_multiple_of(4096),true).unwrap();
    assert_eq!(ring.submit(&w,&mut submit_arg()),Ok(2),"1,048,576 clipped instances are valid");
    drop(stream_charge);drop(ring);assert_eq!(held(),0);
}
#[test] fn each_over_limit_stream_refuses_before_native_allocation_or_ring_publication() {
    let mut draws=prefix();
    for _ in 2..limits::DRAWS { stream::encode::draw(&mut draws,pipeline::SOLID,0,&[]); }
    stream::encode::draw(&mut draws,pipeline::SOLID,0,&[[0,0,16,16]]);
    let mut instances=prefix();instances.extend_from_slice(&many(limits::INSTANCES as usize+1)[4..]);
    let mut buffers=prefix();
    for h in [10,11] { stream::encode::draw_buffer(&mut buffers,pipeline::SOLID,0,h,0,1); }
    for (w,error,index) in [(draws,stream::error::DRAW_LIMIT,limits::DRAWS as u32+2),
        (instances,stream::error::INSTANCE_LIMIT,4),(buffers,stream::error::BUFFER_LIMIT,5)] {
        let mut ring=Ring::new();let before=ring.pictures.clone();let mut a=submit_arg();
        let baseline=held();
        assert_eq!(ring.submit(&w,&mut a),Err(EINVAL));assert_eq!((a.error,a.error_index),(error,index));
        assert_eq!(ring.allocations,0);assert!(ring.frames.is_empty());
        assert_eq!(ring.pictures,before,"every target generation and every byte must survive refusal");
        assert_eq!(held(),baseline,"refusal must not retain a native charge");
        assert!(native::translate(w.as_slice(),&View).is_err(),"direct encoder must also refuse the complete stream");
    }
}
#[test] fn native_reservation_covers_draws_and_real_quota_exhaustion_still_refuses() {
    let mut ring=Ring::new();let mut w=prefix();
    stream::encode::draw(&mut w,pipeline::SOLID,0,&[[0,0,16,16];32]);
    assert_eq!(ring.submit(&w,&mut submit_arg()),Ok(1));
    drop(ring);assert_eq!(held(),0);
    let full=Charge::new(1,MEMORY_QUOTA,true).unwrap();let mut ring=Ring::new();
    let before=ring.pictures.clone();
    assert_eq!(ring.submit(&w,&mut submit_arg()),Err(ENOMEM));
    assert_eq!(ring.allocations,0);assert!(ring.frames.is_empty());assert_eq!(ring.pictures,before);
    drop(full);assert_eq!(held(),0);
}
