"""Heap evidence covers each actual GPU case boundary in the final G12 upload."""
from pathlib import Path
import subprocess,sys,tempfile,unittest
ROOT=Path(__file__).resolve().parents[1]
class Boundaries(unittest.TestCase):
    def test_case_hooks_bracket_the_manifest_and_attach_only_to_final_upload(self):
        runner=(ROOT/'libs/libbreenix/src/suite.rs').read_text();gpu=(ROOT/'userspace/programs/src/suite_gpu.rs').read_text();transport=(ROOT/'userspace/programs/src/gt710_artifacts.rs').read_text()
        self.assertTrue('with_case_observer' in runner,'the parent runner has no case-boundary observer')
        self.assertIn('observe(0, total, "")',runner)
        self.assertIn('observe(done, total, &name)',runner)
        self.assertLess(runner.index('observe(0, total, "")'),runner.index('let mut outcome = run_case('))
        self.assertGreater(runner.index('observe(done, total, &name)'),runner.index('emit(&line);',runner.index('let mut outcome = run_case(')))
        self.assertIn('.with_case_observer(heap_boundary)',gpu)
        self.assertIn('gt710_heap::encode()',gpu)
        self.assertIn('upload_slot_with(',gpu)
        self.assertIn('data.extend_from_slice(extra)',transport)
        self.assertLess(transport.index('data.extend_from_slice(extra)'),transport.index('sha256(&data)'))

    def test_heap_transport_requires_complete_ordered_rows_and_its_digest(self):
        import hashlib,json,re,runpy
        api=runpy.run_path(str(ROOT/'scripts/metal/gt710-suite'))
        names=re.findall(r'"([^"\n]+)"',(ROOT/'libs/gpu-abi/src/heap_trace.rs').read_text().split('pub const NAMES:',1)[1].split('];',1)[0])
        values=dict.fromkeys(names,0);values.update(schema=1,heap_free=32<<20,heap_total=64<<20)
        rows=[dict(schema=1,boundary=n,case=name,values=values.copy()) for n,name in enumerate(['','pipelines/sample'])]
        startup=b'GT710_PIX 0 1:ff003039\n'
        def write(path,records,anchor=True,bad_digest=False):
            body=''.join('GT710_CASE_HEAP '+json.dumps(r,separators=(',',':'))+'\n' for r in records).encode()
            digest=hashlib.sha256(body).hexdigest()
            own=f'GT710_CASE_HEAP_BULK {len(body)} {digest}\n' if anchor else ''
            if bad_digest:own=own.replace(digest,'0'*64)
            serial=('GT710_MODE {"schema":1}\n'+f'GT710_BULK {len(startup)} {hashlib.sha256(startup).hexdigest()}\n'
                +'SUITE gpu START total=1\nSUITE gpu CASE pipelines/sample PASS ms=1\n'
                +'SUITE gpu DONE passed=1 failed=0 skipped=0 total=1\n'
                +'SUITE gpu MEMORY reclaimed=PASS free_before=100 free_after=100 allowance=48\n'+own
                +f'GT710_FINAL_BULK {len(body)} {digest}\nSUITE_SEQUENCE READY gpu\n')
            (path/'serial.txt').write_text(serial);(path/'gt710-pixels.rle').write_bytes(startup);(path/'gt710-final.rle').write_bytes(body)
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp);write(p,rows);self.assertIn('GT710_CASE_HEAP ',api['capture_serial'](p))
            for records in (rows[:1],[rows[0],dict(rows[1],boundary=2)],[rows[0],dict(rows[1],case='pipelines/wrong')],[rows[0],dict(rows[1],schema=2)],[rows[0],dict(rows[1],values=dict(values,heap_free=True))]):
                write(p,records)
                with self.assertRaises(ValueError):api['capture_serial'](p)
            for anchor,bad in ((False,False),(True,True)):
                write(p,rows,anchor,bad)
                with self.assertRaises(ValueError):api['capture_serial'](p)
        producer=(ROOT/'userspace/programs/src/gt710_artifacts.rs').read_text()
        self.assertIn('GT710_CASE_HEAP_BULK',producer)

    def test_complete_ordered_bounded_records_and_bad_snapshots(self):
        source=(ROOT/'userspace/programs/src/gt710_heap.rs').read_text()
        code=r'''
extern crate self as libbreenix;
pub mod gpu{pub use gpu_abi as abi;}
mod heap{SOURCE
#[cfg(test)]mod checks{
 use super::*;
 fn sample()->[u64;wire::WORDS]{let mut a=[0;wire::WORDS];a[0]=1;a[1]=32*1024*1024;a}
 #[test]fn all_manifest_boundaries_are_retained_in_order(){
  let _=boundary as fn(usize,usize,&str);assert!(super::encode().is_err());
  let mut s=Store::new();s.record(0,170,"",Ok(sample()));
  for n in 1..=170{s.record(n,170,"present/owner",Ok(sample()));}
  let encoded=s.encode().unwrap();let text=std::str::from_utf8(&encoded).unwrap();
  assert_eq!(text.lines().filter(|l|l.starts_with("GT710_CASE_HEAP ")).count(),171);
  assert!(text.contains("\"boundary\":170"));assert!(encoded.len()<256*1024);
 }
 #[test]fn missing_reordered_failed_or_oversized_boundaries_are_refused(){
  let mut s=Store::new();s.record(0,2,"",Ok(sample()));assert!(s.encode().is_err());
  s.record(2,2,"present/owner",Ok(sample()));assert!(s.encode().is_err());
  let mut s=Store::new();s.record(0,1,"",Ok(sample()));s.record(1,1,"present/owner",Err(-12));assert!(s.encode().is_err());
  let mut s=Store::new();s.record(0,171,"",Ok(sample()));assert!(s.encode().is_err());
  let mut s=Store::new();s.record(0,1,"",Ok(sample()));s.record(1,1,&"x".repeat(100),Ok(sample()));assert!(s.encode().is_err());
 }
 #[test]fn snapshots_have_a_version_and_exact_length(){
  let a=sample();let bytes:Vec<_>=a.into_iter().flat_map(u64::to_le_bytes).collect();
  assert_eq!(wire::decode(&bytes),Some(a));assert_eq!(wire::decode(&bytes[..bytes.len()-1]),None);
  let mut b=bytes;b[0]=0;assert_eq!(wire::decode(&b),None);
 }
}
}
'''.replace('SOURCE',source)
        priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp);(p/'heap.rs').write_text(code)
            subprocess.run(priority+['rustc','--edition=2021','--crate-name','gpu_abi','--cfg','feature="alloc"','--crate-type','rlib',str(ROOT/'libs/gpu-abi/src/lib.rs'),'-o',str(p/'libgpu_abi.rlib')],check=True)
            subprocess.run(priority+['rustc','--edition=2021','--test','-Dwarnings','--extern',f'gpu_abi={p/"libgpu_abi.rlib"}',str(p/'heap.rs'),'-o',str(p/'heap')],check=True)
            subprocess.run([str(p/'heap'),'--nocapture'],check=True)
if __name__=='__main__':unittest.main()
