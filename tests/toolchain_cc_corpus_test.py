"""Check the C corpus guest case and a real Mac-built toolchain bundle.

Build suite-toolchain and the target's C ABI archive first, then supply the
stage-0 executable pins required by scripts/build-toolchain.sh.
"""
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SUITE = ROOT / "userspace/programs/src/suite_toolchain.rs"
CORPUS = ROOT / "toolchain/corpus"
ARCH = os.environ.get("BREENIX_TOOLCHAIN_TEST_ARCH", "x86_64")
LIBC_TARGET = "x86_64-breenix-c" if ARCH == "x86_64" else "aarch64-breenix"
LIBC = ROOT / "libs/libbreenix-libc/target" / LIBC_TARGET / "release/libc.a"
EXPECTED = """
aggregate-copy
array-index
bitfield-values
bitwise-shifts
boolean-tokens
byte-output
conditional
floating-conversion
function-pointer
goto
integer-arithmetic
integer-promotions
literal-output
loops
macros
pointer-distance
recursion
return-status
scope-shadow
short-circuit
static-duration
switch-enum
unsigned-wrap
usual-conversions
varargs
""".split()


class ToolchainCcCorpusTest(unittest.TestCase):
    def test_guest_case_uses_exact_corpus(self):
        source = SUITE.read_text()
        case = re.search(r"fn cc_corpus_three_producers\(\) -> CaseResult \{(.*?)^\}",
                         source, re.DOTALL | re.MULTILINE)
        self.assertIsNotNone(case)
        loop = re.search(r"for name in (\w+)\.lines\(\) \{", case[1])
        self.assertIsNotNone(loop)
        include = re.search(rf'const {loop[1]}: &str = include_str!\("([^"]+)"\);', source)
        self.assertIsNotNone(include)
        names = (SUITE.parent / include[1]).read_text().splitlines()
        self.assertEqual(EXPECTED, names)
        self.assertNotIn('matches!(name, "aggregate-copy" | "bitfield-values" | "varargs")', case[1])

    def test_mac_bundle_builds_exact_candidate_corpus(self):
        with tempfile.TemporaryDirectory(prefix="toolchain-cc-corpus-") as temporary:
            output = Path(temporary) / "bundle"
            result = subprocess.run(
                [ROOT / "scripts/build-toolchain.sh", "--arch", ARCH,
                 "--output", output, "--libc", LIBC],
                cwd=ROOT, capture_output=True, text=True, timeout=55,
            )
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(result.stderr, "")
            record = json.loads((output / "build.json").read_text())
            self.assertEqual(record["candidateProducer"], "mac-bcc")
            staged = output / "root/usr/share/breenix/toolchain"
            self.assertEqual(sorted(path.name for path in (staged / "c-core/mac").iterdir()),
                             [name + ".elf" for name in EXPECTED])
            reference = (CORPUS / "cases").read_text().splitlines()
            self.assertEqual(len(reference), 30)
            self.assertEqual(sorted(path.stem for path in staged.glob("*.elf")), reference)


if __name__ == "__main__":
    unittest.main()
