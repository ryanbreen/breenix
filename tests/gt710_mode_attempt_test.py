"""Execute the ordered success record with deterministic phase times and MMIO samples."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ModeAttempt(unittest.TestCase):
    def test_success_retains_each_phase_entry_and_duration(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            subprocess.run(['taskpolicy', '-b', 'rustc', '--edition=2021', '-Dwarnings',
                '--crate-name', 'gpu_abi', '--crate-type=rlib',
                str(ROOT / 'libs/gpu-abi/src/lib.rs'), '-o', str(path / 'libgpu_abi.rlib')], check=True)
            source = r'''
#[path="SOURCE"] mod mode_attempt;
use gpu_abi::mode_phase::*;
use mode_attempt::ModeAttempt;
fn main() {
    let mut attempt = ModeAttempt::new(3, DRAIN_FLIP, 100, [8, 32, 64, 0]);
    attempt.phase(RASTER_GET, 150, [0, 64, 96, 0]);
    attempt.phase(RASTER_NOTIFIER, 175, [0, 96, 96, 0]);
    attempt.phase(RASTER_GET, 190, [0, 96, 128, 1]);
    attempt.finish(210);
    let line = attempt.to_string();
    assert_eq!(line, "index=3 phases=[drain-flip:50:8:32:64:0,raster-get:25:0:64:96:0,raster-notifier:15:0:96:96:0,raster-get:20:0:96:128:1] dropped=0");
    assert!(!line.contains('\n'));
    attempt.clock_observation([42, 2, 1, 1400000]);
    assert!(attempt.to_string().ends_with("samples=42 advances=2 rejected=1 max_gap_ns=1400000"));
    let mut refusal = gpu_abi::ModeReport::refused(3, 110, RASTER, 51000000, [0;4], "native vblank edge not observed");
    attempt.annotate_refusal(&mut refusal);
    assert_eq!(refusal.reason(), "native vblank edge not observed samples=42 advances=2 rejected=1 max_gap_ns=1400000");
    attempt.clock_costs([[6000, 7000, 9000], [19500, 22000, 24500]]);
    assert!(attempt.to_string().ends_with("vert_ns=6000/7000/9000 full_ns=19500/22000/24500"));
    let mut refusal = gpu_abi::ModeReport::refused(3, 110, RASTER, 51000000, [0;4], "native vblank edge not observed");
    attempt.annotate_refusal(&mut refusal);
    assert!(refusal.reason().ends_with("vert_ns=6000/7000/9000 full_ns=19500/22000/24500"));
}
'''.replace('SOURCE', str(ROOT / 'kernel/src/gpu/gk208/mode_attempt.rs'))
            (path / 'check.rs').write_text(source)
            subprocess.run(['taskpolicy', '-b', 'rustc', '--edition=2021', '-Dwarnings',
                str(path / 'check.rs'), '--extern', f'gpu_abi={path / "libgpu_abi.rlib"}',
                '-o', str(path / 'check')], check=True)
            subprocess.run([str(path / 'check')], check=True)


if __name__ == '__main__':
    unittest.main()
