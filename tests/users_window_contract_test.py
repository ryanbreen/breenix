"""Keep the users isolation case on the live window interface on both targets."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]


class UsersWindowContract(unittest.TestCase):
    def test_both_targets_exercise_window_isolation(self):
        source = (ROOT / 'userspace/programs/src/suite_users.rs').read_text()
        # Two architecture-specific bodies hid the x86 call to retired FBDRAW.
        self.assertEqual(source.count('fn window_other()'), 1)
        for name in ('make_window', 'listed_windows', 'window_call', 'window_other'):
            declaration = source.index(f'fn {name}')
            attributes = source[source.rfind('\n}', 0, declaration) + 2:declaration]
            self.assertNotRegex(attributes, r'#\[cfg\(target_arch')
        case = source[source.index('fn window_other()'):source.index('// umask\n')]
        self.assertNotIn('fbdraw(', case)
        self.assertRegex(case, r'TAKE_OVER_DISPLAY.*EPERM')
        for op in ('CAPTURE', 'READ_EVENTS', 'MAP_TREE', 'READ_META',
                   'SET_GEOMETRY', 'WRITE_EVENTS'):
            self.assertRegex(case, rf'refused\([^\n]*window_call\(op::{op},[^\n]*EPERM')
        self.assertIn('the owner capturing its own pixels', case)
        self.assertIn('the owner mapping its own tree', case)
        self.assertIn('the owner reading its own metadata', case)
        self.assertIn('mine.finish()', case)


if __name__ == '__main__':
    unittest.main()
