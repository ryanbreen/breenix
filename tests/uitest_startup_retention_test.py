"""Run the supervisor's collector decision with its independent transcript."""
from pathlib import Path
import unittest
from suite_collection_test import run_rust
from gt710_gr_live_unwind_test import item
ROOT=Path(__file__).resolve().parents[1]
class Retention(unittest.TestCase):
    def test_success_and_failure_retain_the_same_startup_phases(self):
        source=(ROOT/'libs/libuitest/src/supervisor_runtime.rs').read_text()
        start=source.index('let record = if let Request::Collect(trigger)')
        body=source[start:source.index('if !stopped { teardown(',start)]
        helper = item(source, 'fn startup_phase_record(') if 'fn startup_phase_record(' in source else ''
        run_rust(self,r'''
use std::{collections::BTreeMap,path::Path,sync::Mutex};
static OUTPUT:Mutex<Vec<u8>>=Mutex::new(Vec::new());
struct Fd;impl Fd{const STDOUT:Self=Self;}
mod io{pub fn write(_:super::Fd,b:&[u8])->Result<usize,String>{super::OUTPUT.lock().unwrap().extend_from_slice(b);Ok(b.len())}}
mod startup{pub struct Trace;impl Trace{pub fn open(_: &std::path::Path)->Result<Self,String>{Ok(Self)}pub fn snapshot(&self)->Vec<(&'static str,u64)>{vec![("connect-begin",100),("first-composite",200)]}}}
@HELPER@
enum Request{Collect(u8),Discard}
struct Accounting;impl Accounting{fn new(_: &str,_: &str,_:&[()],_:bool)->Result<Self,String>{Ok(Self)}}
struct Ring;impl Ring{fn dropped(&self)->u64{0}fn bytes(&self)->Vec<u8>{vec![]}}
struct App{ring:Ring}
type SessionEnvironment=Config;
struct Config;impl Config{fn config_root(&self)->&str{"unused"}}
struct Evidence;impl Evidence{fn collect(&mut self,_:&str,_:&str,_:u8,_:&[(u64,u64,Vec<u8>)])->String{"collected".into()}}
fn run(request:Request)->Result<Option<String>,String>{
 let environment:Option<SessionEnvironment>=Some(Config);let mut startup_record:Option<String>=None;let _=&mut startup_record;let output=BTreeMap::<u64,App>::new();let mut evidence=Evidence;
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
'''.replace('@BODY@',body).replace('@HELPER@',helper))

    def test_unnamed_discard_and_explicit_teardown(self):
        source=(ROOT/'libs/libuitest/src/supervisor_runtime.rs').read_text()
        start=source.index('let record = if let Request::Collect(trigger)')
        collector=source[start:source.index('if !stopped { teardown(',start)]
        helper=item(source,'fn startup_phase_record(') if 'fn startup_phase_record(' in source else ''
        teardown=item(source,'Ok(Some(Message::Teardown)) =>')
        teardown=teardown[teardown.index('{')+1:teardown.rindex('}')]
        run_rust(self,r'''
use std::{collections::BTreeMap,path::Path,sync::Mutex};
static OUTPUT:Mutex<Vec<u8>>=Mutex::new(Vec::new());
struct Fd;impl Fd{const STDOUT:Self=Self;}
mod io{pub fn write(_:super::Fd,b:&[u8])->Result<usize,String>{super::OUTPUT.lock().unwrap().extend_from_slice(b);Ok(b.len())}}
mod startup{pub struct Trace;impl Trace{
 pub fn open(p:&std::path::Path)->Result<Self,String>{if p.join("startup.trace").is_file(){Ok(Self)}else{Err("trace removed".into())}}
 pub fn snapshot(&self)->Vec<(&'static str,u64)>{vec![("connect-begin",100),("first-composite",200)]}
}}
enum Request{Collect(u8),Discard}
enum Message{Stopped}
struct Channel;impl Channel{fn send(&mut self,_:&Message)->Result<(),String>{Ok(())}}
struct Accounting;impl Accounting{fn new(_: &str,_: &str,_:&[()],_:bool)->Result<Self,String>{Ok(Self)}}
struct Ring;impl Ring{fn dropped(&self)->u64{0}fn bytes(&self)->Vec<u8>{vec![]}}
struct App{ring:Ring}
struct SessionEnvironment{root:String}impl SessionEnvironment{fn config_root(&self)->&str{&self.root}}
struct Evidence;impl Evidence{fn collect(&mut self,_:&str,_:&str,_:u8,_:&[(u64,u64,Vec<u8>)])->String{"collected".into()}}
fn teardown(_: &mut Channel,_:&mut BTreeMap<u64,App>,_:bool,_:Option<u64>,_:&mut (),_:&mut Option<()>)->Result<(),String>{Ok(())}
@HELPER@
fn run(request:Request,explicit:bool,root:&Path)->Result<Option<String>,String>{
 let mut environment=Some(SessionEnvironment{root:root.to_str().unwrap().into()});
 let mut startup_record:Option<String>=None;let _=&mut startup_record;
 let mut output=BTreeMap::<u64,App>::new();let mut evidence=Evidence;
 let mut channel=Channel;let compositor=None;let mut ownership=();let mut devices=None;let mut stopped=false;assert!(!stopped);
 if explicit{@TEARDOWN@ assert!(stopped);assert!(environment.is_none());assert!(!root.exists());}
 @COLLECTOR@
 Ok(record)
}
#[test]fn optional_identity_and_trace_survive_session_lifecycle(){
 let root=std::env::temp_dir().join(format!("uitest-retention-{}",std::process::id()));
 std::env::remove_var("BREENIX_COLLECT_SUITE");std::env::remove_var("BREENIX_COLLECT_CASE");
 let unnamed=run(Request::Discard,false,&root);let mut failures=Vec::new();if unnamed.is_err(){failures.push(format!("unnamed discard failed: {unnamed:?}"));}
 assert!(OUTPUT.lock().unwrap().is_empty());
 assert!(run(Request::Collect(1),false,&root).is_err(),"collection still requires names");
 std::env::set_var("BREENIX_COLLECT_SUITE","ui-testing");
 for case in ["sessions/start-desktop","sessions/start-kiosk"]{std::env::set_var("BREENIX_COLLECT_CASE",case);
 for explicit in [false,true]{for request in [Request::Discard,Request::Collect(1)]{
 std::fs::create_dir(&root).unwrap();std::fs::write(root.join("startup.trace"),b"trace").unwrap();OUTPUT.lock().unwrap().clear();
 run(request,explicit,&root).unwrap();
 let got=String::from_utf8(OUTPUT.lock().unwrap().clone()).unwrap();let want=format!("UI_STARTUP ui-testing {case} connect-begin=100 first-composite=200\n");if got!=want{failures.push(format!("explicit={explicit} {case} lost startup phases: {got:?}"));}
 if root.exists(){std::fs::remove_dir_all(&root).unwrap();}
 }}}
 assert!(failures.is_empty(),"{failures:?}");
 let _=Accounting;let _=Path::new("unused");
}
'''.replace('@HELPER@',helper).replace('@TEARDOWN@',teardown).replace('@COLLECTOR@',collector))
if __name__=='__main__':unittest.main()
