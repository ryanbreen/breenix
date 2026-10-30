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
    use crate::{gk208, gpu::dev::Device, device};
    use gpu_abi as abi;
    use abi::{capability, Plain};
    struct Core { dev: Option<Device>, queue_size: usize, status: u32, caps: abi::Caps }
    std::thread_local! { static CORE: std::cell::RefCell<Option<Core>> = const { std::cell::RefCell::new(None) }; }
    fn with_core<R>(f: impl FnOnce(&mut Core) -> R) -> Option<R> {
        CORE.with(|c| c.borrow_mut().as_mut().map(f))
    }
    fn native_console_backing(_: &())->Result<(), &'static str>{Ok(())}
    // INSTALL
    #[test] fn registration_requires_a_live_backend_and_retains_real_caps() {
        CORE.with(|c| *c.borrow_mut() = Some(Core { dev: None, queue_size: 0, status: abi::status::NO_DEVICE, caps: abi::Caps::ZERO }));
        let (d, _) = device();
        assert!(install_native(d).is_err(), "unprobed board must not be registered");
        let (d0, state) = device(); drop(d0);
        let mut board = gk208::caps::BoardState::empty();
        board.chipset = 0x106;
        board.channel = (0x123000, 17);
        board.graphics_ready = true;
        board.created_classes = abi::gk208::CAPSET_IDS;
        let d = gk208::adapter::Device::registered(crate::gk208::gr::channel::NativeChannel(state.clone()), board, 8);
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
        fixture = fixture.replace('pub mod gk208 {', 'pub mod gk208 { pub fn console_surface()->Option<()>{Some(())}')
        fixture = fixture.replace('pub enum Reap {Empty,Done(Done),Fault,Reset(Tag)}',
            'pub enum Reap {Empty,Done(Done),Fault,Reset(Tag)} pub enum Device { Gk208(crate::gk208::adapter::Device), Other }')
        fixture = fixture.replace('../../kernel/', str(ROOT / 'kernel') + '/').replace('../kernel/', str(ROOT / 'kernel') + '/')
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / 'install.rs').write_text(fixture)
            subprocess.run(priority + ['rustc', '--edition=2021', '--crate-name', 'gpu_abi', '--crate-type', 'rlib', str(ROOT / 'libs/gpu-abi/src/lib.rs'), '-o', str(path / 'libgpu_abi.rlib')], check=True)
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings', '--extern', f'gpu_abi={path / "libgpu_abi.rlib"}', str(path / 'install.rs'), '-o', str(path / 'install')], check=True)
            subprocess.run([str(path / 'install'), 'installation::', '--nocapture'], check=True)

    def test_production_caps_initializes_before_reading_the_core(self):
        source = (ROOT / 'kernel/src/gpu/mod.rs').read_text()
        function = item(source, 'fn op_caps(').replace('#[cfg(target_arch = "x86_64")]', '')
        function += '\n' + item(source, 'fn native_caps_startup(')
        fixture = r"""
use std::cell::{Cell,RefCell};
use std::sync::atomic::{AtomicBool,Ordering};
const EINVAL:i64=22;
const EINTR:i64=4;
static NATIVE_STARTING:AtomicBool=AtomicBool::new(false);
mod arch {pub fn with_interrupts<R>(f:impl FnOnce()->R)->R{f()}}
fn notify(){}
fn wait_for<T>(_:Option<u64>,_:bool,mut f:impl FnMut(&mut Core)->Option<T>)->Result<T,()>{
 Ok(with_core(&mut f).unwrap().expect("single caller must not wait"))
}
mod abi {
    #[derive(Clone,Copy,Default)] pub struct Caps { pub backend:u32,pub status:u32,pub missing:u32 }
    pub mod status {pub const NO_DEVICE:u32=1;pub const DEVICE_FAILED:u32=3;}
}
struct Core {dev:Option<()>,status:u32,reason:&'static str}
impl Core {
 fn empty()->Self{Self{dev:None,status:1,reason:"absent"}}
 fn caps_record(&self)->abi::Caps{abi::Caps{backend:if self.dev.is_some(){6}else{0},status:self.status,missing:if self.dev.is_some(){3}else{0}}}
}
thread_local! {
 static CORE:RefCell<Core>=RefCell::new(Core::empty());
 static CALLS:Cell<u32>=const{Cell::new(0)};
 static OUTPUT:Cell<abi::Caps>=Cell::new(abi::Caps::default());
}
fn with_core<R>(f:impl FnOnce(&mut Core)->R)->Option<R>{Some(CORE.with(|c|f(&mut c.borrow_mut())))}
fn write_arg(_:u64,c:&abi::Caps)->Result<(),i64>{OUTPUT.with(|o|o.set(*c));Ok(())}
mod task {pub mod kthread {
 #[allow(dead_code)] pub fn kthread_run(f:impl FnOnce()+Send+'static,_:&str)->Result<(),()>{f();Ok(())}
}}
mod gk208 {
 pub fn takeover_started()->bool{true}
 pub fn initialize_core()->Result<(),&'static str>{
  crate::CALLS.with(|c|c.set(c.get()+1));
  crate::with_core(|c|{c.dev=Some(());c.status=2;});Ok(())
 }
}
// CAPS
fn main(){
 assert_eq!(op_caps(0,0),Err(EINVAL));assert_eq!(CALLS.with(Cell::get),0);
 assert_eq!(op_caps(0,core::mem::size_of::<abi::Caps>() as u64),Ok(0));
 let c=OUTPUT.with(Cell::get);assert_eq!((c.backend,c.status,c.missing),(6,2,3));
 assert_eq!(op_caps(0,core::mem::size_of::<abi::Caps>() as u64),Ok(0));
 assert_eq!(CALLS.with(Cell::get),1,"CAPS must initialize exactly once");
}
""".replace('// CAPS', function)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / 'caps.rs').write_text(fixture)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', str(path / 'caps.rs'), '-o', str(path / 'caps')], check=True)
            subprocess.run([str(path / 'caps')], check=True)


if __name__ == '__main__':
    unittest.main()
