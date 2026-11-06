"""A timing read error must not undo a successful COMPOSITE's client state."""
import unittest
from gt710_gr_live_unwind_test import item
import gt710_receipt_timing_test as timing


class CompletedFrameTiming(unittest.TestCase):
    run_fixture = timing.ReceiptTiming.run_fixture

    def test_success_advances_the_frame_despite_unavailable_timing(self):
        source = timing.source('userspace/programs/src/bwm/compositor.rs')
        success = item(source[source.index('match r {'):], '0 => {')
        success = success.replace('0 =>', '').replace('super::session', 'session')
        fixture = r'''
#[derive(Default)]struct Timing {prepare_ns:u64}
mod session {pub fn timing(_:u64)->Result<super::Timing,String>{Err("status read failed".into())}}
enum FrameError {Failed(String)}
struct Window {id:u64,sample:u64,commit_id:u64,width:u32,height:u32}
struct Sampled {id:u64,sample:u64,commit_id:u64,width:u32,height:u32}
struct Frame {last_timing:Timing,next:usize,last_serial:u64,full:bool,sampled:Vec<Sampled>}
struct Composite {serial:u64}
impl Frame {fn complete(&mut self)->Result<(),FrameError>{
 let c=Composite{serial:42};let userspace_prepare=10;
 let windows=[Window{id:7,sample:8,commit_id:9,width:10,height:11}];
 SUCCESS
}}
#[test]fn completed_frame(){
 let mut f=Frame{last_timing:Timing{prepare_ns:999},next:0,last_serial:0,full:true,sampled:vec![]};
 match f.complete(){Ok(())=>{},Err(FrameError::Failed(reason))=>panic!("completed frame rejected: {reason}")}
 assert_eq!(f.next,1);assert_eq!(f.last_serial,42);assert!(!f.full);
 assert_eq!(f.sampled.len(),1);let s=&f.sampled[0];assert_eq!((s.id,s.sample,s.commit_id,s.width,s.height),(7,8,9,10,11));
 assert_eq!(f.last_timing.prepare_ns,10);
}
'''.replace('SUCCESS', success)
        # The old error conversion constructs this variant; the repaired slice
        # only needs its type for the surrounding frame result.
        fixture = fixture.replace('enum FrameError', '#[allow(dead_code)]\nenum FrameError')
        self.run_fixture(fixture)

    def test_missing_schedule_does_not_reject_a_completed_present(self):
        source = timing.source('kernel/src/gpu/compose.rs')
        start = source.index('    let (period,lead)=dev::presentation_schedule()')
        tail = source[start:source.index('\n}', start)]
        self.run_fixture(r'''
const EIO:i64=-5;
mod dev {pub fn presentation_schedule()->Option<(u64,u64)>{None}}
mod window {pub mod pacing {pub fn publish(serial:u64,at:u64,p:u64,work:u64,lead:u64){assert_eq!((serial,at,p,work,lead),(7,100,0,10,0));}}}
struct Frame {serial:u64}
struct Answer {answer:u64,pixels:u64}
struct Composed {pixels:u64,per_window:u64,presented_ns:u64}
#[test]fn completed_present(){
 let frame=Frame{serial:7};let presented_ns=100;let accepted_ns:u64=90;let began_ns=80;
 let mut a=Answer{answer:0,pixels:0};let answer=4;let scene_work=(123,456);
 let result=(||->Result<Composed,i64>{TAIL})();
 let r=result.expect("completed present rejected for missing timing hint");
 assert_eq!((r.pixels,r.per_window,r.presented_ns),(123,456,100));
 assert_eq!((a.answer,a.pixels),(4,123));
}
'''.replace('TAIL',tail).replace('const EIO:i64=-5;', 'const EIO:i64=-5;' if 'EIO' in tail else ''))


if __name__ == '__main__':
    unittest.main()
