#!/usr/bin/env python3
"""Execute the native selectors: core transport must not select CPU memory tests."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


def function(name):
    text = (ROOT / 'kernel/src/gpu/gk208/native.rs').read_text()
    start = text.index('pub fn ' + name + '(')
    opening = text.index('{', start)
    depth = 0
    for at in range(opening, len(text)):
        depth += (text[at] == '{') - (text[at] == '}')
        if depth == 0:
            return text[start:at + 1]
    raise ValueError('unterminated native selector')


HARNESS = r'''
pub mod boot {
    pub mod intent {
        pub fn get() -> Option<&'static [u8]> {
            let text = std::env::var("PROBE_INTENT").unwrap();
            let bytes: &'static [u8] = Box::leak(text.into_bytes().into_boxed_slice());
            Some(bytes)
        }
    }
    pub mod target {
        pub struct Suite { pub id: String }
        pub enum Target { Probe, Suite(Suite) }
        pub fn parse(text: &str) -> Result<Target, ()> {
            Ok(if text == "probe" { Target::Probe } else { Target::Suite(Suite { id: text.into() }) })
        }
    }
}
__SELECTORS__
fn main() {
    let args: Vec<String> = std::env::args().collect();
    assert_eq!(memory_requested(), args[1] == "true", "CPU memory selection");
    assert_eq!(suite_requested(), args[2] == "true", "native graphics selection");
}
'''


class DiagnosticSelection(unittest.TestCase):
    def test_explicit_diagnostics_keep_their_own_scope(self):
        with tempfile.TemporaryDirectory(prefix='gt710-selection-') as tmp:
            p = Path(tmp)
            source = p / 'main.rs'
            source.write_text(HARNESS.replace('__SELECTORS__', function('suite_requested') + '\n' + function('memory_requested')))
            for feature in (None, 'gt710_core_probe', 'gt710_clock_probe', 'capture_panic_oracle'):
                exe = p / (feature or 'production')
                args = ['rustc', '--edition=2021', '-Dwarnings', str(source), '-o', str(exe)]
                if feature:
                    args += ['--cfg', 'feature="' + feature + '"']
                subprocess.run(args, check=True, capture_output=True, text=True)
                for target in ('probe', 'console'):
                    with self.subTest(feature=feature, target=target):
                        memory = feature is None and target == 'probe'
                        graphics = feature == 'gt710_core_probe' or target == 'console'
                        result = subprocess.run([str(exe), str(memory).lower(), str(graphics).lower()],
                            env=dict(os.environ, PROBE_INTENT=target), capture_output=True, text=True)
                        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == '__main__':
    unittest.main()
