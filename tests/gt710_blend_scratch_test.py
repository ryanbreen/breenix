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
            'fn native_blend_scratch('))
        # Reuse the controlled native channel boundary; allocator/VM, adapter,
        # scratch preparation and submission retirement are production code.
        boundary = (ROOT / 'tests/gt710_adapter_test.rs').read_text().split('fn request(caller:')[0]
        boundary = boundary.replace('pub mod gk208 {', '''pub mod gk208 {
    #[allow(dead_code)] #[path = "memory.rs"] pub mod memory;
    #[allow(dead_code)] #[path = "vm.rs"] pub mod vm;
    #[allow(dead_code)] #[path = "resource.rs"] pub mod resource;
    pub mod ce {
        pub fn with_resources<T>(f: impl FnOnce(&mut super::resource::Resources<crate::gpu::dma::Pages>,
            &mut super::memory::allocator::Allocator, &mut crate::Model) -> Result<T, &'static str>) -> Result<T, &'static str> {
            crate::OWNER.with(|o| { let mut o=o.borrow_mut(); let (r,a,io)=&mut *o; f(r,a,io) })
        }
    }
    pub mod encode {
        // STORAGE
        pub fn storage(ids: &[u32]) -> std::sync::Arc<Storage> {
            let holds=super::ce::with_resources(|r,_,_| ids.iter().map(|&id| r.hold(id)).collect()).unwrap();
            std::sync::Arc::new(Storage { _backing: std::sync::Arc::new(crate::gpu::dma::Pages), holds, resource: 7, address: 1<<32 })
        }
    }
''')
        encode = (ROOT / 'kernel/src/gpu/gk208/encode.rs').read_text()
        storage = item(encode, 'pub(in crate::gk208) struct Storage {') + '\n' + item(encode, 'impl Drop for Storage {')
        boundary = boundary.replace('// STORAGE', storage)
        adapter = (ROOT / 'kernel/src/gpu/gk208/adapter.rs').read_text()
        adapter = adapter.replace('    #[cfg(target_os = "none")]\n    pub(in crate::gk208) _storage:',
                                  '    pub(in crate::gk208) _storage:')
        boundary = boundary.replace('#[path = "adapter.rs"] pub mod adapter;', '// ADAPTER')
        boundary = boundary.replace('../kernel/', str(ROOT / 'kernel') + '/')
        harness = (ROOT / 'tests/fixtures/gt710_blend_scratch.rs').read_text()
        harness = boundary + harness.replace('// OWNERSHIP', ownership).replace(
            '// RETIRE', item(core, '    fn retire_if_done('))
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
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
