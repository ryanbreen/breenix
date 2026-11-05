"""Exercise scene wakeups in the production window syscall dispatcher."""
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class SceneWake(unittest.TestCase):
    def test_progress_publication_does_not_create_scene_work(self):
        source = (ROOT / 'kernel/src/syscall/window.rs').read_text()
        wrapper = item(source, 'fn window(')
        abi = item((ROOT / 'libs/breengel-abi/src/lib.rs').read_text(), 'pub mod op {')
        names = set(re.findall(r'op::([A-Z_]+)', wrapper))
        names.update(('COMPOSITOR_ACK', 'COMPOSITOR_STATUS', 'CREATE'))
        constants = []
        for name in sorted(names):
            declaration = re.search(r'pub const ' + name + r': u32 = [^;]+;', abi)
            self.assertIsNotNone(declaration, name)
            constants.append(declaration.group())
        fixture = r'''
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
const EINVAL: i32 = 22;
const INTERFACE_VERSION: u32 = 2;
static WAKES: AtomicUsize = AtomicUsize::new(0);
static FAIL: AtomicBool = AtomicBool::new(false);
struct Caller;
impl Caller { fn current() -> Result<Self, i32> { Ok(Self) } }
fn arg_size(_: u32) -> Option<usize> { Some(8) }
fn dispatch(_: &Caller, _: u32, _: u64) -> Result<u64, i32> {
    if FAIL.load(Ordering::SeqCst) { Err(5) } else { Ok(0) }
}
mod test_control {
    pub fn enabled() -> bool { false }
    pub fn take(_: u32, _: u64) -> Option<String> { None }
    pub fn emit_diagnostic(_: String) {}
}
mod breengel_abi { pub mod tc_point { pub const EMIT_DIAGNOSTIC: u32 = 0; } }
mod task { pub mod scheduler { pub fn current_thread_id() -> Option<u64> { Some(1) } } }
mod window { pub mod scene {
    pub fn changed() { crate::WAKES.fetch_add(1, crate::Ordering::SeqCst); }
} }
// OPS
// WRAPPER
#[test]
fn acknowledgment_and_status_leave_a_drained_scene_idle() {
    for fail in [false, true] {
        FAIL.store(fail, Ordering::SeqCst);
        for op in [op::COMPOSITOR_ACK, op::COMPOSITOR_STATUS] {
            WAKES.store(0, Ordering::SeqCst);
            let result = window(op as u64, 1, 8);
            assert_eq!(result, if fail { Err(5) } else { Ok(0) });
            assert_eq!(WAKES.load(Ordering::SeqCst), 0,
                "progress operation {op} must not wake its own compositor");
        }
        WAKES.store(0, Ordering::SeqCst);
        let result = window(op::CREATE as u64, 1, 8);
        assert_eq!(result, if fail { Err(5) } else { Ok(0) });
        assert_eq!(WAKES.load(Ordering::SeqCst), 1, "scene mutations still wake bwm");
    }
}
'''.replace('// OPS', 'mod op {\n' + '\n'.join(constants) + '\n}')
        fixture = fixture.replace('// WRAPPER', wrapper)
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / 'wake.rs').write_text(fixture)
            subprocess.run(['rustc', '--edition=2021', '--test', str(path / 'wake.rs'),
                            '-o', str(path / 'wake')], cwd=ROOT, check=True)
            subprocess.run([str(path / 'wake')], check=True)


if __name__ == '__main__':
    unittest.main()
