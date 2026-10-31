"""Compare completed DMA pages directly, including awkward page boundaries."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class Compare(unittest.TestCase):
    def test_direct_page_compare_invalidates_and_counts_every_differing_byte(self):
        dma = (ROOT / 'kernel/src/gpu/dma.rs').read_text()
        implementation = item(dma, 'impl super::observation::Memory for Pages')
        implementation = implementation.replace('super::observation::', 'observation::')
        source = r'''
use std::cell::Cell;
#[path="ROOT/kernel/src/gpu/completion.rs"] pub mod gpu_completion;
#[path="ROOT/kernel/src/gpu/observation.rs"] pub mod observation;
const PAGE:usize=4096;
struct Pages { data:Vec<u8>, copies:Cell<usize>, invalidated:Cell<Option<(usize,usize)>> }
impl Pages {
    fn maintain(&self,at:usize,n:usize,cpu:bool) {assert!(cpu);self.invalidated.set(Some((at,n)));}
    fn read(&self,at:usize,out:&mut[u8]) {self.copies.set(self.copies.get()+out.len());out.copy_from_slice(&self.data[at..at+out.len()]);}
    fn try_spans<E>(&self,at:usize,n:usize,mut f:impl FnMut(usize,*mut u8,usize)->Result<(),E>)->Result<(),E> {
        assert_eq!(self.invalidated.take(),Some((at,n)),"compare without invalidation");
        let mut done=0;
        while done<n {let k=(PAGE-(at+done)%PAGE).min(n-done);
            f(done,unsafe{self.data.as_ptr().add(at+done) as *mut u8},k)?;done+=k;}
        Ok(())
    }
}
IMPLEMENTATION
#[test] fn complete_sixteen_mib_and_page_edges_need_no_scratch_copy() {
    let n=16<<20;let mut data:Vec<_>=(0..n+PAGE).map(|i|(i^(i>>9)) as u8).collect();
    let expected=data.clone();
    // Include both poison values as actual pixels, and mismatches in each word lane.
    data[0..8].fill(0x55);data[8..16].fill(0xaa);
    let memory=Pages{data,copies:Cell::new(0),invalidated:Cell::new(None)};
    for (at,len) in [(0,n),(1,8193),(4095,8201),(4096,4096),(17,63)] {
        let want=&expected[at..at+len];
        let count=want.iter().zip(&memory.data[at..at+len]).filter(|(a,b)|a!=b).count() as u32;
        assert_eq!(observation::compare(&memory,at,want),count);
        assert_eq!(memory.copies.get(),0,"second pass copied through scratch");
    }
    assert_eq!(observation::compare(&memory,0,&memory.data[..n]),0);
}
'''.replace('ROOT', str(ROOT)).replace('IMPLEMENTATION', implementation)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            (p / 'test.rs').write_text(source)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings',
                '--crate-name', 'gpu_abi', '--crate-type', 'rlib', str(ROOT / 'libs/gpu-abi/src/lib.rs'),
                '-o', str(p / 'libgpu_abi.rlib')], check=True)
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                '--extern', f'gpu_abi={p / "libgpu_abi.rlib"}', str(p / 'test.rs'),
                '-o', str(p / 'test')], check=True)
            subprocess.run([str(p / 'test')], check=True)


    def test_cached_mapping_requires_identical_backing_and_confirmed_publication(self):
        source = (ROOT / 'tests/gt710_resource_layout_test.rs').read_text()
        source = source.replace('../kernel/', str(ROOT / 'kernel') + '/')
        source += r'''
#[test] fn cached_transfer_mapping_identity_and_publication() {
    for fail in [false,true] {
        let mut a=Allocator::new(1<<30,&[]).unwrap();let mut io=Model::default();
        let mut r=Resources::<Vec<u8>>::new();r.create(&mut a,&mut io,7,1,1,48,17).unwrap();
        let backing=Arc::new(vec![0;8192]);
        r.attach(&mut a,&mut io,7,backing.clone(),&[],8192).unwrap();
        assert_eq!(r.mapped_backing(7,&backing),None);
        io.fail=fail;
        let mapped=r.map_backing(&mut a,&mut io,7,&backing,&[0x12345000,0x12346000],8192);
        let writes=io.writes;let invalidates=io.invalidates;let used=a.stats();
        if fail { assert!(mapped.is_err());assert_eq!(r.mapped_backing(7,&backing),None); }
        else { assert_eq!(r.mapped_backing(7,&backing),Some(mapped.unwrap())); }
        assert_eq!(r.mapped_backing(7,&Arc::new(vec![0;8192])),None);
        assert_eq!((io.writes,io.invalidates,a.stats()),(writes,invalidates,used));
        r.unref(7).unwrap();assert_eq!(r.mapped_backing(7,&backing),None);
    }
}
'''
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            (p / 'test.rs').write_text(source)
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                str(p / 'test.rs'), '-o', str(p / 'test')], check=True)
            subprocess.run([str(p / 'test'), '--exact',
                'cached_transfer_mapping_identity_and_publication'], check=True)


if __name__ == '__main__':
    unittest.main()
