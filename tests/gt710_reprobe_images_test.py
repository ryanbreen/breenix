"""Retain reprobe failure bytes rather than referring to different startup bytes."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ReprobeImages(unittest.TestCase):
    def test_failed_and_inexact_reprobes_keep_their_actual_images(self):
        source = (ROOT / 'kernel/src/gpu/gk208/caps/executable.rs').read_text()
        images = source[source.index('        let images = if '):source.index('        let row = format!')]
        fixture = r'''
fn hash(bytes: &[u8]) -> String { format!("hash{bytes:?}") }
fn hex(bytes: &[u8]) -> String { bytes.iter().map(|b| format!("{b:02x}")).collect() }
fn main() {
    for epoch in [None, Some(18)] {
        let expected = &[0x11u8, 0x22][..];
        for (first, second) in [(expected, expected), (&[][..], &[][..]),
                                (&[0x11, 0x23][..], expected)] {
            assert!(!expected.is_empty());
IMAGES
            if first != expected || second != expected || epoch.is_none() {
                assert_eq!(images, format!("\"first\":\"{}\",\"second\":\"{}\"",hex(first),hex(second)),
                           "a failed or inexact fresh observation must retain its own bytes");
            }
        }
    }
}
'''.replace('IMAGES', images)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            (p / 'images.rs').write_text(fixture)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', str(p / 'images.rs'),
                                      '-o', str(p / 'images')], check=True)
            subprocess.run([str(p / 'images')], check=True)


if __name__ == '__main__':
    unittest.main()
