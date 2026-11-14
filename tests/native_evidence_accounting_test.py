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
