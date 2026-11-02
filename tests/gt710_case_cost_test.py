"""Long native cases retain cost deltas even if later picture evidence is refused."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class CaseCosts(unittest.TestCase):
    def test_cost_summary_reports_measured_deltas_and_counter_reset(self):
        source = (ROOT / 'userspace/programs/src/gt710_heap.rs').read_text()
        self.assertIn('fn cost_report(', source, 'aborted native runs lose the case costs')
        report = item(source, 'fn cost_report(')
        code = r'''
use std::{fmt::Write,time::Duration};
#[path="ROOT/libs/gpu-abi/src/heap_trace.rs"] pub mod wire;
REPORT
fn main(){let mut before=[0;wire::WORDS];let mut after=before;
let submit=wire::NAMES.iter().position(|n|*n=="submit_ns").unwrap();
before[submit]=100;after[submit]=500;
assert!(cost_report("text/cache-bound",&before,&after,Duration::from_millis(999)).is_none());
let line=cost_report("text/cache-bound",&before,&after,Duration::from_secs(2)).unwrap();
assert!(line.starts_with("GT710_CASE_COSTS case=text/cache-bound sample_ms=2000 "));
assert!(line.split_whitespace().any(|s|s=="submit_ns=400"));
after[submit]=50;
let line=cost_report("text/cache-bound",&before,&after,Duration::from_secs(2)).unwrap();
assert!(line.split_whitespace().any(|s|s=="submit_ns=reset"));
assert!(!line.split_whitespace().any(|s|s=="submit_ns=0"));}
'''.replace('ROOT', str(ROOT)).replace('REPORT', report)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory(prefix='gt710-case-cost-') as tmp:
            p = Path(tmp)
            (p / 'cost.rs').write_text(code)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', str(p / 'cost.rs'), '-o', str(p / 'cost')], check=True)
            subprocess.run([str(p / 'cost')], check=True)


if __name__ == '__main__':
    unittest.main()
