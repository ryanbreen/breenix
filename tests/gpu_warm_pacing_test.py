"""QEMU's finite catalogue must not linger behind recurring foreground traffic."""
from pathlib import Path
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust
ROOT=Path(__file__).resolve().parents[1]
class WarmPacing(unittest.TestCase):
 def test_qemu_finishes_catalogue_without_foreground_spacing(self):
  source=(ROOT/'kernel/src/gpu/mod.rs').read_text()
  if 'fn warm_deadline(' in source:
   policy=item(source,'fn warm_deadline(')
  else:
   # Execute the existing production timing expression before the helper exists.
   warm=item(source,'fn warm_next(');start=warm.index('let due = c.traffic_ns');end=warm.index('if now < due',start)
   body=warm[start:end].replace('c.traffic_ns','traffic').replace('c.warm_started_ns','started')
   policy='fn warm_deadline(now:u64,traffic:u64,held:u64,started:Option<u64>,eager:bool)->u64{let _=(now,eager);'+body+'due}'
  run_rust(self,r'''
const WARM_QUIET_NS:u64=2_000_000;const WARM_GRACE_NS:u64=20_000_000;
@POLICY@
#[test]fn finite_warmup_is_not_spaced_by_foreground_traffic(){
 let mut started=None;
 for step in 0..16 {let now=(step+1)*1_000_000;let due=warm_deadline(now,now,now,started,true);
  assert_eq!(due,now,"QEMU deferred its fixed catalogue behind foreground traffic");started=Some(now);}
 // Native and the other backends retain their traffic/quiet/grace policy.
 assert_eq!(warm_deadline(30_000_000,30_000_000,30_000_000,Some(29_000_000),false),49_000_000);
 assert_eq!(warm_deadline(30_000_000,0,30_000_000,None,false),2_000_000);
 assert_eq!(warm_deadline(100_000_000,100_000_000,10_000_000,None,false),30_000_000);
}
'''.replace('@POLICY@',policy))
if __name__=='__main__':unittest.main()
