"""Capture every active native SM and the live FIFO state before reset."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class StallSnapshot(unittest.TestCase):
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
                    0x42018,0x4205c,0x42084,0x42088,0x42044] {{
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
