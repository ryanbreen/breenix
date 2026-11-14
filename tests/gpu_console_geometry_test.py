"""The kernel and panic console surfaces must follow the admitted kernel mode."""
import pathlib
import re
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]


class ConsoleGeometryTest(unittest.TestCase):
    def test_geometry_comes_from_the_kernel_mode(self):
        screen = (ROOT / "kernel/src/gpu/screen.rs").read_text()
        console = (ROOT / "kernel/src/gpu/console.rs").read_text()
        for source in (screen, console):
            self.assertNotRegex(source, r"\bconst\s+(?:WIDTH|HEIGHT|PITCH)\s*:")
            self.assertRegex(source, r"c\.display\.modes\[display::KERNEL_MODE\]")
        self.assertRegex(console, r"width:\s*self\.width")
        self.assertRegex(console, r"height:\s*self\.height")
        self.assertRegex(console, r"scratch:\s*\[\[self\.width,\s*self\.height\]")


if __name__ == "__main__":
    unittest.main()
