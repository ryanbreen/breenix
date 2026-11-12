"""Keep the GL builder's copyleft source in its pinned external repository."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]


class QemuForkSource(unittest.TestCase):
    def test_recipe_fetches_the_fork_without_in_tree_qemu_patches(self):
        recipe = (ROOT / 'scripts/build-qemu-gl.sh').read_text()
        self.assertIn('QEMU_REPO=https://github.com/ryanbreen/breenix-qemu.git', recipe)
        self.assertRegex(recipe, r'(?m)^QEMU_COMMIT=[0-9a-f]{40}$')
        self.assertIn('git_pin "$QEMU_REPO" "$QEMU_COMMIT" "$QEMU_DIR"', recipe)
        self.assertIn('qemu $QEMU_REPO $QEMU_COMMIT', recipe)
        self.assertFalse(list((ROOT / 'scripts/qemu-gl').glob('*.patch')),
                         'QEMU patches belong in the separate fork')
        self.assertNotIn('$ROOT/scripts/qemu-gl/', recipe)
        self.assertRegex(recipe, r'(?m)^PIN=10\.0\.12-utm-4d73aaf0-b6$')


if __name__ == '__main__':
    unittest.main()
