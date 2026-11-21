"""Check the BreenGL test-control probes against the shared syscall ABI."""
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class TestControlAbi(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        abi = (ROOT / "libs/gpu-abi/src/lib.rs").read_text()
        module = item(abi, "pub mod test_control {")
        # Syscall ops are u32 constants. The nested census actions are u64.
        cls.ops = re.findall(r"pub const (\w+): u32 = ([^;]+);", module)

    def test_isolation_probes_undefined_ops(self):
        source = (ROOT / "userspace/programs/src/suite_breengel/window_cases/isolation.rs").read_text()
        case = item(source, "fn isolation_controls_latched(")
        probes = re.search(r"for op in (\[[^\]]+\])\s*\{", case)
        self.assertIsNotNone(probes, "missing test_control probe list")

        constants = "\n".join(f"pub const {name}: u32 = {value};" for name, value in self.ops)
        defined = ", ".join(f"u64::from(abi::test_control::{name})" for name, _ in self.ops)
        fixture = f"""
mod abi {{ pub mod test_control {{ {constants} }} }}

fn main() {{
    let defined = [{defined}];
    let probes: &[u64] = &{probes[1]};
    for op in probes {{
        assert!(!defined.contains(op), "test_control probe {{op}} is a defined op");
    }}
    assert!(probes.contains(&4), "missing reserved op probe");
    assert!(probes.contains(&(defined.iter().max().unwrap() + 1)), "missing next undefined op probe");
}}
"""
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            rust = path / "probes.rs"
            rust.write_text(fixture)
            binary = path / "probes"
            prefix = ["taskpolicy", "-b"] if sys.platform == "darwin" else []
            subprocess.run([*prefix, "rustc", "--edition=2021", "-Dwarnings", str(rust),
                            "-o", str(binary)], check=True)
            result = subprocess.run([str(binary)], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)

    def test_section_21_lists_each_defined_op_and_reserves_four(self):
        doc = (ROOT / "docs/gui/breengel.md").read_text()
        section = doc.split("\n## 21. Suite-mode test controls\n", 1)[1].split("\n## 22.", 1)[0]
        documented = re.findall(r"\b[Oo]p (\d+) `([A-Z_]+)\b", section)
        self.assertCountEqual(documented, [(value, name) for name, value in self.ops])
        self.assertRegex(section, r"\b[Oo]p 4 is reserved\b")


if __name__ == "__main__":
    unittest.main()
