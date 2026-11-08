"""U18: mismatches stop harness sessions and remain visible in production."""
from pathlib import Path
import json
import sys
import tempfile
import os
import subprocess
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust
ROOT=Path(__file__).resolve().parents[1]
class StartupFailure(unittest.TestCase):
 def test_mismatches_report_in_production_and_exit_in_sessions(self):
  old=os.environ.get('BWM_STARTUP_OLD_REV')
  s=subprocess.check_output(['git','show',old+':userspace/programs/src/bwm.rs'],cwd=ROOT,text=True) if old else (ROOT/'userspace/programs/src/bwm.rs').read_text()
  frames=item(s,'match first {');fills=item(s,'match fills {')
  guards=[];tail=s
  for _ in range(2):
   guard=item(tail,'if let Err(error) = &fills');guards.append(guard);tail=tail[tail.index(guard)+len(guard):]
  run_rust(self,r'''
use std::sync::{atomic::{AtomicI32,Ordering},Mutex};static EXIT:AtomicI32=AtomicI32::new(-1);
static OUTPUT:Mutex<String>=Mutex::new(String::new());
macro_rules! print {($($args:tt)*)=>{OUTPUT.lock().unwrap().push_str(&format!($($args)*))};}
macro_rules! eprintln {($($args:tt)*)=>{OUTPUT.lock().unwrap().push_str(&format!($($args)*))};}
mod process{pub fn exit(status:i32)->!{super::EXIT.store(status,super::Ordering::SeqCst);panic!("startup exited")}}
enum CheckError{Moved(String),Failed(String)}
fn frame(first:Result<u64,CheckError>,strict_startup:bool){let _=strict_startup;@FRAME@}
fn fills(fills:Result<String,String>,strict_startup:bool){let _=strict_startup;@FILLS@}
fn initial_fill(fills:Result<String,String>,strict_startup:bool){let _=strict_startup;@GUARD0@}
fn recovered_fill(fills:Result<String,String>,strict_startup:bool){let _=strict_startup;@GUARD1@}
#[test]fn mismatch_policy(){
 frame(Ok(4096000),true);fills(Ok("verified".into()),true);
 assert_eq!(EXIT.load(Ordering::SeqCst),-1);
 for strict in [false,true]{
  for check in 0..5{
   OUTPUT.lock().unwrap().clear();
   let result=std::panic::catch_unwind(||match check{
    0=>frame(Err(CheckError::Moved("changed binding".into())),strict),
    1=>frame(Err(CheckError::Failed("pixel (2559,1599)".into())),strict),
    2=>fills(Err("wrong scanout pixel".into()),strict),
    3=>initial_fill(Err("wrong scanout pixel".into()),strict),
    _=>recovered_fill(Err("wrong scanout pixel".into()),strict),
   });
   assert_eq!(result.is_err(),strict,"wrong mismatch policy check={check} strict={strict}");
   assert_eq!(EXIT.swap(-1,Ordering::SeqCst),if strict{1}else{-1});
   let output=OUTPUT.lock().unwrap();
   assert!(output.contains("check FAILED:"),"{output}");
   assert!(output.contains(if check==0{"changed binding"}else if check==1{"pixel (2559,1599)"}else{"wrong scanout pixel"}),"{output}");
  }
 }
}
'''.replace('@FRAME@',frames).replace('@FILLS@',fills).replace('@GUARD0@',guards[0]).replace('@GUARD1@',guards[1]))
 def test_default_boot_scorer_refuses_failed_self_checks(self):
  sys.path.insert(0,str(ROOT/'scripts/metal'))
  import metal_score
  definition=json.loads((ROOT/'docs/metal/boot-path.json').read_text())
  desktop=next(m for m in definition['milestones'] if m['id']=='desktop')
  catalog=json.loads((ROOT/'tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-x86_64.json').read_text())
  specs={s['name']:s for s in catalog['stages']}
  lines=[(specs[s] if isinstance(s,str) else s)['marker'] for s in desktop['stages']['default']]
  with tempfile.TemporaryDirectory() as directory:
   d=Path(directory)
   files={'run.json':{'disk':'root','mode':'default'},'definition/docs/metal/boot-path.json':dict(definition,milestones=[desktop]),'definition/tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-x86_64.json':catalog}
   for name,value in files.items():
    p=d/name;p.parent.mkdir(parents=True,exist_ok=True);p.write_text(json.dumps(value))
   (d/'events.jsonl').write_text('')
   (d/'serial.txt').write_text('\n'.join(lines)+'\n')
   self.assertFalse(metal_score.falls_short(metal_score.score(d)))
   for success,failure in [('first frame read back:','first frame check FAILED: pixel (2559,1599)'),('desktop fills the scanout:','desktop fill check FAILED: wrong scanout pixel')]:
    (d/'serial.txt').write_text('\n'.join(line.replace(success,failure) for line in lines)+'\n')
    self.assertTrue(metal_score.falls_short(metal_score.score(d)))
    (d/'serial.txt').write_text('[bwm] '+failure+'\n'+'\n'.join(lines)+'\n')
    self.assertTrue(metal_score.falls_short(metal_score.score(d)),'later success hid FAILED')
if __name__=='__main__':unittest.main()
