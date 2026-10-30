"""Capture every active native SM and the live FIFO state before reset."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class StallSnapshot(unittest.TestCase):
    def test_stall_bulk_record_requires_its_exact_serial_anchor(self):
        import hashlib
        import runpy
        sys.path.insert(0, str(ROOT / 'scripts/metal'))
        api = runpy.run_path(str(ROOT / 'scripts/metal/gt710-suite'))
        startup = b'GT710_PIX 0 1:ff003039\n'
        record = b'GT710_STALL_V1 instance=8192 epoch=17 put=23 registers=[(4196100, 2415924756)]\n'
        anchor = (f'GT710_STALL_BULK epoch=17 put=23 bytes={len(record)-1} '
                  f'sha256={hashlib.sha256(record[:-1]).hexdigest()}\n')
        serial = ('GT710_MODE {"schema":1}\n'
                  f'GT710_BULK {len(startup)} {hashlib.sha256(startup).hexdigest()}\n'
                  'SUITE gpu START total=1\nSUITE gpu CASE pipelines/sample FAIL ms=1 msg=EIO\n'
                  + anchor + 'SUITE gpu DONE passed=0 failed=1 skipped=0 total=1\n'
                  'SUITE gpu MEMORY reclaimed=PASS free_before=100 free_after=100 allowance=48\n'
                  f'GT710_FINAL_BULK {len(record)} {hashlib.sha256(record).hexdigest()}\n'
                  'SUITE_SEQUENCE READY gpu\n')
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'gt710-pixels.rle').write_bytes(startup)
            (path / 'gt710-final.rle').write_bytes(record)
            (path / 'serial.txt').write_text(serial)
            self.assertIn(record.decode() + anchor, api['capture_serial'](path))
            for bad in [anchor.replace('put=23', 'put=24'),
                        anchor.replace('sha256=', 'sha256=0'),
                        anchor.replace(f'bytes={len(record)-1}', 'bytes=1'), '']:
                (path / 'serial.txt').write_text(serial.replace(anchor, bad))
                with self.assertRaises(ValueError):
                    api['capture_serial'](path)

    def test_second_tpc_and_live_fifo_are_retained_without_selector_writes(self):
        source = f'''
extern crate alloc;
#[path = "{ROOT / 'kernel/src/gpu/gk208/gr/topology.rs'}"] pub mod topology;
#[path = "{ROOT / 'kernel/src/gpu/gk208/gr/stall.rs'}"] mod stall;
fn main() {{
    let raw = [[0x409604,1],[0x502608,2],[0x500c30,3]];
    let topology = topology::decode(&raw).unwrap();
    assert_eq!(topology.tpc_nr[0],2);
    assert_eq!(topology.raw,&raw);
    let mut reads = std::collections::BTreeSet::new();
    let rows = stall::capture(&raw,1,|address| {{
        assert!(reads.insert(address), "duplicate register read");
        if address==0x504e48 {{ 1 }} else if address==0x42084 {{ 0x20000110 }} else {{ 0 }}
    }}).unwrap();
    assert!(rows.contains(&(0x504e48,1)), "TPC1's stack error was omitted");
    for address in [0x504614,0x504618,0x504624,0x504628,0x504650,
                    0x504e14,0x504e18,0x504e24,0x504e28,0x504e50,
                    0x504410,0x504c10,0x42018,0x4205c,0x42084,0x42088,0x42044] {{
        assert!(reads.contains(&address), "missing diagnostic register {{address:#x}}");
    }}
    assert!(rows.contains(&(0x42084,0x20000110)), "live FIFO method was omitted");
    assert!(stall::capture(&[[0x409604,1],[0x502608,2],[0x500c30,1]],1,
        |_| panic!("invalid topology must refuse before MMIO")).is_err());
    assert!(stall::capture(&raw,32,|_| panic!("invalid PBDMA must refuse before MMIO")).is_err());
}}
'''
        prefix = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'snapshot.rs').write_text(source)
            subprocess.run(prefix + ['rustc', '--edition=2021', '-Dwarnings',
                           str(path / 'snapshot.rs'), '-o', str(path / 'snapshot')], check=True)
            subprocess.run([str(path / 'snapshot')], check=True)


if __name__ == '__main__':
    unittest.main()
