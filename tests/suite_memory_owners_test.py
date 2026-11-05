import csv
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / 'scripts/suite-memory-owners.py'


class SuiteMemoryOwnersTest(unittest.TestCase):
    def test_retained_boots_and_optional_evidence(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            rows = [
                ('20261104T225647Z-metal', 2013709, 112, 'PASS'),
                ('20261105T011616Z-metal', 2013705, 116, 'FAIL'),
            ]
            for name, free, tables, verdict in rows:
                boot = root / name
                boot.mkdir()
                (boot / 'serial.txt').write_text(
                    'SUITE breengel MEMORY before free=2013754 heap_free_kb=51864 user_stacks=16 user_pages=2729 page_tables=81 page_cache=0 kernel_stacks=848 buffers=18579 other=16498 receipts=0 settle_ms=203\n'
                    f'SUITE breengel MEMORY after free={free} heap_free_kb=48804 user_stacks=12 user_pages=2747 page_tables={tables} page_cache=0 kernel_stacks=848 buffers=18579 other=16498 receipts=0 settle_ms=206\n'
                    f'SUITE breengel MEMORY reclaimed={verdict} free_before=2013754 free_after={free} allowance=48\n'
                    'BREENGEL TABLES before runner_pte_kb=40\n'
                    'BREENGEL TABLES after runner_pte_kb=52\n')
            census = {'schema': 1, 'phase': 401, 'dropped': 0, 'count': 3,
                      'entries': [[1, 16, 1, 0, ''], [2, 16, 1, 7, ''], [3, 16, 1, 7, '']]}
            (root / rows[1][0] / 'gt710-final.rle').write_text('GT710_BREENGL_ALLOCATIONS ' + json.dumps(census) + '\n')
            output = subprocess.check_output([sys.executable, str(SCRIPT), *(str(root / row[0]) for row in rows)], text=True)
            report = list(csv.DictReader(io.StringIO(output), delimiter='\t'))
            self.assertEqual([(r['retained'], r['page_tables'], r['user_pages'], r['user_stacks'], r['reclaimed']) for r in report],
                             [('45', '31', '18', '-4', 'PASS'), ('49', '35', '18', '-4', 'FAIL')])
            self.assertEqual(json.loads(report[1]['allocation_births']), {'0': 1, '7': 2})
            self.assertEqual(report[0]['allocation_births'], '')
            self.assertEqual(json.loads(report[0]['runner_tables_delta']), {'runner_pte_kb': 12})


if __name__ == '__main__':
    unittest.main()
