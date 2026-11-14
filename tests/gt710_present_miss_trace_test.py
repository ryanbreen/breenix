"""Exercise production miss-only trace retention without GPU hardware."""
import unittest

import gt710_receipt_timing_test as timing


class PresentMissTrace(unittest.TestCase):
    run_fixture = timing.ReceiptTiming.run_fixture

    def test_middle_frame_survives_and_success_is_silent(self):
        trace = timing.source('kernel/src/gpu/gk208/presentation_trace.rs').replace('//!', '//')
        width = 16 if 'type Row = [u64; 16]' in trace else 15
        call = 'record(row, PERIOD)' if 'record(row: Row, period_ns: u64)' in trace else 'record(row)'
        fixture = r'''
extern crate alloc;
use std::sync::Mutex;
static OUTPUT: Mutex<Vec<String>> = Mutex::new(Vec::new());
macro_rules! serial_println { ($($arg:tt)*) => { crate::OUTPUT.lock().unwrap().push(format!($($arg)*)) }; }
pub(crate) use serial_println;
mod spin {
    pub struct Mutex<T>(std::sync::Mutex<T>);
    impl<T> Mutex<T> {
        pub const fn new(v:T)->Self { Self(std::sync::Mutex::new(v)) }
        pub fn lock(&self)->std::sync::MutexGuard<'_,T> { self.0.lock().unwrap() }
    }
}
mod time { pub mod wakeup { pub fn stats()->[u64;3] { [0;3] } } }
mod presentation_trace { use crate::spin; TRACE }
const PERIOD:u64 = 16_661_926;
fn collect(count:usize, missed:bool) {
    for i in 0..count {
        let mut row=[0;ROW_WIDTH];
        row[0]=100+(i%2) as u64;
        row[14]=1063;
        if let Some(height)=row.get_mut(15) { *height=1066; }
        row[7]=(i as u64+10+u64::from(missed && i>=60))*PERIOD;
        row[1]=row[7]-if missed && i==60 { PERIOD+1000 } else { PERIOD-1000 };
        row[4]=row[7]+100;
        row[8]=row[7]+200;
        presentation_trace::CALL;
    }
    assert!(OUTPUT.lock().unwrap().is_empty(), "collection printed on the timed path");
    let snapshot=presentation_trace::snapshot();
    assert!(std::mem::size_of_val(&snapshot)<4096, "snapshot consumes a kernel stack");
    assert!(OUTPUT.lock().unwrap().is_empty());
    presentation_trace::export(snapshot);
}
#[test] fn bounded_middle_capture() {
    presentation_trace::dump();
    OUTPUT.lock().unwrap().clear();
    collect(122,false);
    assert!(!OUTPUT.lock().unwrap().iter().any(|s|s.starts_with("GT710_PRESENT_FRAME ")));
    OUTPUT.lock().unwrap().clear();
    collect(122,true);
    {
        let output=OUTPUT.lock().unwrap();
        let frames:Vec<_>=output.iter().filter(|s|s.starts_with("GT710_PRESENT_FRAME ")).collect();
        assert_eq!(frames.len(),122,"missing the middle frames around the missed vblank");
        assert!(frames[60].contains("seq=61 resource=100"));
        assert!(frames[60].contains(&format!("accept_to_latch_ns={}",PERIOD+1000)));
        assert!(output.iter().any(|s|s.contains("retained=122 dropped=0")));
    }
    OUTPUT.lock().unwrap().clear();
    collect(300,true);
    let output=OUTPUT.lock().unwrap();
    let frames:Vec<_>=output.iter().filter(|s|s.starts_with("GT710_PRESENT_FRAME ")).collect();
    assert_eq!(frames.len(),128);
    assert!(frames.first().unwrap().contains("seq=173 "));
    assert!(frames.last().unwrap().contains("seq=300 "));
    assert!(output.iter().any(|s|s.contains("retained=128 dropped=172")));
}
'''.replace('TRACE', trace).replace('CALL', call).replace('ROW_WIDTH', str(width))
        self.run_fixture(fixture)



    def test_miss_definitions(self):
        trace = timing.source('kernel/src/gpu/gk208/presentation_trace.rs').replace('//!', '//')
        width = 16 if 'type Row = [u64; 16]' in trace else 15
        fixture = r'''
extern crate alloc;
use std::sync::Mutex;
static OUTPUT: Mutex<Vec<String>> = Mutex::new(Vec::new());
static RUN: Mutex<()> = Mutex::new(());
macro_rules! serial_println { ($($arg:tt)*) => { crate::OUTPUT.lock().unwrap().push(format!($($arg)*)) }; }
pub(crate) use serial_println;
mod spin {
    pub struct Mutex<T>(std::sync::Mutex<T>);
    impl<T> Mutex<T> {
        pub const fn new(v:T)->Self { Self(std::sync::Mutex::new(v)) }
        pub fn lock(&self)->std::sync::MutexGuard<'_,T> { self.0.lock().unwrap() }
    }
}
mod time { pub mod wakeup { pub fn stats()->[u64;3] { [0;3] } } }
mod presentation_trace { use crate::spin; TRACE }
const PERIOD:u64=16_661_186;
fn row(accept:u64,latch:u64,phase:u64,line:u64)->[u64;ROW_WIDTH] {
    let mut r=[0;ROW_WIDTH];r[0]=7;r[1]=accept;r[2]=line;r[7]=latch;
    r[4]=latch+100;r[8]=latch+200;r[14]=phase;
    if let Some(height)=r.get_mut(15) {*height=1066;}r
}
fn export(rows:&[[u64;ROW_WIDTH]])->Vec<String> {
    presentation_trace::dump();OUTPUT.lock().unwrap().clear();
    for &r in rows { presentation_trace::record(r,PERIOD); }
    assert!(OUTPUT.lock().unwrap().is_empty(),"record printed on the timed path");
    let s=presentation_trace::snapshot();
    assert!(std::mem::size_of_val(&s)<4096);
    presentation_trace::export(s);
    std::mem::take(&mut *OUTPUT.lock().unwrap())
}
fn misses(out:&[String])->usize {out.iter().filter(|s|s.starts_with("GT710_PRESENT_MISS ")).count()}
#[test] fn queued_late_latch() {
    let _guard=RUN.lock().unwrap_or_else(|e|e.into_inner());
    // An eligible submission with one late latch, surrounded by on-time frames.
    let out=export(&[row(9*PERIOD+1000,10*PERIOD,1063,100),
        row(10*PERIOD+1000,12*PERIOD,1063,100),row(12*PERIOD+1000,13*PERIOD,1063,100)]);
    assert_eq!(misses(&out),1);
    assert!(out.iter().any(|s|s.starts_with("GT710_PRESENT_MISS ") && s.contains("misses=1")),"one late presentation must count once: {out:?}");
    // The gap alone must also catch a queued presentation accepted before the prior latch.
    let out=export(&[row(9*PERIOD+1000,10*PERIOD,1063,100),row(10*PERIOD,12*PERIOD,1063,100)]);
    assert_eq!(misses(&out),1);
}
#[test] fn idle_gap() {
    let _guard=RUN.lock().unwrap_or_else(|e|e.into_inner());
    // A two-period gap after the first latch, with an on-time second presentation, is idle.
    let out=export(&[row(9*PERIOD+1000,10*PERIOD,1063,100),row(11*PERIOD+1000,12*PERIOD,1063,100)]);
    assert_eq!(misses(&out),0,"idle was labelled a miss: {out:?}");
}
#[test] fn unknown_phase() {
    let _guard=RUN.lock().unwrap_or_else(|e|e.into_inner());
    // Unknown phase must be exported explicitly, without claiming a missed edge.
    let out=export(&[row(10*PERIOD+1000,12*PERIOD,u64::MAX,100)]);
    assert_eq!(misses(&out),0,"uncalibrated receipt was labelled a miss: {out:?}");
    assert!(out.iter().any(|s|s.starts_with("GT710_PRESENT_UNCALIBRATED ")),"uncalibrated receipt disappeared: {out:?}");
    assert!(out.iter().any(|s|s.starts_with("GT710_PRESENT_EDGE ") && s.contains("latch_phase=18446744073709551615")));
}
#[test] fn deadline_boundary() {
    let _guard=RUN.lock().unwrap_or_else(|e|e.into_inner());
    // The retained fence-identity receipt is accepted AT the learned deadline;
    // its +2 edge is intentional. The next eligible edge must stay strict.
    for (counter_line,line,phase) in [(0,1063,1063),(37,34,1063)] {
        let mut r=row(10*PERIOD-35_798,11*PERIOD,phase,line);r[11]=counter_line;
        assert_eq!(misses(&export(&[r])),0,"legitimate +2 edge was labelled a miss");
        r[7]=r[1]+2*PERIOD+1;
        assert_eq!(misses(&export(&[r])),1,"a late +2 receipt was hidden");
    }
    let r=row(10*PERIOD,11*PERIOD+1,1063,100);
    assert_eq!(misses(&export(&[r])),1,"one nanosecond over the eligible period was hidden");
}
'''.replace('TRACE', trace).replace('ROW_WIDTH', str(width))
        self.run_fixture(fixture)

if __name__ == '__main__':
    unittest.main()
