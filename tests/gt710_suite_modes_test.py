"""The GPU suite's native mode expectations match the recorded monitor EDID."""
import pathlib
import re
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]

class SuiteModesTest(unittest.TestCase):
    def test_native_expectations_match_recorded_edid(self):
        source = (ROOT / 'userspace/programs/src/suite_gpu/present.rs').read_text()
        match = re.search(r'fn expected_modes\(native: bool\) -> \[\(u32, u32, u32\); 2\] \{', source)
        if match:
            start = match.start()
            pos = match.end()
            depth = 1
            while depth:
                depth += (source[pos] == '{') - (source[pos] == '}')
                pos += 1
            expected = source[start:pos]
        else:
            # The old suite encodes its only pair as MODES byte-offset checks.
            body = source.split('pub fn modes_listed()', 1)[1].split('fn full(', 1)[0]
            pairs = dict((int(a), int(b)) for a, b in re.findall(r'\((\d+), (\d+)\)', body))
            expected = ('fn expected_modes(_native: bool) -> [(u32, u32, u32); 2] { '
                        f'[( {pairs[32]}, {pairs[36]}, {pairs[40]}), '
                        f'({pairs[48]}, {pairs[52]}, {pairs[56]})] }}')
        code = f'''#![allow(dead_code)]
extern crate alloc;
#[path="{ROOT}/kernel/src/gpu/gk208/edid.rs"] mod edid;
#[path="{ROOT}/kernel/src/gpu/gk208/modeset.rs"] mod modeset;
{expected}
fn main() {{
 let hex = include_str!("{ROOT}/tests/fixtures/gt710-edid/lcd17-19.hex").trim();
 let bytes: Vec<u8> = (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i+2], 16).unwrap()).collect();
 assert_eq!(expected_modes(true), modeset::display_modes(&bytes).unwrap());
 assert_eq!(expected_modes(false), [(1280,800,1),(2560,1600,2)]);
}}
'''
        with tempfile.TemporaryDirectory() as tmp:
            src = pathlib.Path(tmp) / 'check.rs'
            src.write_text(code)
            subprocess.run(['rustc', '--edition=2021', str(src), '-o', str(pathlib.Path(tmp) / 'check')], check=True)
            subprocess.run([str(pathlib.Path(tmp) / 'check')], check=True)

if __name__ == '__main__':
    unittest.main()
