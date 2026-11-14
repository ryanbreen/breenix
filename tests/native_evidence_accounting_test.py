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
if __name__=='__main__':unittest.main()
