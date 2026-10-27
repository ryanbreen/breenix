"""CE acceptance is based on retained complete bytes and native faults."""
import copy
import json
from pathlib import Path
import runpy
import unittest
ROOT=Path(__file__).resolve().parents[1]
API=runpy.run_path(str(ROOT/'scripts/metal/gt710-suite'))

class CopyCapture(unittest.TestCase):
    def test_missing_or_fabricated_transfer_capture_is_rejected(self):
        for serial in ['', 'GT710_CE {"passed":true}\n',
                       'GT710_CE {"schema":1,"class":41141,"copies":[],"vm":"passed"}\n']:
            with self.assertRaises(ValueError): API['copy_engine'](serial)

    def test_compare_checks_padding_and_both_complementary_poison_transfers(self):
        check=API['copy_bytes']
        seed=0x1234; size=8192
        initial=bytes(API['ce_pattern'](i,seed) for i in range(size))
        expected=bytearray([0xcc])*size
        for y in range(3): expected[4093+y*271:4093+y*271+257]=initial[17+y*263:17+y*263+257]
        e=dict(size=size,seed=seed,width=257,height=3,src_pitch=263,dst_pitch=271,
               src_offset=17,dst_offset=4093,fill=0xcc,poisons=[0x55,0xaa])
        check(e,[bytes(expected),bytes(expected)])
        for mutate in [lambda b:b.__setitem__(4093,b[4093]^1),lambda b:b.__setitem__(0,b[0]^1),
                       lambda b:b.__setitem__(size-1,b[size-1]^1)]:
            bad=bytearray(expected);mutate(bad)
            with self.assertRaises(ValueError):check(e,[bytes(expected),bytes(bad)])
        for snapshots in [[bytes(expected)], [bytes(expected),bytes(expected[:-1])],
                          [bytes([0x55])*size,bytes([0xaa])*size]]:
            with self.assertRaises(ValueError):check(e,snapshots)
        bad=copy.deepcopy(e);bad['poisons']=[0x55,0x55]
        with self.assertRaises(ValueError):check(bad,[bytes(expected),bytes(expected)])

if __name__=='__main__':unittest.main()
