"""Execute end-capture entry with a replacement buffer still in runner custody."""
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest
from screen_evidence_test import function
ROOT = Path(__file__).resolve().parents[1]

class StageCustody(unittest.TestCase):
    def test_runner_stage_is_not_an_end_checkpoint_after_buffer_replacement(self):
        screen = (ROOT/'libs/libbreenix/src/suite_screen.rs').read_text()
        stage = (ROOT/'libs/libbreenix/src/suite_stage.rs').read_text()
        entry = function(screen, 'fn snapshot_end()')
        entry = entry[:entry.index('    let clock =')] + '    result\n}'
        entry = entry.replace('super::running_case()', 'running_case()')
        constants = '\n'.join(re.search(r'const '+name+r':[^\n]+',stage)[0]
                              for name in ('STAGE_CUSTODY','LIVE_CHECKS'))
        policy = function(stage,'pub(super) fn case_takes_display')
        code = r'''
use std::cell::Cell;
thread_local! {static CASE:Cell<&'static str>=const{Cell::new("windows/distinct-content")};static OBSERVATIONS:Cell<u32>=const{Cell::new(0)};}
type Snapshot=Result<(),String>;
fn running_case()->Option<&'static str>{Some(CASE.with(Cell::get))}
// A replaced runner buffer has a new resource, so resource equality alone
// would admit this otherwise successful observation.
fn snapshot(_: &str)->Snapshot{OBSERVATIONS.with(|n|n.set(n.get()+1));Ok(())}
mod suite_stage { CONSTANTS POLICY }
ENTRY
#[test] fn custody_precedes_the_observer(){
 for name in ["panel/judged-frame-shown","panel/serial-agreement","panel/quiet-window","panel/boundary-notice","panel/cursor-restore","panel/cursor-trail-detected"] {
  CASE.with(|v|v.set(name));assert_eq!(running_case(),Some(name));assert!(!suite_stage::case_takes_display(name));
  assert_eq!(snapshot_end(),Err("no-case-scanout".into()),"captured runner stage for {name}");
 }
 assert_eq!(OBSERVATIONS.with(Cell::get),0);
 CASE.with(|v|v.set("windows/distinct-content"));assert_eq!(snapshot_end(),Ok(()));assert_eq!(OBSERVATIONS.with(Cell::get),1);
}
'''.replace('CONSTANTS',constants).replace('POLICY',policy).replace('ENTRY',entry)
        with tempfile.TemporaryDirectory() as tmp:
            path=Path(tmp);(path/'test.rs').write_text(code)
            priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
            subprocess.run(priority+['rustc','--edition=2021','-Dwarnings','--test',str(path/'test.rs'),'-o',str(path/'test')],check=True)
            subprocess.run([str(path/'test')],check=True)

if __name__=='__main__':unittest.main()
