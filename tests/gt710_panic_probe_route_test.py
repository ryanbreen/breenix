"""The panic oracle must not select the separate CPU VRAM memory probe."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class PanicRoute(unittest.TestCase):
    def test_panic_probe_keeps_hardware_startup_without_memory_experiments(self):
        text = (ROOT / 'kernel/src/gpu/gk208/native.rs').read_text()
        functions = '\n'.join(item(text, 'pub fn ' + name + '(')
                              for name in ['measuring', 'suite_requested', 'memory_requested'])
        fixture = r'''
mod boot {
    pub mod intent {
        static TARGET: std::sync::Mutex<Option<&'static [u8]>> = std::sync::Mutex::new(None);
        pub fn get() -> Option<&'static [u8]> { *TARGET.lock().unwrap() }
        pub fn set(v: Option<&'static [u8]>) { *TARGET.lock().unwrap() = v; }
    }
    pub mod target {
        pub struct Suite { pub id: String }
        pub enum Target { Probe, Suite(Suite) }
        pub fn parse(s: &str) -> Result<Target, ()> {
            if s == "probe" { Ok(Target::Probe) }
            else { s.strip_prefix("suite ").map(|id| Target::Suite(Suite { id: id.into() })).ok_or(()) }
        }
    }
}
CODE
fn main() {
    boot::intent::set(Some(b"probe"));
    assert_eq!(memory_requested(), !cfg!(feature="capture_panic_oracle"),
        "the panic profile's probe transport must not run the CPU memory inventory");
    assert_eq!(measuring(), !cfg!(feature="capture_panic_oracle") || cfg!(feature="testing"));
    for target in [b"suite console".as_slice(), b"suite gpu".as_slice()] {
        boot::intent::set(Some(target)); assert!(suite_requested()); assert!(measuring());
        assert!(!memory_requested());
    }
    boot::intent::set(None); assert!(!memory_requested());
    assert_eq!(measuring(), cfg!(feature="testing"));
}
'''.replace('CODE', functions)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as directory:
            p = Path(directory)
            (p / 'route.rs').write_text(fixture)
            for features in [[], ['capture_panic_oracle'], ['testing']]:
                cfg = [argument for f in features for argument in ['--cfg', f'feature="{f}"']]
                subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', *cfg,
                    str(p / 'route.rs'), '-o', str(p / 'route')], check=True)
                subprocess.run([str(p / 'route')], check=True)


if __name__ == '__main__':
    unittest.main()
