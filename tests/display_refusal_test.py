"""A live mode refusal survives the kernel result and the display socket."""
from pathlib import Path
import subprocess
import tempfile
import unittest
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]

class DisplayRefusal(unittest.TestCase):
    def test_refusal_keeps_reason_phase_errno_and_case_reply(self):
        bwm = (ROOT / 'userspace/programs/src/bwm.rs').read_text()
        cases = (ROOT / 'userspace/programs/src/wm_suite/cases.rs').read_text()
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            commands = [['taskpolicy', '-b', 'rustc', '--edition=2021', '--crate-name', 'gpu_abi',
                         '--crate-type=rlib', '-Dwarnings', str(ROOT / 'libs/gpu-abi/src/lib.rs'),
                         '-o', str(path / 'libgpu_abi.rlib')]]
            for command in commands:
                result = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            # First compile the public result contract, before extracting its caller.
            contract = r'''
extern crate gpu_abi;
use gpu_abi::{ModeReport, Plain, mode_phase};
#[test] fn reason_is_bounded_and_specific() {
    assert_eq!(core::mem::size_of::<ModeReport>(), 256);
    let r = ModeReport::refused(3, gpu_abi::errno::ETIMEDOUT,
        mode_phase::RASTER_GET, 2_000_000_000, [7, 64, 96, 0],
        "display register wait timed out");
    assert_eq!((r.index, r.errno, r.elapsed_ns), (3, 110, 2_000_000_000));
    assert_eq!((r.pending, r.get, r.put, r.notifier), (7, 64, 96, 0));
    assert_eq!(r.reason(), "display register wait timed out");
    assert_eq!(r.phase_name(), "raster-get");
    assert_eq!(ModeReport::ZERO.reason(), "");
    let interrupted = ModeReport::refused(1, 4, mode_phase::SHARED_TRANSITION,
        500, [2, 0, 0, 0], "display transition interrupted");
    assert_eq!((interrupted.errno, interrupted.phase_name()), (4, "shared-transition"));
    let long = "x".repeat(500);
    let bounded = ModeReport::refused(0, 5, mode_phase::RASTER, 1, [0;4], &long);
    assert_eq!(bounded.reason().len(), 192);
}
'''
            (path / 'contract.rs').write_text(contract)
            result = subprocess.run(['taskpolicy', '-b', 'rustc', '--edition=2021', '--test', '-Dwarnings',
                str(path / 'contract.rs'), '--extern', f'gpu_abi={path / "libgpu_abi.rlib"}',
                '-o', str(path / 'contract')], cwd=ROOT, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            result = subprocess.run([str(path / 'contract')], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            source = item(bwm, 'fn mode_refusal(')
            case = item(cases, 'fn set_display_mode(')
            harness = r'''
extern crate gpu_abi;
use gpu_abi::{ModeReport, mode_phase};
mod libbreenix { pub mod gpu {
    pub use gpu_abi as abi;
    pub fn current_mode() -> Result<abi::Mode, i64> {
        Ok(abi::Mode {width:1280,height:1024,scale:1,reserved:0})
    }
}}
fn display_request(_: &str) -> Result<String, String> {
    Ok(format!("refused {}\n", mode_refusal(-110, &report())))
}
fn report() -> ModeReport { ModeReport::refused(3, 110, mode_phase::RASTER_GET,
    2_000_000_000, [7,64,96,0], "display register wait timed out") }
FORMATTER
CASE
#[test] fn socket_and_case_keep_failure() {
    let reason = mode_refusal(-110, &report());
    assert!(reason.contains("errno 110"));
    assert!(reason.contains("raster-get"));
    assert!(reason.contains("display register wait timed out"));
    assert!(reason.contains("2000000000"));
    let error = set_display_mode(gpu_abi::Mode {width:1024,height:768,scale:1,reserved:0}).unwrap_err();
    assert_eq!(error, format!("refused {reason}\n"));
}
'''.replace('FORMATTER', source).replace('CASE', case)
            (path / 'caller.rs').write_text(harness)
            result = subprocess.run(['taskpolicy', '-b', 'rustc', '--edition=2021', '--test', '-Dwarnings',
                str(path / 'caller.rs'), '--extern', f'gpu_abi={path / "libgpu_abi.rlib"}',
                '-o', str(path / 'caller')], cwd=ROOT, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            result = subprocess.run([str(path / 'caller')], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

if __name__ == '__main__': unittest.main()
