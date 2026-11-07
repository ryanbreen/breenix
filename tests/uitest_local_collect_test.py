"""Execute the supervisor's producer/teardown/reply ordering with a publication witness."""
from pathlib import Path
import os
import subprocess
import unittest
from suite_collection_test import run_rust
ROOT = Path(__file__).resolve().parents[1]
class LocalCollect(unittest.TestCase):
    def test_local_publication_precedes_teardown_and_done(self):
        rel = 'libs/libuitest/src/supervisor_runtime.rs'
        old = os.environ.get('UITEST_OLD_COMMIT')
        source = subprocess.check_output(['git', 'show', f'{old}:{rel}'], cwd=ROOT, text=True) if old else (ROOT/rel).read_text()
        start = source.index('let record = if let Request::Collect(trigger)')
        body = source[start:source.index('let mut left = reply.as_bytes();', start)]
        run_rust(self, r'''
use std::{collections::BTreeMap,sync::Mutex};
#[path="RESULT"]pub mod result;
#[path="COLLECTION"]pub mod collection;
use result::Accounting;
use collection::{Request,Trigger};
static EVENTS:Mutex<Vec<&str>>=Mutex::new(Vec::new());
struct Ring;impl Ring{fn dropped(&self)->u64{0}fn bytes(&self)->Vec<u8>{b"stderr".to_vec()}}
struct App{ring:Ring}
struct Config;impl Config{fn config_root(&self)->&str{"unused"}}
fn teardown(_: &mut (),_: &mut BTreeMap<u64,App>,_: bool,_: Option<u64>,_: &mut (),_: &mut Option<()>)->Result<(),String>{EVENTS.lock().unwrap().push("teardown");Ok(())}
pub struct Evidence;
impl Evidence{
    pub fn collect(&mut self,suite:&str,case:&str,trigger:Trigger,stderr:&[(u64,u64,Vec<u8>)])->String{
        assert_eq!(stderr[0].2,b"stderr");
        // Stand in for the device producers, but require their publication side effect.
        EVENTS.lock().unwrap().push("synced-renamed");
        format!("SUITE {suite} COLLECT {case} trigger={} screen=at-collection windows=1 remnants=0 artifacts=complete\n",trigger.name())
    }
    pub fn release_remnants(&mut self){EVENTS.lock().unwrap().push("release");}
}
fn run()->Result<String,String>{
    let mut channel=();let mut output=BTreeMap::from([(17,App{ring:Ring})]);let stopped=false;let compositor=None;let mut ownership=();let mut devices=None;
    let mut environment:Option<Config>=None;let mut evidence=Evidence;let request=Request::Collect(Trigger::Crash);
    // Keep inputs referenced when running the old producer branch too.
    let _ = (&mut evidence,&output[&17].ring.dropped(),&output[&17].ring.bytes());
BODY
Ok(reply)
}
#[test]fn collect_then_cleanup_then_done(){
    std::env::set_var("BREENIX_COLLECT_SUITE","test");std::env::set_var("BREENIX_COLLECT_CASE","runner/death");
    let reply=run().unwrap();
    assert!(reply.ends_with("artifacts=complete\ndone\n"),"{reply}");
    assert_eq!(*EVENTS.lock().unwrap(),["synced-renamed","teardown","release"]);
}
'''.replace('RESULT', str(ROOT/'libs/libbreenix/src/suite_result.rs')).replace('COLLECTION', str(ROOT/'libs/libbreenix/src/suite_collection.rs')).replace('BODY', body))
if __name__ == '__main__': unittest.main()
