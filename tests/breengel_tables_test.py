"""The table census retains totals and process attribution, and fails closed."""
import runpy
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]
TOOLS = [runpy.run_path(str(ROOT / path)) for path in
         ('scripts/suite-memory-owners.py', 'scripts/metal/gt710-suite')]
BEFORE = 'BREENGEL TABLES before runner_pte_kb=40 processes=3 all_pte_kb=120 top_pte=7:runner:40,2:init:32,9:worker:16'
AFTER = 'BREENGEL TABLES after runner_pte_kb=52 processes=3 all_pte_kb=136 top_pte=7:runner:52,2:init:32,9:worker:20'


class TablesTest(unittest.TestCase):
    def test_cardinality_and_totals(self):
        for tool in TOOLS:
            parse = tool['parse_breengel_tables']
            for count, owners, total in ((1, '1:init:20', 20),
                                         (2, '1:init:20,2:runner:12', 32),
                                         (4, '1:init:20,2:runner:12,3:worker:8', 44)):
                line = f'BREENGEL TABLES before runner_pte_kb=12 processes={count} all_pte_kb={total} top_pte={owners}'
                result = parse(line + "\n" + line.replace('before', 'after'))
                self.assertEqual(len(result['before']['top_pte']), min(3, count))
                for bad in (line.replace(f'processes={count}', 'processes=0'),
                            line.replace(f'all_pte_kb={total}', 'all_pte_kb=1'),
                            line.replace(f'processes={count}', 'processes=2' if count != 2 else 'processes=4')):
                    with self.assertRaises(ValueError):
                        parse(bad + "\n" + line.replace('before', 'after'))
            unavailable = 'BREENGEL TABLES before runner_pte_kb=40 all_pte_kb=unavailable top_pte=unavailable'
            with self.assertRaises(ValueError):
                parse(unavailable + "\n" + unavailable.replace('before', 'after'))

    def test_complete_and_missing_fields(self):
        for tool in TOOLS:
            parse = tool['parse_breengel_tables']
            result = parse(BEFORE + '\n' + BEFORE.replace('before', 'finale') + '\n' + BEFORE.replace('before', 'retired') + '\n' + AFTER + '\n')
            self.assertEqual(result['after']['all_pte_kb'] - result['before']['all_pte_kb'], 16)
            self.assertEqual(result['after']['top_pte'][0], (7, 'runner', 52))
            for bad in (AFTER.replace(' processes=3 all_pte_kb=136', ''), AFTER.replace(' top_pte=7:runner:52,2:init:32,9:worker:20', '')):
                with self.assertRaises(ValueError):
                    parse(BEFORE + '\n' + bad + '\n')


if __name__ == '__main__':
    unittest.main()
