"""A failed evidence transport must not satisfy a negative pixel comparison."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from screen_evidence_test import function

ROOT = Path(__file__).resolve().parents[1]


class Comparison(unittest.TestCase):
    def test_transport_error_is_not_a_pixel_mismatch(self):
        common = (ROOT / 'userspace/programs/src/suite_gpu/common.rs').read_text()
        compare = function(common, '    pub fn compare(') + '\n' + function(common, '    pub fn matches(') + '\n' + function(common, '    pub fn matches_owned(')
        fixture = r'''
type CaseError = String;
fn fail_msg(s: String) -> CaseError { s }
mod breenix_suite { pub mod suite {
    pub fn judged_readback_owned(_: Vec<u8>, _: u32, _: u32) -> Result<(), String> {
        Err("evidence transport unavailable".into())
    }
} }
struct Canvas { w: u32, h: u32, px: Vec<u8> }
impl Canvas { COMPARE }
#[test] fn comparison_is_independent() {
    let want = Canvas { w: 1, h: 1, px: vec![1, 2, 3, 255] };
    assert!(want.matches(&[1, 2, 3, 255], "same").is_ok(),
        "an evidence error must not masquerade as the expected mismatch");
    assert!(want.matches(&[9, 2, 3, 255], "different").unwrap_err().contains("pixel (0,0)"));
    assert!(want.matches(&[], "short").unwrap_err().contains("read 0 bytes"));
}
'''.replace('COMPARE', compare)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            source = path / 'test.rs'
            source.write_text(fixture)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings', str(source), '-o', str(path / 'test')], check=True)
            subprocess.run([str(path / 'test')], check=True)


if __name__ == '__main__':
    unittest.main()
