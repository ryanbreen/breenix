"""Run native adapter lifecycle checks without a device or a kernel boot."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]

class NativeAdapter(unittest.TestCase):
    def test_attributed_recovery_keeps_accepted_display_flip_until_its_latch(self):
        from screen_evidence_test import function
        adapter = (ROOT / 'kernel/src/gpu/gk208/adapter.rs').read_text()
        recovery = function(adapter, 'pub(crate) fn recover_fault(')
        fixture = (ROOT / 'tests/gt710_adapter_test.rs').read_text()
        fixture = fixture.replace('../kernel/', str(ROOT / 'kernel') + '/')
        fixture += r'''
impl gk208::gr::channel::NativeChannel {
    pub fn record_refusal(&self)->Result<(), &'static str>{Ok(())}
}
#[test]
fn accepted_flip_survives_attributed_recovery() {
    for refusal in ["", "reprobe", "restart"] {
        let (mut d,s)=device();
        assert!(d.send(gk208::present_request(8000)).is_ok());
        s.borrow_mut().release=1;
        assert!(matches!(d.reap(),Reap::Accepted(Tag::Sync(8000))));
        let bad=Tag::Submit{ctx:7,serial:9001,last:true};
        assert!(d.send(Request::undefined_command(bad,7,9001).unwrap()).is_ok());
        mmu_fault(&s,2);
        assert!(matches!(d.reap(),Reap::Empty));
        s.borrow_mut().fail_display=refusal=="reprobe";
        s.borrow_mut().fail_unbind=refusal=="restart";
        let result=d.recover_fault();
        if refusal=="restart" {
            assert!(result.is_err());
            assert_eq!(d.outstanding(),2,"failed restart lost display or render backing");
            continue;
        }
        assert_eq!(result.is_err(),refusal=="reprobe");
        assert!(matches!(d.reap(),Reap::Done(done) if done.tag==bad && done.answer==0x60010000),
            "GR recovery retired the independent accepted display flip as reset");
        assert!(matches!(d.reap(),Reap::Empty),"display completion needs its own latch");
        assert_eq!(d.outstanding(),1);
        s.borrow_mut().display_latched=true;
        let Reap::Done(done)=d.reap() else {panic!("accepted flip lost after GR recovery")};
        assert_eq!((done.tag,done.fence,done.native_fence,done.echoed),
            (Tag::Sync(8000),Some(8000),Some(1),1));
        assert_eq!(done.answer,gpu_abi::gk208::SUCCESS);
        assert!(done.fenced);
        assert_eq!(d.outstanding(),0);
        assert_eq!(s.borrow().events.iter().filter(|&&e|e=="display-update").count(),1,
            "recovery must not replay a display update");
    }
}
'''
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as directory:
            out = Path(directory)
            # Compile the production recovery body with the existing controlled
            # engine boundary. Reprobe must see only its own requests.
            (out / 'adapter.rs').write_text(adapter + '''
impl Device {
    RECOVERY
    fn reprobe(&mut self)->Result<(), &'static str>{
        assert_eq!(self.outstanding(),0,"CAPS consumed a case request");
        if self.channel.0.borrow().fail_display {Err("reprobe refused")} else {Ok(())}
    }
}
'''.replace('RECOVERY', recovery))
            fixture = fixture.replace('#[path = "adapter.rs"] pub mod adapter;',
                f'#[path = "{out / "adapter.rs"}"] pub mod adapter;')
            (out / 'test.rs').write_text(fixture)
            subprocess.run(priority + ['rustc', '--edition=2021', '--crate-name', 'gpu_abi',
                '--crate-type', 'rlib', '-Dwarnings', str(ROOT / 'libs/gpu-abi/src/lib.rs'),
                '-o', str(out / 'libgpu_abi.rlib')], check=True, cwd=ROOT)
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                '--extern', f'gpu_abi={out / "libgpu_abi.rlib"}', str(out / 'test.rs'),
                '-o', str(out / 'test')], check=True, cwd=ROOT)
            subprocess.run([str(out / 'test'), 'accepted_flip_survives_attributed_recovery'], check=True)

    def test_native_device_error_dispatch(self):
        source = (ROOT / 'kernel/src/gpu/dev.rs').read_text()
        function = source.split('pub fn undefined_command(', 1)[1].split('\npub fn ', 1)[0]
        self.assertIn('backend::GK208 =>', function)

    def test_control_checks_its_own_directory_before_publication(self):
        source = (ROOT / 'kernel/src/gpu/gk208/gr/channel.rs').read_text()
        arm = source.split('Operation::Undefined =>', 1)[1].split('Operation::Draw', 1)[0]
        self.assertIn('error::address(sequence)?', arm)
        self.assertIn('s.context.space().directory.range.start', arm)
        self.assertIn('address / (1 << 27) * 8', arm)
        self.assertIn('read(&mut regs, self.total, pde)? != 0', arm)
        self.assertIn('read(&mut regs, self.total, pde + 4)? != 0', arm)
        self.assertIn('return Err("device-error control PDE is mapped")', arm)

    def test_native_completion_and_room(self):
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as directory:
            out = Path(directory)
            subprocess.run(priority + ['rustc', '--edition=2021', '--crate-name', 'gpu_abi',
                '--crate-type', 'rlib', str(ROOT / 'libs/gpu-abi/src/lib.rs'),
                '-o', str(out / 'libgpu_abi.rlib')], check=True, cwd=ROOT)
            source = (ROOT / 'tests/gt710_adapter_test.rs').read_text()
            source = source.replace('../../kernel/', str(ROOT / 'kernel') + '/')
            source = source.replace('../kernel/', str(ROOT / 'kernel') + '/')
            (out / 'adapter.rs').write_text(source)
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                '--extern', f'gpu_abi={out / "libgpu_abi.rlib"}',
                str(out / 'adapter.rs'), '-o', str(out / 'adapter')], check=True, cwd=ROOT)
            subprocess.run([str(out / 'adapter')], check=True)

if __name__ == '__main__':
    unittest.main()
