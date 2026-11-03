"""Retain the producing scope beside the unchanged GPU masked-interval maximum."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tests"))
from gt710_gr_live_unwind_test import item


class Attribution(unittest.TestCase):
    def test_scopes_keep_their_site_and_report_only_the_observed_maximum(self):
        source = item((ROOT / "kernel/src/tracing/providers/gpu.rs").read_text(), "mod gpu_mask {")
        source = source[source.index("{") + 1:-1]
        harness = r'''
extern crate alloc;
use std::cell::Cell;
thread_local! { static CPU: Cell<usize> = const { Cell::new(0) }; }
mod task { pub mod scheduler {
    pub const MAX_CPUS: usize = 2;
    pub fn current_cpu() -> usize { crate::per_cpu::cpu_id() }
} }
mod per_cpu { pub fn cpu_id() -> usize { crate::CPU.with(core::cell::Cell::get) } }
mod attribution { SOURCE }
fn main() {
    let first = core::panic::Location::caller();
    attribution::begin(first);
    attribution::raised(100, 1370);
    CPU.with(|c| c.set(1));
    let second = core::panic::Location::caller();
    attribution::begin(second);
    attribution::raised(200, 2371);
    assert!(attribution::report(0).is_empty(), "a reset maximum cannot report an old sample");
    assert!(attribution::report(500).is_empty(), "the diagnostic does not change the bound");
    let report = attribution::report(2371);
    assert!(report.contains("cpu=1 start=200 us=2371"), "{report}");
    assert!(report.contains(&format!("at={}:{}", second.file(), second.line())), "{report}");
    assert!(!report.contains("us=1370"), "a different maximum is not attribution for this reading");
    assert!(attribution::report(2371).is_empty(), "cold reads do not repeat a sample");
    CPU.with(|c| c.set(0));
    attribution::begin(first);
    attribution::raised(300, 857);
    let report = attribution::report(857);
    assert!(report.contains("cpu=0 start=300 us=857"), "{report}");
    assert!(report.contains(&format!("at={}:{}", first.file(), first.line())), "{report}");
}
'''.replace("SOURCE", "\n".join(line for line in source.splitlines() if not line.startswith("//!")))
        priority = ["taskpolicy", "-b"] if sys.platform == "darwin" else []
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / "scope.rs").write_text(harness)
            subprocess.run(priority + ["rustc", "--edition=2021", "-Dwarnings", str(path / "scope.rs"),
                                      "-o", str(path / "scope")], check=True)
            subprocess.run([str(path / "scope")], check=True)


if __name__ == "__main__":
    unittest.main()
