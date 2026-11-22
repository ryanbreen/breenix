"""Check the 25.5 guest case and a real Mac-built x86-64 toolchain bundle.

Build suite-toolchain and the x86-64 C ABI archive first, then supply the
stage-0 executable pins required by scripts/build-toolchain.sh.
"""
import json
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SUITE = ROOT / "userspace/programs/src/suite_toolchain.rs"
CORPUS = ROOT / "toolchain/corpus"
LIBC = ROOT / "libs/libbreenix-libc/target/x86_64-breenix-c/release/libc.a"
EXPECTED = """
array-index
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
        self.assertEqual((SUITE.parent / include[1]).read_text().splitlines(), EXPECTED)

    def test_mac_bundle_builds_exact_candidate_corpus(self):
        with tempfile.TemporaryDirectory(prefix="toolchain-cc-corpus-") as temporary:
            output = Path(temporary) / "bundle"
            result = subprocess.run(
                [ROOT / "scripts/build-toolchain.sh", "--arch", "x86_64",
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
