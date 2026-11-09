"""A configured modifier-only WM hotkey must not be forwarded to the client."""
from pathlib import Path
import unittest
ROOT = Path(__file__).resolve().parents[1]
class Ownership(unittest.TestCase):
    def test_bound_super_edges_are_consumed_before_key_delivery(self):
        source = (ROOT / 'userspace/programs/src/bwm.rs').read_text()
        loop = source.split('let released_chord =', 1)[1].split('hotkey_actions.extend(', 1)[0]
        self.assertIn('binding.key == 0', loop)
        self.assertIn('binding.modifiers == modifier::SUPER', loop)
        self.assertIn('released_chord || modifier_tap', loop)
        self.assertLess(loop.index('released_chord || modifier_tap'), loop.index('if !consumed'))
if __name__ == '__main__': unittest.main()
