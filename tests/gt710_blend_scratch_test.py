"""Run native scratch ownership with real allocation, VM and request retirement."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class BlendScratch(unittest.TestCase):
    def test_scratch_charge_follows_fence_and_uncertain_storage(self):
        core = (ROOT / 'kernel/src/gpu/mod.rs').read_text()
        self.assertTrue('fn native_blend_scratch(' in core,
                      'native submit still skips plan.scratch')
        ownership = '\n'.join(item(core, marker) for marker in (
            'struct NativeScratch {', 'impl Drop for NativeScratch {',
            'fn native_blend_scratch(', 'fn native_spans('))
        # Reuse the controlled native channel boundary; allocator/VM, adapter,
        # scratch preparation and submission retirement are production code.
        boundary = (ROOT / 'tests/gt710_adapter_test.rs').read_text().split('fn request(caller:')[0]
        boundary = boundary.replace('pub mod dma { pub struct Pages; }', r'''pub mod dma {
        pub struct Pages(std::cell::RefCell<Vec<u8>>);
        impl Pages {
            pub fn new(count: usize) -> Option<Self> {
                if crate::FAIL_BACKING.load(std::sync::atomic::Ordering::SeqCst) { return None; }
                Some(Self(std::cell::RefCell::new(vec![0;count*4096])))
            }
            pub fn count(&self) -> usize { self.bytes()/4096 }
            pub fn bytes(&self) -> usize { self.0.borrow().len() }
            pub fn page_addr(&self, i: usize) -> u64 { 0x12345000+i as u64*4096 }
            pub fn identify(&self, _: &str, _: u32, _: u32) {}
            pub fn write(&self, at: usize, bytes: &[u8]) { self.0.borrow_mut()[at..at+bytes.len()].copy_from_slice(bytes); }
            pub fn read(&self, at: usize, bytes: &mut [u8]) { bytes.copy_from_slice(&self.0.borrow()[at..at+bytes.len()]); }
            pub fn maintain(&self, _: usize, _: usize, _: bool) {}
        }
    }''')
        boundary = boundary.replace('pub mod dev {', r'''
    pub fn native_spans_upload(id: u32, plan: crate::gk208::copy::Plan, backing: std::sync::Arc<dma::Pages>)
        -> Result<(), &'static str> {
        // Substitute only the CE boundary; retain production registry ownership.
        crate::gk208::ce::with_resources(|r,_,io| {
            if io.fail_upload { return Err("injected CE upload failure"); }
            let words=crate::gk208::copy::encode_plan(plan,0x1000,17,23)?;
            assert!(words.contains(&0x686), "upload must launch a tiled CE destination");
            let layout=r.layout(id).unwrap(); let va=r.address(id).unwrap();
            let physical=(io.words[&r.pte_address(va).unwrap()]&0xfffffff0)<<8;
            for y in 0..layout.height { for x in (0..layout.width).step_by(2) {
                let mut pair=[0;8];let n=(layout.width-x).min(2) as usize*4;
                backing.read(y as usize*layout.pitch as usize+x as usize*4,&mut pair[..n]);
                // A modeled GPU store, not vm::Io's CPU PRAMIN write hook.
                io.words.insert(physical+layout.offset(x,y,0).unwrap(),u64::from_le_bytes(pair));
            }}
            Ok(())
        })
    }
    pub fn native_spans_readback(_: u32, width: u32, height: u32, backing: std::sync::Arc<dma::Pages>)
        -> Result<[Vec<u8>;2], &'static str> {
        crate::SPAN_DIAGNOSTICS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut image=vec![0;width as usize*height as usize*4];
        let layout=crate::gk208::resource::Layout::new(width,height,4)?;
        for (y,row) in image.chunks_exact_mut(width as usize*4).enumerate() {
            backing.read(y*layout.pitch as usize,row);
        }
        Ok([image.clone(),image])
    }
    pub mod dev {''')
        boundary = boundary.replace('std::sync::Arc::new(crate::gpu::dma::Pages)',
                                    'std::sync::Arc::new(crate::gpu::dma::Pages::new(1).unwrap())')
        boundary = boundary.replace('pub mod gk208 {', '''pub mod gk208 {
    #[allow(dead_code)] #[path = "memory.rs"] pub mod memory;
    #[allow(dead_code)] #[path = "vm.rs"] pub mod vm;
    #[allow(dead_code)] #[path = "resource.rs"] pub mod resource;
    #[path = "spans_diagnostic.rs"] pub mod spans_diagnostic;
    pub fn hash(_: &[u8]) -> String { String::from("mock boundary digest") }
    pub mod bulk { pub fn line(_: String) -> Result<(), &'static str> { Ok(()) } }
    pub mod ce {
        pub fn with_resources<T>(f: impl FnOnce(&mut super::resource::Resources<crate::gpu::dma::Pages>,
            &mut super::memory::allocator::Allocator, &mut crate::Model) -> Result<T, &'static str>) -> Result<T, &'static str> {
            crate::OWNER.with(|o| { let mut o=o.borrow_mut(); let (r,a,io)=&mut *o; f(r,a,io) })
        }
    }
    pub fn encoded(ids: &[u32]) -> adapter::EncodedDraw {
        let storage=encode::storage(ids);
        assert_eq!((storage.resource,storage.address),(7,1<<32));
        adapter::EncodedDraw { words:vec![0x20010367,0],target:19,transfer:None,_storage:storage }
    }
    pub mod encode {
        // STORAGE
        pub(super) fn storage(ids: &[u32]) -> std::sync::Arc<Storage> {
            let holds=super::ce::with_resources(|r,_,_| ids.iter().map(|&id| r.hold(id)).collect()).unwrap();
            std::sync::Arc::new(Storage { _backing: std::sync::Arc::new(crate::gpu::dma::Pages), holds, resource: 7, address: 1<<32 })
        }
    }
''')
        boundary = boundary.replace('copy::Rect,', '').replace('Device::new(NativeChannel(s.clone()),gk208::caps::BoardState::empty())', 'Device::registered(NativeChannel(s.clone()),gk208::caps::BoardState::empty(),0)')
        encode = (ROOT / 'kernel/src/gpu/gk208/encode.rs').read_text()
        storage = item(encode, 'pub struct Spans {') + '\n#[derive(Clone, Copy)]\n' + item(encode, 'pub struct Target {') + '\n' + item(encode, 'pub(in crate::gk208) struct Storage {') + '\n' + item(encode, 'impl Drop for Storage {')
        boundary = boundary.replace('// STORAGE', storage)
        boundary = boundary.replace('std::sync::Arc::new(crate::gpu::dma::Pages)',
                                    'std::sync::Arc::new(crate::gpu::dma::Pages::new(1).unwrap())')
        adapter = (ROOT / 'kernel/src/gpu/gk208/adapter.rs').read_text()
        adapter = adapter.replace('    #[cfg(target_os = "none")]\n    pub(in crate::gk208) _storage:',
                                  '    pub(in crate::gk208) _storage:')
        boundary = boundary.replace('#[path = "adapter.rs"] pub mod adapter;', '// ADAPTER')
        boundary = boundary.replace('../kernel/', str(ROOT / 'kernel') + '/')
        harness = (ROOT / 'tests/fixtures/gt710_blend_scratch.rs').read_text()
        boundary = boundary.replace('extern crate alloc;', 'extern crate alloc;\n#[macro_export] macro_rules! serial_println { ($($arg:tt)*) => { let _ = format!($($arg)*); }; }')
        harness = boundary + harness.replace('// OWNERSHIP', ownership).replace(
            '// RETIRE', item(core, '    fn retire_if_done('))
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            resource = (ROOT / 'kernel/src/gpu/gk208/resource.rs').read_text()
            resource += '\n' + item(resource, 'pub(crate) fn blend_staging(')
            resource += '\n' + item(resource, 'pub(crate) fn retire_blend_staging(')
            resource += '\n' + item(resource, 'pub(crate) fn span_staging(')
            (p / 'resource.rs').write_text(resource)
            harness = harness.replace('#[path = "resource.rs"]', f'#[path = "{p / "resource.rs"}"]')
            (p / 'adapter.rs').write_text(adapter)
            harness = harness.replace('// ADAPTER', f'#[path = "{p / "adapter.rs"}"] pub mod adapter;')
            (p / 'scratch.rs').write_text(harness)
            subprocess.run(priority + ['rustc', '--edition=2021', '--crate-name', 'gpu_abi',
                           '--crate-type', 'rlib', '--cfg', 'feature="alloc"', '-Dwarnings',
                           str(ROOT / 'libs/gpu-abi/src/lib.rs'), '-o', str(p / 'libgpu_abi.rlib')], check=True)
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                           '--extern', f'gpu_abi={p / "libgpu_abi.rlib"}',
                           str(p / 'scratch.rs'), '-o', str(p / 'scratch')], check=True)
            subprocess.run([str(p / 'scratch'), '--test-threads=1'], check=True)


if __name__ == '__main__':
    unittest.main()
