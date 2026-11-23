"""Compare ARM linker case expectations with tools and a staged ARM bundle.

Set TOOLCHAIN_ARM_BUNDLE to the u4 bundle and TOOLCHAIN_HOST_TOOLS to the
release directory containing host-built bar, bnm and bld.
"""
import ast
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SUITE = ROOT / "userspace/programs/src/suite_toolchain.rs"


class ToolchainSuiteLinkerTest(unittest.TestCase):
    def test_aarch64_linker_expectations_match_tools(self):
        bundle = Path(os.environ["TOOLCHAIN_ARM_BUNDLE"]) / "root"
        tools = Path(os.environ["TOOLCHAIN_HOST_TOOLS"])
        dev = bundle / "opt/breenix-dev/25.2"
        objects = bundle / "usr/share/breenix/toolchain/objs"
        source = SUITE.read_text()

        def arm_constant(name):
            match = re.search(r'#\[cfg\(target_arch = "aarch64"\)\]\s*'
                              rf'const {name}: &str = ("(?:[^"\\]|\\.)*");', source)
            self.assertIsNotNone(match, name)
            return ast.literal_eval(match[1])

        with tempfile.TemporaryDirectory(prefix="suite-linker-") as temp:
            archive = Path(temp) / "symbols.a"
            subprocess.run([tools / "bar", "rcs", archive, objects / "got-entry.o",
                            objects / "archive-value-with-long-name.o"], check=True, timeout=30)
            actual = subprocess.check_output([tools / "bnm", "-Pg", archive], text=True, timeout=30)
            self.assertEqual(actual, arm_constant("LINKER_BNM_SYMBOLS").replace("{0}", str(archive)))
            output = Path(temp) / "undefined.elf"
            main = objects / "got-main.o"
            result = subprocess.run([tools / "bld", "-T", dev / "linker.ld", "--gc-sections",
                                     "-o", output, main, dev / "lib/libc.a"],
                                    capture_output=True, text=True, timeout=30)
            self.assertEqual(result.returncode, 1, result.stderr)
            self.assertFalse(output.exists())
            line = next(line for line in result.stderr.splitlines()
                        if line.startswith(f"{main}: .text+0x")
                        and line.endswith(" archive_entry: undefined symbol"))
            relocation = arm_constant("LINKER_UNDEFINED_RELOCATION")
            self.assertEqual(relocation, "R_AARCH64_CALL26")
            self.assertIn(relocation, line)


if __name__ == "__main__":
    unittest.main()
