"""Execute production native installation and admission with a controlled channel."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class NativeInstall(unittest.TestCase):
    def test_install_registers_device_and_refuses_only_unprobed_formats(self):
        source = (ROOT / 'kernel/src/gpu/mod.rs').read_text()
        install = item(source, 'pub(crate) fn install_native(')
        fixture = (ROOT / 'tests/gt710_adapter_test.rs').read_text()
        fixture += r"""
mod installation {
    use crate::{gk208, gpu::dev::{self, Device}, device};
    use gpu_abi as abi;
    use abi::capability;
    struct Core { dev: Option<Device>, queue_size: usize, status: u32, caps: abi::Caps }
    std::thread_local! { static CORE: std::cell::RefCell<Option<Core>> = const { std::cell::RefCell::new(None) }; }
    fn with_core<R>(f: impl FnOnce(&mut Core) -> R) -> Option<R> {
        CORE.with(|c| c.borrow_mut().as_mut().map(f))
    }
    // INSTALL
    #[test] fn registration_requires_a_live_backend_and_retains_real_caps() {
        CORE.with(|c| *c.borrow_mut() = Some(Core { dev: None, queue_size: 0, status: abi::status::NO_DEVICE, caps: abi::Caps::ZERO }));
        let (d, state) = device();
        assert!(install_native(d).is_err(), "unprobed board must not be registered");
        let (mut d, _) = device();
        let mut board = gk208::caps::BoardState::empty();
        board.chipset = 0x106;
        board.channel = (0x123000, 17);
        board.graphics_ready = true;
        board.created_classes = abi::gk208::CAPSET_IDS;
        d.set_board(board);
        assert!(install_native(d).is_ok());
        with_core(|core| {
            let Some(Device::Gk208(d)) = core.dev.as_ref() else { panic!("native backend missing") };
            assert_eq!(d.admission(None), Err((abi::status::MISSING_CAPABILITY, capability::BGRA_RENDER)));
            assert_eq!(d.admission(Some(capability::FEATURE_3D)), Err((abi::status::MISSING_CAPABILITY, capability::FEATURE_3D)));
            assert_eq!(core.caps.backend, abi::backend::GK208);
            assert_eq!(core.queue_size, 4);
            assert_eq!(core.status, abi::status::MISSING_CAPABILITY);
        }).unwrap();
        state.borrow_mut().fault = true;
    }
}
"""
        fixture = fixture.replace('// INSTALL', install)
        fixture = fixture.replace('pub enum Reap {Empty,Done(Done),Fault}',
            'pub enum Reap {Empty,Done(Done),Fault} pub enum Device { Gk208(crate::gk208::adapter::Device), Other }')
        # Supply observed board facts through the actual constructor, not a fake CAPS response.
        fixture = fixture.replace('d.set_board(board);', 'd = gk208::adapter::Device::new(crate::gk208::gr::channel::NativeChannel(state.clone()), board);')
        fixture = fixture.replace('let (mut d, _) = device();', 'let (d0, state) = device(); drop(d0); let mut d;')
        fixture = fixture.replace('gpu::dev::{self, Device}', 'gpu::dev::Device')
        fixture = fixture.replace('../../kernel/', str(ROOT / 'kernel') + '/').replace('../kernel/', str(ROOT / 'kernel') + '/')
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / 'install.rs').write_text(fixture)
            subprocess.run(priority + ['rustc', '--edition=2021', '--crate-name', 'gpu_abi', '--crate-type', 'rlib', str(ROOT / 'libs/gpu-abi/src/lib.rs'), '-o', str(path / 'libgpu_abi.rlib')], check=True)
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings', '--extern', f'gpu_abi={path / "libgpu_abi.rlib"}', str(path / 'install.rs'), '-o', str(path / 'install')], check=True)
            subprocess.run([str(path / 'install'), 'installation::', '--nocapture'], check=True)


if __name__ == '__main__':
    unittest.main()
