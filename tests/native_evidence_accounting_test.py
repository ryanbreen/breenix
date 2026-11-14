"""Execute the native sender's preflight before the transport with shared bytes."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
ROOT=Path(__file__).resolve().parents[1]

class NativeAccounting(unittest.TestCase):
    def test_native_final_counts_prior_screen_and_diagnostic_bodies(self):
        source=(ROOT/'userspace/programs/src/gt710_artifacts.rs').read_text()
        start=source.index('pub(super) fn transfer(')
        prefix=source[start:source.index('    let data_digest=',start)]
        prefix=prefix.replace('pub(super) fn transfer','fn transfer')
        code='''
#![allow(dead_code)]
const MAX_UPLOAD:usize=2<<20;
const MAX_TOTAL:usize=64<<20;
fn totals()->(usize,usize){(2<<20,1)}
mod breenix_suite {pub mod suite {
    pub fn evidence_check(bytes:usize)->Result<(),String>{
        if bytes>=(1<<20) {Err("total-bound".into())}else{Ok(())}
    }
}}
'''+prefix+'''
    let _=suffix;
    Ok(())
}
#[test] fn final_respects_shared_remaining(){
    // Native startup used 2 MiB, screen/CRC bodies used another 61 MiB plus one byte.
    // The same receiver bound has less than 1 MiB left before final.
    assert!(transfer(&vec![0;1<<20],"/final").is_err(),
        "native sender ignored bytes already sent by the suite sender");
}
'''
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp); (p/'test.rs').write_text(code)
            priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
            result=subprocess.run(priority+['rustc','--edition=2021','--test','-Dwarnings',str(p/'test.rs'),'-o',str(p/'test')],capture_output=True,text=True)
            self.assertEqual(result.returncode,0,result.stdout+result.stderr)
            result=subprocess.run([str(p/'test')],capture_output=True,text=True)
            self.assertEqual(result.returncode,0,result.stdout+result.stderr)
    def test_screen_upload_setup_uses_runtime_budget_and_shared_preflight(self):
        from gt710_gr_live_unwind_test import item
        setup=item((ROOT/'libs/libbreenix/src/suite_screen.rs').read_text(),
                   "pub(super) fn upload_pending(")
        budget=(ROOT/'libs/libbreenix/src/suite_evidence_budget.rs').read_text()
        budget=budget[budget.index('pub(super) struct Budget'):]
        support=(ROOT/'tests/suite_evidence_errno.rs').read_text()
        support=support.replace('../libs/',str(ROOT/'libs')+'/')
        code='#![allow(dead_code)]\n'+support+'\n'+r'''
use std::sync::{Mutex, OnceLock};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
mod evidence_budget {
    use std::sync::Mutex;
    // BUDGET
}
static BUDGET: OnceLock<evidence_budget::Budget> = OnceLock::new();
static INVALID: AtomicBool = AtomicBool::new(false);
static SENT: AtomicUsize = AtomicUsize::new(0);
static ROUND: AtomicUsize = AtomicUsize::new(0);
static LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());
fn native_evidence_budget() -> Result<&'static evidence_budget::Budget, String> {
    if INVALID.load(Ordering::Relaxed) { return Err("evidence-modes".into()); }
    Ok(BUDGET.get_or_init(|| evidence_budget::Budget::new(5_063_743_488)))
}
fn evidence_check(n: usize) -> Result<(), String> { native_evidence_budget()?.check(n) }
fn evidence_transfer(_: &evidence::Route, _: &str, data: &[u8], _: [u8;32]) -> Result<(),String> {
    native_evidence_budget()?.reserve(data.len())?;
    SENT.fetch_add(1, Ordering::Relaxed);
    Ok(())
}
fn evidence_route(_: &str, _: bool) -> Result<Option<evidence::Route>, String> {
    Ok(Some(evidence::Route::parse("http://fixture.invalid/upload", "gpu", false)?))
}
fn upload_diagnostic(_: &str, _: &str, _: &[u8]) -> Result<String,String> { panic!("no CRC queue") }
mod sequence {
    pub fn read(_: &str) -> Option<(Vec<String>, usize)> { None }
}
mod screen_upload {
    pub fn flush(_: &std::path::Path, upload: &mut super::evidence::Upload,
        mut send: impl FnMut(&super::evidence::Route, &str, &[u8], [u8;32]) -> Result<(),String>) -> Vec<String> {
        let round=super::ROUND.fetch_add(1,super::Ordering::Relaxed);
        if round==0 {
            // Drive the retained failure: the real uploader crosses its private 64 MiB.
            let body=vec![7; 2<<20];
            for _ in 0..34 {
                assert!(upload.send_parts(&body,&mut send).is_ok(),
                    "screen uploader retained its private 64 MiB limit");
            }
        } else {
            // The runtime envelope is large, but other senders have used all but 1 MiB.
            let before=super::SENT.load(super::Ordering::Relaxed);
            assert_eq!(upload.send_parts(&vec![9;3<<20],&mut send),Err("total-bound".into()));
            assert_eq!(super::SENT.load(super::Ordering::Relaxed),before,"orphan first part");
        }
        Vec::new()
    }
}
mod screen {
    use super::Mutex;
    fn pending_path()->std::path::PathBuf { std::env::var("PENDING").unwrap().into() }
    fn crc_pending_path()->std::path::PathBuf { pending_path().with_extension("crc") }
    fn flush_crc(_: &std::path::Path, _: &str, _: &str,
        _: impl FnMut(&str,&[u8])->Result<String,String>) -> Vec<String> {Vec::new()}
    fn emit(line: &str) { super::LINES.lock().unwrap().push(line.to_string()); }
    // SETUP
}
#[test] fn screen_setup_obeys_runtime_envelope_and_whole_record_preflight() {
    screen::upload_pending("gpu","performance/coverage-throughput");
    assert_eq!(SENT.load(Ordering::Relaxed),34);
    let budget=native_evidence_budget().unwrap();
    budget.reserve(budget.limit()-(68<<20)-(1<<20)).unwrap();
    screen::upload_pending("gpu","performance/present-rate-2560");
    INVALID.store(true,Ordering::Relaxed);
    screen::upload_pending("gpu","performance/present-rate-2560");
    assert_eq!(ROUND.load(Ordering::Relaxed),2,"bad budget must not reach flush");
    assert_eq!(*LINES.lock().unwrap(),["SCREEN_UPLOAD gpu performance/present-rate-2560 refused=evidence-modes"]);
}
'''.replace('// SETUP',setup).replace('// BUDGET',budget)
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp); (p/'test.rs').write_text(code)
            pending=p/'pending'; pending.touch()
            priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
            r=subprocess.run(priority+['rustc','--edition=2021','--test','-Dwarnings',str(p/'test.rs'),'-O','-o',str(p/'test')],capture_output=True,text=True)
            self.assertEqual(r.returncode,0,r.stdout+r.stderr)
            import os
            r=subprocess.run([str(p/'test'),'screen_setup_obeys_runtime_envelope_and_whole_record_preflight'],capture_output=True,text=True,env=dict(os.environ,PENDING=str(pending)))
            self.assertEqual(r.returncode,0,r.stdout+r.stderr)

    def test_guest_and_receiver_envelopes_match_and_records_preflight_whole(self):
        import json
        from gt710_final_upload_test import upload
        fixture=json.loads((ROOT/'tests/fixtures/native_gpu_upload_sequence.json').read_text())
        intents=(ROOT/'libs/libbreenix/src/suite_intent.rs').read_text()
        intents=intents[intents.index('#[derive(Clone'):intents.index('pub(super) fn configured(')]
        intents=intents.replace('../../../scripts/metal/intent-pictures.tsv',str(ROOT/'scripts/metal/intent-pictures.tsv'))
        budget=(ROOT/'libs/libbreenix/src/suite_evidence_budget.rs').read_text().replace('//!','//')
        for path in ('../../../docs/suites/frames/checkpoints.json','../../../scripts/metal/intent-pictures.tsv'):
            budget=budget.replace(path,str((ROOT/'libs/libbreenix/src'/path).resolve()))
        bound=upload.byte_bound(dict(suite='gpu',evidenceModes=fixture['modes'],evidenceDefault=fixture['default']))
        breengel_bound=upload.byte_bound(dict(suite='breengel',evidenceModes=fixture['modes'],evidenceDefault=fixture['default']))
        source=(ROOT/'tests/suite_evidence_errno.rs').read_text()
        source=source.replace('../libs/',str(ROOT/'libs')+'/')
        code='#![allow(dead_code)]\n'+source+'\nmod intent {use std::vec::Vec;\n'+intents+'\n}\nmod budget {\n'+budget+'\n}\n'+r'''
#[test] fn same_envelope_and_preflight() {
    let modes=[[1024,768,1],[1280,1024,1],[2560,2048,2]];
    assert_eq!(budget::limit("gpu", &modes,1).unwrap(),BOUND);
    assert_eq!(budget::limit("breengel", &modes,1).unwrap(),BREENGL);
    let budget=budget::Budget::new(64<<20);
    budget.reserve((63<<20)+1).unwrap();
    assert!(budget.check(1<<20).is_err());
    assert!(budget.reserve(1<<20).is_err());
    static SHARED:std::sync::OnceLock<budget::Budget>=std::sync::OnceLock::new();
    let shared=SHARED.get_or_init(||budget::Budget::new(64<<20));
    shared.reserve((63<<20)+1).unwrap();
    let mut upload=evidence::Upload::new("gpu",Ok(Some(evidence::Route::parse("http://fixture.invalid/upload/screens","gpu",false).unwrap())));
    upload.set_bound(64<<20,|n|SHARED.get().unwrap().check(n));
    assert!(upload.send_parts(&vec![0;3<<20],|_,_,_,_|panic!("no orphan first part")).is_err());
}
'''.replace('BOUND',str(bound)).replace('BREENGL',str(breengel_bound))
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp);(p/'test.rs').write_text(code)
            priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
            r=subprocess.run(priority+['rustc','--edition=2021','--test','-Dwarnings',str(p/'test.rs'),'-o',str(p/'test')],capture_output=True,text=True)
            self.assertEqual(r.returncode,0,r.stdout+r.stderr)
            r=subprocess.run([str(p/'test')],capture_output=True,text=True)
            self.assertEqual(r.returncode,0,r.stdout+r.stderr)

if __name__=='__main__':unittest.main()
