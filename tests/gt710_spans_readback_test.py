"""Check the span readback comparison and its digest-bound evidence transport."""
import hashlib
from pathlib import Path
import runpy
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class SpansReadback(unittest.TestCase):
    def test_compare_every_byte_and_refuse_incomplete_or_disagreeing_reads(self):
        source = f'''
extern crate alloc;
#[path = "{ROOT / 'kernel/src/gpu/gk208/spans_diagnostic.rs'}"] mod diagnostic;
fn main() {{
    let expected: Vec<u8> = (0..1024).map(|i| (i * 37) as u8).collect();
    assert_eq!(diagnostic::compare(&expected, &expected, &expected).unwrap(), None);
    for at in [0, 15, 16, 31, 32, 255, 256, 511, 512, 1023] {{
        let mut actual = expected.clone(); actual[at] ^= 0xff;
        if at < 1023 {{ actual[1023] ^= 1; }}
        assert_eq!(diagnostic::compare(&expected, &actual, &actual).unwrap(),
            Some((at, expected[at], actual[at])));
        assert!(diagnostic::compare(&expected, &actual, &expected).is_err());
    }}
    assert!(diagnostic::compare(&expected, &expected[..1023], &expected[..1023]).is_err());
    assert!(diagnostic::compare(&[], &[], &[]).is_err());
}}
'''
        prefix = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as directory:
            p = Path(directory)
            (p / 'check.rs').write_text(source)
            subprocess.run(prefix + ['rustc', '--edition=2021', '-Dwarnings',
                           str(p / 'check.rs'), '-o', str(p / 'check')], check=True)
            subprocess.run([str(p / 'check')], check=True)

    def test_span_record_requires_matching_anchor_and_both_upload_digests(self):
        sys.path.insert(0, str(ROOT / 'scripts/metal'))
        api = runpy.run_path(str(ROOT / 'scripts/metal/gt710-suite'))
        startup = b'GT710_PIX 0 1:ff003039\n'
        row = ('GT710_SPANS_V1 {"id":1,"resource":7,"width":16384,"height":1,'
               '"bytes":65536,"first_mismatch":32,"expected":2,"actual":0}\n').encode()
        anchor = (f'GT710_SPANS_BULK id=1 bytes={len(row)-1} '
                  f'sha256={hashlib.sha256(row[:-1]).hexdigest()}\n')
        serial = ('GT710_MODE {"schema":1}\n'
                  f'GT710_BULK {len(startup)} {hashlib.sha256(startup).hexdigest()}\n'
                  'SUITE gpu START total=1\n' + anchor +
                  'SUITE gpu CASE pipelines/coverage-spans-square FAIL ms=1 msg=EIO\n'
                  'SUITE gpu DONE passed=0 failed=1 skipped=0 total=1\n'
                  'SUITE gpu MEMORY reclaimed=PASS free_before=100 free_after=100 allowance=48\n'
                  f'GT710_FINAL_BULK {len(row)} {hashlib.sha256(row).hexdigest()}\n'
                  'SUITE_SEQUENCE READY gpu\n')
        with tempfile.TemporaryDirectory() as directory:
            p = Path(directory)
            (p / 'gt710-pixels.rle').write_bytes(startup)
            (p / 'gt710-final.rle').write_bytes(row)
            (p / 'serial.txt').write_text(serial)
            self.assertIn(row.decode() + anchor, api['capture_serial'](p))
            for bad in [anchor.replace('id=1', 'id=2'),
                        anchor.replace('sha256=', 'sha256=0'),
                        anchor.replace(f'bytes={len(row)-1}', 'bytes=1'), '']:
                (p / 'serial.txt').write_text(serial.replace(anchor, bad))
                with self.assertRaises(ValueError): api['capture_serial'](p)
            (p / 'serial.txt').write_text(serial)
            (p / 'gt710-final.rle').write_bytes(row.replace(b'"actual":0', b'"actual":1'))
            with self.assertRaises(ValueError): api['capture_serial'](p)


if __name__ == '__main__':
    unittest.main()
