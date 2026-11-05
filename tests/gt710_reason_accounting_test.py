"""Native retained-reason accounting must be measured, complete and bounded."""
import copy
import json
from pathlib import Path
import re
import runpy
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
API = runpy.run_path(str(ROOT / 'scripts/metal/gt710-suite'))


def fixture():
    # Two excluded failures add two one-page malloc blocks; ordinary variation is 47 pages.
    manifest=json.loads((ROOT/'docs/suites/breengel.json').read_text())
    names=[c['id']+'/'+case['id'] for c in manifest['categories'] for case in c['cases']]
    failed={'panel/cursor-restore','panel/cursor-trail-detected'}
    owner='gpu_status=0 contexts=0 device_contexts=0 resources=0 resource_bytes=16384000 copy_bytes=0 picture_bytes=0 readback_bytes=0 panes=0 thumbnails=0 runner_threads=1'
    lines = ['SUITE breengel START cases=401',
        'BREENGEL OWNERS before '+owner,
        'BREENGEL TABLES before runner_pte_kb=40 all_pte_kb=120 top_pte=7:runner:40,2:init:32,9:worker:16',
        'SUITE breengel MEMORY before free=1000 heap_free_kb=100 user_stacks=16 user_pages=100 page_tables=10 page_cache=0 kernel_stacks=8 buffers=20 other=5 receipts=0 settle_ms=200']
    lines += [f'SUITE breengel CASE {name} '+('FAIL ms=1 msg=pointer unavailable' if name in failed else 'PASS ms=1') for name in names]
    lines += ['SUITE breengel DONE passed=399 failed=2 skipped=0 total=401',
        'SUITE breengel MEMORY after free=951 heap_free_kb=100 user_stacks=16 user_pages=102 page_tables=57 page_cache=0 kernel_stacks=8 buffers=20 other=5 receipts=0 settle_ms=200',
        'SUITE breengel MEMORY reclaimed=FAIL free_before=1000 free_after=951 allowance=48',
        'BREENGEL OWNERS after '+owner,
        'BREENGEL TABLES after runner_pte_kb=52 all_pte_kb=136 top_pte=7:runner:52,2:init:32,9:worker:20']
    rows = [[0x100010+i*4096,64,1,names.index(name),(b'pointer unavailable'+bytes(45)).hex()] for i,name in enumerate(names) if name in failed]
    record = dict(schema=1, phase=401, dropped=0, count=2, entries=rows)
    return lines, record


def serial(lines, record):
    return '\n'.join(lines + ['GT710_BREENGL_ALLOCATIONS '+json.dumps(record,separators=(',',':'))])+'\n'


class ReasonAccounting(unittest.TestCase):
    def test_live_tracker_retirement_reallocation_and_capture(self):
        path=ROOT/'userspace/programs/src/gt710_allocs.rs'
        self.assertTrue(path.exists(), 'retain actual live allocation evidence')
        code=r'''
mod census {SOURCE
 pub fn capacity_probe(){let mut t=Table::new();
 for i in 1..=SLOTS{assert!(t.insert(Slot{pointer:i*4096+16,size:32,align:1,birth:1}));}
 assert!(!t.insert(Slot{pointer:(SLOTS+1)*4096+16,size:32,align:1,birth:1}));
 for i in (1..=SLOTS).step_by(2){assert!(t.remove(i*4096+16));}
 for i in (2..=SLOTS).step_by(2){assert!(t.remove(i*4096+16));}
 assert!(t.slots.iter().all(|s|s.pointer==0));
 }
}
fn main(){census::capacity_probe();census::prepare().unwrap();
 unsafe extern "C" {fn fork()->i32;fn waitpid(pid:i32,status:*mut i32,flags:i32)->i32;fn _exit(status:i32)->!;}
 let child=unsafe{fork()};assert!(child>=0);
 if child==0 {let mut b=vec![0u8;64];census::allocated(b.as_mut_ptr(),64,1);
  unsafe{_exit(if census::encode().is_err(){0}else{17})};}
 let mut status=0;assert_eq!(unsafe{waitpid(child,&mut status,0)},child);
 assert_eq!(status,0,"forked cases must disarm their inherited census");
 let mut a=vec![0u8;80];a[..19].copy_from_slice(b"pointer unavailable");
 let b=vec![0u8;64];census::boundary(17);
 census::allocated(a.as_mut_ptr(),64,1);census::allocated(b.as_ptr() as *mut u8,64,1);
 census::deallocated(b.as_ptr() as *mut u8,64,1);
 census::reallocated(a.as_mut_ptr(),64,1,std::ptr::null_mut(),80);
 census::reallocated(a.as_mut_ptr(),64,1,a.as_mut_ptr(),80);
 census::allocated(a.as_mut_ptr(),80,1); // the same allocation is not counted twice
 census::boundary(401);
 println!("{}",String::from_utf8(census::encode().unwrap()).unwrap());
 assert!(census::encode().is_err(),"the snapshot freezes live ownership once");
}
'''.replace('SOURCE',path.read_text())
        priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
        with tempfile.TemporaryDirectory(prefix='gt710-reason-') as tmp:
            p=Path(tmp);(p/'reason.rs').write_text(code)
            subprocess.run(priority+['rustc','--edition=2021','-Dwarnings',str(p/'reason.rs'),'-o',str(p/'reason')],check=True)
            result=subprocess.check_output([str(p/'reason')],text=True)
        row=json.loads(result.removeprefix('GT710_BREENGL_ALLOCATIONS '))
        self.assertEqual((row['phase'],row['dropped'],row['count']),(401,0,1))
        self.assertEqual(row['entries'][0][1:4],[80,1,17])
        self.assertTrue(bytes.fromhex(row['entries'][0][4]).startswith(b'pointer unavailable'))

    def test_exception_requires_real_distinct_case_allocations_and_no_extra_loss(self):
        check=API.get('breengel_memory_accounting')
        self.assertTrue(callable(check), 'add the coordinator-authorized measured accounting gate')
        lines,row=fixture()
        result=check(serial(lines,row))
        self.assertEqual((result['reason_pages'],result['residual_pages']),(2,47))
        self.assertEqual(result['reason_bytes'],38)
        for mutate in [lambda r:r.update(dropped=1),lambda r:r.update(phase=400),
                       lambda r:r['entries'].pop(),
                       lambda r:r['entries'][1].__setitem__(0,r['entries'][0][0]),
                       lambda r:r['entries'][0].__setitem__(3,0),
                       lambda r:r['entries'][0].__setitem__(4,bytes(64).hex()),
                       lambda r:(r['entries'][0].__setitem__(1,4096),r['entries'][0].__setitem__(4,(b'pointer unavailable'+bytes(141)).hex()))]:
            bad=copy.deepcopy(row);mutate(bad)
            with self.subTest(record=bad),self.assertRaises(ValueError):check(serial(lines,bad))
        bad=[l.replace('free=951','free=949').replace('free_after=951','free_after=949').replace('page_tables=57','page_tables=59') for l in lines]
        with self.assertRaises(ValueError):check(serial(bad,row))
        with self.assertRaises(ValueError):check(serial(lines,row).replace('runner_threads=1','runner_threads=2'))
        with self.assertRaises(ValueError):check(serial(lines,row).replace('resources=0','resources=1'))
        with self.assertRaises(ValueError):check(serial(lines,row).replace('CASE panel/cursor-restore FAIL','CASE panel/cursor-restore PASS'))
        with self.assertRaises(ValueError):check(serial(lines,row).replace('allowance=48','allowance=64'))
        # A 401/401 passing run must reclaim memory without the intermediate exception.
        all_pass=[re.sub(r'FAIL ms=1 msg=.*','PASS ms=1',l).replace('passed=399 failed=2','passed=401 failed=0') for l in lines]
        with self.assertRaises(ValueError):check(serial(all_pass,dict(row,count=0,entries=[])))


if __name__=='__main__':unittest.main()
