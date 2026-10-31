"""The startup CE reader must use the relocated logical-console BAR mapping."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]

class ReadbackAperture(unittest.TestCase):
    def test_reader_uses_current_table_and_bar_base(self):
        source = (ROOT / 'kernel/src/gpu/gk208/ce/readback.rs').read_text()
        reader = source[source.index('impl Reader {'):source.index('    pub(in crate::gk208) fn image(')]
        self.assertIn('bar1-table', reader)
        self.assertIn('console.width', reader)
        self.assertIn('16 << 20', reader)
        self.assertNotIn('let pte = native + 0x4000', reader)

if __name__ == '__main__':
    unittest.main()
