"""Exercise production miss-only trace retention without GPU hardware."""
import unittest

import gt710_receipt_timing_test as timing


class PresentMissTrace(unittest.TestCase):
    run_fixture = timing.ReceiptTiming.run_fixture

    def test_middle_frame_survives_and_success_is_silent(self):
        trace = timing.source('kernel/src/gpu/gk208/presentation_trace.rs').replace('//!', '//')
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
        let mut row=[0;15];
        row[0]=100+(i%2) as u64;
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
'''.replace('TRACE', trace).replace('CALL', call)
        self.run_fixture(fixture)


if __name__ == '__main__':
    unittest.main()
