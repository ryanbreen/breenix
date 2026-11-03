"""Execute display-consumer entry paths against a lazy native GPU stub."""
from pathlib import Path
import os
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class DisplayAdmissionTest(unittest.TestCase):
    def test_consumers_admit_before_modes_and_reject_refusal(self):
        target = 'x86_64-apple-darwin' if sys.platform == 'darwin' else 'x86_64-unknown-linux-gnu'
        def rustc(*args):
            cmd = ['rustc', '--edition=2021', '--target', target, '-Dwarnings', *args]
            if sys.platform == 'darwin':
                cmd = ['taskpolicy', '-b', *cmd]
            result = subprocess.run(cmd, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            (p / 'gpu.rs').write_text('''
pub mod gpu {
    use std::sync::atomic::{AtomicBool, Ordering};
    static READY: AtomicBool = AtomicBool::new(false);
    pub mod abi { pub mod status { pub const ADMITTED: u32 = 1; } }
    pub struct Caps { pub status: u32 }
    #[derive(Clone, Copy)]
    pub struct Mode { pub width: u32, pub height: u32, pub scale: u32 }
    pub struct Modes { pub current: u32, pub modes: [Mode; 1] }
    pub fn caps() -> (i64, Caps) {
        let ok = std::env::var_os("REFUSE_CAPS").is_none();
        READY.store(ok, Ordering::SeqCst);
        (if ok {0} else {-19}, Caps {status: if ok {abi::status::ADMITTED} else {0}})
    }
    pub fn modes() -> (i64, Modes) {
        assert!(READY.load(Ordering::SeqCst), "MODES called before native CAPS admission");
        (0, Modes {current:0, modes:[Mode {width:2560,height:1600,scale:2}]})
    }
}
''')
            rustc('--crate-name', 'libbreenix', '--crate-type', 'rlib', str(p / 'gpu.rs'), '-o', str(p / 'liblibbreenix.rlib'))
            (p / 'bgl.rs').write_text('''
pub fn modes() -> Result<libbreenix::gpu::Modes, i64> {
    let (r, modes) = libbreenix::gpu::modes();
    if r == 0 {Ok(modes)} else {Err(r)}
}
''')
            extern = ['--extern', 'libbreenix=' + str(p / 'liblibbreenix.rlib'), '-L', str(p)]
            rustc('--crate-name', 'bgl', '--crate-type', 'rlib', str(p / 'bgl.rs'), *extern, '-o', str(p / 'libbgl.rlib'))
            extern += ['--extern', 'bgl=' + str(p / 'libbgl.rlib')]
            programs = ROOT / 'userspace/programs/src'
            resolution = (programs / 'resolution.rs').read_text()
            usb = (programs / 'usb_pointer_check.rs').read_text().split('fn screen_size()', 1)[1].split('\nfn observe()', 1)[0]
            fixtures = {'resolution': resolution,
                        'usb': 'fn sample()' + usb + '\nfn main() { assert_eq!(sample().is_err(), std::env::var_os("REFUSE_CAPS").is_some()); }'}
            probe = (programs / 'probe.rs').read_text()
            for name, start, result, tail in [
                ('probe_check', 'fn scanout_check()', "Result<(), &'static str>", 'Ok(())'),
                ('probe_panel', 'fn open_screen()', 'Option<()>', 'Some(())'),
            ]:
                # Keep actual statements through the mode request, stopping before
                # target allocation, which this initialization check cannot reach.
                prefix = probe.split(start, 1)[1].split('{', 1)[1].split('    let mode =', 1)[0]
                method = 'is_err' if name == 'probe_check' else 'is_none'
                fixtures[name] = ('fn sample() -> ' + result + ' {' + prefix
                                  + 'let _ = (modes, gpu::abi::status::ADMITTED, abi::status::ADMITTED);' + tail + '}\n'
                                  + 'fn main() { assert_eq!(sample().' + method
                                  + '(), std::env::var_os("REFUSE_CAPS").is_some()); }')
            for name, source in fixtures.items():
                with self.subTest(consumer=name):
                    (p / (name + '.rs')).write_text(source)
                    rustc(str(p / (name + '.rs')), *extern, '-o', str(p / name))
                    for refused in (False, True):
                        env = dict(os.environ)
                        env.pop('REFUSE_CAPS', None)
                        if refused:
                            env['REFUSE_CAPS'] = '1'
                        result = subprocess.run([str(p / name)], env=env, capture_output=True, text=True)
                        expected = 1 if name == 'resolution' and refused else 0
                        self.assertEqual(result.returncode, expected, name + '\n' + result.stdout + result.stderr)


if __name__ == '__main__':
    unittest.main()
