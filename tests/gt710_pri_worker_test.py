"""Exercise the production CAPS loop with a deferred native PRI sample."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class PriWorker(unittest.TestCase):
    def test_direct_caps_poll_services_pri_outside_core(self):
        source = (ROOT / 'kernel/src/gpu/gk208/caps/executable.rs').read_text()
        execute = item(source, '    fn execute(')
        fixture = (ROOT / 'tests/gt710_adapter_test.rs').read_text()
        fixture = fixture.replace('../kernel/', str(ROOT / 'kernel') + '/')
        fixture = fixture.replace('pub mod gk208 {', '''pub mod gk208 {
            pub mod channel {
                pub fn elapsed(_: u64) -> u64 {
                    crate::arch_impl::x86_64::timer::rdtsc()
                }
            }
        ''', 1)
        fixture += r'''
mod arch_impl {pub mod x86_64 {pub mod timer {
    use std::sync::atomic::{AtomicU64, Ordering};
    static TICKS: AtomicU64 = AtomicU64::new(0);
    pub fn rdtsc() -> u64 {TICKS.fetch_add(10000, Ordering::Relaxed)}
}}}
mod direct_caps {
    use crate::{gk208::adapter, gpu::dev::Reap};
    enum Request {Gk208(adapter::Request), Other}
    impl Request {fn fence(&self) -> Option<u64> {
        match self {Self::Gk208(r) => r.fence, Self::Other => None}
    }}
    impl crate::gpu::dev::Done {
        fn matches_fence(&self, fence: u64) -> bool {
            self.fenced && self.fence == Some(fence)
        }
    }
    struct Runner<'a> {device: &'a mut adapter::Device}
    impl Runner<'_> {
        // EXECUTE
    }
    #[test]
    fn transient_pri_resumes_startup_without_reset_or_timeout() {
        let _ = Request::Other;
        let (mut device, state) = crate::device();
        let mut runner = Runner {device: &mut device};
        {let mut s = state.borrow_mut();
            s.transient_trap = true;
            s.registers.insert(0x400100, 0xbadf1201);
        }
        let request = adapter::Request::storage(crate::Tag::Sync(42), None);
        runner.execute(Some(Request::Gk208(request))).unwrap();
        assert_eq!(state.borrow().now, 10);
        assert!(state.borrow().writes.is_empty());
        assert_eq!(runner.device.outstanding(), 0);
    }
}
'''.replace('// EXECUTE', execute)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'caps.rs').write_text(fixture)
            subprocess.run(priority + ['rustc', '--edition=2021', '--crate-name', 'gpu_abi',
                           '--crate-type', 'rlib', '-Dwarnings', str(ROOT / 'libs/gpu-abi/src/lib.rs'),
                           '-o', str(path / 'libgpu_abi.rlib')], check=True)
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                           '--extern', f'gpu_abi={path / "libgpu_abi.rlib"}',
                           str(path / 'caps.rs'), '-o', str(path / 'caps')], check=True)
            subprocess.run([str(path / 'caps'), 'direct_caps::', '--nocapture'], check=True)


if __name__ == '__main__':
    unittest.main()
