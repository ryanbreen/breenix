"""Run the production case boundary and Helper destructor with a deterministic clock."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from screen_evidence_test import function

ROOT = Path(__file__).resolve().parents[1]


class Lifecycle(unittest.TestCase):
    def test_verdict_and_cleanup_time_survive_a_failed_pre_drop_checkpoint(self):
        suite = (ROOT / 'libs/libbreenix/src/suite.rs').read_text()
        helper = (ROOT / 'userspace/programs/src/suite_breengel/window_cases/kit.rs').read_text()
        screen = (ROOT / 'libs/libbreenix/src/suite_screen.rs').read_text()
        fixture = r'''
use std::sync::atomic::{AtomicU64,AtomicBool,Ordering};
static NOW:AtomicU64=AtomicU64::new(10_000_000);
static LIVE:AtomicBool=AtomicBool::new(false);
static ON:AtomicBool=AtomicBool::new(false);
static CAPTURES:AtomicU64=AtomicU64::new(0);
fn monotonic_ns()->Option<i128>{Some(NOW.load(Ordering::Relaxed) as i128)}
fn plain(s:&str)->String{s.into()}
#[derive(Debug,PartialEq)] enum Outcome{Pass{ms:u64},Fail{ms:u64,msg:String},Skip{msg:String}}
enum CaseError{Fail(String),Skip(String)}
type CaseResult=Result<(),CaseError>;
struct Case{run:fn()->CaseResult}
fn publish_judged()->CaseResult{Ok(())}
fn publish_input()->CaseResult{Ok(())}
fn publish_values()->CaseResult{Ok(())}
mod screen {
 use super::*;
 CLOCK_STRUCT
 CLOCK_IMPL
 static CLOCK:CheckpointClock=CheckpointClock::new();
 pub fn enter_case(){CLOCK.excluded.store(0,Ordering::Relaxed);CLOCK.active.store(0,Ordering::Relaxed);}
 pub fn elapsed_ns(start:u64,now:u64)->u64{CLOCK.elapsed(start,now)}
 pub fn before_helper_drop(){
   if !ON.load(Ordering::Relaxed){return;}
   assert!(LIVE.load(Ordering::Relaxed),"owner was torn down before the hook");
   CLOCK.start(NOW.load(Ordering::Relaxed));
   NOW.fetch_add(25_000_000,Ordering::Relaxed);
   CAPTURES.fetch_add(1,Ordering::Relaxed); // a failing SCREEN observation, separate from CaseResult
   CLOCK.stop(NOW.load(Ordering::Relaxed));
 }
}
mod breenix_suite{pub mod suite{pub fn screen_before_helper_drop(){crate::screen::before_helper_drop();}}}
mod io{pub fn close(_:u64)->Result<(),()>{super::NOW.fetch_add(1_000_000,super::Ordering::Relaxed);Ok(())}}
struct Helper{pid:i32,to:u64,from:u64}
impl Helper{fn kill(&mut self){assert!(LIVE.swap(false,Ordering::Relaxed));NOW.fetch_add(7_000_000,Ordering::Relaxed);self.pid=0;}}
HELPER_DROP
ELAPSED
RUN_INLINE
fn work()->CaseResult{let _helper=Helper{pid:1,to:2,from:3};LIVE.store(true,Ordering::Relaxed);NOW.fetch_add(3_000_000,Ordering::Relaxed);Ok(())}
fn fail()->CaseResult{work()?;Err(CaseError::Fail("judged mismatch".into()))}
fn skip()->CaseResult{work()?;Err(CaseError::Skip("explicit skip".into()))}
#[test] fn on_off(){
 for (run,want) in [(work as fn()->CaseResult,Outcome::Pass{ms:12}),
 (fail,Outcome::Fail{ms:12,msg:"judged mismatch".into()}),(skip,Outcome::Skip{msg:"explicit skip".into()})]{
  for enabled in [false,true]{ON.store(enabled,Ordering::Relaxed);CAPTURES.store(0,Ordering::Relaxed);
   assert_eq!(run_inline(&Case{run}),want);assert!(!LIVE.load(Ordering::Relaxed));
   assert_eq!(CAPTURES.load(Ordering::Relaxed),u64::from(enabled));
  }
 }
}
'''
        replacements = {
            'CLOCK_STRUCT': function(screen, 'pub(super) struct CheckpointClock'),
            'CLOCK_IMPL': function(screen, 'impl CheckpointClock'),
            'HELPER_DROP': function(helper, 'impl Drop for Helper'),
            'ELAPSED': function(suite, 'fn screen_ms_since('),
            'RUN_INLINE': function(suite, 'fn run_inline('),
        }
        for key, value in replacements.items():
            fixture = fixture.replace(key, value)
        with tempfile.TemporaryDirectory() as d:
            p = Path(d)
            (p / 'test.rs').write_text(fixture)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings', str(p / 'test.rs'), '-o', str(p / 'test')], check=True)
            subprocess.run([str(p / 'test')], check=True)


if __name__ == '__main__':
    unittest.main()
