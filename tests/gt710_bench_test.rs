extern crate self as libbreenix;
#[path = "../userspace/programs/src/probe_gpu_bench.rs"]
mod probe_gpu_bench;

use std::cell::RefCell;
thread_local! { static STATE: RefCell<State> = RefCell::new(State::default()); }
#[derive(Default)]
struct State { images: Vec<(u32,u32,u32)>, submits: usize, waits: usize, clocks: usize,
    warming: bool, pending: bool, fail_wait: bool }

pub mod time {
    pub struct Stamp(u64);
    impl Stamp { pub fn as_nanos(&self)->u64 { self.0 } }
    pub fn now_monotonic()->Result<Stamp,&'static str> {
        Ok(crate::STATE.with_borrow_mut(|s| {s.clocks+=1;Stamp(s.clocks as u64*1000)}))
    }
    pub fn sleep_ms(_:u64)->Result<(), &'static str> { Ok(()) }
}
pub mod gpu {
    pub use gpu_abi as abi;
    use abi::Plain;
    use crate::STATE;
    pub fn context_create()->Result<u64,i64> {Ok(91)}
    pub fn context_destroy(c:u64)->i64 {assert_eq!(c,91);0}
    pub fn counters(c:u64)->(i64,abi::Counters) {
        assert_eq!(c,91);
        STATE.with_borrow_mut(|s| { let mut k=abi::Counters::ZERO;
            k.state=if s.warming {abi::context_state::LIVE} else {abi::context_state::WARMING};
            s.warming=true;(0,k) })
    }
    pub fn target_create(f:u32,w:u32,h:u32)->Result<u64,i64> {
        assert_eq!(f,abi::format::B8G8R8A8);
        STATE.with_borrow_mut(|s| {assert!(s.warming);s.images.push((f,w,h));Ok(s.images.len() as u64)})
    }
    pub fn resource_destroy(h:u64)->i64 { assert!((1..=3).contains(&h));0 }
    pub fn submit(c:u64,words:&[u32])->(i64,abi::Submit) {
        assert_eq!(c,91);
        STATE.with_borrow_mut(|s| {
            assert!(!s.pending,"previous benchmark was not completed");
            assert_eq!(s.clocks%2,1,"submission must be inside the timed interval");
            assert_eq!(s.images,vec![(0,2560,1600),(0,4096,4096),(0,4096,4096)]);
            struct View;
            impl abi::stream::Resolver for View {
                fn target(&self,h:u64)->Option<abi::stream::TargetInfo> {
                    let (width,height)=match h {1=>(2560,1600),2|3=>(4096,4096),_=>return None};
                    Some(abi::stream::TargetInfo {width,height,format:0,invalid:false})
                }
                fn source(&self,h:u64)->Option<abi::stream::SourceInfo> {
                    let t=self.target(h)?;Some(abi::stream::SourceInfo {width:t.width,height:t.height,format:t.format,invalid:false})
                }
                fn buffer_size(&self,_:u64)->Option<u64>{None}
                fn buffer_rect(&self,_:u64,_:u64)->[i32;4]{panic!("no CPU pixel/buffer input")}
            }
            let plan=abi::stream::validate(words,&View).expect("production validator");
            if s.submits<3 {
                assert_eq!(&words[..4],&[1,4,1,0]);
                assert_eq!(words.len(),4+200*9);
                for (i,d) in words[4..].chunks_exact(9).enumerate() {
                    assert_eq!(&d[..3],&[3,9,abi::pipeline::SOLID]);
                    assert_eq!(d[3],0xff000000|i as u32);
                    assert_eq!(&d[4..],&[1,0,0,2560,1600]);
                }
                assert_eq!(plan.drawn,200);assert_eq!(plan.estimate,819_200_000);
            } else {
                assert_eq!(words,&[5,12,2,0,0,0,4096,4096,3,0,0,0]);
                assert_eq!(plan.estimate,16_777_216);
            }
            s.submits+=1;s.pending=true;
            let mut a=abi::Submit::ZERO;a.fence=s.submits as u64;(0,a)
        })
    }
    pub fn wait(c:u64,f:u64,timeout:u64)->(i64,abi::Wait) {
        assert_eq!(c,91);assert!(timeout>0);
        STATE.with_borrow_mut(|s| { assert!(s.pending);assert_eq!(f,s.submits as u64);
            assert_eq!(s.clocks%2,1);s.pending=false;s.waits+=1;
            let mut a=abi::Wait::ZERO;a.status=abi::fence_status::OK;
            (if s.fail_wait {-5} else {0},a) })
    }
}

#[test]
fn six_trials_use_real_validated_streams_and_one_wait_each() {
    STATE.with_borrow_mut(|s|*s=State::default());
    probe_gpu_bench::run().unwrap();
    STATE.with_borrow(|s| {assert_eq!((s.submits,s.waits,s.clocks),(6,6,12));assert!(!s.pending)});
}

#[test]
fn failed_completion_aborts_without_a_successful_trial() {
    STATE.with_borrow_mut(|s|*s=State {fail_wait:true,..State::default()});
    assert!(probe_gpu_bench::run().is_err());
    STATE.with_borrow(|s|assert_eq!((s.submits,s.waits,s.clocks),(1,1,1)));
}
