extern crate self as libbreenix;
#[path = "../userspace/programs/src/probe_gpu_bench.rs"]
mod probe_gpu_bench;

use std::cell::RefCell;
thread_local! { static STATE: RefCell<State> = RefCell::new(State::default()); }
#[derive(Default)]
struct State { images: Vec<(u32,u32,u32)>, submits: usize, waits: usize, clocks: usize,
    warming: bool, pending: bool, fail_wait: bool, low_too_fast:bool, sets:Vec<u32>, thermal:bool, samples:Vec<(String,u32)>, idle_sleeps:u32, c3:bool }

pub mod time {
    pub struct Stamp(i128);
    impl Stamp { pub fn as_nanos(&self)->i128 { self.0 } }
    pub fn now_monotonic()->Result<Stamp,&'static str> {
        Ok(crate::STATE.with_borrow_mut(|s| {s.clocks+=1;
            let end=s.clocks%2==0;
            let duration=if s.submits>=10 && s.submits<=12 && !s.low_too_fast {1_070_000_000}else{716_000_000};
            Stamp(((s.clocks-1) as i128/2)*2_000_000_000+if end {duration}else{0})}))
    }
    pub fn sleep_ms(n:u64)->Result<(), &'static str> {
        if n==1000 {crate::STATE.with_borrow_mut(|s|s.idle_sleeps+=1);} Ok(())
    }
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
            if !s.c3 {assert_eq!(s.images,vec![(0,2560,1600)]);}
            struct View;
            impl abi::stream::Resolver for View {
                fn target(&self,h:u64)->Option<abi::stream::TargetInfo> {
                    let (width,height)=match h {1=>(2560,1600),2=>(64,64),3=>(4096,4096),_=>return None};
                    Some(abi::stream::TargetInfo {width,height,format:0,invalid:false})
                }
                fn source(&self,h:u64)->Option<abi::stream::SourceInfo> {
                    let t=self.target(h)?;Some(abi::stream::SourceInfo {width:t.width,height:t.height,format:t.format,invalid:false})
                }
                fn buffer_size(&self,_:u64)->Option<u64>{None}
                fn buffer_rect(&self,_:u64,_:u64)->[i32;4]{panic!("no CPU pixel/buffer input")}
            }
            let plan=abi::stream::validate(words,&View).expect("production validator");
            let gpc=s.c3 && (6..30).contains(&s.submits) || s.c3 && (30..102).contains(&s.submits) || s.c3 && s.submits>=105;
            if gpc {
                assert_eq!(&words[..4],&[1,4,2,0]);
                assert_eq!(words.len(),4+16*30);
                for d in words[4..].chunks_exact(30) {
                    assert_eq!(&d[..10],&[9,30,abi::pipeline::GRADIENT_V,0xff204080,0,0,0,0,0,1]);
                    assert_eq!(&d[10..17],&[0,0,64,64,0,64,0xffe0c080]);
                    assert!(d[17..].iter().all(|v|*v==0));
                }
                assert_eq!(plan.drawn,16);assert_eq!(plan.estimate,1<<31);
            } else {
                assert_eq!(&words[..4],&[1,4,1,0]);
                assert_eq!(words.len(),4+200*9);
                for (i,d) in words[4..].chunks_exact(9).enumerate() {
                    assert_eq!(&d[..3],&[3,9,abi::pipeline::SOLID]);
                    assert_eq!(d[3],0xff000000|i as u32);
                    assert_eq!(&d[4..],&[1,0,0,2560,1600]);
                }
                assert_eq!(plan.drawn,200);assert_eq!(plan.estimate,819_200_000);
            }
            if !s.thermal {assert_eq!(s.sets,match s.submits {0..=8=>vec![],9..=11=>vec![540_000],_=>vec![540_000,810_000]});}
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
fn fifteen_trials_use_real_validated_streams_with_down_and_up_between_waits() {
    STATE.with_borrow_mut(|s|*s=State::default());
    probe_gpu_bench::run_c2(phase).unwrap();
    STATE.with_borrow(|s| {assert_eq!((s.submits,s.waits,s.clocks),(15,15,30));assert!(!s.pending)});
}

#[test]
fn failed_completion_aborts_without_a_successful_trial() {
    STATE.with_borrow_mut(|s|*s=State {fail_wait:true,..State::default()});
    assert!(probe_gpu_bench::run_c2(phase).is_err());
    STATE.with_borrow(|s|assert_eq!((s.submits,s.waits,s.clocks),(1,1,1)));
}

fn phase(phase:probe_gpu_bench::ClockPhase)->Result<(),String> {
    use probe_gpu_bench::ClockPhase;
    let khz=match phase {ClockPhase::Post=>{
        STATE.with_borrow(|s|assert_eq!(s.submits,0));return Ok(());
    },ClockPhase::Low=>540_000,ClockPhase::Restore=>810_000};
    STATE.with_borrow_mut(|s| {
        assert!(!s.pending);assert_eq!(s.clocks%2,0);
        assert_eq!(s.submits,if khz==540_000 {9}else{12});
        s.sets.push(khz);
    });Ok(())
}
#[test]
fn low_trial_under_900_ms_completes_all_trials_and_restores_clock() {
    STATE.with_borrow_mut(|s|*s=State {low_too_fast:true,..State::default()});
    probe_gpu_bench::run_c2(phase).unwrap();
    STATE.with_borrow(|s|{assert_eq!((s.submits,s.waits),(15,15));assert_eq!(s.sets,[540_000,810_000]);});
}

#[test]
fn measurement_failure_stops_before_any_benchmark() {
    STATE.with_borrow_mut(|s|*s=State::default());
    assert!(probe_gpu_bench::run_c2(|_|Err(String::from("counter stalled"))).is_err());
    STATE.with_borrow(|s|assert_eq!((s.submits,s.waits,s.clocks),(0,0,0)));
}

#[test]
fn c3a_reads_post_then_before_each_of_twelve_trials_then_ten_idle_seconds() {
    STATE.with_borrow_mut(|s|*s=State {thermal:true,..State::default()});
    probe_gpu_bench::run_c3a(|phase,n| {
        STATE.with_borrow_mut(|s| {
            assert!(!s.pending);
            match phase {
                "post"=>assert_eq!((n,s.submits),(0,0)),
                "bench"=>assert_eq!(s.submits,n as usize-1),
                "idle"=>{assert_eq!(s.submits,12);assert_eq!(s.idle_sleeps,n);},
                _=>panic!("unexpected phase"),
            }
            s.samples.push((phase.into(),n));
        });Ok(())
    }).unwrap();
    STATE.with_borrow(|s| {
        assert_eq!((s.submits,s.waits),(12,12));assert!(s.sets.is_empty());
        assert_eq!(s.samples.len(),23);assert_eq!(s.idle_sleeps,10);
    });
}
#[test]
fn c3a_failed_completion_or_hot_reading_stops_new_work() {
    STATE.with_borrow_mut(|s|*s=State {thermal:true,fail_wait:true,..State::default()});
    assert!(probe_gpu_bench::run_c3a(|_,_|Ok(())).is_err());
    STATE.with_borrow(|s|assert_eq!((s.submits,s.waits),(1,1)));
    STATE.with_borrow_mut(|s|*s=State {thermal:true,..State::default()});
    assert!(probe_gpu_bench::run_c3a(|p,n|if p=="bench" && n==3 {Err("hot".into())}else{Ok(())}).is_err());
    STATE.with_borrow(|s|assert_eq!((s.submits,s.waits),(2,2)));
}

#[test]
fn c3a_sensor_errors_continue_and_the_decoded_threshold_stops_at_equality() {
    use probe_gpu_bench::temperature_record as record;
    assert!(record("GT710_SENSOR celsius=null raw=null down_clock=85 status=sensor-unavailable registers=512:3760267629\n","post",0).is_ok());
    assert!(record("GT710_SENSOR celsius=84 raw=84 down_clock=85 status=ok registers=512:3760267629\n","bench",1).is_ok());
    assert!(record("GT710_SENSOR celsius=85 raw=85 down_clock=85 status=ok registers=512:3760267629\n","bench",2).is_err());
    assert!(record("GT710_SENSOR celsius=94 raw=94 down_clock=95 status=ok registers=512:3760267629\n","bench",3).is_ok());
    assert!(record("GT710_SENSOR celsius=95 raw=95 down_clock=95 status=ok registers=512:3760267629\n","bench",4).is_err());
    assert!(record("malformed","post",0).is_err());
}

#[test] fn c3b_rejects_lost_or_malformed_raw_state() {
    use probe_gpu_bench::temperature_record as record;
    for bad in ["", " registers=bad", " registers=512:bad", " registers=512:1:2"] {
        assert!(record(&format!("GT710_SENSOR celsius=47 raw=47 down_clock=85 status=ok{bad}\n"),"bench",1).is_err());
    }
}

#[test]
fn c3_six_controls_then_gpc_trials_at_post_low_a_c_and_restore() {
    STATE.with_borrow_mut(|s|*s=State {thermal:true,c3:true,..State::default()});
    let mut stages=Vec::new();
    probe_gpu_bench::run_c3(|stage,n| {
        STATE.with_borrow(|s| {
            assert!(!s.pending);
            let before=match stage {"post"=>0,"low"=>30,"A"=>54,"C"=>78,"D"=>105,_=>panic!("stage")};
            if n==0 {assert_eq!(s.submits,before);}
        });
        stages.push((stage.to_string(),n));Ok(())
    }).unwrap();
    STATE.with_borrow(|s|assert_eq!((s.submits,s.waits),(129,129)));
    assert_eq!(stages.iter().filter(|p|p.1==0).map(|p|p.0.as_str()).collect::<Vec<_>>(),["post","low","A","C","D"]);
    assert_eq!(stages.iter().filter(|p|p.1!=0).map(|p|p.1).collect::<Vec<_>>(),(1..=24).collect::<Vec<_>>());
}

#[test]
fn c3_failed_stage_or_completion_does_not_advance() {
    STATE.with_borrow_mut(|s|*s=State {thermal:true,c3:true,..State::default()});
    assert!(probe_gpu_bench::run_c3(|stage,_|if stage=="C" {Err("counter mismatch".into())}else{Ok(())}).is_err());
    STATE.with_borrow(|s|assert_eq!((s.submits,s.waits),(78,78)));
    STATE.with_borrow_mut(|s|*s=State {thermal:true,c3:true,fail_wait:true,..State::default()});
    assert!(probe_gpu_bench::run_c3(|_,_|Ok(())).is_err());
    STATE.with_borrow(|s|assert_eq!((s.submits,s.waits),(1,1)));
}
