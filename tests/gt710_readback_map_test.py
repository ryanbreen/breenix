"""Exercise transfer mapping with the real resource owner, allocator and VM."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]
PRIORITY = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []


class ReadbackMap(unittest.TestCase):
    def fixture(self, whole_backing=False):
        source = (ROOT / 'kernel/src/gpu/gk208/ce/readback.rs').read_text()
        transfer = item(source, 'pub(crate) fn transfer(')
        if whole_backing:
            marker = 'let img = image(r,id)?;'
            assert marker in transfer
            transfer = transfer.replace(marker, marker + '''
        let pages: Vec<_> = (0..backing.count()).map(|i| backing.page_addr(i)).collect();
        r.map_backing(a,io,id,&backing,&pages,backing.bytes() as u64)?;
''', 1)
        harness = (ROOT / 'tests/gt710_resource_layout_test.rs').read_text().split('#[test]', 1)[0]
        harness = harness.replace('../kernel/', str(ROOT / 'kernel') + '/')
        # Place the real modules under the same namespace as the transfer builder.
        harness = harness[harness.index('extern crate alloc;'):]
        model = harness[harness.index('#[derive(Default)]'):]
        modules = harness[harness.index('#[path'):harness.index('use memory::')].replace(' mod ', ' pub mod ')
        return r'''
#![allow(dead_code)] // Hardware-independent fixture retains complete native modules.
extern crate alloc;
mod gpu {
    pub mod dev { pub struct Tag; }
    pub mod dma {
        pub struct Pages { pub count: usize }
        impl Pages {
            pub fn count(&self)->usize {self.count}
            pub fn bytes(&self)->usize {self.count*4096}
            pub fn page_addr(&self,i:usize)->u64 {assert!(i<self.count);0x12345000+i as u64*8192}
        }
    }
}
mod gk208 {
MODULES
    pub mod cost {
        pub enum Slot {TransferMap}
        pub fn phase(_:Slot) {}
    }
    pub mod adapter {
        pub struct Request { pub plan:super::copy::Plan, _hold:super::ce::readback::TransferHold,
            _backing:Option<std::sync::Arc<crate::gpu::dma::Pages>> }
        impl Request {
            pub(in crate::gk208) fn transfer(_:crate::gpu::dev::Tag,_:u64,_:u64,plan:super::copy::Plan,_:bool,
                hold:super::ce::readback::TransferHold,backing:Option<std::sync::Arc<crate::gpu::dma::Pages>>)->Self {
                Self{plan,_hold:hold,_backing:backing}
            }
        }
    }
    pub mod ce {
        use std::cell::RefCell;
        use super::{resource::Resources,memory::allocator::Allocator};
        pub struct State {pub r:Resources<crate::gpu::dma::Pages>,pub a:Allocator,pub io:crate::Model}
        thread_local! { pub static STATE:RefCell<State>=RefCell::new(State {
            r:Resources::new(),a:Allocator::new(1<<30,&[]).unwrap(),io:crate::Model::default()}); }
        pub fn with_resources<T>(f:impl FnOnce(&mut Resources<crate::gpu::dma::Pages>,&mut Allocator,&mut crate::Model)
            ->Result<T,&'static str>)->Result<T,&'static str> {
            STATE.with(|s| {let State{r,a,io}=&mut *s.borrow_mut();f(r,a,io)})
        }
        pub mod readback {
            use alloc::vec::Vec;
HOLD
DROP
IMAGE
TRANSFER
        }
    }
}
use gk208::{ce,vm};
use std::{collections::{BTreeMap,BTreeSet},sync::Arc};
MODEL
fn mapped(base:u64,count:usize)->BTreeSet<usize> {
    ce::with_resources(|r,_,io| Ok((0..count).filter(|i| r.pte_address(base+*i as u64*4096)
        .is_some_and(|pte| io.words.get(&pte).copied().unwrap_or(0)!=0)).collect())).unwrap()
}
fn expected(offset:u64,stride:u32,width:u32,height:u32)->BTreeSet<usize> {
    (0..height).flat_map(|y| {
        let start=offset+y as u64*stride as u64;
        (start as usize/4096)..=((start+width as u64*4-1) as usize/4096)
    }).collect()
}
fn address(plan:gk208::copy::Plan)->u64 {
    let words=gk208::copy::encode_plan(plan,0x1000,1,1).unwrap();
    let mut at=0;
    while at<words.len() {let h=words[at];let n=((h>>16)&0x1fff) as usize;
        if (h&0x1fff)*4==0x400 {return (u64::from(words[at+3])<<32)|u64::from(words[at+4]);}
        at+=n+1;
    }
    panic!("readback has no CE transfer");
}
#[test] fn tiny_readback_maps_only_row_pages_and_retires_them() {
    let count=2560*1600*4/4096;
    let backing=Arc::new(gpu::dma::Pages{count});
    let baseline=ce::with_resources(|r,a,io| {
        let baseline=a.stats();r.create(a,io,7,1,1,2560,1600)?;
        r.attach(a,io,7,backing.clone(),&[],backing.bytes() as u64)?;Ok(baseline)
    }).unwrap();
    let stride=2560*4;
    let mut want=BTreeSet::new();
    // A tiny top-left read, unaligned rows spanning pages, clipping at the
    // target edge, and overlapping reads must map only the union of row pages.
    let mut retained=Vec::new();
    for (rect,offset) in [([0,0,16,16],0),([3,5,16,16],4090),([2552,1592,16,16],200000)] {
        let request=ce::readback::transfer(gpu::dev::Tag,1,7,backing.clone(),rect,offset,stride,false).unwrap();
        let base=address(request.plan)-offset;
        want.extend(expected(offset,stride,rect[2].min(2560-rect[0]),rect[3].min(1600-rect[1])));
        let got=mapped(base,count);
        assert_eq!(got,want,"readback mapped pages outside its rows");
        if offset==0 {assert_eq!(got.len(),16);}
        for &page in &want {
            ce::with_resources(|r,_,io| {
                assert_eq!(io.words[&r.pte_address(base+page as u64*4096).unwrap()],
                    (backing.page_addr(page)>>8)|1|(1<<32)|(2<<33));Ok(())
            }).unwrap();
        }
        let writes=ce::with_resources(|_,_,io| Ok(io.writes)).unwrap();
        let again=ce::readback::transfer(gpu::dev::Tag,2,7,backing.clone(),rect,offset,stride,false).unwrap();
        assert_eq!(address(again.plan),base+offset);
        assert_eq!(ce::with_resources(|_,_,io| Ok(io.writes)).unwrap(),writes,"repeated read double-mapped pages");
        retained.extend([request,again]);
    }
    ce::with_resources(|r,a,io| {r.unref(7)?;r.collect(a,io)?;assert_ne!(a.stats(),baseline);Ok(())}).unwrap();
    drop(retained);
    ce::with_resources(|r,a,io| {r.collect(a,io)?;assert_eq!(a.stats(),baseline);assert_eq!(r.vram_bytes(),0);Ok(())}).unwrap();
    assert_eq!(Arc::strong_count(&backing),1,"readback backing leaked");
}
'''.replace('MODULES', modules).replace('MODEL', model).replace('HOLD', item(source, 'pub(in crate::gk208) struct TransferHold')) \
    .replace('DROP', item(source, 'impl Drop for TransferHold')).replace('IMAGE', item(source, 'fn image(r:')).replace('TRANSFER', transfer)

    def run_fixture(self, source):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / 'test.rs').write_text(source)
            subprocess.run(PRIORITY + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                str(path / 'test.rs'), '-o', str(path / 'test')], check=True)
            return subprocess.run([str(path / 'test'), '--nocapture'], text=True, capture_output=True)

    def test_row_mappings_and_whole_backing_mutation(self):
        clean = self.run_fixture(self.fixture())
        self.assertEqual(clean.returncode, 0, clean.stdout + clean.stderr)
        mutation = self.run_fixture(self.fixture(whole_backing=True))
        self.assertNotEqual(mutation.returncode, 0, 'whole-backing mutation survived')
        self.assertIn('readback mapped pages outside its rows', mutation.stdout + mutation.stderr)


if __name__ == '__main__':
    unittest.main()
