"""Run the supervisor's collector decision with its independent transcript."""
from pathlib import Path
import unittest
from suite_collection_test import run_rust
ROOT=Path(__file__).resolve().parents[1]
class Retention(unittest.TestCase):
    def test_success_and_failure_retain_the_same_startup_phases(self):
        source=(ROOT/'libs/libuitest/src/supervisor_runtime.rs').read_text()
        start=source.index('let record = if let Request::Collect(trigger)')
        body=source[start:source.index('if !stopped { teardown(',start)]
        run_rust(self,r'''
use std::{collections::BTreeMap,path::Path,sync::Mutex};
static OUTPUT:Mutex<Vec<u8>>=Mutex::new(Vec::new());
struct Fd;impl Fd{const STDOUT:Self=Self;}
mod io{pub fn write(_:super::Fd,b:&[u8])->Result<usize,String>{super::OUTPUT.lock().unwrap().extend_from_slice(b);Ok(b.len())}}
mod startup{pub struct Trace;impl Trace{pub fn open(_: &std::path::Path)->Result<Self,String>{Ok(Self)}pub fn snapshot(&self)->Vec<(&'static str,u64)>{vec![("connect-begin",100),("first-composite",200)]}}}
enum Request{Collect(u8),Discard}
struct Accounting;impl Accounting{fn new(_: &str,_: &str,_:&[()],_:bool)->Result<Self,String>{Ok(Self)}}
struct Ring;impl Ring{fn dropped(&self)->u64{0}fn bytes(&self)->Vec<u8>{vec![]}}
struct App{ring:Ring}
struct Config;impl Config{fn config_root(&self)->&str{"unused"}}
struct Evidence;impl Evidence{fn collect(&mut self,_:&str,_:&str,_:u8,_:&[(u64,u64,Vec<u8>)])->String{"collected".into()}}
fn run(request:Request)->Result<Option<String>,String>{
 let environment=Some(Config);let output=BTreeMap::<u64,App>::new();let mut evidence=Evidence;
 @BODY@
 Ok(record)
}
#[test]fn independent_transcript_has_pass_and_fail(){
 std::env::set_var("BREENIX_COLLECT_SUITE","ui-testing");
 for case in ["sessions/start-desktop","sessions/start-kiosk"]{std::env::set_var("BREENIX_COLLECT_CASE",case);
 for request in [Request::Discard,Request::Collect(1)]{OUTPUT.lock().unwrap().clear();run(request).unwrap();
 assert_eq!(String::from_utf8(OUTPUT.lock().unwrap().clone()).unwrap(),format!("UI_STARTUP ui-testing {case} connect-begin=100 first-composite=200\n"),"startup phases stayed in the silent case pipe");}}
 let _=Accounting;let _=Path::new("unused");
}
'''.replace('@BODY@',body))
if __name__=='__main__':unittest.main()
