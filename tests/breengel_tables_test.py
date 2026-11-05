"""The table census retains totals and process attribution, and fails closed."""
import runpy
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]
TOOLS = [runpy.run_path(str(ROOT / path)) for path in
         ('scripts/suite-memory-owners.py', 'scripts/metal/gt710-suite')]
BEFORE = 'BREENGEL TABLES before runner_pte_kb=40 all_pte_kb=120 top_pte=7:runner:40,2:init:32,9:worker:16'
AFTER = 'BREENGEL TABLES after runner_pte_kb=52 all_pte_kb=136 top_pte=7:runner:52,2:init:32,9:worker:20'


class TablesTest(unittest.TestCase):
    def test_complete_and_missing_fields(self):
        for tool in TOOLS:
            parse = tool['parse_breengel_tables']
            result = parse(BEFORE + '\n' + AFTER + '\n')
            self.assertEqual(result['after']['all_pte_kb'] - result['before']['all_pte_kb'], 16)
            self.assertEqual(result['after']['top_pte'][0], (7, 'runner', 52))
            for bad in (AFTER.replace(' all_pte_kb=136', ''), AFTER.replace(' top_pte=7:runner:52,2:init:32,9:worker:20', '')):
                with self.assertRaises(ValueError):
                    parse(BEFORE + '\n' + bad + '\n')


if __name__ == '__main__':
    unittest.main()
