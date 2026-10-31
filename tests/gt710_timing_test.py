"""Check measured native corpus timings and their digest-bound final upload."""
from pathlib import Path
import hashlib
import runpy
import subprocess
import sys
import tempfile
import unittest
ROOT = Path(__file__).resolve().parents[1]

class Timing(unittest.TestCase):
    def test_elapsed_rows_keep_owner_draw_phase_and_report_overflow(self):
        source = f'''extern crate alloc;
#[path="{ROOT / 'kernel/src/gpu/gk208/timing.rs'}"] mod timing;
fn main() {{
 assert!(!timing::collecting(false,false));
 assert!(timing::collecting(false,true));assert!(timing::collecting(true,false));assert!(timing::collecting(true,true));
 let mut r=timing::Recorder::new();
 r.finish(42,0,7,10,20);
 assert!(r.drain(1).is_none());
 assert_eq!(r.begin_draw(42),1);
 r.finish(42,1,7,100,170);
 r.finish(77,1,7,100,999);
 assert_eq!(r.begin_draw(42),2);
 r.finish(42,2,103,200,240);
 let row=r.drain(2).unwrap();
 assert_eq!(row, "GT710_TIMING_V1 {{\\\"batch\\\":2,\\\"dropped\\\":0,\\\"rows\\\":[[42,1,7,100,70],[42,2,103,200,40]]}}");
 assert!(r.drain(3).is_none());
 for n in 0..20000 {{r.finish(42,2,7,n,n+1);}}
 let row=r.drain(4).unwrap();assert!(row.contains("\\\"dropped\\\":3616"));
}}
'''
        priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
        with tempfile.TemporaryDirectory() as d:
            p=Path(d);(p/'timing.rs').write_text(source)
            subprocess.run(priority+['rustc','--edition=2021','-Dwarnings',str(p/'timing.rs'),'-o',str(p/'timing')],check=True)
            subprocess.run([str(p/'timing')],check=True)

    def test_large_timing_export_refuses_allocation_failure_and_retains_rows(self):
        sys.path.insert(0,str(ROOT/'tests'))
        from gt710_allocation_failure_test import ALLOC,run
        fixture=f'#[path="{ROOT / "kernel/src/gpu/gk208/timing.rs"}"] mod timing;\n'+ALLOC+r'''
#[test]fn timing_export_allocation_is_a_refusal(){
 assert!(!timing::collecting(false,false));let mut r=timing::Recorder::new();r.begin_draw(42);
 for n in 0..20000{r.finish(42,1,7,n,n+1);}
 arm();let refused=r.drain(7);ARMED.store(false,Ordering::Relaxed);
 assert!(HIT.load(Ordering::Relaxed));assert!(refused.is_none());
 let row=r.drain(8).unwrap();assert!(row.contains("\"dropped\":3616"));
 assert!(row.contains("[42,1,7,0,1]"));assert!(row.contains("[42,1,7,16383,1]"));
}
'''
        run(fixture,'timing_export_allocation_is_a_refusal',[0])

    def test_timing_bulk_requires_exact_anchor_and_length(self):
        sys.path.insert(0,str(ROOT/'scripts/metal'))
        api=runpy.run_path(str(ROOT/'scripts/metal/gt710-suite'))
        startup=b'GT710_PIX 0 1:ff003039\n'
        row=b'GT710_TIMING_V1 {"batch":1,"dropped":0,"rows":[[42,1,7,100,70]]}\n'
        anchor=f'GT710_TIMING_BULK batch=1 bytes={len(row)-1} sha256={hashlib.sha256(row[:-1]).hexdigest()}\n'
        serial=('GT710_MODE {"schema":1}\n'+f'GT710_BULK {len(startup)} {hashlib.sha256(startup).hexdigest()}\n'
                +'SUITE gpu START total=1\n'+anchor+'SUITE gpu CASE pipelines/sample PASS ms=1\n'
                +'SUITE gpu DONE passed=1 failed=0 skipped=0 total=1\n'
                +'SUITE gpu MEMORY reclaimed=PASS free_before=100 free_after=100 allowance=48\n'
                +f'GT710_FINAL_BULK {len(row)} {hashlib.sha256(row).hexdigest()}\nSUITE_SEQUENCE READY gpu\n')
        with tempfile.TemporaryDirectory() as d:
            p=Path(d);(p/'gt710-pixels.rle').write_bytes(startup);(p/'gt710-final.rle').write_bytes(row)
            (p/'serial.txt').write_text(serial)
            self.assertIn(row.decode()+anchor,api['capture_serial'](p))
            for bad in ['',anchor.replace('batch=1','batch=2'),anchor.replace('bytes=', 'bytes=1'),anchor.replace('sha256=','sha256=0')]:
                (p/'serial.txt').write_text(serial.replace(anchor,bad))
                with self.assertRaises(ValueError):api['capture_serial'](p)
if __name__=='__main__':unittest.main()
